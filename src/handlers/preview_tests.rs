// Unit tests for src/handlers/preview.rs — request/response types and helpers.

use super::*;

// ---- CreatePreviewRequest deserialization ----

#[test]
fn create_preview_request_minimal() {
    let json = r#"{"branch": "feature/foo"}"#;
    let req: CreatePreviewRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.branch, "feature/foo");
    assert!(req.pr_number.is_none());
    assert!(req.ttl_hours.is_none());
    assert!(req.host_suffix.is_none());
    assert!(req.ingress_class.is_none());
}

#[test]
fn create_preview_request_full() {
    let json = r#"{
        "branch": "release/v2",
        "pr_number": 42,
        "ttl_hours": 48,
        "host_suffix": "preview.example.com",
        "ingress_class": "nginx"
    }"#;
    let req: CreatePreviewRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.branch, "release/v2");
    assert_eq!(req.pr_number, Some(42));
    assert_eq!(req.ttl_hours, Some(48));
    assert_eq!(req.host_suffix.as_deref(), Some("preview.example.com"));
    assert_eq!(req.ingress_class.as_deref(), Some("nginx"));
}

#[test]
fn create_preview_request_rejects_missing_branch() {
    let json = r#"{"pr_number": 1}"#;
    let result = serde_json::from_str::<CreatePreviewRequest>(json);
    assert!(result.is_err());
}

#[test]
fn create_preview_request_defaults_optional_fields() {
    let json = r#"{"branch": "main"}"#;
    let req: CreatePreviewRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.pr_number, None);
    assert_eq!(req.ttl_hours, None);
    assert_eq!(req.host_suffix, None);
    assert_eq!(req.ingress_class, None);
}

// ---- PreviewSummary serialization ----

#[test]
fn preview_summary_serializes_all_fields() {
    let summary = PreviewSummary {
        name: "api-preview-feat".to_string(),
        namespace: "default".to_string(),
        source_deployment: "api".to_string(),
        branch: "feat".to_string(),
        pr_number: Some(7),
        expires_at: "2026-01-01T00:00:00Z".to_string(),
        host: Some("feat.preview.example.com".to_string()),
        image: "registry/api:abc123".to_string(),
        replicas_desired: 2,
        replicas_ready: 1,
        created_at: Some("2025-12-31T12:00:00Z".to_string()),
    };
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(json["name"], "api-preview-feat");
    assert_eq!(json["namespace"], "default");
    assert_eq!(json["source_deployment"], "api");
    assert_eq!(json["branch"], "feat");
    assert_eq!(json["pr_number"], 7);
    assert_eq!(json["host"], "feat.preview.example.com");
    assert_eq!(json["replicas_desired"], 2);
    assert_eq!(json["replicas_ready"], 1);
}

#[test]
fn preview_summary_serializes_none_pr_as_null() {
    let summary = PreviewSummary {
        name: "x".to_string(),
        namespace: "ns".to_string(),
        source_deployment: "src".to_string(),
        branch: "b".to_string(),
        pr_number: None,
        expires_at: "".to_string(),
        host: None,
        image: "".to_string(),
        replicas_desired: 0,
        replicas_ready: 0,
        created_at: None,
    };
    let json = serde_json::to_value(&summary).unwrap();
    assert!(json["pr_number"].is_null());
    assert!(json["host"].is_null());
    assert!(json["created_at"].is_null());
}

// ---- PreviewListResponse serialization ----

#[test]
fn preview_list_response_serializes_empty() {
    let resp = PreviewListResponse {
        previews: Vec::new(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["previews"].as_array().unwrap().is_empty());
}

#[test]
fn preview_list_response_serializes_multiple() {
    let resp = PreviewListResponse {
        previews: vec![
            PreviewSummary {
                name: "a-preview-x".to_string(),
                namespace: "ns".to_string(),
                source_deployment: "a".to_string(),
                branch: "x".to_string(),
                pr_number: None,
                expires_at: "".to_string(),
                host: None,
                image: "".to_string(),
                replicas_desired: 1,
                replicas_ready: 1,
                created_at: None,
            },
            PreviewSummary {
                name: "b-preview-y".to_string(),
                namespace: "ns".to_string(),
                source_deployment: "b".to_string(),
                branch: "y".to_string(),
                pr_number: Some(3),
                expires_at: "".to_string(),
                host: Some("y.preview.dev".to_string()),
                image: "img:latest".to_string(),
                replicas_desired: 2,
                replicas_ready: 0,
                created_at: None,
            },
        ],
    };
    let json = serde_json::to_value(&resp).unwrap();
    let arr = json["previews"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "a-preview-x");
    assert_eq!(arr[1]["pr_number"], 3);
}

// ---- helper: sanitize_branch (extending the inline tests) ----

#[test]
fn sanitize_branch_with_slashes_and_underscores() {
    assert_eq!(sanitize_branch("fix/BUG_42"), "fix-bug-42");
}

#[test]
fn sanitize_branch_collapses_multiple_special_chars() {
    assert_eq!(sanitize_branch("a//b__c"), "a-b-c");
}

#[test]
fn sanitize_branch_strips_trailing_dashes() {
    assert_eq!(sanitize_branch("foo-"), "foo");
    assert_eq!(sanitize_branch("bar---"), "bar");
}

// ---- helper: derive_preview_name ----

#[test]
fn derive_preview_name_short_inputs() {
    assert_eq!(derive_preview_name("web", "main"), "web-preview-main");
}

#[test]
fn derive_preview_name_at_63_chars_exactly() {
    // "src" (3) + "-preview-" (9) = 12 chars, so branch can be 51 chars
    let branch = "a".repeat(51);
    let name = derive_preview_name("src", &branch);
    assert_eq!(name.len(), 63);
}

#[test]
fn derive_preview_name_truncation_preserves_source() {
    let branch = "x".repeat(100);
    let name = derive_preview_name("myapp", &branch);
    assert!(name.len() <= 63);
    assert!(name.starts_with("myapp-preview-"));
}

// ---- helper: strip_managed_annotations ----

#[test]
fn strip_managed_annotations_removes_k8s_managed() {
    let mut anns = BTreeMap::new();
    anns.insert(
        "deployment.kubernetes.io/revision".to_string(),
        "1".to_string(),
    );
    anns.insert(
        "kubectl.kubernetes.io/last-applied-configuration".to_string(),
        "{}".to_string(),
    );
    anns.insert("deckwatch.io/git-enabled".to_string(), "true".to_string());
    anns.insert("custom/annotation".to_string(), "value".to_string());

    let result = strip_managed_annotations(anns);
    assert!(!result.contains_key("deployment.kubernetes.io/revision"));
    assert!(!result.contains_key("kubectl.kubernetes.io/last-applied-configuration"));
    assert!(result.contains_key("deckwatch.io/git-enabled"));
    assert!(result.contains_key("custom/annotation"));
}

// ---- helper: preview_ingress_name ----

#[test]
fn preview_ingress_name_appends_suffix() {
    assert_eq!(
        preview_ingress_name("api-preview-feat"),
        "api-preview-feat-preview"
    );
}

// ---- helper: source_service_port ----

#[test]
fn source_service_port_defaults_to_80() {
    let dep = Deployment::default();
    assert_eq!(source_service_port(&dep), 80);
}
