//! MCP telemetry through the real `/mcp` handler: one span, one duration
//! metric event and one log line per request, for deckwatch's own tools,
//! the mcp-k8s tools it intercepts and the ones it forwards; the client from
//! `initialize`; bounded metric labels; and the canary test proving no
//! argument value of a deckwatch tool reaches telemetry unless mcp-k8s's
//! allowlist lets it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use axum::response::IntoResponse;
use mcp_k8s::telemetry::{semconv, ALLOWED_ARGUMENTS};
use serde_json::{json, Value};

use super::tests::build_test_state;
use super::*;
use crate::handlers::mcp_telemetry::{DECKWATCH_TOOLS, SESSION_HEADER};
use crate::test_tracing::{capture, Captured, CapturedEvent, CapturedSpan};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// POST one JSON-RPC message to `/mcp`, as axum calls the handler.
pub(super) async fn post(
    state: &AppState,
    headers: HeaderMap,
    message: Value,
) -> (HeaderMap, Value) {
    let request: JsonRpcRequest = serde_json::from_value(message).expect("JSON-RPC request");
    let response = handle_mcp(State(state.clone()), headers, Json(request))
        .await
        .into_response();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON body")
    };
    (headers, body)
}

pub(super) fn call(tool: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

pub(super) fn message(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
}

/// The telemetry of one request.
struct Request<'a> {
    span: &'a CapturedSpan,
    metric: &'a CapturedEvent,
    log: &'a CapturedEvent,
}

impl Request<'_> {
    fn span_field(&self, name: &str) -> Option<&str> {
        self.span.fields.get(name).map(String::as_str)
    }

    fn metric_field(&self, name: &str) -> Option<&str> {
        self.metric.fields.get(name).map(String::as_str)
    }
}

/// Exactly one request span, one metric event and one log event, the events
/// inside the span.
fn one_request<'a>(captured: &'a Captured, what: &str) -> Request<'a> {
    let spans = captured.spans_named(semconv::TARGET, semconv::SERVER_SPAN_NAME);
    let metrics = captured.events_on(semconv::METRICS_TARGET);
    let logs = captured.events_on(semconv::TARGET);
    assert_eq!(
        (spans.len(), metrics.len(), logs.len()),
        (1, 1, 1),
        "{what}: spans, metric events, log events: {captured:#?}"
    );
    let span_index = captured
        .spans
        .iter()
        .position(|s| std::ptr::eq(s, spans[0]))
        .unwrap();
    assert_eq!(
        metrics[0].span,
        Some(span_index),
        "{what}: metric event span"
    );
    assert_eq!(logs[0].span, Some(span_index), "{what}: log event span");
    for key in metrics[0].fields.keys() {
        assert!(
            key == semconv::HISTOGRAM_MCP_SERVER_OPERATION_DURATION
                || semconv::METRIC_ATTRIBUTES.contains(&key.as_str()),
            "{what}: metric event carries `{key}`"
        );
    }
    Request {
        span: spans[0],
        metric: metrics[0],
        log: logs[0],
    }
}

/// A Kubernetes API that answers every request with a 404 Status whose
/// message echoes the request URL and body, as real API servers echo names
/// and invalid values. Telemetry must never record error text.
pub(super) fn echo_client() -> kube::Client {
    let service = tower::service_fn(|req: axum::http::Request<kube::client::Body>| async move {
        let uri = req.uri().to_string();
        let body = req.into_body().collect_bytes().await.unwrap_or_default();
        let status = json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {},
            "status": "Failure", "reason": "NotFound", "code": 404,
            "message": format!("not found: {uri} {}", String::from_utf8_lossy(&body)),
        });
        Ok::<_, std::convert::Infallible>(
            axum::http::Response::builder()
                .status(404)
                .header("content-type", "application/json")
                .body(kube::client::Body::from(
                    serde_json::to_vec(&status).unwrap(),
                ))
                .unwrap(),
        )
    });
    kube::Client::new(service, "default")
}

async fn echo_state() -> AppState {
    let mut state = build_test_state().await;
    state.kube_client = echo_client();
    state
}

