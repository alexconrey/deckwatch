// Unit tests for src/handlers/tracing_handler.rs — request/response types,
// URL construction, validation, and summarization helpers.

use super::*;

// ---- ListTracesQuery deserialization ----

#[test]
fn list_traces_query_full() {
    let json = r#"{"service": "checkout-api", "limit": 10}"#;
    let q: ListTracesQuery = serde_json::from_str(json).unwrap();
    assert_eq!(q.service, "checkout-api");
    assert_eq!(q.limit, Some(10));
}

#[test]
fn list_traces_query_limit_defaults_to_none() {
    let json = r#"{"service": "svc"}"#;
    let q: ListTracesQuery = serde_json::from_str(json).unwrap();
    assert!(q.limit.is_none());
}

// ---- TraceSummary serialization ----

#[test]
fn trace_summary_serializes_all_fields() {
    let ts = TraceSummary {
        trace_id: "abc123".to_string(),
        root_span_name: "GET /api/users".to_string(),
        duration_ms: 150,
        span_count: 12,
        timestamp_ms: 1700000000000,
    };
    let json = serde_json::to_value(&ts).unwrap();
    assert_eq!(json["trace_id"], "abc123");
    assert_eq!(json["root_span_name"], "GET /api/users");
    assert_eq!(json["duration_ms"], 150);
    assert_eq!(json["span_count"], 12);
    assert_eq!(json["timestamp_ms"], 1700000000000_i64);
}

// ---- ListTracesResponse serialization ----

#[test]
fn list_traces_response_omits_unavailable_when_none() {
    let resp = ListTracesResponse {
        traces: Vec::new(),
        unavailable_reason: None,
        ui_url: "https://grafana.example.com".to_string(),
        backend_kind: "tempo".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json.get("unavailable_reason").is_none());
    assert_eq!(json["ui_url"], "https://grafana.example.com");
    assert_eq!(json["backend_kind"], "tempo");
}

#[test]
fn list_traces_response_includes_unavailable_when_set() {
    let resp = ListTracesResponse {
        traces: Vec::new(),
        unavailable_reason: Some("tracing not configured".to_string()),
        ui_url: "".to_string(),
        backend_kind: "tempo".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["unavailable_reason"], "tracing not configured");
}

#[test]
fn list_traces_response_serializes_traces() {
    let resp = ListTracesResponse {
        traces: vec![TraceSummary {
            trace_id: "t1".to_string(),
            root_span_name: "POST /order".to_string(),
            duration_ms: 200,
            span_count: 5,
            timestamp_ms: 1700000000000,
        }],
        unavailable_reason: None,
        ui_url: "".to_string(),
        backend_kind: "jaeger".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    let traces = json["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0]["trace_id"], "t1");
}

// ---- normalized_query_url ----

#[test]
fn normalized_query_url_trims_and_strips_trailing_slash() {
    let mut s = TracingSettings::default();
    s.query_url = "  http://tempo:3200/  ".to_string();
    assert_eq!(
        normalized_query_url(&s).as_deref(),
        Some("http://tempo:3200")
    );
}

#[test]
fn normalized_query_url_returns_none_for_empty() {
    let s = TracingSettings::default();
    assert!(normalized_query_url(&s).is_none());
}

#[test]
fn normalized_query_url_returns_none_for_whitespace_only() {
    let mut s = TracingSettings::default();
    s.query_url = "   ".to_string();
    assert!(normalized_query_url(&s).is_none());
}

// ---- BackendKind::parse ----

#[test]
fn backend_kind_parse_tempo() {
    assert!(matches!(BackendKind::parse("tempo"), BackendKind::Tempo));
    assert!(matches!(BackendKind::parse("Tempo"), BackendKind::Tempo));
    assert!(matches!(BackendKind::parse("TEMPO"), BackendKind::Tempo));
}

#[test]
fn backend_kind_parse_jaeger() {
    assert!(matches!(BackendKind::parse("jaeger"), BackendKind::Jaeger));
    assert!(matches!(BackendKind::parse("JAEGER"), BackendKind::Jaeger));
    assert!(matches!(BackendKind::parse("Jaeger"), BackendKind::Jaeger));
}

#[test]
fn backend_kind_parse_unknown_defaults_to_tempo() {
    assert!(matches!(BackendKind::parse("zipkin"), BackendKind::Tempo));
    assert!(matches!(BackendKind::parse(""), BackendKind::Tempo));
}

// ---- validate_service_name (extending the inline tests) ----

#[test]
fn validate_service_name_accepts_dots_underscores_dashes() {
    assert!(validate_service_name("my.svc-name_v2").is_ok());
}

#[test]
fn validate_service_name_rejects_slashes() {
    assert!(validate_service_name("path/injection").is_err());
}

#[test]
fn validate_service_name_rejects_too_long() {
    let long = "x".repeat(254);
    assert!(validate_service_name(&long).is_err());
}

#[test]
fn validate_service_name_accepts_max_length() {
    let max = "x".repeat(253);
    assert!(validate_service_name(&max).is_ok());
}

// ---- summarize_jaeger_trace (extending the inline tests) ----

#[test]
fn summarize_converts_microseconds_to_milliseconds() {
    let t = JaegerTrace {
        trace_id: "t1".to_string(),
        spans: vec![JaegerSpan {
            operation_name: "GET /".to_string(),
            start_time: 1_700_000_000_000_000, // us
            duration: 150_000,                 // 150ms in us
            references: vec![],
        }],
    };
    let s = summarize_jaeger_trace(t);
    assert_eq!(s.duration_ms, 150);
    assert_eq!(s.timestamp_ms, 1_700_000_000_000);
}

#[test]
fn summarize_empty_spans_returns_unknown() {
    let t = JaegerTrace {
        trace_id: "t2".to_string(),
        spans: vec![],
    };
    let s = summarize_jaeger_trace(t);
    assert_eq!(s.root_span_name, "unknown");
    assert_eq!(s.duration_ms, 0);
    assert_eq!(s.span_count, 0);
}

#[test]
fn summarize_preserves_trace_id() {
    let t = JaegerTrace {
        trace_id: "0123456789abcdef".to_string(),
        spans: vec![JaegerSpan {
            operation_name: "op".to_string(),
            start_time: 0,
            duration: 0,
            references: vec![],
        }],
    };
    let s = summarize_jaeger_trace(t);
    assert_eq!(s.trace_id, "0123456789abcdef");
}
