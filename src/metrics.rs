#![allow(dead_code, unused_imports)]
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serde::Deserialize;

/// Global handle to the Prometheus recorder. Set once on startup so the
/// `/metrics` endpoint can render the current snapshot on demand.
static PROM_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus recorder as the global `metrics` sink and register
/// the histogram buckets we care about. Call once from `main`.
pub fn init() {
    let handle = exporter()
        .install_recorder()
        .expect("install prometheus recorder");

    if PROM_HANDLE.set(handle).is_err() {
        tracing::warn!("metrics::init called more than once");
    }
}

/// The Prometheus exporter with deckwatch's histogram buckets: every metric
/// ending in `_duration_seconds` is a histogram with the latency buckets
/// (anything else recorded with `histogram!` renders as a summary, without
/// `_bucket` series).
///
/// Latency buckets are geared toward interactive dashboard traffic (a few ms
/// up to a couple seconds) plus a long tail for slow k8s api calls / log
/// history fetches.
pub(crate) fn exporter() -> PrometheusBuilder {
    let latency_buckets: &[f64] = &[
        0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Suffix("_duration_seconds".to_string()),
            latency_buckets,
        )
        .expect("configure latency buckets")
}

/// Render the current Prometheus exposition text. Cheap enough to call per
/// scrape; the recorder aggregates in-memory.
pub fn render() -> String {
    PROM_HANDLE
        .get()
        .map(|h| h.render())
        .unwrap_or_else(|| String::from("# metrics recorder not initialized\n"))
}

/// GET /metrics — Prometheus scrape endpoint. No auth, no state.
pub async fn metrics_handler() -> impl IntoResponse {
    let body = render();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
}

/// Axum middleware that records `deckwatch_http_requests_total` and
/// `deckwatch_http_request_duration_seconds` for every request.
///
/// Uses `MatchedPath` (e.g. `/api/namespaces/{ns}/deployments/{name}`) as the
/// `path` label so cardinality stays bounded — raw request URIs would explode.
/// Requests that don't match a route (SPA fallback, unknown paths) are
/// bucketed under `unmatched`.
pub async fn track_http(req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());

    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    counter!(
        "deckwatch_http_requests_total",
        "method" => method.clone(),
        "path" => path.clone(),
        "status_code" => status,
    )
    .increment(1);

    histogram!(
        "deckwatch_http_request_duration_seconds",
        "method" => method,
        "path" => path,
    )
    .record(elapsed);

    response
}

/// Record a Kubernetes API call. Call from wrappers around kube-rs operations.
///
/// - `resource` is the kind (e.g. `deployments`, `pods`)
/// - `operation` is `list`, `get`, `create`, `update`, `delete`, `patch`, etc.
/// - `status` is `ok` or `err`
pub fn record_k8s_call(resource: &str, operation: &str, status: &str, duration_s: f64) {
    counter!(
        "deckwatch_kube_api_requests_total",
        "resource" => resource.to_owned(),
        "verb" => operation.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);

    histogram!(
        "deckwatch_kube_api_request_duration_seconds",
        "resource" => resource.to_owned(),
        "verb" => operation.to_owned(),
    )
    .record(duration_s);
}

/// Timer helper for k8s api call instrumentation. Drop-guard pattern lets
/// callers wrap an `.await` in a single line without manual `Instant` math.
///
/// ```ignore
/// let _t = K8sTimer::new("deployments", "list");
/// let list = api.list(&Default::default()).await;
/// _t.finish(list.is_ok());
/// ```
pub struct K8sTimer {
    resource: &'static str,
    operation: &'static str,
    start: Instant,
    finished: bool,
}

impl K8sTimer {
    pub fn new(resource: &'static str, operation: &'static str) -> Self {
        Self {
            resource,
            operation,
            start: Instant::now(),
            finished: false,
        }
    }