// ---------------------------------------------------------------------------
// One span, one metric point, one log line
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tool_call_is_one_span_one_metric_event_one_log_line() {
    let state = echo_state().await;
    let manifest =
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: prod\n";
    // (tool, arguments, action, served by)
    let cases = [
        // deckwatch's own tools, successful and failing
        ("list_ingress_templates", json!({}), "read", "ok"),
        (
            "trigger_build",
            json!({"namespace": "prod", "name": "web"}),
            "write",
            "error",
        ),
        ("validate_plugin", json!({}), "exec", "error"),
        // mcp-k8s tools deckwatch forwards
        (
            "list_deployments",
            json!({"namespace": "prod"}),
            "read",
            "error",
        ),
        (
            "delete_namespace",
            json!({"name": "prod"}),
            "delete",
            "error",
        ),
        // mcp-k8s tools, and interceptors that hand the call to mcp-k8s:
        // still one call
        (
            "create_deployment",
            json!({"namespace": "prod", "name": "web", "image": "nginx"}),
            "write",
            "error",
        ),
        (
            "delete_deployment",
            json!({"namespace": "prod", "name": "web"}),
            "delete",
            "error",
        ),
        (
            "scale_deployment",
            json!({"namespace": "prod", "name": "web", "replicas": 2}),
            "write",
            "error",
        ),
        (
            "restart_deployment",
            json!({"namespace": "prod", "name": "web"}),
            "write",
            "error",
        ),
        (
            "update_deployment",
            json!({"namespace": "prod", "name": "web", "image": "nginx:2"}),
            "write",
            "error",
        ),
        (
            "create_pod",
            json!({"namespace": "prod", "name": "job", "image": "busybox"}),
            "write",
            "error",
        ),
        (
            "apply_manifest",
            json!({"manifest": manifest}),
            "write",
            "error",
        ),
        // deckwatch's own exec_pod
        (
            "exec_pod",
            json!({"namespace": "prod", "pod_name": "web-1", "command": "ls"}),
            "exec",
            "error",
        ),
    ];
    for (tool, args, action, outcome) in cases {
        let (_, captured) = capture(post(&state, HeaderMap::new(), call(tool, args))).await;
        let r = one_request(&captured, tool);
        assert_eq!(
            r.span_field(semconv::OTEL_NAME),
            Some(format!("tools/call {tool}").as_str())
        );
        assert_eq!(r.span_field(semconv::OTEL_KIND), Some("server"));
        assert_eq!(r.span_field(semconv::GEN_AI_TOOL_NAME), Some(tool));
        assert_eq!(
            r.span_field(semconv::MCP_K8S_ACTION),
            Some(action),
            "{tool}"
        );
        assert_eq!(r.span_field(semconv::MCP_K8S_DECISION), Some("allowed"));
        assert_eq!(
            r.span_field(semconv::MCP_K8S_OUTCOME),
            Some(outcome),
            "{tool}"
        );
        assert_eq!(r.span_field(semconv::JSONRPC_REQUEST_ID), Some("7"));
        assert!(r.span_field(semconv::MCP_K8S_REQUEST_ID).is_some());
        assert_eq!(r.metric_field(semconv::GEN_AI_TOOL_NAME), Some(tool));
        assert_eq!(r.metric_field(semconv::MCP_K8S_ACTION), Some(action));
        assert_eq!(r.log.message, "tool call finished");
        if outcome == "error" {
            assert_eq!(r.span_field(semconv::ERROR_TYPE), Some("tool_error"));
            assert_eq!(r.span_field(semconv::RPC_STATUS_CODE), Some("-32000"));
        }
    }
}

#[tokio::test]
async fn a_yaml_manifest_reports_its_target() {
    let state = echo_state().await;
    let manifest = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: prod\nspec:\n  replicas: 1\n";
    let (_, captured) = capture(post(
        &state,
        HeaderMap::new(),
        call("apply_manifest", json!({ "manifest": manifest })),
    ))
    .await;
    let r = one_request(&captured, "apply_manifest");
    assert_eq!(
        r.span_field(semconv::MCP_K8S_RESOURCE_KIND),
        Some("Deployment")
    );
    assert_eq!(r.span_field(semconv::MCP_K8S_RESOURCE_NAME), Some("web"));
    assert_eq!(r.span_field(semconv::K8S_NAMESPACE_NAME), Some("prod"));
    assert_eq!(
        r.span_field(semconv::MCP_K8S_RESOURCE_API_VERSION),
        Some("apps/v1")
    );
    assert_eq!(
        r.span_field(semconv::MCP_K8S_TOOL_ARGUMENT_KEYS),
        Some("manifest")
    );
}

