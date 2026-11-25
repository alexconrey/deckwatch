//! The MCP activity endpoint: its fixed PromQL, the per-tool aggregation,
//! and a full round trip against a stand-in Prometheus.

use std::sync::{Arc, Mutex};

use axum::extract::Form;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Map, Value};

use super::*;

fn labels(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect()
}

#[test]
fn queries_cover_both_counter_families_and_use_the_window() {
    let q = activity_queries(ActivityWindow::Day);
    for query in [&q.calls, &q.apply_manifest, &q.tool_totals] {
        assert!(query.contains("deckwatch_mcp_tool_calls_total"), "{query}");
        assert!(query.contains("mcp_k8s_tool_calls_total"), "{query}");
        assert!(query.contains(r#""family", "deckwatch""#), "{query}");
        assert!(query.contains(r#""family", "mcp_k8s""#), "{query}");
    }
    assert!(q.calls.starts_with("sum by (status) ("));
    assert!(q.calls.contains("[15m]") && q.calls.ends_with("* 60"));
    assert!(q.apply_manifest.contains(r#"{tool="apply_manifest"}[15m]"#));
    assert!(q.tool_totals.contains("increase(") && q.tool_totals.contains("[24h]"));
    assert!(q.p95.starts_with("histogram_quantile(0.95, sum by (le) ("));
    assert!(q.tool_p95.contains("sum by (le, tool)") && q.tool_p95.contains("[24h]"));
    for query in [&q.p95, &q.tool_p95] {
        assert!(query.contains("deckwatch_mcp_tool_call_duration_seconds_bucket"));
        assert!(query.contains("mcp_k8s_tool_call_duration_seconds_bucket"));
    }

    let hour = activity_queries(ActivityWindow::Hour);
    assert!(hour.calls.contains("[5m]") && hour.tool_totals.contains("[1h]"));
    let week = activity_queries(ActivityWindow::Week);
    assert!(week.calls.contains("[1h]") && week.tool_totals.contains("[7d]"));
}

#[test]
fn windows_stay_under_the_point_cap() {
    for window in [ActivityWindow::Hour, ActivityWindow::Day, ActivityWindow::Week] {
        let points = window.seconds() / i64::from(window.step());
        assert!(points <= i64::from(MAX_POINTS), "{window:?}: {points}");
    }
    let parsed: McpActivityQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(parsed.window, ActivityWindow::Day);
    let parsed: McpActivityQuery = serde_json::from_value(json!({"window": "7d"})).unwrap();
    assert_eq!(parsed.window, ActivityWindow::Week);
    assert!(serde_json::from_value::<McpActivityQuery>(json!({"window": "1y"})).is_err());
}

#[test]
fn tool_activity_adds_up_statuses_and_ranks_by_calls() {
    let totals = vec![
        (labels(&[("tool", "list_pods"), ("status", "ok")]), 40.0),
        (labels(&[("tool", "list_pods"), ("status", "error")]), 2.0),
        (labels(&[("tool", "apply_manifest"), ("status", "ok")]), 3.0),
        (labels(&[("tool", "apply_manifest"), ("status", "denied")]), 1.0),
        (labels(&[("tool", "get_pod"), ("status", "ok")]), 0.0),
        // No tool label: ignored.
        (labels(&[("status", "ok")]), 9.0),
    ];
    let p95 = vec![
        (labels(&[("tool", "apply_manifest")]), 0.8),
        (labels(&[("tool", "not_called")]), 0.1),
    ];
    let tools = tool_activity(totals, p95);
    assert_eq!(
        tools,
        vec![
            ToolActivity {
                tool: "list_pods".into(),
                calls: 42.0,
                errors: 2.0,
                denied: 0.0,
                p95_seconds: None,
            },
            ToolActivity {
                tool: "apply_manifest".into(),
                calls: 4.0,
                errors: 0.0,
                denied: 1.0,
                p95_seconds: Some(0.8),
            },
        ]
    );
}

#[test]
fn tool_activity_keeps_the_busiest_tools() {
    let totals = (0..40)
        .map(|i| {
            (
                labels(&[("tool", &format!("tool_{i:02}")), ("status", "ok")]),
                f64::from(i),
            )
        })
        .collect();
    let tools = tool_activity(totals, vec![]);
    assert_eq!(tools.len(), MAX_TOOLS);
    assert_eq!(tools[0].tool, "tool_39");
    assert!(tools.iter().all(|t| t.calls >= 15.0));
}

#[tokio::test]
async fn without_prometheus_the_page_says_why() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::new();
    let response = activity(None, &http, ActivityWindow::Day, 1_795_000_000).await;
    assert!(response
        .unavailable_reason
        .as_deref()
        .unwrap()
        .contains("prometheus not configured"));
    assert!(response.calls_per_minute.is_empty() && response.tools.is_empty());
    assert_eq!(response.window_seconds, 86_400);
}

/// A stand-in Prometheus: answers each query from `answer(path, query)`
/// and records what it was asked.
async fn fake_prometheus(
    answer: fn(&str, &str) -> Value,
) -> (String, Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let route = |path: &'static str, asked: Arc<Mutex<Vec<_>>>| {
        post(move |Form(form): Form<Vec<(String, String)>>| async move {
            let query = form
                .iter()
                .find(|(k, _)| k == "query")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            asked.lock().unwrap().push((path.to_string(), form));
            axum::Json(answer(path, &query))
        })
    };
    let app = Router::new()
        .route("/api/v1/query_range", route("query_range", asked.clone()))
        .route("/api/v1/query", route("query", asked.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), asked)
}

fn matrix(series: Value) -> Value {
    json!({"status": "success", "data": {"resultType": "matrix", "result": series}})
}

fn vector(samples: Value) -> Value {
    json!({"status": "success", "data": {"resultType": "vector", "result": samples}})
}

fn answer(path: &str, query: &str) -> Value {
    match path {
        "query_range" if query.starts_with("histogram_quantile") => matrix(json!([
            {"metric": {}, "values": [[1795000000.0, "0.25"], [1795000300.0, "NaN"]]}
        ])),
        "query_range" if query.contains("apply_manifest") => matrix(json!([
            {"metric": {"status": "ok"}, "values": [[1795000000.0, "0.5"]]}
        ])),
        "query_range" => matrix(json!([
            {"metric": {"status": "ok"}, "values": [[1795000000.0, "12"], [1795000300.0, "14"]]},
            {"metric": {"status": "error"}, "values": [[1795000000.0, "1"]]}
        ])),
        _ if query.starts_with("histogram_quantile") => vector(json!([
            {"metric": {"tool": "list_pods"}, "value": [1795000000.0, "0.12"]},
            {"metric": {"tool": "get_pod"}, "value": [1795000000.0, "NaN"]}
        ])),
        _ => vector(json!([
            {"metric": {"tool": "list_pods", "status": "ok"}, "value": [1795000000.0, "30"]},
            {"metric": {"tool": "list_pods", "status": "error"}, "value": [1795000000.0, "3"]},
            {"metric": {"tool": "get_pod", "status": "ok"}, "value": [1795000000.0, "5"]}
        ])),
    }
}

#[tokio::test]
async fn a_round_trip_against_prometheus() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, asked) = fake_prometheus(answer).await;
    let http = reqwest::Client::new();
    let now = 1_795_086_400;
    let r = activity(Some(&url), &http, ActivityWindow::Day, now).await;
    assert_eq!(r.unavailable_reason, None);

    let labels: Vec<&str> = r.calls_per_minute.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["error", "ok"]);
    let ok = &r.calls_per_minute[1];
    assert_eq!(ok.points.len(), 2);
    assert_eq!((ok.points[0].t, ok.points[0].v), (1_795_000_000_000, 12.0));
    assert_eq!(r.apply_manifest_per_minute[0].label, "ok");
    // The NaN point is dropped.
    assert_eq!(r.p95_seconds.len(), 1);
    assert_eq!(r.p95_seconds[0].v, 0.25);
    assert_eq!(r.tools.len(), 2);
    assert_eq!(r.tools[0].tool, "list_pods");
    assert_eq!((r.tools[0].calls, r.tools[0].errors), (33.0, 3.0));
    assert_eq!(r.tools[0].p95_seconds, Some(0.12));
    assert_eq!(r.tools[1].p95_seconds, None);

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 5);
    for (path, form) in asked.iter() {
        let get = |k: &str| form.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        if path == "query_range" {
            assert_eq!(get("start"), Some("1795000000"));
            assert_eq!(get("end"), Some("1795086400"));
            assert_eq!(get("step"), Some("300s"));
        } else {
            assert_eq!(get("time"), Some("1795086400"));
        }
    }
}

#[tokio::test]
async fn a_failing_prometheus_is_reported_not_raised() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, _) = fake_prometheus(|_, _| {
        json!({"status": "error", "errorType": "bad_data", "error": "parse error"})
    })
    .await;
    let http = reqwest::Client::new();
    let r = activity(Some(&url), &http, ActivityWindow::Hour, 1_795_000_000).await;
    let reason = r.unavailable_reason.unwrap();
    assert!(reason.contains("parse error"), "{reason}");
    assert!(r.calls_per_minute.is_empty() && r.tools.is_empty());
}
