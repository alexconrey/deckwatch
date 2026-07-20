// Unit tests for the serializable types, pure helpers, and admission-error
// splitting in src/handlers/deployments_ux.rs.

use super::*;
use k8s_openapi::api::apps::v1::ReplicaSetSpec;
use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec};
use std::collections::BTreeMap;

// ---- RevisionSummary serialization ----

#[test]
fn revision_summary_serializes_all_fields() {
    let entry = RevisionSummary {
        revision: 3,
        replica_set_name: "nginx-abc123".to_string(),
        image: "nginx:1.25".to_string(),
        replicas: 2,
        ready_replicas: 2,
        created_at: Some("2025-01-15T10:00:00Z".to_string()),
        change_cause: Some("image update".to_string()),
        is_current: true,
    };
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["revision"], 3);
    assert_eq!(json["replica_set_name"], "nginx-abc123");
    assert_eq!(json["image"], "nginx:1.25");
    assert_eq!(json["replicas"], 2);
    assert_eq!(json["ready_replicas"], 2);
    assert_eq!(json["created_at"], "2025-01-15T10:00:00Z");
    assert_eq!(json["change_cause"], "image update");
    assert_eq!(json["is_current"], true);
}

#[test]
fn revision_summary_serializes_none_fields_as_null() {
    let entry = RevisionSummary {
        revision: 1,
        replica_set_name: "app-xyz".to_string(),
        image: "app:latest".to_string(),
        replicas: 1,
        ready_replicas: 0,
        created_at: None,
        change_cause: None,
        is_current: false,
    };
    let json = serde_json::to_value(&entry).unwrap();
    assert!(json["created_at"].is_null());
    assert!(json["change_cause"].is_null());
    assert_eq!(json["is_current"], false);
}

// ---- HistoryResponse serialization ----

#[test]
fn history_response_serializes_multiple_revisions() {
    let resp = HistoryResponse {
        revisions: vec![
            RevisionSummary {
                revision: 2,
                replica_set_name: "app-rev2".to_string(),
                image: "app:v2".to_string(),
                replicas: 3,
                ready_replicas: 3,
                created_at: Some("2025-06-01T00:00:00Z".to_string()),
                change_cause: None,
                is_current: true,
            },
            RevisionSummary {
                revision: 1,
                replica_set_name: "app-rev1".to_string(),
                image: "app:v1".to_string(),
                replicas: 3,
                ready_replicas: 0,
                created_at: Some("2025-05-01T00:00:00Z".to_string()),
                change_cause: Some("initial deploy".to_string()),
                is_current: false,
            },
        ],
    };
    let json = serde_json::to_value(&resp).unwrap();
    let revisions = json["revisions"].as_array().unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0]["revision"], 2);
    assert_eq!(revisions[1]["revision"], 1);
}

#[test]
fn history_response_serializes_empty_revisions() {
    let resp = HistoryResponse { revisions: vec![] };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["revisions"].as_array().unwrap().is_empty());
}

// ---- RollbackRequest deserialization ----

#[test]
fn rollback_request_deserializes_valid_revision() {
    let raw = r#"{"revision": 5}"#;
    let req: RollbackRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(req.revision, 5);
}

#[test]
fn rollback_request_rejects_missing_revision() {
    let raw = r#"{}"#;
    let result = serde_json::from_str::<RollbackRequest>(raw);
    assert!(result.is_err());
}

#[test]
fn rollback_request_rejects_string_revision() {
    let raw = r#"{"revision": "two"}"#;
    let result = serde_json::from_str::<RollbackRequest>(raw);
    assert!(result.is_err());
}

// ---- ValidateResponse serialization ----

#[test]
fn validate_response_serializes_success() {
    let resp = ValidateResponse {
        ok: true,
        errors: vec![],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["errors"].as_array().unwrap().is_empty());
}

