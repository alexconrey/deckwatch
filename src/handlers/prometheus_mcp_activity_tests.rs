//! The MCP activity endpoint: the fixed PromQL of both sources, how the
//! page picks one, the per-tool aggregation, and full round trips against a
//! stand-in Prometheus.

use std::sync::{Arc, Mutex};

use axum::extract::Form;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Map, Value};

use super::*;

const JOB: &str = "deckwatch";

const SOURCES: [ActivitySource; 2] = [ActivitySource::Otlp, ActivitySource::Scrape];
const WINDOWS: [ActivityWindow; 3] = [
    ActivityWindow::Hour,
    ActivityWindow::Day,
    ActivityWindow::Week,
];

fn labels(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect()
}

fn all_queries(source: ActivitySource, window: ActivityWindow) -> Vec<String> {
    let q = activity_queries(source, window, JOB);
    vec![
        has_data_query(source, window, JOB),
        q.calls,
        q.apply_manifest,
        q.p95,
        q.tool_totals,
        q.tool_p95,
    ]
}

/// The metric names a query selects: identifiers right before `{` or `[`,
/// outside string literals.
fn metric_names(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut ident = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in query.chars() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
            ident.push(c);
            continue;
        }
        if (c == '{' || c == '[') && !ident.is_empty() {
            names.push(ident.clone());
        }
        ident.clear();
        in_string = c == '"';
    }
    names
}

