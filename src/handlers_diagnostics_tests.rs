// Unit tests for src/handlers/diagnostics.rs

use super::*;

// ---- DiagnoseRequest deserialization ----

#[test]
fn diagnose_request_deserializes_full_fields() {
    let json = serde_json::json!({
        "pod_name": "my-pod-abc123",
        "container": "main",
        "logs": "ERROR: something broke",
        "agent": "claude"
    });
    let req: DiagnoseRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.pod_name, "my-pod-abc123");
    assert_eq!(req.container.as_deref(), Some("main"));
    assert_eq!(req.logs, "ERROR: something broke");
    assert_eq!(req.agent, DiagAgent::Claude);
}

#[test]
fn diagnose_request_deserializes_minimal_fields() {
    let json = serde_json::json!({
        "pod_name": "pod-1",
        "logs": "some logs",
        "agent": "codex"
    });
    let req: DiagnoseRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.pod_name, "pod-1");
    assert!(req.container.is_none());
    assert_eq!(req.agent, DiagAgent::Codex);
}

#[test]
fn diagnose_request_rejects_missing_pod_name() {
    let json = serde_json::json!({
        "logs": "some logs",
        "agent": "claude"
    });
    assert!(serde_json::from_value::<DiagnoseRequest>(json).is_err());
}

#[test]
fn diagnose_request_rejects_missing_agent() {
    let json = serde_json::json!({
        "pod_name": "pod-1",
        "logs": "log data"
    });
    assert!(serde_json::from_value::<DiagnoseRequest>(json).is_err());
}

// ---- DiagnoseResponse serialization ----

#[test]
fn diagnose_response_serializes_correctly() {
    let resp = DiagnoseResponse {
        job_name: "dw-diag-claude-abcd1234".to_string(),
        status: DiagStatus::Pending,
        agent: DiagAgent::Claude,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["job_name"], "dw-diag-claude-abcd1234");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["agent"], "claude");
}

// ---- DiagnosticStatusResponse serialization ----

#[test]
fn diagnostic_status_response_serializes_all_statuses() {
    for (status, expected) in [
        (DiagStatus::Pending, "pending"),
        (DiagStatus::Running, "running"),
        (DiagStatus::Succeeded, "succeeded"),
        (DiagStatus::Failed, "failed"),
    ] {
        let resp = DiagnosticStatusResponse {
            job_name: "job-1".to_string(),
            status,
            agent: Some(DiagAgent::Codex),
            source_pod: Some("my-pod".to_string()),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            completed_at: None,
            message: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], expected);
    }
}

#[test]
fn diagnostic_status_response_serializes_with_all_optional_fields() {
    let resp = DiagnosticStatusResponse {
        job_name: "job-2".to_string(),
        status: DiagStatus::Succeeded,
        agent: Some(DiagAgent::Claude),
        source_pod: Some("nginx-abc123".to_string()),
        started_at: Some("2024-06-01T10:00:00Z".to_string()),
        completed_at: Some("2024-06-01T10:05:00Z".to_string()),
        message: Some("Job completed".to_string()),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["agent"], "claude");
    assert_eq!(json["source_pod"], "nginx-abc123");
    assert_eq!(json["started_at"], "2024-06-01T10:00:00Z");
    assert_eq!(json["completed_at"], "2024-06-01T10:05:00Z");
    assert_eq!(json["message"], "Job completed");
}

#[test]
fn diagnostic_status_response_serializes_with_no_optional_fields() {
    let resp = DiagnosticStatusResponse {
        job_name: "job-3".to_string(),
        status: DiagStatus::Pending,
        agent: None,
        source_pod: None,
        started_at: None,
        completed_at: None,
        message: None,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["job_name"], "job-3");
    assert_eq!(json["status"], "pending");
    assert!(json["agent"].is_null());
    assert!(json["source_pod"].is_null());
}

// ---- DiagAgent enum serialization/deserialization ----

#[test]
fn diag_agent_serializes_lowercase() {
    let claude_json = serde_json::to_value(DiagAgent::Claude).unwrap();
    let codex_json = serde_json::to_value(DiagAgent::Codex).unwrap();
    assert_eq!(claude_json, "claude");
    assert_eq!(codex_json, "codex");
}