    pub fn finish(mut self, ok: bool) {
        self.finished = true;
        let status = if ok { "ok" } else { "err" };
        record_k8s_call(
            self.resource,
            self.operation,
            status,
            self.start.elapsed().as_secs_f64(),
        );
    }
}

impl Drop for K8sTimer {
    fn drop(&mut self) {
        if !self.finished {
            // Timer was dropped without .finish() — likely a panic or `?`
            // short-circuit. Record as an error so the miss is visible.
            record_k8s_call(
                self.resource,
                self.operation,
                "err",
                self.start.elapsed().as_secs_f64(),
            );
        }
    }
}

/// Update the gauge tracking how many deployments deckwatch is managing in
/// each namespace, broken down by status. Call from the watcher poll cycle.
pub fn set_deployments_managed(namespace: &str, status: &str, count: f64) {
    gauge!(
        "deckwatch_deployments_managed_total",
        "namespace" => namespace.to_owned(),
        "status" => status.to_owned(),
    )
    .set(count);
}

/// Update the gauge tracking the number of active gitops configurations in
/// the database. Call from the watcher poll cycle.
pub fn set_gitops_watchers(count: f64) {
    gauge!("deckwatch_gitops_watchers_total").set(count);
}

/// Update the gauge tracking how many ingresses deckwatch is managing per
/// namespace. Call from the watcher poll cycle.
pub fn set_ingresses_managed(namespace: &str, count: f64) {
    gauge!(
        "deckwatch_ingresses_managed_total",
        "namespace" => namespace.to_owned(),
    )
    .set(count);
}

/// Record an audit event. Call from `audit::log_action`.
pub fn record_audit_event(action: &str, resource_type: &str) {
    counter!(
        "deckwatch_audit_events_total",
        "action" => action.to_owned(),
        "resource_type" => resource_type.to_owned(),
    )
    .increment(1);
}

/// Record an application error. Call from the `AppError` response path.
pub fn record_error(handler: &str, error_type: &str) {
    counter!(
        "deckwatch_errors_total",
        "handler" => handler.to_owned(),
        "error_type" => error_type.to_owned(),
    )
    .increment(1);
}

/// Record how long a single gitops poll cycle took.
pub fn record_gitops_poll_duration(duration_s: f64) {
    histogram!("deckwatch_gitops_poll_duration_seconds").record(duration_s);
}

/// Increment/decrement the active SSE connection gauge. Pair `sse_opened`
/// with `sse_closed` — one on stream start, one on stream end (including
/// error paths).
pub fn sse_opened() {
    gauge!("deckwatch_active_sse_connections").increment(1.0);
}

pub fn sse_closed() {
    gauge!("deckwatch_active_sse_connections").decrement(1.0);
}

/// Record a gitops build result. Call once per build start or completion.
/// - `deployment`: the deployment name (e.g. `my-app`)
pub fn record_gitops_build(namespace: &str, deployment: &str, status: &str) {
    counter!(
        "deckwatch_gitops_builds_total",
        "namespace" => namespace.to_owned(),
        "deployment" => deployment.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);
}

/// Record a single MCP JSON-RPC request.
/// - `method` is the JSON-RPC method (e.g. `tools/call`, `initialize`), or
///   `_OTHER` for one outside the MCP specification: the client picks it, so
///   pass it through `handlers::mcp_telemetry::metric_method`
/// - `status` is `ok` or `error`
pub fn record_mcp_request(method: &str, status: &str, duration_s: f64) {
    counter!(
        "deckwatch_mcp_requests_total",
        "method" => method.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);

    histogram!(
        "deckwatch_mcp_request_duration_seconds",
        "method" => method.to_owned(),
    )
    .record(duration_s);
}

// -----------------------------------------------------------------------------
// Plugin metrics
// -----------------------------------------------------------------------------