#[tokio::test]
async fn an_unknown_tool_is_reported_without_its_name() {
    let state = echo_state().await;
    let name = "no_such_tool_canary4711";
    let (_, captured) = capture(post(
        &state,
        HeaderMap::new(),
        call(name, json!({ "namespace": "canary4712" })),
    ))
    .await;
    let r = one_request(&captured, name);
    assert_eq!(r.span_field(semconv::OTEL_NAME), Some("tools/call"));
    assert_eq!(r.span_field(semconv::ERROR_TYPE), Some("unknown_tool"));
    assert_eq!(r.metric_field(semconv::ERROR_TYPE), Some("unknown_tool"));
    assert_eq!(r.log.message, "request finished");
    let haystack = captured.haystack();
    assert!(!haystack.contains("canary47"), "{haystack}");
}

#[tokio::test]
async fn other_methods_are_one_span_one_metric_event_one_log_line() {
    let state = echo_state().await;
    let cases = [
        (
            message("initialize", json!({"protocolVersion": "2025-11-25"})),
            "initialize",
            None,
        ),
        (message("tools/list", json!({})), "tools/list", None),
        (message("ping", json!({})), "ping", None),
        (
            message("prompts/get", json!({"name": "pre-deployment-check"})),
            "prompts/get pre-deployment-check",
            None,
        ),
        (
            message("prompts/get", json!({"name": "made-up-canary"})),
            "prompts/get",
            Some("-32602"),
        ),
        (
            message("evil/canary-method", json!({})),
            "_OTHER",
            Some("-32601"),
        ),
        (
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            "notifications/initialized",
            None,
        ),
    ];
    for (msg, span_name, error_type) in cases {
        let (_, captured) = capture(post(&state, HeaderMap::new(), msg)).await;
        let r = one_request(&captured, span_name);
        assert_eq!(r.span_field(semconv::OTEL_NAME), Some(span_name));
        assert_eq!(r.log.message, "request finished");
        assert_eq!(
            r.metric_field(semconv::ERROR_TYPE),
            error_type,
            "{span_name}"
        );
        // An unknown method or prompt is client input: never in the span
        // name, the metric or the log line. (mcp-k8s keeps the method,
        // clipped, as the span's `mcp.method.name` attribute.)
        let method = r.metric_field(semconv::MCP_METHOD_NAME).unwrap();
        assert_eq!(
            Some(method),
            r.log
                .fields
                .get(semconv::MCP_METHOD_NAME)
                .map(String::as_str)
        );
        for event in [r.metric, r.log] {
            assert!(
                !event.fields.values().any(|v| v.contains("canary")),
                "{span_name}: {event:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Client info and sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_starts_a_session_that_labels_later_calls() {
    let state = echo_state().await;
    let init = message(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "clientInfo": { "name": "claude-code", "version": "2.1.0" },
        }),
    );
    let ((headers, body), captured) = capture(post(&state, HeaderMap::new(), init)).await;
    assert!(body["result"]["serverInfo"].is_object());
    let session = headers
        .get(SESSION_HEADER)
        .expect("Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_string();
    let r = one_request(&captured, "initialize");
    assert_eq!(
        r.span_field(semconv::MCP_K8S_CLIENT_NAME),
        Some("claude-code")
    );
    assert_eq!(r.span_field(semconv::MCP_K8S_CLIENT_VERSION), Some("2.1.0"));
    assert_eq!(
        r.span_field(semconv::MCP_SESSION_ID),
        Some(session.as_str())
    );
    assert_eq!(
        r.span_field(semconv::MCP_PROTOCOL_VERSION),
        Some("2025-11-25")
    );

    // A later call in the session carries the client.
    let mut in_session = HeaderMap::new();
    in_session.insert(SESSION_HEADER, session.parse().unwrap());
    in_session.insert("mcp-protocol-version", "2025-11-25".parse().unwrap());
    let (_, captured) = capture(post(&state, in_session, call("list_templates", json!({})))).await;
    let r = one_request(&captured, "list_templates");
    assert_eq!(
        r.span_field(semconv::MCP_SESSION_ID),
        Some(session.as_str())
    );
    assert_eq!(
        r.span_field(semconv::MCP_K8S_CLIENT_NAME),
        Some("claude-code")
    );
    assert_eq!(r.span_field(semconv::MCP_K8S_CLIENT_VERSION), Some("2.1.0"));
    assert_eq!(
        r.span_field(semconv::MCP_PROTOCOL_VERSION),
        Some("2025-11-25")
    );
    // The client is a span attribute, never a metric attribute.
    assert!(r.metric_field(semconv::MCP_K8S_CLIENT_NAME).is_none());

    // An id deckwatch never issued is accepted, but labels nothing.
    let mut unknown = HeaderMap::new();
    unknown.insert(SESSION_HEADER, "canary-session-4713".parse().unwrap());
    let ((_, body), captured) =
        capture(post(&state, unknown, call("list_templates", json!({})))).await;
    assert!(body["result"].is_object(), "{body}");
    let r = one_request(&captured, "list_templates");
    assert!(r.span_field(semconv::MCP_SESSION_ID).is_none());
    assert!(r.span_field(semconv::MCP_K8S_CLIENT_NAME).is_none());
    assert!(!captured.haystack().contains("canary-session"));
}

#[tokio::test]
async fn a_failed_initialize_starts_no_session() {
    let state = echo_state().await;
    let (headers, _) = post(&state, HeaderMap::new(), message("tools/list", json!({}))).await;
    assert!(headers.get(SESSION_HEADER).is_none());
}

// ---------------------------------------------------------------------------
// Prometheus counters
// ---------------------------------------------------------------------------

/// The Prometheus exposition after running `calls` against a recorder of
/// their own.
fn prometheus_after(calls: Vec<Value>) -> String {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let state = echo_state().await;
                for call in calls {
                    post(&state, HeaderMap::new(), call).await;
                }
            });
    });
    handle.render()
}

