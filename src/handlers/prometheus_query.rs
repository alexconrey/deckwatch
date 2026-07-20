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

use std::time::Duration;

use axum::extract::{Query, State};
use axum::Json;
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

/// Prometheus query timeout. Deckwatch requests should not stack up on a
/// shared observability backend.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

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

    let http = reqwest::Client::builder()
        .timeout(QUERY_TIMEOUT)
        .build()
        .map_err(|e| AppError::BadRequest(format!("failed to build http client: {e}")))?;

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
    #[serde(default)]
    values: Vec<(f64, String)>,
}

async fn execute_range(
    http: &reqwest::Client,
    base_url: &str,
    query: &str,
    start: i64,
    end: i64,
    step: u32,
) -> Result<Vec<PodSeries>, String> {
    let url = format!("{base_url}/api/v1/query_range");
    let start_s = start.to_string();
    let end_s = end.to_string();
    let step_s = format!("{step}s");
    let form = [
        ("query", query),
        ("start", start_s.as_str()),
        ("end", end_s.as_str()),
        ("step", step_s.as_str()),
    ];

    let timer = K8sTimer::new("prom_query", "range");
    let res = http.post(&url).form(&form).send().await;

    let response = match res {
        Ok(r) => r,
        Err(e) => {
            timer.finish(false);
            return Err(format!("failed to reach prometheus at {base_url}: {e}"));
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

    if data.result_type != "matrix" {
        return Err(format!("expected matrix result, got {}", data.result_type));
    }

    Ok(data
        .result
        .into_iter()
        .map(|m| PodSeries {
            pod: m
                .metric
                .get("pod")
                .and_then(|v| v.as_str())
                .unwrap_or("aggregate")
                .to_string(),
            points: m
                .values
                .into_iter()
                .filter_map(|(ts, val)| {
                    val.parse::<f64>().ok().map(|v| RangePoint {
                        t: (ts * 1000.0) as i64,
                        v,
                    })
                })
                .collect(),
        })
        .collect())
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
#[path = "../handlers_prometheus_query_tests.rs"]
mod handlers_prometheus_query_tests;