/// Record a plugin load attempt. Call once per plugin from `fetch_plugins`.
/// - `status`: `loaded`, `fetch_failed`, `disabled`
pub fn record_plugin_load(plugin: &str, status: &str) {
    counter!(
        "deckwatch_plugin_loads_total",
        "plugin" => plugin.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);
}

/// Record a plugin execution (apply, provision, or deprovision).
/// - `operation`: `apply`, `provision`, `deprovision`, `validate`
/// - `status`: `ok` or `error`
pub fn record_plugin_execution(plugin: &str, operation: &str, status: &str, duration_s: f64) {
    counter!(
        "deckwatch_plugin_executions_total",
        "plugin" => plugin.to_owned(),
        "operation" => operation.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);

    histogram!(
        "deckwatch_plugin_execution_duration_seconds",
        "plugin" => plugin.to_owned(),
        "operation" => operation.to_owned(),
    )
    .record(duration_s);
}

/// Record a plugin error. Covers both WASM-level failures and errors the
/// plugin itself reported.
/// - `error_type`: a host event code without `plugin.` (`fetch_failed`,
///   `checksum_mismatch`, `load_failed`, `trap`, `timeout`, `output_invalid`,
///   `k8s_apply_failed`, `policy_rejected`, `unspecified`; see
///   `plugin_events::codes::metric_label`), `reported_error` for an error
///   event the plugin reported, or `version_incompatible`,
///   `dependency_cycle`, `unmet_dependency`. Never a plugin-supplied string.
pub fn record_plugin_error(plugin: &str, error_type: &str) {
    counter!(
        "deckwatch_plugin_errors_total",
        "plugin" => plugin.to_owned(),
        "error_type" => error_type.to_owned(),
    )
    .increment(1);
}

/// Set the gauge of currently loaded (active) plugins.
pub fn set_plugins_loaded(count: f64) {
    gauge!("deckwatch_plugins_loaded").set(count);
}

/// Record application of a plugin-emitted Kubernetes resource.
/// - `kind`: the object's kind, which the plugin chose. Kinds outside
///   [`PLUGIN_K8S_KINDS`] are recorded as `other` so a plugin can't create
///   label values.
/// - `status`: `ok` or `error`
pub fn record_plugin_k8s_resource(kind: &str, status: &str) {
    counter!(
        "deckwatch_plugin_k8s_resources_total",
        "kind" => bounded_plugin_kind(kind),
        "status" => status.to_owned(),
    )
    .increment(1);
}

/// Kinds plugins commonly emit, kept as their own `kind` label value.
pub const PLUGIN_K8S_KINDS: &[&str] = &[
    "ConfigMap",
    "CronJob",
    "Deployment",
    "ExternalSecret",
    "Ingress",
    "Job",
    "NetworkPolicy",
    "PersistentVolumeClaim",
    "PodMonitor",
    "Role",
    "RoleBinding",
    "Secret",
    "SecretProviderClass",
    "Service",
    "ServiceAccount",
    "ServiceMonitor",
];

fn bounded_plugin_kind(kind: &str) -> &'static str {
    PLUGIN_K8S_KINDS
        .iter()
        .find(|k| **k == kind)
        .copied()
        .unwrap_or("other")
}

/// Count one normalized plugin event.
/// - `severity`: `info`, `warning` or `error`
/// - `source`: `plugin` (the plugin reported it) or `host` (deckwatch
///   detected it)
pub fn record_plugin_event(plugin: &str, severity: &'static str, source: &'static str) {
    counter!(
        "deckwatch_plugin_events_total",
        "plugin" => plugin.to_owned(),
        "severity" => severity,
        "source" => source,
    )
    .increment(1);
}

/// Count Kubernetes Events about plugins.
/// - `result`: `published`, `folded` (a repeat within the republish
///   interval), `dropped` (too many in flight) or `error`
pub fn record_plugin_k8s_event(result: &'static str) {
    counter!("deckwatch_plugin_k8s_events_total", "result" => result).increment(1);
}