/// The value of the counter line starting with `series`, if any.
fn counter(exposition: &str, series: &str) -> Option<u64> {
    exposition
        .lines()
        .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

#[test]
fn a_tool_call_is_counted_once() {
    let text = prometheus_after(vec![
        // Interceptors that hand the call to mcp-k8s: counted by mcp-k8s.
        call(
            "create_deployment",
            json!({"namespace": "prod", "name": "web", "image": "nginx"}),
        ),
        call(
            "restart_deployment",
            json!({"namespace": "prod", "name": "web"}),
        ),
        // deckwatch serves these itself: counted by deckwatch.
        call(
            "exec_pod",
            json!({"namespace": "prod", "pod_name": "web-1", "command": "ls"}),
        ),
        call(
            "update_deployment",
            json!({"namespace": "prod", "name": "web", "service_account": "builder"}),
        ),
        call("list_templates", json!({})),
        // Forwarded to mcp-k8s.
        call("list_pods", json!({"namespace": "prod"})),
    ]);
    let deckwatch = |tool: &str, status: &str| {
        counter(
            &text,
            &format!("deckwatch_mcp_tool_calls_total{{tool=\"{tool}\",status=\"{status}\"}}"),
        )
    };
    let library = |tool: &str, status: &str| {
        counter(
            &text,
            &format!("mcp_k8s_tool_calls_total{{tool=\"{tool}\",status=\"{status}\"}}"),
        )
    };
    assert_eq!(library("create_deployment", "error"), Some(1), "{text}");
    assert_eq!(library("restart_deployment", "error"), Some(1), "{text}");
    assert_eq!(library("list_pods", "error"), Some(1), "{text}");
    assert_eq!(deckwatch("exec_pod", "error"), Some(1), "{text}");
    assert_eq!(deckwatch("update_deployment", "error"), Some(1), "{text}");
    assert_eq!(deckwatch("list_templates", "ok"), Some(1), "{text}");
    for tool in ["create_deployment", "restart_deployment", "list_pods"] {
        assert!(
            !text.contains(&format!("deckwatch_mcp_tool_calls_total{{tool=\"{tool}\"")),
            "{tool} counted twice:\n{text}"
        );
    }
    for tool in ["exec_pod", "update_deployment", "list_templates"] {
        assert!(
            !text.contains(&format!("mcp_k8s_tool_calls_total{{tool=\"{tool}\"")),
            "{tool} counted twice:\n{text}"
        );
    }
}

#[test]
fn request_method_labels_are_bounded() {
    let text = prometheus_after(vec![
        message("evil/canary-method-4714", json!({})),
        message("ping", json!({})),
        call("no_such_tool_canary4715", json!({})),
    ]);
    assert_eq!(
        counter(
            &text,
            "deckwatch_mcp_requests_total{method=\"_OTHER\",status=\"error\"}"
        ),
        Some(1),
        "{text}"
    );
    assert_eq!(
        counter(
            &text,
            "deckwatch_mcp_requests_total{method=\"ping\",status=\"ok\"}"
        ),
        Some(1),
        "{text}"
    );
    assert!(!text.contains("canary"), "{text}");
}

// ---------------------------------------------------------------------------
// The tool table covers every tool deckwatch serves
// ---------------------------------------------------------------------------

/// Every tool definition deckwatch lists, the deckwatch-served definition
/// winning where mcp-k8s defines the same name.
pub(super) async fn listed_definitions(state: &AppState) -> BTreeMap<String, Value> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "tools/list".to_string(),
        params: json!({}),
    };
    let resp = handle_tools_list(state, &req).await;
    let mut defs = BTreeMap::new();
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
    // Listed only when agent feedback is enabled in settings.
    for tool in tools.into_iter().chain([agent_feedback_tool_definition()]) {
        defs.insert(tool["name"].as_str().unwrap().to_string(), tool);
    }
    defs
}