#[test]
fn validate_response_serializes_failures() {
    let resp = ValidateResponse {
        ok: false,
        errors: vec!["exceeds quota".to_string(), "PSA violation".to_string()],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["ok"], false);
    let errors = json["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0], "exceeds quota");
    assert_eq!(errors[1], "PSA violation");
}

// ---- CloneRequest deserialization ----

#[test]
fn clone_request_deserializes_minimal() {
    let raw = r#"{"target_namespace": "staging"}"#;
    let req: CloneRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(req.target_namespace, "staging");
    assert!(req.new_name.is_none());
    assert!(!req.overwrite);
}

#[test]
fn clone_request_deserializes_full() {
    let raw = r#"{"target_namespace": "prod", "new_name": "app-clone", "overwrite": true}"#;
    let req: CloneRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(req.target_namespace, "prod");
    assert_eq!(req.new_name.as_deref(), Some("app-clone"));
    assert!(req.overwrite);
}

#[test]
fn clone_request_overwrite_defaults_to_false() {
    let raw = r#"{"target_namespace": "test", "new_name": "foo"}"#;
    let req: CloneRequest = serde_json::from_str(raw).unwrap();
    assert!(!req.overwrite);
}

#[test]
fn clone_request_rejects_missing_target_namespace() {
    let raw = r#"{"new_name": "foo"}"#;
    let result = serde_json::from_str::<CloneRequest>(raw);
    assert!(result.is_err());
}

// ---- CloneResponse serialization ----

#[test]
fn clone_response_flattens_detail() {
    // CloneResponse uses #[serde(flatten)] on detail — verify the JSON
    // shape merges the DeploymentDetailResponse fields at the top level
    // rather than nesting under a "detail" key.
    let resp = CloneResponse {
        detail: DeploymentDetailResponse {
            detail: crate::kube_ext::deployment_detail(&minimal_deployment("cloned")),
            pods: vec![],
            ingresses: vec![],
        },
        source_namespace: "default".to_string(),
        source_name: "app".to_string(),
        target_namespace: "staging".to_string(),
        target_name: "app-clone".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["source_namespace"], "default");
    assert_eq!(json["target_name"], "app-clone");
    // "name" comes from the flattened DeploymentDetail, not from a nested
    // "detail" key.
    assert_eq!(json["name"], "cloned");
    assert!(json.get("detail").is_none(), "detail should be flattened");
}

// ---- AutoRollbackRequest / AutoRollbackResponse ----

#[test]
fn auto_rollback_request_deserializes() {
    let raw = r#"{"enabled": true}"#;
    let req: AutoRollbackRequest = serde_json::from_str(raw).unwrap();
    assert!(req.enabled);

    let raw2 = r#"{"enabled": false}"#;
    let req2: AutoRollbackRequest = serde_json::from_str(raw2).unwrap();
    assert!(!req2.enabled);
}

#[test]
fn auto_rollback_request_rejects_missing_enabled() {
    let raw = r#"{}"#;
    assert!(serde_json::from_str::<AutoRollbackRequest>(raw).is_err());
}

#[test]
fn auto_rollback_response_serializes() {
    let resp = AutoRollbackResponse { enabled: true };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["enabled"], true);
}

// ---- split_admission_errors ----

#[test]
fn split_admission_errors_splits_on_newlines() {
    let errors = split_admission_errors("quota exceeded\nPSA violation");
    assert_eq!(errors, vec!["quota exceeded", "PSA violation"]);
}

#[test]
fn split_admission_errors_splits_on_semicolons() {
    let errors = split_admission_errors("error one; error two; error three");
    assert_eq!(errors, vec!["error one", "error two", "error three"]);
}

#[test]
fn split_admission_errors_trims_whitespace() {
    let errors = split_admission_errors("  padded  \n  also padded  ");
    assert_eq!(errors, vec!["padded", "also padded"]);
}

#[test]
fn split_admission_errors_drops_empty_segments() {
    let errors = split_admission_errors("one;;two\n\nthree");
    assert_eq!(errors, vec!["one", "two", "three"]);
}

#[test]
fn split_admission_errors_single_message() {
    let errors = split_admission_errors("single error");
    assert_eq!(errors, vec!["single error"]);
}

#[test]
fn split_admission_errors_empty_string() {
    let errors = split_admission_errors("");
    assert!(errors.is_empty());
}

// ---- strip_managed_annotations ----

