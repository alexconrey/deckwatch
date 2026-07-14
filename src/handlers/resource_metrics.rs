//! Handlers for pod- and node-level resource usage metrics sourced from the
//! Kubernetes metrics-server (`metrics.k8s.io/v1beta1`).
//!
//! metrics-server exposes *point-in-time* CPU and memory readings — the last
//! kubelet stats window (typically 15-60 s). There is no history: for
//! sparklines the frontend polls and accumulates client-side, or (Phase 2) a
//! backend ring buffer keeps N samples per pod.
//!
//! Design notes:
//! - We access the metrics API via `Client::request` because kube-rs does not
//!   ship first-class typed bindings for `metrics.k8s.io/v1beta1` and the
//!   `DynamicObject` route loses the strongly-typed `usage` map. A small,
//!   local `serde` model is cheaper than fighting the dynamic layer.
//! - If metrics-server is not installed, the API returns 404. We surface this
//!   as an empty list + hint string so the UI can render an
//!   "install metrics-server" callout instead of a red error banner.
//! - Quantities (e.g. `"250m"`, `"1Gi"`) are returned as raw strings AND
//!   parsed to canonical units (millicores, bytes) so the frontend does not
//!   need to reimplement K8s quantity parsing.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::Request;
use axum::Json;
use kube::api::ListParams;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