/// The Prometheus name of an OpenTelemetry attribute under the OTLP
/// receiver's default translation, for names like mcp-k8s's: every
/// character outside `[A-Za-z0-9_]` becomes `_`.
fn prometheus_name(attribute: &str) -> String {
    attribute
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Query construction
// ---------------------------------------------------------------------------

#[test]
fn otlp_names_are_mcp_k8s_semconv_as_the_otlp_receiver_translates_it() {
    assert_eq!(semconv::MCP_SERVER_OPERATION_DURATION_UNIT, "s");
    assert_eq!(
        OTLP_DURATION,
        format!(
            "{}_seconds",
            prometheus_name(semconv::MCP_SERVER_OPERATION_DURATION)
        )
    );
    for (label, attribute) in [
        (OTLP_TOOL, semconv::GEN_AI_TOOL_NAME),
        (OTLP_METHOD, semconv::MCP_METHOD_NAME),
        (OTLP_ERROR_TYPE, semconv::ERROR_TYPE),
        (OTLP_DECISION, semconv::MCP_K8S_DECISION),
    ] {
        assert_eq!(label, prometheus_name(attribute));
        // deckwatch's metrics view keeps only these attributes.
        assert!(
            semconv::METRIC_ATTRIBUTES.contains(&attribute),
            "{attribute} is not a metric attribute"
        );
    }
    assert_eq!(semconv::TOOLS_CALL, "tools/call");
    assert_eq!(
        semconv::error_type::PERMISSION_DENIED,
        "permission_denied",
        "a denial has an error type: the denial rule must come last"
    );
}

#[test]
fn otlp_queries_read_deckwatch_s_tool_calls_from_the_duration_histogram() {
    let q = activity_queries(ActivitySource::Otlp, ActivityWindow::Day, JOB);
    let count = r#"mcp_server_operation_duration_seconds_count{job="deckwatch",mcp_method_name="tools/call",gen_ai_tool_name!=""}"#;
    let bucket = r#"mcp_server_operation_duration_seconds_bucket{job="deckwatch",mcp_method_name="tools/call",gen_ai_tool_name!=""}"#;
    let status = |inner: &str| {
        format!(
            r#"label_replace(label_replace(label_replace({inner}, "status", "ok", "", ""), "status", "error", "error_type", ".+"), "status", "denied", "mcp_k8s_decision", "denied")"#
        )
    };
    assert_eq!(
        q.calls,
        format!(
            "sum by (status) ({}) * 60",
            status(&format!("rate({count}[15m])"))
        )
    );
    assert_eq!(
        q.apply_manifest,
        format!(
            "sum by (status) ({}) * 60",
            status(
                r#"rate(mcp_server_operation_duration_seconds_count{job="deckwatch",mcp_method_name="tools/call",gen_ai_tool_name="apply_manifest"}[15m])"#
            )
        )
    );
    assert_eq!(
        q.p95,
        format!("histogram_quantile(0.95, sum by (le) (rate({bucket}[15m])))")
    );
    assert_eq!(
        q.tool_totals,
        format!(
            "sum by (gen_ai_tool_name, status) ({})",
            status(&format!("increase({count}[24h])"))
        )
    );
    assert_eq!(
        q.tool_p95,
        format!(
            "histogram_quantile(0.95, sum by (le, gen_ai_tool_name) (increase({bucket}[24h])))"
        )
    );
    assert_eq!(ActivitySource::Otlp.tool_label(), "gen_ai_tool_name");

    let hour = activity_queries(ActivitySource::Otlp, ActivityWindow::Hour, JOB);
    assert!(hour.calls.contains("[5m]") && hour.tool_totals.contains("[1h]"));
    let week = activity_queries(ActivitySource::Otlp, ActivityWindow::Week, JOB);
    assert!(week.calls.contains("[1h]") && week.tool_totals.contains("[7d]"));
}

#[test]
fn fallback_queries_add_up_deckwatch_s_two_counters() {
    let q = activity_queries(ActivitySource::Scrape, ActivityWindow::Day, JOB);
    for query in [&q.calls, &q.apply_manifest, &q.tool_totals] {
        assert!(query.contains("deckwatch_mcp_tool_calls_total"), "{query}");
        assert!(
            query.contains("deckwatch_mcp_passthrough_tool_calls_total"),
            "{query}"
        );
        assert!(query.contains(r#""family", "deckwatch""#), "{query}");
        assert!(query.contains(r#""family", "passthrough""#), "{query}");
    }
    assert!(q.calls.starts_with("sum by (status) ("));
    assert!(q.calls.contains("[15m]") && q.calls.ends_with("* 60"));
    assert!(q.apply_manifest.contains(r#"{tool="apply_manifest"}[15m]"#));
    assert!(q.tool_totals.starts_with("sum by (tool, status) ("));
    assert!(q.tool_totals.contains("increase(") && q.tool_totals.contains("[24h]"));
    assert!(q.p95.starts_with("histogram_quantile(0.95, sum by (le) ("));
    assert!(q.tool_p95.contains("sum by (le, tool)") && q.tool_p95.contains("[24h]"));
    for query in [&q.p95, &q.tool_p95] {
        assert!(query.contains("deckwatch_mcp_tool_call_duration_seconds_bucket"));
        assert!(query.contains("deckwatch_mcp_passthrough_tool_call_duration_seconds_bucket"));
    }
    assert_eq!(ActivitySource::Scrape.tool_label(), "tool");

    let hour = activity_queries(ActivitySource::Scrape, ActivityWindow::Hour, JOB);
    assert!(hour.calls.contains("[5m]") && hour.tool_totals.contains("[1h]"));
    let week = activity_queries(ActivitySource::Scrape, ActivityWindow::Week, JOB);
    assert!(week.calls.contains("[1h]") && week.tool_totals.contains("[7d]"));
}

#[test]
fn the_fallback_reads_the_series_deckwatch_records() {
    assert_eq!(
        SCRAPE_COUNTERS,
        [
            crate::metrics::MCP_TOOL_CALLS,
            crate::metrics::MCP_PASSTHROUGH_TOOL_CALLS
        ]
    );
    let recorder = crate::metrics::exporter().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        crate::metrics::record_mcp_tool_call("list_templates", "ok", 0.01);
        crate::metrics::record_mcp_passthrough_tool_call("list_pods", "error", 0.2);
    });
    let text = handle.render();
    // Both durations are histograms with the same buckets, so the p95
    // queries can add up their `_bucket` series.
    for name in SCRAPE_COUNTERS.into_iter().chain(SCRAPE_DURATION_BUCKETS) {
        assert!(
            text.lines().any(|l| l.starts_with(&format!("{name}{{"))),
            "{name} isn't recorded:\n{text}"
        );
    }
    let bounds = |histogram: &str| -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with(&format!("{histogram}{{")))
            .filter_map(|l| l.split("le=\"").nth(1)?.split('"').next())
            .map(str::to_string)
            .collect()
    };
    let deckwatch = bounds(SCRAPE_DURATION_BUCKETS[0]);
    assert!(deckwatch.len() > 5, "{text}");
    assert_eq!(deckwatch, bounds(SCRAPE_DURATION_BUCKETS[1]));
}

#[test]
fn existence_queries_only_look_for_series_in_the_window() {
    assert_eq!(
        has_data_query(ActivitySource::Otlp, ActivityWindow::Week, JOB),
        r#"group(present_over_time(mcp_server_operation_duration_seconds_count{job="deckwatch",mcp_method_name="tools/call",gen_ai_tool_name!=""}[7d]))"#
    );
    assert_eq!(
        has_data_query(ActivitySource::Scrape, ActivityWindow::Hour, JOB),
        "group(present_over_time(deckwatch_mcp_tool_calls_total[1h])) or \
         group(present_over_time(deckwatch_mcp_passthrough_tool_calls_total[1h]))"
    );
}

#[test]
fn no_query_reads_a_metric_mcp_k8s_deprecated() {
    let allowed = [
        "mcp_server_operation_duration_seconds_count",
        "mcp_server_operation_duration_seconds_bucket",
        "deckwatch_mcp_tool_calls_total",
        "deckwatch_mcp_passthrough_tool_calls_total",
        "deckwatch_mcp_tool_call_duration_seconds_bucket",
        "deckwatch_mcp_passthrough_tool_call_duration_seconds_bucket",
    ];
    for source in SOURCES {
        for window in WINDOWS {
            for query in all_queries(source, window) {
                let names = metric_names(&query);
                assert!(!names.is_empty(), "{query}");
                for name in names {
                    assert!(
                        allowed.contains(&name.as_str()),
                        "{source:?} reads {name}: {query}"
                    );
                }
                for deprecated in [
                    "mcp_k8s_tool_calls_total",
                    "mcp_k8s_tool_call_duration_seconds",
                    "mcp_requests_total",
                ] {
                    assert!(!query.contains(deprecated), "{query}");
                }
            }
        }
    }
}

#[test]
fn the_otlp_job_is_matched_exactly() {
    assert_eq!(job_label(None, "deckwatch"), "deckwatch");
    assert_eq!(
        job_label(Some("platform"), "deckwatch"),
        "platform/deckwatch"
    );
    // An operator's service name can't break out of the matcher.
    let q = activity_queries(
        ActivitySource::Otlp,
        ActivityWindow::Day,
        "a\"} or vector(1) or {x=\"\\",
    );
    assert!(
        q.calls
            .contains(r#"{job="a\"} or vector(1) or {x=\"\\",mcp_method_name="tools/call","#),
        "{}",
        q.calls
    );
    assert_eq!(
        metric_names(&q.calls),
        [OTLP_DURATION.to_string() + "_count"]
    );
    // The fallback's metric names are deckwatch's own: no job needed.
    for query in all_queries(ActivitySource::Scrape, ActivityWindow::Day) {
        assert!(!query.contains("job="), "{query}");
    }
}

#[test]
fn windows_stay_under_the_point_cap() {
    for window in WINDOWS {
        let points = window.seconds() / i64::from(window.step());
        assert!(points <= i64::from(MAX_POINTS), "{window:?}: {points}");
    }
    let parsed: McpActivityQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(parsed.window, ActivityWindow::Day);
    let parsed: McpActivityQuery = serde_json::from_value(json!({"window": "7d"})).unwrap();
    assert_eq!(parsed.window, ActivityWindow::Week);
    assert!(serde_json::from_value::<McpActivityQuery>(json!({"window": "1y"})).is_err());
}

// ---------------------------------------------------------------------------
// Per-tool aggregation
// ---------------------------------------------------------------------------

#[test]
fn tool_activity_adds_up_statuses_and_ranks_by_calls() {
    let totals = vec![
        (labels(&[("tool", "list_pods"), ("status", "ok")]), 40.0),
        (labels(&[("tool", "list_pods"), ("status", "error")]), 2.0),
        (labels(&[("tool", "apply_manifest"), ("status", "ok")]), 3.0),
        (
            labels(&[("tool", "apply_manifest"), ("status", "denied")]),
            1.0,
        ),
        (labels(&[("tool", "get_pod"), ("status", "ok")]), 0.0),
        // No tool label: ignored.
        (labels(&[("status", "ok")]), 9.0),
    ];
    let p95 = vec![
        (labels(&[("tool", "apply_manifest")]), 0.8),
        (labels(&[("tool", "not_called")]), 0.1),
    ];
    let tools = tool_activity(totals, p95, "tool");
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
fn tool_activity_reads_the_tool_from_the_source_s_label() {
    let totals = vec![
        (
            labels(&[("gen_ai_tool_name", "exec_pod"), ("status", "ok")]),
            5.0,
        ),
        (
            labels(&[("gen_ai_tool_name", "exec_pod"), ("status", "denied")]),
            1.0,
        ),
        // The other source's label: not this source's tool.
        (labels(&[("tool", "list_pods"), ("status", "ok")]), 9.0),
    ];
    let p95 = vec![(labels(&[("gen_ai_tool_name", "exec_pod")]), 2.5)];
    let tools = tool_activity(totals, p95, ActivitySource::Otlp.tool_label());
    assert_eq!(
        tools,
        vec![ToolActivity {
            tool: "exec_pod".into(),
            calls: 6.0,
            errors: 0.0,
            denied: 1.0,
            p95_seconds: Some(2.5),
        }]
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
    let tools = tool_activity(totals, vec![], "tool");
    assert_eq!(tools.len(), MAX_TOOLS);
    assert_eq!(tools[0].tool, "tool_39");
    assert!(tools.iter().all(|t| t.calls >= 15.0));
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn without_prometheus_the_page_says_why() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::new();
    let response = activity(None, &http, ActivityWindow::Day, 1_795_000_000, JOB).await;
    assert!(response
        .unavailable_reason
        .as_deref()
        .unwrap()
        .contains("prometheus not configured"));
    assert!(response.calls_per_minute.is_empty() && response.tools.is_empty());
    assert_eq!(response.source, None);
    assert_eq!(response.window_seconds, 86_400);
}

type Asked = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;
type Answer = Arc<dyn Fn(&str, &str) -> Value + Send + Sync>;

/// A stand-in Prometheus: answers each query from `answer(path, query)`
/// and records what it was asked.
async fn fake_prometheus(
    answer: impl Fn(&str, &str) -> Value + Send + Sync + 'static,
) -> (String, Asked) {
    let answer: Answer = Arc::new(answer);
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let route = |path: &'static str, answer: Answer, asked: Asked| {
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
        .route(
            "/api/v1/query_range",
            route("query_range", answer.clone(), asked.clone()),
        )
        .route("/api/v1/query", route("query", answer, asked.clone()));
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

/// A Prometheus that has OTLP series if `otlp`, the `/metrics` counters if
/// `scrape`, and answers the data queries of either source.
fn prometheus_with(otlp: bool, scrape: bool) -> impl Fn(&str, &str) -> Value {
    move |path, query| {
        let is_otlp = query.contains(OTLP_DURATION);
        let tool = if is_otlp { "gen_ai_tool_name" } else { "tool" };
        match path {
            _ if query.starts_with("group(present_over_time(") => {
                if (is_otlp && otlp) || (!is_otlp && scrape) {
                    vector(json!([{"metric": {}, "value": [1795086400.0, "1"]}]))
                } else {
                    vector(json!([]))
                }
            }
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
                {"metric": {tool: "list_pods"}, "value": [1795000000.0, "0.12"]},
                {"metric": {tool: "get_pod"}, "value": [1795000000.0, "NaN"]}
            ])),
            _ => vector(json!([
                {"metric": {tool: "list_pods", "status": "ok"}, "value": [1795000000.0, "30"]},
                {"metric": {tool: "list_pods", "status": "error"}, "value": [1795000000.0, "3"]},
                {"metric": {tool: "get_pod", "status": "ok"}, "value": [1795000000.0, "5"]}
            ])),
        }
    }
}

fn queries_asked(asked: &Asked) -> Vec<String> {
    asked
        .lock()
        .unwrap()
        .iter()
        .map(|(_, form)| {
            form.iter()
                .find(|(k, _)| k == "query")
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        })
        .collect()
}

#[tokio::test]
async fn a_round_trip_against_prometheus_with_otlp_series() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, asked) = fake_prometheus(prometheus_with(true, true)).await;
    let http = reqwest::Client::new();
    let now = 1_795_086_400;
    let r = activity(Some(&url), &http, ActivityWindow::Day, now, JOB).await;
    assert_eq!(r.unavailable_reason, None);
    assert_eq!(r.source, Some(ActivitySource::Otlp));

    let labels: Vec<&str> = r
        .calls_per_minute
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(labels, ["error", "ok"]);
    let ok = &r.calls_per_minute[1];
    assert_eq!(ok.points.len(), 2);
    assert_eq!((ok.points[0].t, ok.points[0].v), (1_795_000_000_000, 12.0));
    assert_eq!(r.apply_manifest_per_minute[0].label, "ok");
    // The NaN point is dropped.
    assert_eq!(r.p95_seconds.len(), 1);
    assert_eq!(r.p95_seconds[0].v, 0.25);
    // Tools are named by gen_ai_tool_name.
    assert_eq!(r.tools.len(), 2);
    assert_eq!(r.tools[0].tool, "list_pods");
    assert_eq!((r.tools[0].calls, r.tools[0].errors), (33.0, 3.0));
    assert_eq!(r.tools[0].p95_seconds, Some(0.12));
    assert_eq!(r.tools[1].p95_seconds, None);

    // Two existence checks, then the five OTLP queries.
    let queries = queries_asked(&asked);
    assert_eq!(queries.len(), 7, "{queries:#?}");
    assert_eq!(
        queries[..2]
            .iter()
            .filter(|q| q.starts_with("group("))
            .count(),
        2
    );
    for query in &queries[2..] {
        assert!(query.contains(OTLP_DURATION), "{query}");
        assert!(!query.contains("deckwatch_mcp"), "{query}");
    }
    let asked = asked.lock().unwrap();
    for (path, form) in asked.iter() {
        let get = |k: &str| {
            form.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
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
async fn without_otlp_series_the_page_falls_back_to_deckwatch_s_counters() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, asked) = fake_prometheus(prometheus_with(false, true)).await;
    let http = reqwest::Client::new();
    let r = activity(Some(&url), &http, ActivityWindow::Week, 1_795_086_400, JOB).await;
    assert_eq!(r.unavailable_reason, None);
    assert_eq!(r.source, Some(ActivitySource::Scrape));
    assert_eq!(r.tools[0].tool, "list_pods");
    assert_eq!(r.tools[0].calls, 33.0);

    let queries = queries_asked(&asked);
    assert_eq!(queries.len(), 7, "{queries:#?}");
    for query in &queries[2..] {
        assert!(query.contains("deckwatch_mcp_passthrough_"), "{query}");
        assert!(!query.contains(OTLP_DURATION), "{query}");
    }
}

#[tokio::test]
async fn with_neither_source_the_page_has_nothing_to_show() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, asked) = fake_prometheus(prometheus_with(false, false)).await;
    let http = reqwest::Client::new();
    let r = activity(Some(&url), &http, ActivityWindow::Hour, 1_795_086_400, JOB).await;
    assert_eq!(r.unavailable_reason, None);
    assert_eq!(r.source, None);
    assert!(r.calls_per_minute.is_empty() && r.p95_seconds.is_empty() && r.tools.is_empty());
    // Only the two existence checks.
    assert_eq!(queries_asked(&asked).len(), 2);
}

#[tokio::test]
async fn a_failing_prometheus_is_reported_not_raised() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (url, _) = fake_prometheus(
        |_, _| json!({"status": "error", "errorType": "bad_data", "error": "parse error"}),
    )
    .await;
    let http = reqwest::Client::new();
    let r = activity(Some(&url), &http, ActivityWindow::Hour, 1_795_000_000, JOB).await;
    let reason = r.unavailable_reason.unwrap();
    assert!(reason.contains("parse error"), "{reason}");
    assert_eq!(r.source, None);
    assert!(r.calls_per_minute.is_empty() && r.tools.is_empty());
}

