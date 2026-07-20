// Unit tests for request/response types and helpers in src/handlers/promote.rs

use super::*;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};

// ---- PromoteRequest deserialization ----

#[test]
fn promote_request_minimal() {
    let json = r#"{"target_namespace": "production"}"#;
    let req: PromoteRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.target_namespace, "production");
    assert!(req.target_name.is_none());
    assert!(req.change_cause.is_none());
}

#[test]
fn promote_request_full() {
    let json = r#"{
        "target_namespace": "staging",
        "target_name": "api-v2",
        "change_cause": "hotfix: rollback stripe integration"
    }"#;
    let req: PromoteRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.target_namespace, "staging");
    assert_eq!(req.target_name.as_deref(), Some("api-v2"));
    assert_eq!(
        req.change_cause.as_deref(),
        Some("hotfix: rollback stripe integration")
    );
}

// ---- PromoteQuery deserialization ----

#[test]
fn promote_query_defaults_dry_run_false() {
    let json = r#"{}"#;
    let q: PromoteQuery = serde_json::from_str(json).unwrap();
    assert!(!q.dry_run);
}

#[test]
fn promote_query_dry_run_true() {
    let json = r#"{"dry_run": true}"#;
    let q: PromoteQuery = serde_json::from_str(json).unwrap();
    assert!(q.dry_run);
}

// ---- PromotePreview serialization ----

#[test]
fn promote_preview_serializes() {
    let preview = PromotePreview {
        source_namespace: "dev".to_string(),
        source_name: "api".to_string(),
        target_namespace: "prod".to_string(),
        target_name: "api".to_string(),
        changes: vec![PromoteFieldChange {
            field: "image".to_string(),
            from: Some("nginx:1.26".to_string()),
            to: Some("nginx:1.27".to_string()),
        }],
        no_op: false,
    };
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(json["source_namespace"], "dev");
    assert_eq!(json["source_name"], "api");
    assert_eq!(json["target_namespace"], "prod");
    assert_eq!(json["target_name"], "api");
    assert!(!json["no_op"].as_bool().unwrap());
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["field"], "image");
    assert_eq!(changes[0]["from"], "nginx:1.26");
    assert_eq!(changes[0]["to"], "nginx:1.27");
}

#[test]
fn promote_preview_no_op() {
    let preview = PromotePreview {
        source_namespace: "dev".to_string(),
        source_name: "api".to_string(),
        target_namespace: "prod".to_string(),
        target_name: "api".to_string(),
        changes: vec![],
        no_op: true,
    };
    let json = serde_json::to_value(&preview).unwrap();
    assert!(json["no_op"].as_bool().unwrap());
    assert!(json["changes"].as_array().unwrap().is_empty());
}

// ---- PromoteResponse serialization ----

#[test]
fn promote_response_flattens_preview() {
    let resp = PromoteResponse {
        preview: PromotePreview {
            source_namespace: "dev".to_string(),
            source_name: "web".to_string(),
            target_namespace: "staging".to_string(),
            target_name: "web".to_string(),
            changes: vec![],
            no_op: true,
        },
        target: None,
    };
    let json = serde_json::to_value(&resp).unwrap();
    // `#[serde(flatten)]` merges preview fields into the top level.
    assert_eq!(json["source_namespace"], "dev");
    assert_eq!(json["source_name"], "web");
    assert!(json["no_op"].as_bool().unwrap());
    // `target` is None with `skip_serializing_if`, so it should be absent.
    assert!(!json.as_object().unwrap().contains_key("target"));
}

// ---- Helper: make_deployment ----