#[test]
fn strip_managed_annotations_removes_deployment_kubernetes_io() {
    let mut a = BTreeMap::new();
    a.insert(
        "deployment.kubernetes.io/revision".to_string(),
        "3".to_string(),
    );
    a.insert(
        "deployment.kubernetes.io/desired-replicas".to_string(),
        "2".to_string(),
    );
    a.insert(
        "kubernetes.io/change-cause".to_string(),
        "rollback".to_string(),
    );
    a.insert("custom/annotation".to_string(), "keep-me".to_string());

    let result = strip_managed_annotations(a);
    assert!(!result.contains_key("deployment.kubernetes.io/revision"));
    assert!(!result.contains_key("deployment.kubernetes.io/desired-replicas"));
    assert_eq!(
        result.get("kubernetes.io/change-cause").unwrap(),
        "rollback"
    );
    assert_eq!(result.get("custom/annotation").unwrap(), "keep-me");
}

#[test]
fn strip_managed_annotations_preserves_all_when_no_managed_keys() {
    let mut a = BTreeMap::new();
    a.insert("app".to_string(), "nginx".to_string());
    let result = strip_managed_annotations(a);
    assert_eq!(result.len(), 1);
    assert_eq!(result.get("app").unwrap(), "nginx");
}

#[test]
fn strip_managed_annotations_handles_empty_map() {
    let result = strip_managed_annotations(BTreeMap::new());
    assert!(result.is_empty());
}

// ---- replica_set_to_revision ----