/// Count plugin events that were dropped instead of being logged or
/// exported.
/// - `reason`: `rate_limited` (per-plugin rate limit), `over_cap` (more than
///   the per-call maximum), `queue_full` (the background sinks fell
///   behind), or `notify_queue_full` (a transition notification dropped
///   because notifications waiting to be sent filled their queue)
pub fn record_plugin_events_dropped(plugin: &str, reason: &'static str, count: u64) {
    counter!(
        "deckwatch_plugin_events_dropped_total",
        "plugin" => plugin.to_owned(),
        "reason" => reason,
    )
    .increment(count);
}

/// Record how long a single plugin reconciliation took for an application.
/// Call from `reconcile_application_plugins` once per application per cycle.
/// - `plugin`: the application name being reconciled (scoping label)
/// - `namespace`: the Kubernetes namespace being reconciled
pub fn record_plugin_reconcile_duration(plugin: &str, namespace: &str, duration_s: f64) {
    histogram!(
        "deckwatch_plugin_reconcile_duration_seconds",
        "plugin" => plugin.to_owned(),
        "namespace" => namespace.to_owned(),
    )
    .record(duration_s);
}

/// Increment the auto-rollback counter. Call once per rollback that fires.
pub fn record_auto_rollback(namespace: &str, deployment: &str) {
    counter!(
        "deckwatch_auto_rollbacks_total",
        "namespace" => namespace.to_owned(),
        "deployment" => deployment.to_owned(),
    )
    .increment(1);
}

/// Record the wall-clock duration of a completed build (success or failure).
/// - `status`: `succeeded` or `failed`
pub fn record_build_duration(namespace: &str, deployment: &str, status: &str, duration_s: f64) {
    histogram!(
        "deckwatch_build_duration_seconds",
        "namespace" => namespace.to_owned(),
        "deployment" => deployment.to_owned(),
        "status" => status.to_owned(),
    )
    .record(duration_s);
}

/// MCP tool calls deckwatch served itself, and calls a guardrail refused.
pub const MCP_TOOL_CALLS: &str = "deckwatch_mcp_tool_calls_total";
/// The duration of the calls in [`MCP_TOOL_CALLS`].
pub const MCP_TOOL_CALL_DURATION: &str = "deckwatch_mcp_tool_call_duration_seconds";
/// MCP tool calls deckwatch handed to mcp-k8s.
pub const MCP_PASSTHROUGH_TOOL_CALLS: &str = "deckwatch_mcp_passthrough_tool_calls_total";
/// The duration of the calls in [`MCP_PASSTHROUGH_TOOL_CALLS`].
pub const MCP_PASSTHROUGH_TOOL_CALL_DURATION: &str =
    "deckwatch_mcp_passthrough_tool_call_duration_seconds";

/// Record an MCP tool call deckwatch served itself (e.g. `create_application`,
/// `exec_pod`), or that a guardrail refused before any tool ran
/// (`status="denied"`, for any tool). Calls deckwatch hands to mcp-k8s go to
/// [`record_mcp_passthrough_tool_call`], so every call is counted exactly once
/// across the two.
pub fn record_mcp_tool_call(tool: &str, status: &str, duration_s: f64) {
    record_tool_call(
        MCP_TOOL_CALLS,
        MCP_TOOL_CALL_DURATION,
        tool,
        status,
        duration_s,
    );
}

/// Record an MCP tool call mcp-k8s ran: an mcp-k8s tool deckwatch forwards,
/// or an intercepted tool deckwatch hands on (`apply_manifest`, and
/// `create_pod`/`update_deployment` when they delegate). `status` is `ok` or
/// `error`.
///
/// These are the calls mcp-k8s counts in its own `mcp_k8s_tool_calls_total`,
/// which mcp-k8s v0.7.0 deprecated. A new metric rather than a label on
/// [`MCP_TOOL_CALLS`]: dashboards that add `deckwatch_mcp_tool_calls_total`
/// and `mcp_k8s_tool_calls_total` (the bundled ones do) would count these
/// calls twice while mcp-k8s still exports its counter.
pub fn record_mcp_passthrough_tool_call(tool: &str, status: &str, duration_s: f64) {
    record_tool_call(
        MCP_PASSTHROUGH_TOOL_CALLS,
        MCP_PASSTHROUGH_TOOL_CALL_DURATION,
        tool,
        status,
        duration_s,
    );
}