fn make_deployment(name: &str, image: &str) -> Deployment {
    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            selector: LabelSelector::default(),
            template: PodTemplateSpec {
                metadata: None,
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: name.to_string(),
                        image: Some(image.to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn make_deployment_with_cmd(
    name: &str,
    image: &str,
    command: Option<Vec<String>>,
    args: Option<Vec<String>>,
) -> Deployment {
    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            selector: LabelSelector::default(),
            template: PodTemplateSpec {
                metadata: None,
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: name.to_string(),
                        image: Some(image.to_string()),
                        command,
                        args,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ---- compute_changes ----

#[test]
fn compute_changes_identical_deployments_no_changes() {
    let src = make_deployment("api", "myapp:v1.0");
    let dst = make_deployment("api", "myapp:v1.0");
    let changes = compute_changes(&src, &dst);
    assert!(changes.is_empty());
}

#[test]
fn compute_changes_detects_image_diff() {
    let src = make_deployment("api", "myapp:v2.0");
    let dst = make_deployment("api", "myapp:v1.0");
    let changes = compute_changes(&src, &dst);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "image");
    assert_eq!(changes[0].from.as_deref(), Some("myapp:v1.0"));
    assert_eq!(changes[0].to.as_deref(), Some("myapp:v2.0"));
}

#[test]
fn compute_changes_detects_command_diff() {
    let src = make_deployment_with_cmd(
        "worker",
        "myapp:v1",
        Some(vec!["/bin/sh".to_string()]),
        None,
    );
    let dst = make_deployment_with_cmd(
        "worker",
        "myapp:v1",
        Some(vec!["/bin/bash".to_string()]),
        None,
    );
    let changes = compute_changes(&src, &dst);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "command");
}

#[test]
fn compute_changes_detects_args_diff() {
    let src = make_deployment_with_cmd(
        "job",
        "myapp:v1",
        None,
        Some(vec!["--mode=prod".to_string()]),
    );
    let dst = make_deployment_with_cmd(
        "job",
        "myapp:v1",
        None,
        Some(vec!["--mode=dev".to_string()]),
    );
    let changes = compute_changes(&src, &dst);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "args");
}

#[test]
fn compute_changes_detects_multiple_diffs() {
    let src = make_deployment_with_cmd(
        "api",
        "myapp:v2",
        Some(vec!["/app/server".to_string()]),
        Some(vec!["--port=8080".to_string()]),
    );
    let dst = make_deployment_with_cmd(
        "api",
        "myapp:v1",
        Some(vec!["/app/old-server".to_string()]),
        Some(vec!["--port=3000".to_string()]),
    );
    let changes = compute_changes(&src, &dst);
    assert_eq!(changes.len(), 3);
    let fields: Vec<&str> = changes.iter().map(|c| c.field.as_str()).collect();
    assert!(fields.contains(&"image"));
    assert!(fields.contains(&"command"));
    assert!(fields.contains(&"args"));
}

// ---- primary_container ----

#[test]
fn primary_container_returns_first() {
    let dep = make_deployment("my-app", "nginx:latest");
    let container = primary_container(&dep).unwrap();
    assert_eq!(container.name, "my-app");
    assert_eq!(container.image.as_deref(), Some("nginx:latest"));
}

#[test]
fn primary_container_returns_none_for_empty_spec() {
    let dep = Deployment::default();
    assert!(primary_container(&dep).is_none());
}

// ---- build_promote_patch ----

#[test]
fn build_promote_patch_includes_image() {
    let src = make_deployment("api", "myapp:v2.0");
    let patch = build_promote_patch(&src, None);
    let containers = &patch["spec"]["template"]["spec"]["containers"];
    assert_eq!(containers[0]["image"], "myapp:v2.0");
    assert_eq!(containers[0]["name"], "api");
}

#[test]
fn build_promote_patch_default_change_cause() {
    let src = make_deployment("api", "myapp:v1");
    let patch = build_promote_patch(&src, None);
    let cause = patch["metadata"]["annotations"]["kubernetes.io/change-cause"]
        .as_str()
        .unwrap();
    assert_eq!(cause, "promoted via deckwatch");
}

#[test]
fn build_promote_patch_custom_change_cause() {
    let src = make_deployment("api", "myapp:v1");
    let patch = build_promote_patch(&src, Some("PR-1234: deploy new billing logic"));
    let cause = patch["metadata"]["annotations"]["kubernetes.io/change-cause"]
        .as_str()
        .unwrap();
    assert_eq!(cause, "PR-1234: deploy new billing logic");
}

#[test]
fn build_promote_patch_includes_command_and_args() {
    let src = make_deployment_with_cmd(
        "worker",
        "myapp:v1",
        Some(vec!["/app/run".to_string()]),
        Some(vec!["--verbose".to_string()]),
    );
    let patch = build_promote_patch(&src, None);
    let container = &patch["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(container["command"][0], "/app/run");
    assert_eq!(container["args"][0], "--verbose");
}