#[tokio::test]
async fn a_failing_data_query_drops_the_source() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let has_data = prometheus_with(true, false);
    let (url, _) = fake_prometheus(move |path, query| {
        if path == "query_range" {
            json!({"status": "error", "errorType": "timeout", "error": "query timed out"})
        } else {
            has_data(path, query)
        }
    })
    .await;
    let http = reqwest::Client::new();
    let r = activity(Some(&url), &http, ActivityWindow::Day, 1_795_000_000, JOB).await;
    assert!(r.unavailable_reason.unwrap().contains("query timed out"));
    assert_eq!(r.source, None);
    assert!(r.calls_per_minute.is_empty() && r.tools.is_empty());
}

#[test]
fn the_source_is_on_the_wire() {
    let response = |source| McpActivityResponse {
        window_seconds: 3600,
        step_seconds: 60,
        source,
        calls_per_minute: vec![],
        apply_manifest_per_minute: vec![],
        p95_seconds: vec![],
        tools: vec![],
        unavailable_reason: None,
    };
    for (source, wire) in [
        (Some(ActivitySource::Otlp), json!("otlp")),
        (Some(ActivitySource::Scrape), json!("scrape")),
        (None, Value::Null),
    ] {
        let body = serde_json::to_value(response(source)).unwrap();
        assert_eq!(body["source"], wire, "{body}");
        assert!(body.as_object().unwrap().contains_key("source"));
    }
}
