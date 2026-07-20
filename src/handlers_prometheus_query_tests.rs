// Unit tests for src/handlers/prometheus_query.rs — query params, response
// types, query building, and validation helpers.

use super::*;

// ---- QueryKind deserialization ----

#[test]
fn query_kind_deserializes_snake_case() {
    let kinds = [
        ("\"cpu_usage\"", "CpuUsage"),
        ("\"memory_usage\"", "MemoryUsage"),
        ("\"request_rate\"", "RequestRate"),
        ("\"error_rate\"", "ErrorRate"),
    ];
    for (json, label) in &kinds {
        let result: Result<QueryKind, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "failed to deserialize {label}: {json}");
    }
}

#[test]
fn query_kind_rejects_unknown() {
    let result: Result<QueryKind, _> = serde_json::from_str("\"disk_usage\"");
    assert!(result.is_err());
}

// ---- RangeQuery deserialization ----

#[test]
fn range_query_full() {
    let json = r#"{
        "query": "cpu_usage",
        "start": 1700000000,
        "end": 1700003600,
        "step": 60,
        "namespace": "prod",
        "deployment": "api"
    }"#;
    let q: RangeQuery = serde_json::from_str(json).unwrap();
    assert_eq!(q.start, 1700000000);
    assert_eq!(q.end, 1700003600);
    assert_eq!(q.step, Some(60));
    assert_eq!(q.namespace, "prod");
    assert_eq!(q.deployment, "api");
}

#[test]
fn range_query_step_defaults_to_none() {
    let json = r#"{
        "query": "memory_usage",
        "start": 0,
        "end": 3600,
        "namespace": "ns",
        "deployment": "dep"
    }"#;
    let q: RangeQuery = serde_json::from_str(json).unwrap();
    assert!(q.step.is_none());
}

// ---- RangeResponse serialization ----

#[test]
fn range_response_omits_unavailable_when_none() {
    let resp = RangeResponse {
        series: Vec::new(),
        unit: "cores",
        unavailable_reason: None,
        warnings: Vec::new(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json.get("unavailable_reason").is_none());
    assert!(json.get("warnings").is_none());
}

#[test]
fn range_response_includes_unavailable_when_set() {
    let resp = RangeResponse {
        series: Vec::new(),
        unit: "bytes",
        unavailable_reason: Some("prometheus not configured".to_string()),
        warnings: Vec::new(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["unavailable_reason"], "prometheus not configured");
}

#[test]
fn range_response_includes_warnings_when_nonempty() {
    let resp = RangeResponse {
        series: Vec::new(),
        unit: "req/s",
        unavailable_reason: None,
        warnings: vec!["fell back to http_server_requests_total".to_string()],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(
        json["warnings"][0],
        "fell back to http_server_requests_total"
    );
}

// ---- PodSeries serialization ----

#[test]
fn pod_series_serializes_points() {
    let series = PodSeries {
        pod: "api-pod-abc".to_string(),
        points: vec![
            RangePoint {
                t: 1700000000000,
                v: 0.42,
            },
            RangePoint {
                t: 1700000060000,
                v: 0.55,
            },
        ],
    };
    let json = serde_json::to_value(&series).unwrap();
    assert_eq!(json["pod"], "api-pod-abc");
    let pts = json["points"].as_array().unwrap();
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0]["t"], 1700000000000_i64);
}

// ---- build_query ----

#[test]
fn build_query_cpu_contains_namespace_and_deployment() {
    let (q, unit) = build_query(QueryKind::CpuUsage, "staging", "web-app");
    assert!(q.contains("namespace=\"staging\""));
    assert!(q.contains("pod=~\"web-app-.*\""));
    assert_eq!(unit, "cores");
}

#[test]
fn build_query_request_rate_uses_http_requests_total() {
    let (q, unit) = build_query(QueryKind::RequestRate, "ns", "svc");
    assert!(q.contains("http_requests_total"));
    assert_eq!(unit, "req/s");
}

// ---- build_fallback_query ----

#[test]
fn build_fallback_query_uses_server_requests_total() {
    let (q, _) = build_fallback_query(QueryKind::RequestRate, "ns", "svc");
    assert!(q.contains("http_server_requests_total"));
}

#[test]
fn build_fallback_query_error_rate_filters_5xx() {
    let (q, _) = build_fallback_query(QueryKind::ErrorRate, "ns", "svc");
    assert!(q.contains("http_server_requests_total"));
    assert!(q.contains("status=~\"5..\""));
}

// ---- has_fallback ----

#[test]
fn has_fallback_true_for_rate_queries() {
    assert!(has_fallback(QueryKind::RequestRate));
    assert!(has_fallback(QueryKind::ErrorRate));
}

#[test]
fn has_fallback_false_for_resource_queries() {
    assert!(!has_fallback(QueryKind::CpuUsage));
    assert!(!has_fallback(QueryKind::MemoryUsage));
}

// ---- validate_selector (extending the inline tests) ----

#[test]
fn validate_selector_accepts_dotted_names() {
    assert!(validate_selector("app.v2.beta").is_ok());
}

#[test]
fn validate_selector_accepts_underscored_names() {
    assert!(validate_selector("my_app_v3").is_ok());
}

#[test]
fn validate_selector_rejects_too_long() {
    let long = "a".repeat(254);
    assert!(validate_selector(&long).is_err());
}

#[test]
fn validate_selector_accepts_max_length() {
    let max = "a".repeat(253);
    assert!(validate_selector(&max).is_ok());
}

// ---- clamp_step (extending the inline tests) ----

#[test]
fn clamp_step_floors_at_15() {
    // 60s range / MAX_POINTS = tiny. Should still be at least 15.
    let s = clamp_step(Some(1), 60);
    assert!(s >= 15);
}

#[test]
fn clamp_step_user_above_minimum_is_honored() {
    // 1h range, min_step = max(3600/2000, 15) = 15. User wants 30 -> OK.
    assert_eq!(clamp_step(Some(30), 3600), 30);
}