#[test]
fn diag_agent_deserializes_lowercase() {
    let claude: DiagAgent = serde_json::from_str(r#""claude""#).unwrap();
    let codex: DiagAgent = serde_json::from_str(r#""codex""#).unwrap();
    assert_eq!(claude, DiagAgent::Claude);
    assert_eq!(codex, DiagAgent::Codex);
}

#[test]
fn diag_agent_rejects_unknown_variant() {
    let result = serde_json::from_str::<DiagAgent>(r#""gemini""#);
    assert!(result.is_err());
}

#[test]
fn diag_agent_round_trips() {
    for agent in [DiagAgent::Claude, DiagAgent::Codex] {
        let serialized = serde_json::to_string(&agent).unwrap();
        let deserialized: DiagAgent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(agent, deserialized);
    }
}

#[test]
fn diag_agent_as_str() {
    assert_eq!(DiagAgent::Claude.as_str(), "claude");
    assert_eq!(DiagAgent::Codex.as_str(), "codex");
}

// ---- truncate_label_value ----

#[test]
fn truncate_label_value_short_string_unchanged() {
    let input = "my-pod-name";
    assert_eq!(truncate_label_value(input), input);
}

#[test]
fn truncate_label_value_exactly_63_chars_unchanged() {
    let input = "a".repeat(63);
    assert_eq!(truncate_label_value(&input), input);
}

#[test]
fn truncate_label_value_over_63_chars_truncated() {
    let input = "a".repeat(100);
    let result = truncate_label_value(&input);
    assert!(result.len() <= 63);
}

#[test]
fn truncate_label_value_trims_trailing_dashes() {
    // Create a string where char 63 is mid-dash-run so the trimming fires.
    let mut input = "a".repeat(60);
    input.push_str("---extra-stuff");
    let result = truncate_label_value(&input);
    assert!(result.len() <= 63);
    assert!(!result.ends_with('-'));
}

// ---- make_short_name ----

#[test]
fn make_short_name_fits_within_63_chars() {
    let name = make_short_name(
        "dw-diag",
        "claude",
        "some-very-long-pod-name-that-goes-on",
        1234567890,
    );
    assert!(
        name.len() <= 63,
        "name '{}' is {} chars, exceeds 63",
        name,
        name.len()
    );
}

#[test]
fn make_short_name_starts_with_prefix() {
    let name = make_short_name("dw-diag", "codex", "pod-1", 100);
    assert!(name.starts_with("dw-diag-codex-"));
}

#[test]
fn make_short_name_deterministic_same_inputs() {
    let a = make_short_name("dw-diag", "claude", "pod-1", 100);
    let b = make_short_name("dw-diag", "claude", "pod-1", 100);
    assert_eq!(a, b);
}

#[test]
fn make_short_name_different_for_different_timestamps() {
    let a = make_short_name("dw-diag", "claude", "pod-1", 100);
    let b = make_short_name("dw-diag", "claude", "pod-1", 200);
    assert_ne!(a, b);
}

#[test]
fn make_short_name_different_for_different_sources() {
    let a = make_short_name("dw-diag", "claude", "pod-1", 100);
    let b = make_short_name("dw-diag", "claude", "pod-2", 100);
    assert_ne!(a, b);
}

// ---- sanitize_name_segment ----

#[test]
fn sanitize_name_segment_lowercases_and_replaces_specials() {
    let result = sanitize_name_segment("My_Pod.Name-123");
    assert_eq!(result, "my-pod-name-123");
}

#[test]
fn sanitize_name_segment_trims_leading_trailing_dashes() {
    let result = sanitize_name_segment("---pod---");
    assert_eq!(result, "pod");
}

#[test]
fn sanitize_name_segment_empty_input_falls_back() {
    let result = sanitize_name_segment("---");
    assert_eq!(result, "pod");
}

#[test]
fn sanitize_name_segment_truncates_to_40_chars() {
    let long = "a".repeat(60);
    let result = sanitize_name_segment(&long);
    assert_eq!(result.len(), 40);
}

// ---- truncate_logs ----

#[test]
fn truncate_logs_short_input_unchanged() {
    let logs = "short log output";
    assert_eq!(truncate_logs(logs, 1024), logs);
}

#[test]
fn truncate_logs_long_input_keeps_tail() {
    let logs = format!("{}tail-end", "x".repeat(1000));
    let result = truncate_logs(&logs, 100);
    assert!(result.contains("tail-end"));
    assert!(result.contains("[...truncated"));
}

// ---- DiagnosticResultResponse serialization ----

#[test]
fn diagnostic_result_response_serializes() {
    let resp = DiagnosticResultResponse {
        job_name: "job-1".to_string(),
        status: DiagStatus::Succeeded,
        output: "The issue is a misconfigured probe.".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["job_name"], "job-1");
    assert_eq!(json["status"], "succeeded");
    assert_eq!(json["output"], "The issue is a misconfigured probe.");
}

// ---- DiagnosticHistoryItem / DiagnosticHistoryResponse serialization ----

#[test]
fn diagnostic_history_response_serializes() {
    let resp = DiagnosticHistoryResponse {
        items: vec![
            DiagnosticHistoryItem {
                job_name: "dw-diag-claude-aabb1122".to_string(),
                status: DiagStatus::Succeeded,
                agent: Some(DiagAgent::Claude),
                source_pod: Some("web-abc".to_string()),
                started_at: Some("2024-01-01T00:00:00Z".to_string()),
                completed_at: Some("2024-01-01T00:01:00Z".to_string()),
                created_at: Some("2024-01-01T00:00:00Z".to_string()),
            },
            DiagnosticHistoryItem {
                job_name: "dw-diag-codex-ccdd3344".to_string(),
                status: DiagStatus::Failed,
                agent: Some(DiagAgent::Codex),
                source_pod: None,
                started_at: None,
                completed_at: None,
                created_at: None,
            },
        ],
    };
    let json = serde_json::to_value(&resp).unwrap();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["agent"], "claude");
    assert_eq!(items[1]["status"], "failed");
}

// ---- parse_agent ----

#[test]
fn parse_agent_known_values() {
    assert_eq!(parse_agent("claude"), Some(DiagAgent::Claude));
    assert_eq!(parse_agent("codex"), Some(DiagAgent::Codex));
}

#[test]
fn parse_agent_unknown_returns_none() {
    assert_eq!(parse_agent("gemini"), None);
    assert_eq!(parse_agent(""), None);
    assert_eq!(parse_agent("Claude"), None); // case-sensitive
}

// ---- Tests for direct Anthropic API integration ----

#[test]
fn test_diagnose_request_with_agent() {
    // Verify DiagnoseRequest still deserializes correctly with both agent variants.
    for agent_str in &["claude", "codex"] {
        let json = serde_json::json!({
            "pod_name": "test-pod",
            "logs": "some error output",
            "agent": agent_str
        });
        let req: DiagnoseRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.pod_name, "test-pod");
        assert_eq!(req.logs, "some error output");
        assert!(req.container.is_none());
    }
}

#[test]
fn test_diag_prompt_is_non_empty_and_contains_keywords() {
    // DIAG_PROMPT is the system instruction sent to the AI. Verify it
    // contains expected keywords for Kubernetes diagnostics.
    assert!(!DIAG_PROMPT.is_empty());
    assert!(
        DIAG_PROMPT.contains("Kubernetes"),
        "DIAG_PROMPT should mention Kubernetes"
    );
    assert!(
        DIAG_PROMPT.contains("logs"),
        "DIAG_PROMPT should mention logs"
    );
    assert!(
        DIAG_PROMPT.contains("diagnose") || DIAG_PROMPT.contains("Analyze"),
        "DIAG_PROMPT should mention diagnosing or analyzing"
    );
}

#[test]
fn test_diagnostic_status_response_serde_simplified() {
    // With the direct API refactor, DiagnosticStatusResponse still needs
    // to serialize correctly even when most fields are None (the simplified
    // model used in direct API mode).
    let resp = DiagnosticStatusResponse {
        job_name: "inline-stream".to_string(),
        status: DiagStatus::Succeeded,
        agent: None,
        source_pod: None,
        started_at: None,
        completed_at: None,
        message: None,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["job_name"], "inline-stream");
    assert_eq!(json["status"], "succeeded");
    // All optional fields should be present as null in the JSON output.
    assert!(json["agent"].is_null());
    assert!(json["source_pod"].is_null());
    assert!(json["started_at"].is_null());
    assert!(json["completed_at"].is_null());
    assert!(json["message"].is_null());
}

#[test]
fn test_list_diagnostics_empty() {
    // In direct API mode, list_diagnostics returns an empty history.
    // Verify DiagnosticHistoryResponse serializes correctly with no items.
    let resp = DiagnosticHistoryResponse { items: vec![] };
    let json = serde_json::to_value(&resp).unwrap();
    let items = json["items"].as_array().unwrap();
    assert!(items.is_empty());
}