fn make_replica_set(
    name: &str,
    revision: &str,
    image: &str,
    replicas: i32,
    ready_replicas: i32,
    hash: Option<&str>,
    change_cause: Option<&str>,
) -> ReplicaSet {
    let mut annotations = BTreeMap::new();
    annotations.insert(REVISION_ANNOTATION.to_string(), revision.to_string());
    if let Some(cause) = change_cause {
        annotations.insert(CHANGE_CAUSE_ANNOTATION.to_string(), cause.to_string());
    }

    let mut labels = BTreeMap::new();
    if let Some(h) = hash {
        labels.insert("pod-template-hash".to_string(), h.to_string());
    }

    ReplicaSet {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            annotations: Some(annotations),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(ReplicaSetSpec {
            replicas: Some(replicas),
            template: Some(PodTemplateSpec {
                spec: Some(PodSpec {
                    containers: vec![Container {
                        image: Some(image.to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        status: Some(k8s_openapi::api::apps::v1::ReplicaSetStatus {
            ready_replicas: Some(ready_replicas),
            ..Default::default()
        }),
    }
}

#[test]
fn replica_set_to_revision_extracts_basic_fields() {
    let rs = make_replica_set("nginx-abc", "3", "nginx:1.25", 2, 2, Some("abc"), None);
    let rev = replica_set_to_revision(&rs, Some("abc")).unwrap();
    assert_eq!(rev.revision, 3);
    assert_eq!(rev.replica_set_name, "nginx-abc");
    assert_eq!(rev.image, "nginx:1.25");
    assert_eq!(rev.replicas, 2);
    assert_eq!(rev.ready_replicas, 2);
    assert!(rev.is_current);
    assert!(rev.change_cause.is_none());
}

#[test]
fn replica_set_to_revision_marks_not_current_when_hash_differs() {
    let rs = make_replica_set("nginx-old", "1", "nginx:1.24", 3, 0, Some("old"), None);
    let rev = replica_set_to_revision(&rs, Some("current")).unwrap();
    assert!(!rev.is_current);
}

#[test]
fn replica_set_to_revision_marks_not_current_when_no_current_hash() {
    let rs = make_replica_set("nginx-old", "1", "nginx:1.24", 3, 0, Some("abc"), None);
    let rev = replica_set_to_revision(&rs, None).unwrap();
    assert!(!rev.is_current);
}

#[test]
fn replica_set_to_revision_includes_change_cause() {
    let rs = make_replica_set("app-v2", "2", "app:v2", 1, 1, None, Some("image update"));
    let rev = replica_set_to_revision(&rs, None).unwrap();
    assert_eq!(rev.change_cause.as_deref(), Some("image update"));
}

#[test]
fn replica_set_to_revision_returns_none_when_no_annotations() {
    let rs = ReplicaSet {
        metadata: ObjectMeta {
            name: Some("no-annot".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(replica_set_to_revision(&rs, None).is_none());
}

#[test]
fn replica_set_to_revision_returns_none_when_revision_annotation_missing() {
    let mut annotations = BTreeMap::new();
    annotations.insert("other-key".to_string(), "value".to_string());
    let rs = ReplicaSet {
        metadata: ObjectMeta {
            name: Some("missing-rev".to_string()),
            annotations: Some(annotations),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(replica_set_to_revision(&rs, None).is_none());
}

#[test]
fn replica_set_to_revision_returns_none_when_revision_is_not_numeric() {
    let mut annotations = BTreeMap::new();
    annotations.insert(REVISION_ANNOTATION.to_string(), "notanumber".to_string());
    let rs = ReplicaSet {
        metadata: ObjectMeta {
            annotations: Some(annotations),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(replica_set_to_revision(&rs, None).is_none());
}

// ---- build_deployment_from_request ----

#[test]
fn build_deployment_from_request_minimal() {
    let req = CreateDeploymentRequest {
        name: "myapp".to_string(),
        image: "myapp:v1".to_string(),
        replicas: None,
        port: None,
        ports: None,
        env: None,
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let dep = build_deployment_from_request("default", &req).unwrap();
    let meta = dep.metadata;
    assert_eq!(meta.name.as_deref(), Some("myapp"));
    assert_eq!(meta.namespace.as_deref(), Some("default"));

    let labels = meta.labels.unwrap();
    assert_eq!(labels.get("app").unwrap(), "myapp");
    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").unwrap(),
        "deckwatch"
    );

    let spec = dep.spec.unwrap();
    assert_eq!(spec.replicas, Some(1)); // default
    let container = &spec.template.spec.unwrap().containers[0];
    assert_eq!(container.image.as_deref(), Some("myapp:v1"));
    assert!(container.ports.is_none());
    assert!(container.env.is_none());
}

#[test]
fn build_deployment_from_request_rejects_empty_name() {
    let req = CreateDeploymentRequest {
        name: "".to_string(),
        image: "img:v1".to_string(),
        replicas: None,
        port: None,
        ports: None,
        env: None,
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let result = build_deployment_from_request("ns", &req);
    assert!(result.is_err());
}

#[test]
fn build_deployment_from_request_rejects_empty_image() {
    let req = CreateDeploymentRequest {
        name: "myapp".to_string(),
        image: "".to_string(),
        replicas: None,
        port: None,
        ports: None,
        env: None,
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let result = build_deployment_from_request("ns", &req);
    assert!(result.is_err());
}

#[test]
fn build_deployment_from_request_sets_replicas() {
    let req = CreateDeploymentRequest {
        name: "app".to_string(),
        image: "app:v1".to_string(),
        replicas: Some(5),
        port: None,
        ports: None,
        env: None,
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let dep = build_deployment_from_request("ns", &req).unwrap();
    assert_eq!(dep.spec.unwrap().replicas, Some(5));
}

#[test]
fn build_deployment_from_request_sets_env() {
    let req = CreateDeploymentRequest {
        name: "app".to_string(),
        image: "app:v1".to_string(),
        replicas: None,
        port: None,
        ports: None,
        env: Some(vec![
            crate::handlers::deployments::EnvVarInput {
                name: "FOO".to_string(),
                value: "bar".to_string(),
            },
            crate::handlers::deployments::EnvVarInput {
                name: "BAZ".to_string(),
                value: "qux".to_string(),
            },
        ]),
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let dep = build_deployment_from_request("ns", &req).unwrap();
    let container = &dep.spec.unwrap().template.spec.unwrap().containers[0];
    let env = container.env.as_ref().unwrap();
    assert_eq!(env.len(), 2);
    assert_eq!(env[0].name, "FOO");
    assert_eq!(env[0].value.as_deref(), Some("bar"));
}

#[test]
fn build_deployment_from_request_sets_port() {
    let req = CreateDeploymentRequest {
        name: "app".to_string(),
        image: "app:v1".to_string(),
        replicas: None,
        port: Some(8080),
        ports: None,
        env: None,
        labels: None,
        command: None,
        args: None,
        resource_limits: None,
        resource_requests: None,
        liveness_probe: None,
        readiness_probe: None,
        startup_probe: None,
    };
    let dep = build_deployment_from_request("ns", &req).unwrap();
    let container = &dep.spec.unwrap().template.spec.unwrap().containers[0];
    let ports = container.ports.as_ref().unwrap();
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].container_port, 8080);
}

// ---- helper for building a minimal Deployment ----

fn minimal_deployment(name: &str) -> Deployment {
    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}
