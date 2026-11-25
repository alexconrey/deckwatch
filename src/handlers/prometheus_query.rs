#![allow(dead_code, unused_imports)]
//! Prometheus range-query proxy for Metrics Visualization Phase 3.
//!
//! Deckwatch does not run its own Prometheus -- this handler proxies a small,
//! curated set of PromQL range queries to a Prometheus HTTP API configured
//! by the operator via `settings.metrics.prometheus.url` (or the
//! `PROMETHEUS_URL` env var). See `docs/METRICS_VISUALIZATION.md` sec 3 tier
//! 3 and `docs/PROMETHEUS_INTEGRATION.md` sec 6.4.
//!
//! Safety posture:
//! - The user picks from an enumerated `query` name. We never accept raw
//!   PromQL -- that would be a query-injection footgun and a DoS lever.
//! - Namespace and pod-name selectors are interpolated with strict char
//!   allowlists so a hostile `deployment` param cannot break out of the
//!   label matcher.
//! - `step` is clamped to keep any single range at <= 2000 points so a
//!   crafted request cannot fan out into a huge query on the operator's
//!   Prometheus instance.
//! - HTTP timeout is fixed at 10s.
//!
//! Graceful degrade:
//! - Prometheus URL unset -> 200 with `{ series: [], unavailable_reason:
//!   "prometheus not configured" }`. Same pattern as `resource_metrics.rs`
//!   for missing metrics-server.
//! - Prometheus unreachable / non-2xx -> 200 with `unavailable_reason` set
//!   to a human-readable message. UI renders a callout, not a red banner.

use std::sync::LazyLock;

use axum::extract::{Query, State};
use axum::Json;
use mcp_k8s::telemetry::semconv;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Public request / response types
// ---------------------------------------------------------------------------

/// Curated PromQL identifiers. The wire enum uses snake_case to match the
/// existing frontend metric-key convention (`cpu_usage` / `memory_usage`).
#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    CpuUsage,
    MemoryUsage,
    RequestRate,
    ErrorRate,
}