#[tokio::test]
async fn every_listed_tool_is_classified_or_served_by_mcp_k8s() {
    let state = build_test_state().await;
    let defs = listed_definitions(&state).await;
    for name in defs.keys() {
        assert!(
            mcp_telemetry::deckwatch_tool(name).is_some() || telemetry::is_known_tool(name),
            "{name} is listed, but neither classified in DECKWATCH_TOOLS nor an mcp-k8s tool"
        );
    }
    for tool in DECKWATCH_TOOLS {
        assert!(
            defs.contains_key(tool.name),
            "{} is never listed",
            tool.name
        );
    }
}

#[tokio::test]
async fn every_classified_tool_is_dispatched() {
    let state = build_test_state().await;
    for tool in DECKWATCH_TOOLS {
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            dispatch_deckwatch_tool(&state, tool.name, &json!({})),
        )
        .await
        .unwrap_or_else(|_| panic!("{} timed out", tool.name));
        if let Err(e) = result {
            assert!(
                !e.starts_with("Unknown tool"),
                "{} is classified but not dispatched",
                tool.name
            );
        }
    }
    assert_eq!(
        dispatch_deckwatch_tool(&state, "list_pods", &json!({})).await,
        Err("Unknown tool: list_pods".to_string())
    );
}

// ---------------------------------------------------------------------------
// Canary test
// ---------------------------------------------------------------------------

/// Unique, non-overlapping markers: fixed width plus a terminator, so no
/// canary is a substring of another.
#[derive(Default)]
pub(super) struct Canaries {
    pub(super) by_marker: Vec<(String, String)>,
}

impl Canaries {
    fn value(&mut self, path: &str) -> String {
        let marker = format!("canary{:05}x", self.by_marker.len());
        self.by_marker.push((marker.clone(), path.to_string()));
        marker
    }

    fn key(&mut self, path: &str) -> String {
        let marker = format!("canarykey{:05}x", self.by_marker.len());
        self.by_marker
            .push((marker.clone(), format!("{path} (map key)")));
        marker
    }

    /// A manifest-like object with a canary in every string, for free-form
    /// objects.
    fn object(&mut self, p: &str) -> Value {
        json!({
            "apiVersion": self.value(&format!("{p}.apiVersion")),
            "kind": self.value(&format!("{p}.kind")),
            "metadata": {
                "name": self.value(&format!("{p}.metadata.name")),
                "namespace": self.value(&format!("{p}.metadata.namespace")),
                "labels": { self.key(&format!("{p}.metadata.labels")): self.value(&format!("{p}.metadata.labels.*")) },
                "annotations": { self.key(&format!("{p}.metadata.annotations")): self.value(&format!("{p}.metadata.annotations.*")) },
            },
            "spec": {
                "containers": [{
                    "name": self.value(&format!("{p}.spec.containers[].name")),
                    "image": self.value(&format!("{p}.spec.containers[].image")),
                    "command": [self.value(&format!("{p}.spec.containers[].command[]"))],
                    "env": [{"name": self.value(&format!("{p}.spec.containers[].env[].name")),
                             "value": self.value(&format!("{p}.spec.containers[].env[].value"))}],
                }],
            },
            "data": { self.key(&format!("{p}.data")): self.value(&format!("{p}.data.*")) },
        })
    }