// -----------------------------------------------------------------------------
// Wire types — mirrors metrics.k8s.io/v1beta1
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MetricsList<T> {
    items: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct MetricsMeta {
    name: String,
    #[serde(default)]
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPodMetrics {
    metadata: MetricsMeta,
    timestamp: String,
    window: String,
    #[serde(default)]
    containers: Vec<RawContainerMetrics>,
}

#[derive(Debug, Deserialize)]
struct RawContainerMetrics {
    name: String,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct RawNodeMetrics {
    metadata: MetricsMeta,
    timestamp: String,
    window: String,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct Usage {
    cpu: String,
    memory: String,
}

// -----------------------------------------------------------------------------
// Response types — what the frontend gets
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ContainerUsage {
    pub name: String,
    /// Raw K8s quantity string (e.g. `"250m"`).
    pub cpu: String,
    /// CPU normalized to millicores. `250m` -> 250, `1` -> 1000.
    pub cpu_millicores: u64,
    /// Raw K8s quantity string (e.g. `"128Mi"`).
    pub memory: String,
    /// Memory normalized to bytes.
    pub memory_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct PodUsage {
    pub name: String,
    pub namespace: String,
    /// ISO-8601 timestamp when metrics-server sampled this pod.
    pub timestamp: String,
    /// Collection window (e.g. `"30s"`).
    pub window: String,
    pub containers: Vec<ContainerUsage>,
    /// Sum across all containers, for at-a-glance sparklines.
    pub total_cpu_millicores: u64,
    pub total_memory_bytes: u64,
    /// Aggregate restart count across all containers in the pod. Sampled from
    /// the Pod object at the same tick as the metrics-server usage read, so
    /// the frontend can plot restarts on the same time axis as CPU/memory.
    /// `None` when the Pod object could not be resolved (rare — usually a
    /// permission or RBAC scope mismatch between metrics-server and pods API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_count: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct PodMetricsResponse {
    pub pods: Vec<PodUsage>,
    /// When metrics-server is unreachable, we return an empty list plus this
    /// hint so the UI can render an install-me panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NodeUsage {
    pub name: String,
    pub timestamp: String,
    pub window: String,
    pub cpu: String,
    pub cpu_millicores: u64,
    pub memory: String,
    pub memory_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct NodeMetricsResponse {
    pub nodes: Vec<NodeUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

// -----------------------------------------------------------------------------
// Handlers
// -----------------------------------------------------------------------------

/// GET /api/namespaces/{ns}/pods/metrics
///
/// Optional query `?label_selector=app=foo` to scope to one deployment.
#[derive(Debug, Deserialize, Default)]
pub struct PodMetricsQuery {
    #[serde(default)]
    pub label_selector: Option<String>,
}

pub async fn list_pod_metrics(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Query(query): Query<PodMetricsQuery>,
) -> Result<Json<PodMetricsResponse>, AppError> {
    if !state.is_namespace_allowed(&ns) {
        return Err(AppError::NamespaceNotAllowed(ns));
    }

    let mut uri = format!("/apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods");
    if let Some(sel) = query.label_selector.as_deref().filter(|s| !s.is_empty()) {
        uri.push_str("?labelSelector=");
        uri.push_str(&urlencode(sel));
    }

    let timer = K8sTimer::new("podmetrics", "list");
    let req = Request::builder()
        .uri(uri)
        .body(Vec::new())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let result: Result<MetricsList<RawPodMetrics>, kube::Error> =
        state.kube_client.request(req).await;

    match result {
        Ok(list) => {
            timer.finish(true);
            let restarts = fetch_pod_restart_counts(&state, &ns, query.label_selector.as_deref())
                .await
                .unwrap_or_default();
            let pods = list
                .items
                .into_iter()
                .map(|raw| {
                    let restart_count = restarts.get(&raw.metadata.name).copied();
                    to_pod_usage(raw, restart_count)
                })
                .collect();
            Ok(Json(PodMetricsResponse {
                pods,
                unavailable_reason: None,
            }))
        }
        Err(err) => {
            timer.finish(false);
            if let Some(reason) = metrics_unavailable_reason(&err) {
                Ok(Json(PodMetricsResponse {
                    pods: Vec::new(),
                    unavailable_reason: Some(reason),
                }))
            } else {
                Err(AppError::Kube(err))
            }
        }
    }
}

/// Sum restart_count across all containers per pod. Runs *after* metrics-server
/// succeeds so a broken metrics-server does not force a pods-API call, and a
/// missing pods permission returns `None` rather than failing the metrics
/// response (restart trends are a nice-to-have, not the primary payload).
async fn fetch_pod_restart_counts(
    state: &AppState,
    ns: &str,
    label_selector: Option<&str>,
) -> Option<HashMap<String, i32>> {
    let pods_api = state.pods_api(ns).ok()?;
    let mut lp = ListParams::default();
    if let Some(sel) = label_selector.filter(|s| !s.is_empty()) {
        lp = lp.labels(sel);
    }
    let timer = K8sTimer::new("pods", "list");
    let pods = pods_api.list(&lp).await;
    timer.finish(pods.is_ok());
    let pods = pods.ok()?;

    let mut out: HashMap<String, i32> = HashMap::with_capacity(pods.items.len());
    for pod in &pods.items {
        let Some(name) = pod.metadata.name.clone() else {
            continue;
        };
        let total: i32 = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .map(|cs| cs.iter().map(|c| c.restart_count).sum())
            .unwrap_or(0);
        out.insert(name, total);
    }
    Some(out)
}

/// GET /api/nodes/metrics
pub async fn list_node_metrics(
    State(state): State<AppState>,
) -> Result<Json<NodeMetricsResponse>, AppError> {
    let timer = K8sTimer::new("nodemetrics", "list");
    let req = Request::builder()
        .uri("/apis/metrics.k8s.io/v1beta1/nodes")
        .body(Vec::new())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let result: Result<MetricsList<RawNodeMetrics>, kube::Error> =
        state.kube_client.request(req).await;

    match result {
        Ok(list) => {
            timer.finish(true);
            let nodes = list.items.into_iter().map(to_node_usage).collect();
            Ok(Json(NodeMetricsResponse {
                nodes,
                unavailable_reason: None,
            }))
        }
        Err(err) => {
            timer.finish(false);
            if let Some(reason) = metrics_unavailable_reason(&err) {
                Ok(Json(NodeMetricsResponse {
                    nodes: Vec::new(),
                    unavailable_reason: Some(reason),
                }))
            } else {
                Err(AppError::Kube(err))
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Conversion helpers
// -----------------------------------------------------------------------------

fn to_pod_usage(raw: RawPodMetrics, restart_count: Option<i32>) -> PodUsage {
    let containers: Vec<ContainerUsage> = raw
        .containers
        .into_iter()
        .map(|c| ContainerUsage {
            cpu_millicores: parse_cpu_millicores(&c.usage.cpu),
            memory_bytes: parse_memory_bytes(&c.usage.memory),
            name: c.name,
            cpu: c.usage.cpu,
            memory: c.usage.memory,
        })
        .collect();

    let total_cpu_millicores = containers.iter().map(|c| c.cpu_millicores).sum();
    let total_memory_bytes = containers.iter().map(|c| c.memory_bytes).sum();

    PodUsage {
        name: raw.metadata.name,
        namespace: raw.metadata.namespace.unwrap_or_default(),
        timestamp: raw.timestamp,
        window: raw.window,
        containers,
        total_cpu_millicores,
        total_memory_bytes,
        restart_count,
    }
}

fn to_node_usage(raw: RawNodeMetrics) -> NodeUsage {
    NodeUsage {
        name: raw.metadata.name,
        timestamp: raw.timestamp,
        window: raw.window,
        cpu_millicores: parse_cpu_millicores(&raw.usage.cpu),
        memory_bytes: parse_memory_bytes(&raw.usage.memory),
        cpu: raw.usage.cpu,
        memory: raw.usage.memory,
    }
}

/// Detect the "metrics-server not installed / not ready" case and surface a
/// friendly message instead of a 502.
///
/// - metrics-server not installed  -> 404 NotFound
/// - APIService present but backend unavailable -> 503 ServiceUnavailable
fn metrics_unavailable_reason(err: &kube::Error) -> Option<String> {
    if let kube::Error::Api(api_err) = err {
        match api_err.code {
            404 => Some(
                "metrics-server does not appear to be installed in this \
                 cluster. Install it to see CPU / memory usage."
                    .to_string(),
            ),
            503 => Some(
                "metrics-server is installed but not ready. Give it 60s \
                 after startup, then retry."
                    .to_string(),
            ),
            _ => None,
        }
    } else {
        None
    }
}

/// Parse a K8s CPU quantity into millicores.
///
/// - `"250m"`        -> 250
/// - `"1"` / `"2"`   -> 1000 / 2000
/// - `"0.5"`         -> 500
/// - `"123456789n"`  -> 123   (metrics-server usually emits nanocores)
fn parse_cpu_millicores(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    if let Some(num) = s.strip_suffix('m') {
        return num.parse::<f64>().map(|v| v.round() as u64).unwrap_or(0);
    }
    if let Some(num) = s.strip_suffix('u') {
        return num
            .parse::<f64>()
            .map(|v| (v / 1000.0).round() as u64)
            .unwrap_or(0);
    }
    if let Some(num) = s.strip_suffix('n') {
        return num
            .parse::<f64>()
            .map(|v| (v / 1_000_000.0).round() as u64)
            .unwrap_or(0);
    }
    s.parse::<f64>()
        .map(|v| (v * 1000.0).round() as u64)
        .unwrap_or(0)
}

/// Parse a K8s memory quantity into bytes. Handles binary (Ki/Mi/Gi/Ti/Pi)
/// and decimal (k/M/G/T/P) suffixes plus bare byte counts.
fn parse_memory_bytes(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    const K: u64 = 1_000;
    const KI: u64 = 1_024;

    // Two-char binary suffixes before single-char decimals — else "Mi" would match "M".
    let (num_str, multiplier): (&str, u64) = if let Some(n) = s.strip_suffix("Ki") {
        (n, KI)
    } else if let Some(n) = s.strip_suffix("Mi") {
        (n, KI.pow(2))
    } else if let Some(n) = s.strip_suffix("Gi") {
        (n, KI.pow(3))
    } else if let Some(n) = s.strip_suffix("Ti") {
        (n, KI.pow(4))
    } else if let Some(n) = s.strip_suffix("Pi") {
        (n, KI.pow(5))
    } else if let Some(n) = s.strip_suffix('k') {
        (n, K)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, K.pow(2))
    } else if let Some(n) = s.strip_suffix('G') {
        (n, K.pow(3))
    } else if let Some(n) = s.strip_suffix('T') {
        (n, K.pow(4))
    } else if let Some(n) = s.strip_suffix('P') {
        (n, K.pow(5))
    } else {
        (s, 1)
    };

    num_str
        .parse::<f64>()
        .map(|v| (v * multiplier as f64) as u64)
        .unwrap_or(0)
}

/// Bare-bones percent-encoder good enough for K8s label selectors
/// (`app=foo,env=prod`). Avoids pulling in the `urlencoding` crate.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'=' | b',' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_millicores_millisuffix() {
        assert_eq!(parse_cpu_millicores("250m"), 250);
        assert_eq!(parse_cpu_millicores("1500m"), 1500);
    }

    #[test]
    fn cpu_millicores_bare_cores() {
        assert_eq!(parse_cpu_millicores("1"), 1000);
        assert_eq!(parse_cpu_millicores("0.5"), 500);
        assert_eq!(parse_cpu_millicores("2"), 2000);
    }

    #[test]
    fn cpu_millicores_nanocores() {
        assert_eq!(parse_cpu_millicores("123456789n"), 123);
    }

    #[test]
    fn cpu_millicores_garbage() {
        assert_eq!(parse_cpu_millicores(""), 0);
        assert_eq!(parse_cpu_millicores("garbage"), 0);
    }

    #[test]
    fn memory_bytes_binary_suffixes() {
        assert_eq!(parse_memory_bytes("1Ki"), 1024);
        assert_eq!(parse_memory_bytes("1Mi"), 1024 * 1024);
        assert_eq!(parse_memory_bytes("128Mi"), 128 * 1024 * 1024);
        assert_eq!(parse_memory_bytes("1Gi"), 1024u64.pow(3));
    }

    #[test]
    fn memory_bytes_decimal_suffixes() {
        assert_eq!(parse_memory_bytes("1k"), 1_000);
        assert_eq!(parse_memory_bytes("1M"), 1_000_000);
        assert_eq!(parse_memory_bytes("1G"), 1_000_000_000);
    }

    #[test]
    fn memory_bytes_bare_and_empty() {
        assert_eq!(parse_memory_bytes("1024"), 1024);
        assert_eq!(parse_memory_bytes(""), 0);
    }

    #[test]
    fn urlencode_preserves_label_selector_chars() {
        assert_eq!(urlencode("app=foo,env=prod"), "app=foo,env=prod");
        assert_eq!(urlencode("app in (a, b)"), "app%20in%20%28a%2C%20b%29");
    }
}