fn record_tool_call(
    calls: &'static str,
    duration: &'static str,
    tool: &str,
    status: &str,
    duration_s: f64,
) {
    counter!(
        calls,
        "tool" => tool.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);

    histogram!(duration, "tool" => tool.to_owned()).record(duration_s);
}

// -----------------------------------------------------------------------------
// Frontend metrics ingestion
// -----------------------------------------------------------------------------

/// Payload the browser posts to `/api/frontend-metrics`. Kept small enough
/// to fit in a `navigator.sendBeacon()` blob.
#[derive(Debug, Deserialize)]
pub struct FrontendMetrics {
    /// Session identifier — used only for dedup/log correlation, NOT emitted
    /// as a metric label (would blow up cardinality).
    pub session_id: Option<String>,
    #[serde(default)]
    pub page_views: Vec<PageView>,
    #[serde(default)]
    pub api_calls: Vec<ApiCall>,
    #[serde(default)]
    pub errors: Vec<FrontendError>,
    #[serde(default)]
    pub navigation_timing: Option<NavigationTiming>,
    /// Core Web Vitals samples (LCP, CLS, INP, FCP, TTFB). One entry per
    /// vital per batch — the `web-vitals` library emits a metric at most
    /// once per page-view (INP updates as the user interacts, and the
    /// final value fires on visibilitychange).
    #[serde(default)]
    pub web_vitals: Vec<WebVital>,
}

#[derive(Debug, Deserialize)]
pub struct WebVital {
    /// One of `LCP`, `CLS`, `INP`, `FCP`, `TTFB`. Cased to match the
    /// `web-vitals` library so the label is self-describing.
    pub name: String,
    /// Raw metric value from `web-vitals`. Unit is milliseconds for LCP /
    /// INP / FCP / TTFB and a unitless score for CLS.
    pub value: f64,
    pub route: String,
}

#[derive(Debug, Deserialize)]
pub struct PageView {
    pub route: String,
    #[serde(default = "one")]
    pub count: u64,
}

#[derive(Debug, Deserialize)]
pub struct ApiCall {
    pub path: String,
    pub method: String,
    pub status: u16,
    pub duration_ms: f64,
}

#[derive(Debug, Deserialize)]
pub struct FrontendError {
    /// Coarse bucket: `network`, `api`, `js`, `unhandled_rejection`, etc.
    pub kind: String,
    pub route: String,
}

#[derive(Debug, Deserialize)]
pub struct NavigationTiming {
    pub route: String,
    pub load_time_ms: f64,
}

fn one() -> u64 {
    1
}

// ── label bounding for the unauthenticated /api/frontend-metrics endpoint ──
//
// Every distinct label value becomes a Prometheus series that lives for the
// life of the process and is rendered on every /metrics scrape. The payload
// comes from an unauthenticated client, so unbounded labels let anyone grow
// memory and scrape size without limit. Free-form labels are capped in
// length, charset and number of distinct values; enumerable ones are mapped
// onto fixed sets. Anything else is recorded as "other".

const MAX_FRONTEND_LABEL_LEN: usize = 128;
const MAX_FRONTEND_LABEL_VALUES: usize = 512;
const OTHER_LABEL: &str = "other";

/// Distinct values seen so far for one free-form label.
#[derive(Default)]
struct LabelSet(Mutex<HashSet<String>>);

