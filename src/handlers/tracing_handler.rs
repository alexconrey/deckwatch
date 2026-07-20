//! Distributed tracing query proxy for the tracing addon UI.
//!
//! Deckwatch does not itself store spans -- this handler proxies a curated
//! set of trace queries to whatever backend the operator configured
//! (Grafana Tempo by default, Jaeger optional). See `docs/TRACING.md` sec 6
//! for the design rationale and sec 4 for how the backend gets deployed.
//!
//! Safety posture (mirrors `handlers::prometheus_query`):
//! - The service name is interpolated with a strict char allowlist so a
//!   hostile `service` param cannot break out of Tempo's search-tag matcher.
//! - The HTTP client is capped at a 10s timeout; the response limit is
//!   `MAX_TRACES_PER_QUERY` regardless of what the caller requests.
//! - The caller must be inside a namespace they are already allowed to
//!   read -- the namespace path segment goes through the standard allow-list
//!   check.
//!
//! Graceful degrade:
//! - `settings.tracing.query_url` unset -> 200 with `{ traces: [],
//!   unavailable_reason: "tracing not configured" }`. Same pattern as
//!   `handlers::prometheus_query` for missing Prometheus.
//! - Backend unreachable / non-2xx -> 200 with a human-readable reason. The
//!   UI renders a callout rather than a red banner so operators know it is
//!   an infra issue rather than an app error.
//!
//! The response shape is deliberately backend-agnostic -- it exposes only
//! the trace summary fields the UI needs (id, duration, span count, root
//! span name, timestamp). Adding Zipkin or a managed backend later means
//! wiring one more match arm in `fetch_traces`, not changing the UI.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::settings::{DeckwatchSettings, TracingSettings};
use crate::metrics::K8sTimer;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Public request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListTracesQuery {
    /// Service name to search for. Matches `service.name` in OTLP semantic
    /// conventions and is what the collector sidecar sets to the deployment
    /// name by default (see `handlers::addons::catalog()`).
    pub service: String,
    /// Cap on returned trace summaries. Backend-side hard limit is
    /// `MAX_TRACES_PER_QUERY`; user values above that are silently clamped.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TraceSummary {
    /// Backend trace ID as a hex string, suitable for deep-linking into the
    /// backend UI. Format is backend-defined (Tempo/Jaeger both use hex).
    pub trace_id: String,
    /// Root span operation name if the backend reports one, else the string
    /// "unknown". Never null so the UI does not need a nullish guard.
    pub root_span_name: String,
    /// Total wall-clock duration in milliseconds. Backends report
    /// microseconds; we normalize here so the frontend uses one unit.
    pub duration_ms: u64,
    /// Number of spans in the trace, when reported. `0` means the backend
    /// did not include a span count -- treat as "unknown", not "empty".
    pub span_count: u32,
    /// Trace start time as milliseconds since epoch. Frontend formats to
    /// local time; keeps parity with the metrics panel timestamp field.
    pub timestamp_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct ListTracesResponse {
    pub traces: Vec<TraceSummary>,
    /// `Some(_)` when the query could not be served (tracing not configured,
    /// backend unreachable, backend returned an error). `traces` is empty in
    /// that case. Matches `RangeResponse.unavailable_reason` semantics from
    /// `handlers::prometheus_query`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Public deep-link base for the backend UI, echoed here so the frontend
    /// does not need to re-read the settings endpoint on every render. Empty
    /// string when unconfigured -- the UI hides the "Open in UI" affordance.
    pub ui_url: String,
    /// Which backend kind the settings claim we are talking to (`tempo` or
    /// `jaeger`). Drives the trace-URL template used by the frontend.
    pub backend_kind: String,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Hard cap on how many trace summaries we will pull back per request.
/// Tempo's jaeger-compat search caps at 20 by default; keeping the same
/// value keeps the UI's initial paint fast and mirrors the design doc.
const MAX_TRACES_PER_QUERY: u32 = 20;

/// Trace-backend query timeout. Kept short -- the UI polls this, so a
/// backend that is slow to respond should surface as unavailable rather
/// than blocking the request path.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn list_traces(
    State(state): State<AppState>,
    Path((ns, _deployment)): Path<(String, String)>,
    Query(params): Query<ListTracesQuery>,
) -> Result<Json<ListTracesResponse>, AppError> {
    if !state.is_namespace_allowed(&ns) {
        return Err(AppError::NamespaceNotAllowed(ns));
    }
    validate_service_name(&params.service)
        .map_err(|_| AppError::BadRequest("service contains invalid characters".to_string()))?;

    let settings = load_tracing_settings(&state).await;

    let backend_kind = settings.backend_kind.clone();
    let ui_url = settings.ui_url.clone();

    let Some(query_url) = normalized_query_url(&settings) else {
        return Ok(Json(ListTracesResponse {
            traces: Vec::new(),
            unavailable_reason: Some(
                "tracing not configured (set tracing.query_url in deckwatch settings)".to_string(),
            ),
            ui_url,
            backend_kind,
        }));
    };

    let limit = params
        .limit
        .unwrap_or(MAX_TRACES_PER_QUERY)
        .min(MAX_TRACES_PER_QUERY)
        .max(1);

    let http = reqwest::Client::builder()
        .timeout(QUERY_TIMEOUT)
        .build()
        .map_err(|e| AppError::BadRequest(format!("failed to build http client: {e}")))?;

    let kind = BackendKind::parse(&backend_kind);
    match fetch_traces(&http, kind, &query_url, &params.service, limit).await {
        Ok(traces) => Ok(Json(ListTracesResponse {
            traces,
            unavailable_reason: None,
            ui_url,
            backend_kind,
        })),
        Err(e) => Ok(Json(ListTracesResponse {
            traces: Vec::new(),
            unavailable_reason: Some(e),
            ui_url,
            backend_kind,
        })),
    }
}

// ---------------------------------------------------------------------------
// Settings discovery
// ---------------------------------------------------------------------------

/// Read the tracing settings block from the deckwatch settings ConfigMap.
/// A missing block, an unreadable CM, or a parse failure all resolve to the
/// default settings -- mirrors the `settings::get_settings` pattern so an
/// unbootstrapped install still returns 200 with `unavailable_reason` set.
async fn load_tracing_settings(state: &AppState) -> TracingSettings {
    let Ok(api) = state.configmaps_api(&state.settings_namespace) else {
        return TracingSettings::default();
    };
    let t = K8sTimer::new("configmaps", "get");
    let cm = api.get(&state.settings_configmap_name).await;
    let ok = cm.is_ok() || matches!(&cm, Err(kube::Error::Api(e)) if e.code == 404);
    t.finish(ok);
    let Ok(cm) = cm else {
        return TracingSettings::default();
    };
    let parsed: Option<DeckwatchSettings> = cm
        .data
        .as_ref()
        .and_then(|d| d.get("settings"))
        .and_then(|s| serde_json::from_str::<DeckwatchSettings>(s).ok());
    parsed.and_then(|s| s.tracing).unwrap_or_default()
}

fn normalized_query_url(settings: &TracingSettings) -> Option<String> {
    let raw = settings.query_url.trim();
    if raw.is_empty() {
        return None;
    }
    Some(raw.trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------------------
// Backend fan-out
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
enum BackendKind {
    Tempo,
    Jaeger,
}

impl BackendKind {
    fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "jaeger" => Self::Jaeger,
            // Tempo is the deckwatch default per docs/TRACING.md sec 3, and
            // exposes a jaeger-compat query API -- so a mistyped/blank value
            // falls through to the Tempo path (which is what the bundled
            // sub-chart deploys).
            _ => Self::Tempo,
        }
    }
}

async fn fetch_traces(
    http: &reqwest::Client,
    kind: BackendKind,
    base_url: &str,
    service: &str,
    limit: u32,
) -> Result<Vec<TraceSummary>, String> {
    match kind {
        // Both Tempo and Jaeger expose a `/api/traces?service=` endpoint
        // that returns Jaeger-format JSON. Tempo's native TraceQL API is
        // richer but only Tempo speaks it -- picking the compat path
        // keeps the proxy one code path across backends.
        BackendKind::Tempo | BackendKind::Jaeger => {
            fetch_jaeger_compat(http, base_url, service, limit).await
        }
    }
}

// ---------------------------------------------------------------------------
// Jaeger-compat HTTP client (works for Jaeger native and Tempo's shim)
// ---------------------------------------------------------------------------

async fn fetch_jaeger_compat(
    http: &reqwest::Client,
    base_url: &str,
    service: &str,
    limit: u32,
) -> Result<Vec<TraceSummary>, String> {
    let url = format!("{base_url}/api/traces");
    let limit_s = limit.to_string();
    let query = [("service", service), ("limit", limit_s.as_str())];

    let timer = K8sTimer::new("tracing_query", "list");
    let res = http.get(&url).query(&query).send().await;
    let response = match res {
        Ok(r) => r,
        Err(e) => {
            timer.finish(false);
            return Err(format!(
                "failed to reach tracing backend at {base_url}: {e}"
            ));
        }
    };
    if !response.status().is_success() {
        let code = response.status();
        timer.finish(false);
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "tracing backend returned HTTP {code}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let parsed: JaegerResponse = match response.json().await {
        Ok(p) => p,
        Err(e) => {
            timer.finish(false);
            return Err(format!("failed to parse tracing response: {e}"));
        }
    };
    timer.finish(true);

    Ok(parsed
        .data
        .into_iter()
        .take(limit as usize)
        .map(summarize_jaeger_trace)
        .collect())
}

fn summarize_jaeger_trace(t: JaegerTrace) -> TraceSummary {
    // Root span = the first span with no `references` list, or fall back to
    // the earliest-starting span if the backend did not set references (some
    // Tempo versions emit references only for child spans, which means the
    // root has an empty vec -- the same predicate catches both).
    let root = t
        .spans
        .iter()
        .find(|s| s.references.is_empty())
        .or_else(|| t.spans.iter().min_by_key(|s| s.start_time));

    let (root_span_name, start_us, duration_us) = match root {
        Some(s) => (s.operation_name.clone(), s.start_time, s.duration),
        None => ("unknown".to_string(), 0u64, 0u64),
    };

    TraceSummary {
        trace_id: t.trace_id,
        root_span_name: if root_span_name.is_empty() {
            "unknown".to_string()
        } else {
            root_span_name
        },
        duration_ms: duration_us / 1_000,
        span_count: u32::try_from(t.spans.len()).unwrap_or(u32::MAX),
        timestamp_ms: i64::try_from(start_us / 1_000).unwrap_or(0),
    }
}

#[derive(Debug, Deserialize)]
struct JaegerResponse {
    #[serde(default)]
    data: Vec<JaegerTrace>,
}

#[derive(Debug, Deserialize)]
struct JaegerTrace {
    #[serde(rename = "traceID", default)]
    trace_id: String,
    #[serde(default)]
    spans: Vec<JaegerSpan>,
}

#[derive(Debug, Deserialize)]
struct JaegerSpan {
    #[serde(rename = "operationName", default)]
    operation_name: String,
    // Jaeger/Tempo use microseconds since epoch here.
    #[serde(rename = "startTime", default)]
    start_time: u64,
    // Duration is in microseconds.
    #[serde(default)]
    duration: u64,
    #[serde(default)]
    references: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// Same char allowlist used for the prometheus_query selectors. Service names
/// generally follow the OTLP `service.name` convention (letters, digits,
/// dashes, dots, underscores) which is a subset of what we accept for K8s
/// object names -- so a valid service name is a valid selector too.
fn validate_service_name(s: &str) -> Result<(), ()> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_accepts_typical_workload_names() {
        assert!(validate_service_name("checkout-api").is_ok());
        assert!(validate_service_name("my.svc_v2").is_ok());
    }

    #[test]
    fn service_name_rejects_injection_attempts() {
        assert!(validate_service_name("").is_err());
        assert!(validate_service_name("bad\"} or 1=1").is_err());
        assert!(validate_service_name("has space").is_err());
        assert!(validate_service_name("foo,bar").is_err());
    }

    #[test]
    fn backend_kind_defaults_to_tempo() {
        assert!(matches!(BackendKind::parse(""), BackendKind::Tempo));
        assert!(matches!(BackendKind::parse("unknown"), BackendKind::Tempo));
        assert!(matches!(BackendKind::parse("Tempo"), BackendKind::Tempo));
        assert!(matches!(BackendKind::parse("JAEGER"), BackendKind::Jaeger));
    }

    #[test]
    fn normalized_query_url_strips_trailing_slash() {
        let mut s = TracingSettings::default();
        s.query_url = "http://tempo:3200/".to_string();
        assert_eq!(
            normalized_query_url(&s).as_deref(),
            Some("http://tempo:3200")
        );
    }

    #[test]
    fn normalized_query_url_empty_maps_to_none() {
        let s = TracingSettings::default();
        assert!(normalized_query_url(&s).is_none());
    }

    #[test]
    fn summarize_prefers_referenceless_root() {
        let t = JaegerTrace {
            trace_id: "abc".to_string(),
            spans: vec![
                JaegerSpan {
                    operation_name: "child".to_string(),
                    start_time: 2_000_000,
                    duration: 1_000,
                    references: vec![serde_json::json!({"refType":"CHILD_OF"})],
                },
                JaegerSpan {
                    operation_name: "root".to_string(),
                    start_time: 1_000_000,
                    duration: 5_000,
                    references: vec![],
                },
            ],
        };
        let s = summarize_jaeger_trace(t);
        assert_eq!(s.root_span_name, "root");
        assert_eq!(s.duration_ms, 5);
        assert_eq!(s.span_count, 2);
        assert_eq!(s.timestamp_ms, 1_000);
    }

    #[test]
    fn summarize_falls_back_to_earliest_span_when_all_have_references() {
        let t = JaegerTrace {
            trace_id: "abc".to_string(),
            spans: vec![
                JaegerSpan {
                    operation_name: "b".to_string(),
                    start_time: 2_000_000,
                    duration: 1_000,
                    references: vec![serde_json::json!({})],
                },
                JaegerSpan {
                    operation_name: "a".to_string(),
                    start_time: 1_000_000,
                    duration: 1_000,
                    references: vec![serde_json::json!({})],
                },
            ],
        };
        let s = summarize_jaeger_trace(t);
        assert_eq!(s.root_span_name, "a");
    }

    #[test]
    fn summarize_never_returns_null_root_name() {
        let t = JaegerTrace {
            trace_id: "abc".to_string(),
            spans: vec![JaegerSpan {
                operation_name: "".to_string(),
                start_time: 0,
                duration: 0,
                references: vec![],
            }],
        };
        let s = summarize_jaeger_trace(t);
        assert_eq!(s.root_span_name, "unknown");
    }
}

#[cfg(test)]
#[path = "../handlers_tracing_handler_tests.rs"]
mod handlers_tracing_handler_tests;
