#![allow(dead_code, unused_imports)]
use std::sync::OnceLock;
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
///
/// Latency buckets are geared toward interactive dashboard traffic (a few ms
/// up to a couple seconds) plus a long tail for slow k8s api calls / log
/// history fetches.
pub fn init() {
    let latency_buckets: &[f64] = &[
        0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    let handle = PrometheusBuilder::new()
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Suffix("_duration_seconds".to_string()),
            latency_buckets,
        )
        .expect("configure latency buckets")
        .install_recorder()
        .expect("install prometheus recorder");

    if PROM_HANDLE.set(handle).is_err() {
        tracing::warn!("metrics::init called more than once");
    }
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

/// Record a gitops build result. Call once per build completion.
pub fn record_gitops_build(namespace: &str, status: &str) {
    counter!(
        "deckwatch_gitops_builds_total",
        "namespace" => namespace.to_owned(),
        "status" => status.to_owned(),
    )
    .increment(1);
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
        counter!("deckwatch_frontend_page_views_total", "route" => pv.route).increment(pv.count);
    }

    for call in metrics.api_calls {
        let status_class = format!("{}xx", call.status / 100);
        counter!(
            "deckwatch_frontend_api_calls_total",
            "path" => call.path.clone(),
            "method" => call.method.clone(),
            "status" => status_class,
        )
        .increment(1);

        histogram!(
            "deckwatch_frontend_api_call_duration_seconds",
            "path" => call.path,
            "method" => call.method,
        )
        .record(call.duration_ms / 1000.0);
    }

    for err in metrics.errors {
        counter!(
            "deckwatch_frontend_errors_total",
            "kind" => err.kind,
            "route" => err.route,
        )
        .increment(1);
    }

    if let Some(nav) = metrics.navigation_timing {
        histogram!(
            "deckwatch_frontend_page_load_seconds",
            "route" => nav.route,
        )
        .record(nav.load_time_ms / 1000.0);
    }

    for vital in metrics.web_vitals {
        // CLS is unitless; the rest are milliseconds. Convert to seconds
        // for the ms-family so histogram buckets match the rest of the
        // frontend timing metrics.
        let value = match vital.name.as_str() {
            "CLS" => vital.value,
            _ => vital.value / 1000.0,
        };
        histogram!(
            "deckwatch_frontend_web_vital",
            "name" => vital.name,
            "route" => vital.route,
        )
        .record(value);
    }

    StatusCode::NO_CONTENT
}