impl LabelSet {
    fn bound(&self, value: String, max_values: usize) -> String {
        let well_formed = !value.is_empty()
            && value.len() <= MAX_FRONTEND_LABEL_LEN
            && value.bytes().all(|b| b.is_ascii_graphic());
        if !well_formed {
            return OTHER_LABEL.to_string();
        }
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(&value) || seen.len() < max_values {
            seen.insert(value.clone());
            value
        } else {
            OTHER_LABEL.to_string()
        }
    }
}

static FRONTEND_ROUTES: LazyLock<LabelSet> = LazyLock::new(LabelSet::default);
static FRONTEND_API_PATHS: LazyLock<LabelSet> = LazyLock::new(LabelSet::default);
static FRONTEND_ERROR_KINDS: LazyLock<LabelSet> = LazyLock::new(LabelSet::default);

fn route_label(route: String) -> String {
    FRONTEND_ROUTES.bound(route, MAX_FRONTEND_LABEL_VALUES)
}

fn api_path_label(path: String) -> String {
    FRONTEND_API_PATHS.bound(path, MAX_FRONTEND_LABEL_VALUES)
}

fn error_kind_label(kind: String) -> String {
    FRONTEND_ERROR_KINDS.bound(kind, MAX_FRONTEND_LABEL_VALUES)
}

fn method_label(method: &str) -> &'static str {
    match method.to_ascii_uppercase().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => OTHER_LABEL,
    }
}

fn status_class_label(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => OTHER_LABEL,
    }
}

/// Metric names emitted by the `web-vitals` library.
fn web_vital_label(name: &str) -> &'static str {
    match name {
        "CLS" => "CLS",
        "FCP" => "FCP",
        "FID" => "FID",
        "INP" => "INP",
        "LCP" => "LCP",
        "TTFB" => "TTFB",
        _ => OTHER_LABEL,
    }
}

/// POST /api/frontend-metrics — accepts a batch, records into the same
/// prometheus recorder as the backend. Silently drops malformed batches
/// (return 204 either way) so the browser never retries with a bad payload.
pub async fn ingest_frontend_metrics(
    _state: State<crate::state::AppState>,
    payload: Result<Json<FrontendMetrics>, axum::extract::rejection::JsonRejection>,
) -> StatusCode {
    let Ok(Json(metrics)) = payload else {
        return StatusCode::NO_CONTENT;
    };

    for pv in metrics.page_views {
        counter!("deckwatch_frontend_page_views_total", "route" => route_label(pv.route))
            .increment(pv.count);
    }

    for call in metrics.api_calls {
        let path = api_path_label(call.path);
        let method = method_label(&call.method);
        counter!(
            "deckwatch_frontend_api_calls_total",
            "path" => path.clone(),
            "method" => method,
            "status" => status_class_label(call.status),
        )
        .increment(1);

        histogram!(
            "deckwatch_frontend_api_call_duration_seconds",
            "path" => path,
            "method" => method,
        )
        .record(call.duration_ms / 1000.0);
    }

    for err in metrics.errors {
        counter!(
            "deckwatch_frontend_errors_total",
            "kind" => error_kind_label(err.kind),
            "route" => route_label(err.route),
        )
        .increment(1);
    }

    if let Some(nav) = metrics.navigation_timing {
        histogram!(
            "deckwatch_frontend_page_load_seconds",
            "route" => route_label(nav.route),
        )
        .record(nav.load_time_ms / 1000.0);
    }

    for vital in metrics.web_vitals {
        // CLS is unitless; the rest are milliseconds. Convert to seconds
        // for the ms-family so histogram buckets match the rest of the
        // frontend timing metrics.
        let name = web_vital_label(&vital.name);
        let value = match name {
            "CLS" => vital.value,
            _ => vital.value / 1000.0,
        };
        histogram!(
            "deckwatch_frontend_web_vital",
            "name" => name,
            "route" => route_label(vital.route),
        )
        .record(value);
    }

    StatusCode::NO_CONTENT
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod metrics_tests;
