// Unit tests for src/handlers/ai_fix.rs

use super::*;

// ---- AiFixRequest deserialization ----

#[test]
fn ai_fix_request_deserializes_claude() {
    let json = serde_json::json!({ "agent": "claude" });
    let req: AiFixRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.agent, DiagAgent::Claude);
}

#[test]
fn ai_fix_request_deserializes_codex() {
    let json = serde_json::json!({ "agent": "codex" });
    let req: AiFixRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.agent, DiagAgent::Codex);
}

#[test]
fn ai_fix_request_rejects_missing_agent() {
    let json = serde_json::json!({});
    assert!(serde_json::from_value::<AiFixRequest>(json).is_err());
}

#[test]
fn ai_fix_request_rejects_unknown_agent() {
    let json = serde_json::json!({ "agent": "gemini" });
    assert!(serde_json::from_value::<AiFixRequest>(json).is_err());
}

// ---- AiFixResponse serialization ----

#[test]
fn ai_fix_response_serializes_correctly() {
    let resp = AiFixResponse {
        job_name: "dw-aifix-claude-aabb1122".to_string(),
        status: DiagStatus::Pending,
        agent: DiagAgent::Claude,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["job_name"], "dw-aifix-claude-aabb1122");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["agent"], "claude");
}

#[test]
fn ai_fix_response_serializes_codex_agent() {
    let resp = AiFixResponse {
        job_name: "dw-aifix-codex-ccdd3344".to_string(),
        status: DiagStatus::Running,
        agent: DiagAgent::Codex,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["agent"], "codex");
    assert_eq!(json["status"], "running");
}

// ---- truncate_label_value ----

#[test]
fn truncate_label_value_short_string_unchanged() {
    let input = "my-app";
    assert_eq!(truncate_label_value(input), input);
}

#[test]
fn truncate_label_value_exactly_63_chars_unchanged() {
    let input = "a".repeat(63);
    assert_eq!(truncate_label_value(&input), input);
}

#[test]
fn truncate_label_value_over_63_chars_truncated() {
    let input = "b".repeat(100);
    let result = truncate_label_value(&input);
    assert!(result.len() <= 63);
}

#[test]
fn truncate_label_value_trims_trailing_dashes_after_cut() {
    let mut input = "c".repeat(60);
    input.push_str("---more-stuff");
    let result = truncate_label_value(&input);
    assert!(result.len() <= 63);
    assert!(!result.ends_with('-'));
}

// ---- make_short_name ----

#[test]
fn make_short_name_fits_within_63_chars() {
    let name = make_short_name(
        "dw-aifix",
        "claude",
        "a-very-long-application-name-that-keeps-going",
        9999999999,
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
    let name = make_short_name("dw-aifix", "codex", "my-app", 100);
    assert!(name.starts_with("dw-aifix-codex-"));
}

#[test]
fn make_short_name_deterministic_same_inputs() {
    let a = make_short_name("dw-aifix", "claude", "app-1", 500);
    let b = make_short_name("dw-aifix", "claude", "app-1", 500);
    assert_eq!(a, b);
}

#[test]
fn make_short_name_different_for_different_timestamps() {
    let a = make_short_name("dw-aifix", "claude", "app-1", 100);
    let b = make_short_name("dw-aifix", "claude", "app-1", 200);
    assert_ne!(a, b);
}

#[test]
fn make_short_name_different_for_different_sources() {
    let a = make_short_name("dw-aifix", "claude", "app-1", 100);
    let b = make_short_name("dw-aifix", "claude", "app-2", 100);
    assert_ne!(a, b);
}

// ---- sanitize_name_segment ----

#[test]
fn sanitize_name_segment_lowercases_and_replaces_specials() {
    let result = sanitize_name_segment("My_App.Name-v2");
    assert_eq!(result, "my-app-name-v2");
}

#[test]
fn sanitize_name_segment_trims_leading_trailing_dashes() {
    let result = sanitize_name_segment("---app---");
    assert_eq!(result, "app");
}

#[test]
fn sanitize_name_segment_empty_input_falls_back_to_app() {
    // The ai_fix version falls back to "app" for empty input
    let result = sanitize_name_segment("---");
    assert_eq!(result, "app");
}

#[test]
fn sanitize_name_segment_truncates_to_40_chars() {
    let long = "x".repeat(60);
    let result = sanitize_name_segment(&long);
    assert_eq!(result.len(), 40);
}

// ---- truncate_tail ----

#[test]
fn truncate_tail_short_input_unchanged() {
    let logs = "brief crash output";
    assert_eq!(truncate_tail(logs, 1024), logs);
}

#[test]
fn truncate_tail_long_input_keeps_tail() {
    let logs = format!("{}the-actual-error", "padding-".repeat(200));
    let result = truncate_tail(&logs, 100);
    assert!(result.contains("the-actual-error"));
    assert!(result.contains("[...truncated"));
}

#[test]
fn truncate_tail_exact_boundary() {
    let logs = "exactly";
    assert_eq!(truncate_tail(logs, logs.len()), logs);
}

// ---- ApplicationData round-trip ----

#[test]
fn application_data_deserializes_full() {
    let json = serde_json::json!({
        "name": "my-app",
        "description": "A test application",
        "git": {
            "repo_url": "https://github.com/example/repo",
            "branch": "develop",
            "token_secret": "my-git-secret"
        }
    });
    let data: ApplicationData = serde_json::from_value(json).unwrap();
    assert_eq!(data.name, "my-app");
    assert_eq!(data.description, "A test application");
    let git = data.git.unwrap();
    assert_eq!(git.repo_url, "https://github.com/example/repo");
    assert_eq!(git.branch.as_deref(), Some("develop"));
}

#[test]
fn application_data_deserializes_minimal() {
    let json = serde_json::json!({ "name": "bare-app" });
    let data: ApplicationData = serde_json::from_value(json).unwrap();
    assert_eq!(data.name, "bare-app");
    assert!(data.description.is_empty());
    assert!(data.git.is_none());
}

// ---- cm_name / member_selector helpers ----

#[test]
fn cm_name_formats_correctly() {
    assert_eq!(cm_name("web"), "deckwatch-app-web");
    assert_eq!(cm_name("billing-api"), "deckwatch-app-billing-api");
}

#[test]
fn member_selector_formats_correctly() {
    assert_eq!(member_selector("web"), "deckwatch.io/application=web");
}