    /// Arguments for a JSON schema, every string a canary unique to its path.
    pub(super) fn fill(&mut self, schema: &Value, path: &str) -> Value {
        if let Some(alt) = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(|a| a.get(0))
        {
            return self.fill(alt, path);
        }
        match schema.get("type").and_then(Value::as_str) {
            // apply_manifest takes the manifest as a string: canaries inside.
            Some("string") if path == "manifest" => Value::String(self.object(path).to_string()),
            Some("string") => Value::String(self.value(path)),
            Some("integer") => json!(3),
            Some("number") => json!(1.5),
            Some("boolean") => json!(true),
            Some("array") => {
                let items = schema.get("items").cloned().unwrap_or(json!({}));
                json!([self.fill(&items, &format!("{path}[]"))])
            }
            Some("object") | None if schema.get("properties").is_some() => {
                let mut out = serde_json::Map::new();
                for (k, v) in schema["properties"].as_object().unwrap() {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    out.insert(k.clone(), self.fill(v, &p));
                }
                if let Some(ap) = schema.get("additionalProperties").filter(|a| a.is_object()) {
                    out.insert(self.key(path), self.fill(ap, &format!("{path}.*")));
                }
                Value::Object(out)
            }
            Some("object") => match schema.get("additionalProperties") {
                Some(ap) if ap.is_object() => {
                    json!({ self.key(path): self.fill(ap, &format!("{path}.*")) })
                }
                _ => self.object(path),
            },
            _ => {
                if path.ends_with("[]") {
                    self.object(path)
                } else {
                    Value::String(self.value(path))
                }
            }
        }
    }
}

#[tokio::test]
async fn canary_no_argument_value_of_a_deckwatch_tool_reaches_telemetry() {
    let state = echo_state().await;
    let defs = listed_definitions(&state).await;
    let allowed: BTreeSet<&str> = ALLOWED_ARGUMENTS.iter().map(|(k, _)| *k).collect();
    // A manifest's target fields may be recorded (apply_manifest).
    let manifest_target = [
        "manifest.kind",
        "manifest.apiVersion",
        "manifest.metadata.name",
        "manifest.metadata.namespace",
    ];

    let mut failures = Vec::new();
    for tool in DECKWATCH_TOOLS {
        let mut canaries = Canaries::default();
        let args = canaries.fill(&defs[tool.name]["inputSchema"], "");
        let (outcome, captured) = capture(tokio::time::timeout(
            Duration::from_secs(5),
            post(&state, HeaderMap::new(), call(tool.name, args)),
        ))
        .await;
        // watch_build polls for its (made-up) build until cut off here: a
        // cancelled call, which reports like any other.
        if outcome.is_err() && tool.name != "watch_build" {
            failures.push(format!("{}: timed out", tool.name));
        }

        let spans = captured
            .spans_named(semconv::TARGET, semconv::SERVER_SPAN_NAME)
            .len();
        let metrics = captured.events_on(semconv::METRICS_TARGET).len();
        let logs = captured.events_on(semconv::TARGET).len();
        if (spans, metrics, logs) != (1, 1, 1) {
            failures.push(format!(
                "{}: {spans} spans, {metrics} metric events, {logs} log events",
                tool.name
            ));
        }

        let haystack = captured.haystack();
        for (marker, path) in &canaries.by_marker {
            let top_level_allowed = allowed.contains(path.as_str());
            let may_appear = top_level_allowed || manifest_target.contains(&path.as_str());
            let appears = haystack.contains(marker.as_str());
            if appears && !may_appear {
                failures.push(format!("{}: value of `{path}` leaked", tool.name));
            }
            if top_level_allowed && !appears {
                failures.push(format!("{}: allowlisted `{path}` missing", tool.name));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} problems:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