/// Query params for GET /api/prometheus/query_range.
///
/// `start` / `end` are unix seconds (Prometheus's native format). `step` is
/// in seconds. `namespace` + `deployment` scope the query; a request with
/// neither is rejected so we never accidentally issue a cluster-wide query.
#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    pub query: QueryKind,
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub step: Option<u32>,
    pub namespace: String,
    pub deployment: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct RangePoint {
    /// Milliseconds since epoch, to match `Sample.t` in `useResourceMetrics.ts`.
    pub t: i64,
    /// Prometheus values are floats. We forward them unscaled; the frontend
    /// unit-formatter already handles cores / bytes / req/s.
    pub v: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct PodSeries {
    /// The `pod` label from the Prometheus response, or `"aggregate"` when
    /// the query returned an unlabeled scalar.
    pub pod: String,
    pub points: Vec<RangePoint>,
}

#[derive(Debug, Serialize)]
pub struct RangeResponse {
    pub series: Vec<PodSeries>,
    /// One of `cores`, `bytes`, `req/s`. Frontend picks a formatter from
    /// this rather than re-deriving it from the query name.
    pub unit: &'static str,
    /// `Some(_)` when the query could not be served (Prometheus disabled,
    /// unreachable, or returned an error). `series` is empty in that case.
    /// The UI renders this as a callout instead of an error banner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Non-fatal notes -- e.g. the request-rate fallback query fired because
    /// `http_requests_total` was empty. Rendered as a subtle chip on the
    /// panel so the operator understands which metric name they see.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Absolute cap on points per range. Prometheus itself caps at 11k by
/// default, but 2k is plenty for a dashboard line and keeps rendering
/// snappy on the frontend. Also serves as a step-clamp lever below.
const MAX_POINTS: u32 = 2000;

// The query timeout (10 s, so deckwatch requests do not stack up on a shared
// observability backend) is `outbound::Purpose::Observability`.

pub async fn query_range(
    State(state): State<AppState>,
    Query(params): Query<RangeQuery>,
) -> Result<Json<RangeResponse>, AppError> {
    // Namespace allow-list mirrors every other namespaced handler. This is
    // the primary authorization surface -- a caller without access to the
    // namespace cannot smuggle a Prometheus query for it either.
    if !state.is_namespace_allowed(&params.namespace) {
        return Err(AppError::NamespaceNotAllowed(params.namespace));
    }

    validate_selector(&params.namespace)
        .map_err(|_| AppError::BadRequest("namespace contains invalid characters".to_string()))?;
    validate_selector(&params.deployment)
        .map_err(|_| AppError::BadRequest("deployment contains invalid characters".to_string()))?;

    if params.end <= params.start {
        return Err(AppError::BadRequest(
            "end must be strictly greater than start".to_string(),
        ));
    }

    let Some(prom_url) = prometheus_url() else {
        return Ok(Json(RangeResponse {
            series: Vec::new(),
            unit: unit_for(params.query),
            unavailable_reason: Some(
                "prometheus not configured (set PROMETHEUS_URL or metrics.prometheus.url in \
                 settings)"
                    .to_string(),
            ),
            warnings: Vec::new(),
        }));
    };

    let range_secs = (params.end - params.start) as u32;
    let step = clamp_step(params.step, range_secs);

    let http = crate::outbound::global().client(crate::outbound::Purpose::Observability);

    let mut warnings = Vec::new();

    let (promql, unit) = build_query(params.query, &params.namespace, &params.deployment);
    let series = match execute_range(&http, &prom_url, &promql, params.start, params.end, step)
        .await
    {
        Ok(s) if s.is_empty() && has_fallback(params.query) => {
            // Try the fallback metric name -- some workloads instrument with
            // `http_server_requests_total` (Spring / .NET convention) instead
            // of `http_requests_total`. Surface which one we used so the UI
            // can hint at it.
            let (fb_promql, _) =
                build_fallback_query(params.query, &params.namespace, &params.deployment);
            match execute_range(&http, &prom_url, &fb_promql, params.start, params.end, step).await
            {
                Ok(fb) if !fb.is_empty() => {
                    warnings.push(format!(
                        "no data for primary metric; fell back to {}",
                        fallback_metric_name(params.query)
                    ));
                    fb
                }
                Ok(_) => Vec::new(),
                Err(e) => {
                    return Ok(Json(RangeResponse {
                        series: Vec::new(),
                        unit,
                        unavailable_reason: Some(e),
                        warnings,
                    }));
                }
            }
        }
        Ok(s) => s,
        Err(e) => {
            return Ok(Json(RangeResponse {
                series: Vec::new(),
                unit,
                unavailable_reason: Some(e),
                warnings,
            }));
        }
    };

    Ok(Json(RangeResponse {
        series,
        unit,
        unavailable_reason: None,
        warnings,
    }))
}

// ---------------------------------------------------------------------------
// Prometheus URL discovery
// ---------------------------------------------------------------------------

/// Resolve the Prometheus URL from env. The settings-ConfigMap channel is a
/// TODO -- the surface exists in `docs/PROMETHEUS_INTEGRATION.md` sec 6.5
/// but is not plumbed through `AppState` yet. Reading env each call is fine
/// (a std::env lookup is trivially cheap) and avoids a restart on rotation.
fn prometheus_url() -> Option<String> {
    match std::env::var("PROMETHEUS_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().trim_end_matches('/').to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Query catalog
// ---------------------------------------------------------------------------

fn unit_for(kind: QueryKind) -> &'static str {
    match kind {
        QueryKind::CpuUsage => "cores",
        QueryKind::MemoryUsage => "bytes",
        QueryKind::RequestRate | QueryKind::ErrorRate => "req/s",
    }
}

fn has_fallback(kind: QueryKind) -> bool {
    matches!(kind, QueryKind::RequestRate | QueryKind::ErrorRate)
}

fn fallback_metric_name(kind: QueryKind) -> &'static str {
    match kind {
        QueryKind::RequestRate | QueryKind::ErrorRate => "http_server_requests_total",
        _ => "",
    }
}

fn build_query(kind: QueryKind, ns: &str, name: &str) -> (String, &'static str) {
    let promql = match kind {
        QueryKind::CpuUsage => format!(
            "sum by (pod) (rate(container_cpu_usage_seconds_total{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\",container!=\"\",container!=\"POD\"}}[2m]))"
        ),
        QueryKind::MemoryUsage => format!(
            "sum by (pod) (container_memory_working_set_bytes{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\",container!=\"\",container!=\"POD\"}})"
        ),
        QueryKind::RequestRate => format!(
            "sum by (pod) (rate(http_requests_total{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\"}}[2m]))"
        ),
        QueryKind::ErrorRate => format!(
            "sum by (pod) (rate(http_requests_total{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\",status=~\"5..\"}}[2m]))"
        ),
    };
    (promql, unit_for(kind))
}

fn build_fallback_query(kind: QueryKind, ns: &str, name: &str) -> (String, &'static str) {
    let promql = match kind {
        QueryKind::RequestRate => format!(
            "sum by (pod) (rate(http_server_requests_total{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\"}}[2m]))"
        ),
        QueryKind::ErrorRate => format!(
            "sum by (pod) (rate(http_server_requests_total{{namespace=\"{ns}\",\
             pod=~\"{name}-.*\",status=~\"5..\"}}[2m]))"
        ),
        // CPU/Memory have no meaningful fallback -- should never be called.
        _ => build_query(kind, ns, name).0,
    };
    (promql, unit_for(kind))
}

// ---------------------------------------------------------------------------
// Prometheus HTTP client
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PromResponse {
    status: String,
    #[serde(default)]
    data: Option<PromData>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    #[serde(rename = "errorType")]
    error_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PromData {
    #[serde(rename = "resultType")]
    result_type: String,
    result: Vec<PromMatrix>,
}

#[derive(Debug, Deserialize)]
struct PromMatrix {
    metric: serde_json::Map<String, serde_json::Value>,
    /// Range queries (`matrix`).
    #[serde(default)]
    values: Vec<(f64, String)>,
    /// Instant queries (`vector`).
    #[serde(default)]
    value: Option<(f64, String)>,
}

/// POST a PromQL range query (form-encoded) to `{base_url}/api/v1/query_range`
/// and convert the matrix result into per-pod series.
pub async fn execute_range(
    http: &reqwest::Client,
    base_url: &str,
    query: &str,
    start: i64,
    end: i64,
    step: u32,
) -> Result<Vec<PodSeries>, String> {
    Ok(
        execute_range_by(http, base_url, query, start, end, step, "pod")
            .await?
            .into_iter()
            .map(|(pod, points)| PodSeries {
                pod: pod.unwrap_or_else(|| "aggregate".to_string()),
                points,
            })
            .collect(),
    )
}

/// A range query's series, each keyed by its `label` value (`None` when the
/// series has no such label). Non-finite values (Prometheus `NaN`, e.g. a
/// quantile of no data) are dropped.
async fn execute_range_by(
    http: &reqwest::Client,
    base_url: &str,
    query: &str,
    start: i64,
    end: i64,
    step: u32,
    label: &str,
) -> Result<Vec<(Option<String>, Vec<RangePoint>)>, String> {
    let start_s = start.to_string();
    let end_s = end.to_string();
    let step_s = format!("{step}s");
    let form = [
        ("query", query),
        ("start", start_s.as_str()),
        ("end", end_s.as_str()),
        ("step", step_s.as_str()),
    ];
    let data = post_query(http, base_url, "query_range", &form, "matrix").await?;
    Ok(data
        .result
        .into_iter()
        .map(|m| {
            let key = label_of(&m.metric, label);
            let points = m
                .values
                .into_iter()
                .filter_map(|(ts, val)| {
                    let v = val.parse::<f64>().ok().filter(|v| v.is_finite())?;
                    Some(RangePoint {
                        t: (ts * 1000.0) as i64,
                        v,
                    })
                })
                .collect();
            (key, points)
        })
        .collect())
}

/// An instant query's samples at `time` (unix seconds), as (labels, value).
/// Non-finite values are dropped.
async fn execute_instant(
    http: &reqwest::Client,
    base_url: &str,
    query: &str,
    time: i64,
) -> Result<Vec<(serde_json::Map<String, serde_json::Value>, f64)>, String> {
    let time_s = time.to_string();
    let form = [("query", query), ("time", time_s.as_str())];
    let data = post_query(http, base_url, "query", &form, "vector").await?;
    Ok(data
        .result
        .into_iter()
        .filter_map(|m| {
            let v = m.value?.1.parse::<f64>().ok().filter(|v| v.is_finite())?;
            Some((m.metric, v))
        })
        .collect())
}

fn label_of(metric: &serde_json::Map<String, serde_json::Value>, label: &str) -> Option<String> {
    metric
        .get(label)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// POST a form-encoded query to `{base_url}/api/v1/{endpoint}` and return
/// its data, which must be of `result_type`.
async fn post_query(
    http: &reqwest::Client,
    base_url: &str,
    endpoint: &str,
    form: &[(&str, &str)],
    result_type: &str,
) -> Result<PromData, String> {
    let url = format!("{base_url}/api/v1/{endpoint}");
    let timer = K8sTimer::new(
        "prom_query",
        if endpoint == "query" {
            "instant"
        } else {
            "range"
        },
    );
    let res = http.post(&url).form(form).send().await;

    let response = match res {
        Ok(r) => r,
        Err(e) => {
            timer.finish(false);
            return Err(format!(
                "failed to reach prometheus at {base_url}: {}",
                crate::outbound::error_chain(&e)
            ));
        }
    };

    if !response.status().is_success() {
        let code = response.status();
        timer.finish(false);
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "prometheus returned HTTP {code}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let parsed: PromResponse = match response.json().await {
        Ok(p) => p,
        Err(e) => {
            timer.finish(false);
            return Err(format!("failed to parse prometheus response: {e}"));
        }
    };
    timer.finish(true);

    if parsed.status != "success" {
        return Err(format!(
            "prometheus query failed: {} ({})",
            parsed.error.unwrap_or_else(|| "unknown".to_string()),
            parsed.error_type.unwrap_or_else(|| "unknown".to_string()),
        ));
    }

    let data = parsed
        .data
        .ok_or_else(|| "prometheus success response missing data".to_string())?;

    if data.result_type != result_type {
        return Err(format!(
            "expected {result_type} result, got {}",
            data.result_type
        ));
    }
    Ok(data)
}

// ---------------------------------------------------------------------------
// MCP activity: deckwatch's MCP tool call metrics
// ---------------------------------------------------------------------------
//
// The page reads one of two sources, the first that Prometheus has tool call
// series from in the window (`detect_source`):
//
// 1. OTLP, the primary source: the `mcp.server.operation.duration` histogram
//    deckwatch exports over OTLP (src/telemetry.rs), as Prometheus's OTLP
//    receiver stores it with its default translation
//    (`UnderscoreEscapingWithSuffixes`):
//    `mcp_server_operation_duration_seconds_{bucket,sum,count}`. Every tool
//    call is one observation, whether deckwatch serves the tool or mcp-k8s
//    runs it (`super::mcp_telemetry`).
// 2. Scrape, the fallback for one release: deckwatch's own counters on
//    `/metrics`. `deckwatch_mcp_tool_calls_total` counts the calls deckwatch
//    serves itself and the ones a guardrail refuses, and
//    `deckwatch_mcp_passthrough_tool_calls_total` the ones it hands to
//    mcp-k8s, so each call is in exactly one of them.
//
// Neither reads mcp-k8s's own `mcp_k8s_*` metrics, which mcp-k8s v0.7.0
// deprecated.

/// The time windows the MCP activity page offers.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityWindow {
    #[serde(rename = "1h")]
    Hour,
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
}

impl ActivityWindow {
    fn seconds(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 24 * 3600,
            Self::Week => 7 * 24 * 3600,
        }
    }

    /// Points every this many seconds: 60, 288 or 336 points per window.
    fn step(self) -> u32 {
        match self {
            Self::Hour => 60,
            Self::Day => 300,
            Self::Week => 1800,
        }
    }

    /// The `rate()` range for the time series.
    fn rate_range(self) -> &'static str {
        match self {
            Self::Hour => "5m",
            Self::Day => "15m",
            Self::Week => "1h",
        }
    }

    /// The whole window, for `increase()`.
    fn range(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }
}

/// Query params for GET /api/prometheus/mcp_activity.
#[derive(Debug, Deserialize)]
pub struct McpActivityQuery {
    #[serde(default)]
    pub window: ActivityWindow,
}

#[derive(Debug, Serialize, Clone)]
pub struct LabeledSeries {
    /// The series' label value: the call outcome (`ok`, `error`, `denied`).
    pub label: String,
    pub points: Vec<RangePoint>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ToolActivity {
    pub tool: String,
    /// Calls in the window (Prometheus `increase`, so not always whole).
    pub calls: f64,
    pub errors: f64,
    pub denied: f64,
    /// The 95th percentile duration in the window, if there were timed calls.
    pub p95_seconds: Option<f64>,
}

/// Where the MCP activity page's tool call figures come from.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivitySource {
    /// `mcp_server_operation_duration_seconds_*`: deckwatch's OTLP metrics,
    /// through Prometheus's OTLP receiver.
    Otlp,
    /// deckwatch's own counters, scraped from `/metrics`: the fallback.
    Scrape,
}

impl ActivitySource {
    /// The label that names the tool in this source's series.
    fn tool_label(self) -> &'static str {
        match self {
            Self::Otlp => OTLP_TOOL,
            Self::Scrape => "tool",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct McpActivityResponse {
    pub window_seconds: i64,
    pub step_seconds: u32,
    /// Where the figures come from: `otlp` when Prometheus has deckwatch's
    /// OTLP tool call series for the window, otherwise `scrape` when it has
    /// the `/metrics` counters. `None` when it has neither (the figures are
    /// then empty), or with `unavailable_reason`.
    pub source: Option<ActivitySource>,
    /// Tool calls per minute by outcome.
    pub calls_per_minute: Vec<LabeledSeries>,
    /// `apply_manifest` calls per minute by outcome.
    pub apply_manifest_per_minute: Vec<LabeledSeries>,
    /// The 95th percentile tool call duration over time, in seconds.
    pub p95_seconds: Vec<RangePoint>,
    /// Per tool over the window, most calls first, at most [`MAX_TOOLS`].
    pub tools: Vec<ToolActivity>,
    /// Set when Prometheus isn't configured or a query failed; the rest is
    /// then empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// The most tools [`McpActivityResponse::tools`] lists.
pub const MAX_TOOLS: usize = 25;

/// mcp-k8s's duration histogram (`semconv::MCP_SERVER_OPERATION_DURATION`,
/// unit `s`) as Prometheus's OTLP receiver names it.
const OTLP_DURATION: &str = "mcp_server_operation_duration_seconds";
/// Labels of [`OTLP_DURATION`]: mcp-k8s's metric attributes with `.` turned
/// into `_` (`semconv::GEN_AI_TOOL_NAME` and so on; the tests check them).
const OTLP_TOOL: &str = "gen_ai_tool_name";
const OTLP_METHOD: &str = "mcp_method_name";
const OTLP_ERROR_TYPE: &str = "error_type";
const OTLP_DECISION: &str = "mcp_k8s_decision";

/// The fallback's tool call counters (`crate::metrics::MCP_TOOL_CALLS` and
/// `MCP_PASSTHROUGH_TOOL_CALLS`). Each call is counted in exactly one.
const SCRAPE_COUNTERS: [&str; 2] = [
    "deckwatch_mcp_tool_calls_total",
    "deckwatch_mcp_passthrough_tool_calls_total",
];
/// The matching duration histograms' buckets (same buckets, one recorder).
const SCRAPE_DURATION_BUCKETS: [&str; 2] = [
    "deckwatch_mcp_tool_call_duration_seconds_bucket",
    "deckwatch_mcp_passthrough_tool_call_duration_seconds_bucket",
];

/// `inner` applied to both fallback metric families, each marked with its
/// own `family` label so `or` keeps the series of both and `sum` adds them
/// up. A tool can be in both: an mcp-k8s tool's refused calls are
/// deckwatch's.
fn both_families(metrics: [&str; 2], inner: impl Fn(&str) -> String) -> String {
    format!(
        "label_replace({}, \"family\", \"deckwatch\", \"\", \"\") or \
         label_replace({}, \"family\", \"passthrough\", \"\", \"\")",
        inner(metrics[0]),
        inner(metrics[1])
    )
}

/// `s` inside a double-quoted PromQL string.
fn promql_escape(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
}

/// deckwatch's tool calls in the OTLP histogram's `series` (`count` or
/// `bucket`): the `tools/call` observations with deckwatch's `job` whose tool
/// name matches `tool` (`!=""`, or `="apply_manifest"`). Calls of a tool
/// deckwatch doesn't have carry no tool name; the fallback doesn't count them
/// either.
fn otlp_selector(series: &str, job: &str, tool: &str) -> String {
    format!(
        "{OTLP_DURATION}_{series}{{job=\"{}\",{OTLP_METHOD}=\"{}\",{OTLP_TOOL}{tool}}}",
        promql_escape(job),
        semconv::TOOLS_CALL,
    )
}

/// `inner`, OTLP series, with the fallback's `status` label: `denied` for the
/// calls a guardrail refused (`mcp_k8s_decision="denied"`), `error` for the
/// other calls with an `error_type`, `ok` for the rest. A refused call has an
/// `error_type` too (`permission_denied`), so the denial rule comes last.
fn with_status(inner: &str) -> String {
    format!(
        "label_replace(label_replace(label_replace({inner}, \
         \"status\", \"ok\", \"\", \"\"), \
         \"status\", \"error\", \"{OTLP_ERROR_TYPE}\", \".+\"), \
         \"status\", \"denied\", \"{OTLP_DECISION}\", \"denied\")"
    )
}

/// An instant query that returns one sample when Prometheus has any tool call
/// series of `source` in `window`, and none otherwise. It only checks for
/// series: no rates, no histogram buckets.
fn has_data_query(source: ActivitySource, window: ActivityWindow, job: &str) -> String {
    let w = window.range();
    let present = |selector: &str| format!("group(present_over_time({selector}[{w}]))");
    match source {
        ActivitySource::Otlp => present(&otlp_selector("count", job, "!=\"\"")),
        ActivitySource::Scrape => format!(
            "{} or {}",
            present(SCRAPE_COUNTERS[0]),
            present(SCRAPE_COUNTERS[1])
        ),
    }
}

/// The PromQL for the MCP activity page. Fixed queries: only the source and
/// the window, server-side enums, and deckwatch's own `job` vary.
///
/// Both sources produce the same `status` label, and name the tool with
/// [`ActivitySource::tool_label`].
struct ActivityQueries {
    calls: String,
    apply_manifest: String,
    p95: String,
    tool_totals: String,
    tool_p95: String,
}

fn activity_queries(source: ActivitySource, window: ActivityWindow, job: &str) -> ActivityQueries {
    let r = window.rate_range();
    let w = window.range();
    match source {
        ActivitySource::Otlp => {
            let calls = otlp_selector("count", job, "!=\"\"");
            let apply_manifest = otlp_selector("count", job, "=\"apply_manifest\"");
            let buckets = otlp_selector("bucket", job, "!=\"\"");
            ActivityQueries {
                calls: format!(
                    "sum by (status) ({}) * 60",
                    with_status(&format!("rate({calls}[{r}])"))
                ),
                apply_manifest: format!(
                    "sum by (status) ({}) * 60",
                    with_status(&format!("rate({apply_manifest}[{r}])"))
                ),
                p95: format!("histogram_quantile(0.95, sum by (le) (rate({buckets}[{r}])))"),
                tool_totals: format!(
                    "sum by ({OTLP_TOOL}, status) ({})",
                    with_status(&format!("increase({calls}[{w}])"))
                ),
                tool_p95: format!(
                    "histogram_quantile(0.95, sum by (le, {OTLP_TOOL}) \
                     (increase({buckets}[{w}])))"
                ),
            }
        }
        ActivitySource::Scrape => ActivityQueries {
            calls: format!(
                "sum by (status) ({}) * 60",
                both_families(SCRAPE_COUNTERS, |m| format!("rate({m}[{r}])"))
            ),
            apply_manifest: format!(
                "sum by (status) ({}) * 60",
                both_families(SCRAPE_COUNTERS, |m| format!(
                    "rate({m}{{tool=\"apply_manifest\"}}[{r}])"
                ))
            ),
            p95: format!(
                "histogram_quantile(0.95, sum by (le) ({}))",
                both_families(SCRAPE_DURATION_BUCKETS, |m| format!("rate({m}[{r}])"))
            ),
            tool_totals: format!(
                "sum by (tool, status) ({})",
                both_families(SCRAPE_COUNTERS, |m| format!("increase({m}[{w}])"))
            ),
            tool_p95: format!(
                "histogram_quantile(0.95, sum by (le, tool) ({}))",
                both_families(SCRAPE_DURATION_BUCKETS, |m| format!("increase({m}[{w}])"))
            ),
        },
    }
}

/// Per-tool totals from `sum by (<tool>, status) (increase(...))` samples and
/// `histogram_quantile(...) by (<tool>)` samples, the tool named by
/// `tool_label`: most calls first, at most [`MAX_TOOLS`].
fn tool_activity(
    totals: Vec<(serde_json::Map<String, serde_json::Value>, f64)>,
    p95: Vec<(serde_json::Map<String, serde_json::Value>, f64)>,
    tool_label: &str,
) -> Vec<ToolActivity> {
    let mut by_tool: std::collections::BTreeMap<String, ToolActivity> = Default::default();
    for (labels, value) in totals {
        let Some(tool) = label_of(&labels, tool_label) else {
            continue;
        };
        let entry = by_tool.entry(tool.clone()).or_insert(ToolActivity {
            tool,
            calls: 0.0,
            errors: 0.0,
            denied: 0.0,
            p95_seconds: None,
        });
        entry.calls += value;
        match label_of(&labels, "status").as_deref() {
            Some("error") => entry.errors += value,
            Some("denied") => entry.denied += value,
            _ => {}
        }
    }
    for (labels, value) in p95 {
        if let Some(entry) = label_of(&labels, tool_label).and_then(|t| by_tool.get_mut(&t)) {
            entry.p95_seconds = Some(value);
        }
    }
    let mut tools: Vec<ToolActivity> = by_tool.into_values().filter(|t| t.calls > 0.0).collect();
    tools.sort_by(|a, b| {
        b.calls
            .total_cmp(&a.calls)
            .then_with(|| a.tool.cmp(&b.tool))
    });
    tools.truncate(MAX_TOOLS);
    tools
}

fn labeled(series: Vec<(Option<String>, Vec<RangePoint>)>) -> Vec<LabeledSeries> {
    let mut out: Vec<LabeledSeries> = series
        .into_iter()
        .map(|(label, points)| LabeledSeries {
            label: label.unwrap_or_else(|| "unknown".to_string()),
            points,
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// The `job` label of deckwatch's OTLP series, from the resource deckwatch
/// exports with. Matching it keeps out other services that export the same
/// OpenTelemetry metric, such as a standalone mcp-k8s.
fn otlp_job() -> &'static str {
    static JOB: LazyLock<String> = LazyLock::new(|| {
        let resource = crate::telemetry::resource();
        let attribute = |key: &'static str| {
            resource
                .get(&opentelemetry::Key::from_static_str(key))
                .map(|v| v.to_string())
                .filter(|v| !v.is_empty())
        };
        job_label(
            attribute("service.namespace").as_deref(),
            &attribute("service.name")
                .unwrap_or_else(|| crate::telemetry::DEFAULT_SERVICE_NAME.to_string()),
        )
    });
    &JOB
}

/// The `job` label Prometheus's OTLP receiver gives a resource:
/// `service.namespace/service.name`, or `service.name` alone.
fn job_label(service_namespace: Option<&str>, service_name: &str) -> String {
    match service_namespace {
        Some(namespace) => format!("{namespace}/{service_name}"),
        None => service_name.to_string(),
    }
}

/// `GET /api/prometheus/mcp_activity?window=1h|24h|7d`: deckwatch's MCP
/// tool call metrics for the MCP activity page, from the Prometheus at
/// `PROMETHEUS_URL`. Like the bundled Grafana dashboard, it sums every
/// deckwatch instance that Prometheus has series from.
pub async fn mcp_activity(
    State(_state): State<AppState>,
    Query(params): Query<McpActivityQuery>,
) -> Json<McpActivityResponse> {
    let http = crate::outbound::global().client(crate::outbound::Purpose::Observability);
    let now = jiff::Timestamp::now().as_second();
    Json(
        activity(
            prometheus_url().as_deref(),
            &http,
            params.window,
            now,
            otlp_job(),
        )
        .await,
    )
}

/// The source the page reads for `window` ending at `now`: OTLP if
/// Prometheus has deckwatch's OTLP tool call series in the window, otherwise
/// the `/metrics` counters if it has those, otherwise `None`. One cheap
/// instant query per source, both at once.
async fn detect_source(
    http: &reqwest::Client,
    prom_url: &str,
    window: ActivityWindow,
    now: i64,
    job: &str,
) -> Result<Option<ActivitySource>, String> {
    let otlp = has_data_query(ActivitySource::Otlp, window, job);
    let scrape = has_data_query(ActivitySource::Scrape, window, job);
    let (otlp, scrape) = tokio::join!(
        execute_instant(http, prom_url, &otlp, now),
        execute_instant(http, prom_url, &scrape, now),
    );
    if !otlp?.is_empty() {
        return Ok(Some(ActivitySource::Otlp));
    }
    Ok((!scrape?.is_empty()).then_some(ActivitySource::Scrape))
}

/// The MCP activity for `window` ending at `now` (unix seconds), from the
/// Prometheus at `prom_url`. `job` is the `job` label of deckwatch's OTLP
/// series ([`otlp_job`]).
async fn activity(
    prom_url: Option<&str>,
    http: &reqwest::Client,
    window: ActivityWindow,
    now: i64,
    job: &str,
) -> McpActivityResponse {
    let mut response = McpActivityResponse {
        window_seconds: window.seconds(),
        step_seconds: window.step(),
        source: None,
        calls_per_minute: Vec::new(),
        apply_manifest_per_minute: Vec::new(),
        p95_seconds: Vec::new(),
        tools: Vec::new(),
        unavailable_reason: None,
    };
    let Some(prom_url) = prom_url else {
        response.unavailable_reason = Some(
            "prometheus not configured (set PROMETHEUS_URL to the Prometheus that stores \
             deckwatch's metrics)"
                .to_string(),
        );
        return response;
    };
    let source = match detect_source(http, prom_url, window, now, job).await {
        Ok(Some(source)) => source,
        // Neither source has series in the window: nothing to show.
        Ok(None) => return response,
        Err(e) => {
            response.unavailable_reason = Some(e);
            return response;
        }
    };
    let start = now - window.seconds();
    let step = window.step();
    let q = activity_queries(source, window, job);

    let (calls, apply_manifest, p95, totals, tool_p95) = tokio::join!(
        execute_range_by(http, prom_url, &q.calls, start, now, step, "status"),
        execute_range_by(
            http,
            prom_url,
            &q.apply_manifest,
            start,
            now,
            step,
            "status"
        ),
        execute_range_by(http, prom_url, &q.p95, start, now, step, "le"),
        execute_instant(http, prom_url, &q.tool_totals, now),
        execute_instant(http, prom_url, &q.tool_p95, now),
    );
    match (calls, apply_manifest, p95, totals, tool_p95) {
        (Ok(calls), Ok(apply_manifest), Ok(p95), Ok(totals), Ok(tool_p95)) => {
            response.source = Some(source);
            response.calls_per_minute = labeled(calls);
            response.apply_manifest_per_minute = labeled(apply_manifest);
            response.p95_seconds = p95.into_iter().next().map(|(_, p)| p).unwrap_or_default();
            response.tools = tool_activity(totals, tool_p95, source.tool_label());
        }
        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), ..) | (.., Err(e), _) | (.., Err(e)) => {
            response.unavailable_reason = Some(e);
        }
    }
    response
}

// ---------------------------------------------------------------------------
// Input validation & step clamping
// ---------------------------------------------------------------------------

/// Only allow characters that are legal in K8s object names and label
/// values, so a hostile caller cannot inject `"` and escape the label
/// matcher. Kubernetes names are already RFC-1123 subset, but users may
/// pass label selectors with `=,()` -- we keep the strictest workable set
/// for the deployment/namespace params.
fn validate_selector(s: &str) -> Result<(), ()> {
    if s.is_empty() || s.len() > 253 {
        return Err(());
    }
    for c in s.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
            return Err(());
        }
    }
    Ok(())
}

/// Given a range in seconds and an optional user-supplied step, return a
/// step that keeps `(end - start) / step <= MAX_POINTS`. Also floors the
/// step at 15s (finer resolution than most scrape intervals is wasted).
fn clamp_step(user: Option<u32>, range_secs: u32) -> u32 {
    let min_step = (range_secs / MAX_POINTS).max(15);
    match user {
        Some(s) if s >= min_step => s,
        _ => min_step,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_allows_normal_k8s_names() {
        assert!(validate_selector("prod").is_ok());
        assert!(validate_selector("my-app.v2_beta").is_ok());
        assert!(validate_selector("web-abc-123").is_ok());
    }

    #[test]
    fn selector_rejects_injection() {
        assert!(validate_selector("prod\"} or ignore").is_err());
        assert!(validate_selector("").is_err());
        assert!(validate_selector("has space").is_err());
        assert!(validate_selector("evil,label").is_err());
    }

    #[test]
    fn step_clamps_to_max_points() {
        // 24h @ 1s user step -> would be 86_400 points; must clamp.
        let s = clamp_step(Some(1), 24 * 60 * 60);
        assert!(s >= (24 * 60 * 60) / MAX_POINTS);
    }

    #[test]
    fn step_honors_user_when_reasonable() {
        // 1h @ 60s user step -> 60 points, fine.
        assert_eq!(clamp_step(Some(60), 3600), 60);
    }

    #[test]
    fn step_defaults_to_min_when_absent() {
        let s = clamp_step(None, 3600);
        assert_eq!(s, 15);
    }

    #[test]
    fn units_match_query_kind() {
        assert_eq!(unit_for(QueryKind::CpuUsage), "cores");
        assert_eq!(unit_for(QueryKind::MemoryUsage), "bytes");
        assert_eq!(unit_for(QueryKind::RequestRate), "req/s");
        assert_eq!(unit_for(QueryKind::ErrorRate), "req/s");
    }

    #[test]
    fn cpu_query_uses_2m_rate_and_excludes_pause() {
        let (q, _) = build_query(QueryKind::CpuUsage, "prod", "web");
        assert!(q.contains("rate("));
        assert!(q.contains("[2m]"));
        assert!(q.contains("container!=\"POD\""));
        assert!(q.contains("namespace=\"prod\""));
        assert!(q.contains("pod=~\"web-.*\""));
    }

    #[test]
    fn memory_query_uses_working_set() {
        let (q, _) = build_query(QueryKind::MemoryUsage, "prod", "web");
        assert!(q.contains("container_memory_working_set_bytes"));
        assert!(!q.contains("rate("));
    }

    #[test]
    fn error_query_filters_5xx() {
        let (q, _) = build_query(QueryKind::ErrorRate, "prod", "web");
        assert!(q.contains("status=~\"5..\""));
    }

    #[test]
    fn fallback_metric_names() {
        assert_eq!(
            fallback_metric_name(QueryKind::RequestRate),
            "http_server_requests_total"
        );
        assert_eq!(
            fallback_metric_name(QueryKind::ErrorRate),
            "http_server_requests_total"
        );
        assert_eq!(fallback_metric_name(QueryKind::CpuUsage), "");
    }
}

#[cfg(test)]
#[path = "prometheus_query_tests.rs"]
mod handlers_prometheus_query_tests;

#[cfg(test)]
#[path = "prometheus_mcp_activity_tests.rs"]
mod mcp_activity_tests;
