// Unit tests for src/handlers/admission.rs — response construction, serialization, policies.

use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

// ---- AdmissionReview response construction ----

#[test]
fn empty_review_returns_allowed() {
    let review = empty_review("test-uid-123");
    assert_eq!(review.api_version, "admission.k8s.io/v1");
    assert_eq!(review.kind, "AdmissionReview");
    assert!(review.request.is_none());
    let resp = review.response.unwrap();
    assert_eq!(resp.uid, "test-uid-123");
    assert!(resp.allowed);
    assert!(resp.status.is_none());
    assert!(resp.warnings.is_empty());
}

// ---- AdmissionReview serialization round-trip ----

#[test]
fn admission_review_round_trips_through_json() {
    let review = AdmissionReview {
        api_version: "admission.k8s.io/v1".to_string(),
        kind: "AdmissionReview".to_string(),
        request: None,
        response: Some(AdmissionResponse {
            uid: "abc".to_string(),
            allowed: true,
            status: None,
            warnings: vec!["watch out".to_string()],
        }),
    };
    let json = serde_json::to_string(&review).unwrap();
    let back: AdmissionReview = serde_json::from_str(&json).unwrap();
    assert_eq!(back.api_version, "admission.k8s.io/v1");
    let resp = back.response.unwrap();
    assert_eq!(resp.uid, "abc");
    assert!(resp.allowed);
    assert_eq!(resp.warnings, vec!["watch out"]);
}

// ---- Allowed response serialization ----

#[test]
fn allowed_response_omits_status_and_empty_warnings() {
    let resp = AdmissionResponse {
        uid: "u1".to_string(),
        allowed: true,
        status: None,
        warnings: Vec::new(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json.get("status").is_none());
    assert!(json.get("warnings").is_none());
}

// ---- Denied response serialization ----

#[test]
fn denied_response_includes_status() {
    let resp = AdmissionResponse {
        uid: "u2".to_string(),
        allowed: false,
        status: Some(AdmissionStatus {
            code: 403,
            message: "forbidden".to_string(),
        }),
        warnings: Vec::new(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["allowed"], false);
    assert_eq!(json["status"]["code"], 403);
    assert_eq!(json["status"]["message"], "forbidden");
}

// ---- evaluate: normal workload passes with no warnings ----

#[test]
fn evaluate_normal_deployment_passes_cleanly() {
    let container = serde_json::json!({
        "name": "app",
        "readinessProbe": {"httpGet": {"path": "/healthz", "port": 8080}},
        "resources": {
            "requests": {"memory": "128Mi"},
            "limits": {"memory": "256Mi"}
        }
    });
    let dep = serde_json::json!({
        "metadata": {"name": "svc", "namespace": "prod"},
        "spec": {
            "selector": {"matchLabels": {"app": "svc"}},
            "template": {
                "metadata": {},
                "spec": {"containers": [container]}
            }
        }
    });
    let req = AdmissionRequest {
        uid: "clean".to_string(),
        kind: GroupVersionKind {
            kind: "Deployment".to_string(),
            ..Default::default()
        },
        resource: GroupVersionResource::default(),
        name: Some("svc".to_string()),
        namespace: Some("prod".to_string()),
        operation: "CREATE".to_string(),
        user_info: UserInfo::default(),
        object: Some(dep),
        old_object: None,
        dry_run: false,
    };
    let resp = evaluate(&req, &WebhookConfig::default());
    assert!(resp.allowed);
    assert!(resp.warnings.is_empty());
}

// ---- evaluate: protected namespace denied for external actors ----

#[test]
fn evaluate_denies_kube_node_lease() {
    let req = AdmissionRequest {
        uid: "kns".to_string(),
        kind: GroupVersionKind::default(),
        resource: GroupVersionResource::default(),
        name: Some("x".to_string()),
        namespace: Some("kube-node-lease".to_string()),
        operation: "CREATE".to_string(),
        user_info: UserInfo {
            username: "external-user".to_string(),
            ..Default::default()
        },
        object: None,
        old_object: None,
        dry_run: false,
    };
    let resp = evaluate(&req, &WebhookConfig::default());
    assert!(!resp.allowed);
    assert!(resp.status.is_some());
    assert_eq!(resp.status.as_ref().unwrap().code, 403);
}

// ---- evaluate: disabled policies ----

#[test]
fn evaluate_skips_disabled_readiness_check() {
    let container = serde_json::json!({"name": "app"});
    let dep = serde_json::json!({
        "metadata": {"name": "svc", "namespace": "dev"},
        "spec": {
            "selector": {"matchLabels": {}},
            "template": {"spec": {"containers": [container]}}
        }
    });
    let cfg = WebhookConfig {
        enabled: true,
        policies: WebhookPolicies {
            deployment_missing_readiness_probe: false,
            ..Default::default()
        },
    };
    let req = AdmissionRequest {
        uid: "skip".to_string(),
        kind: GroupVersionKind {
            kind: "Deployment".to_string(),
            ..Default::default()
        },
        resource: GroupVersionResource::default(),
        name: Some("svc".to_string()),
        namespace: Some("dev".to_string()),
        operation: "CREATE".to_string(),
        user_info: UserInfo::default(),
        object: Some(dep),
        old_object: None,
        dry_run: false,
    };
    let resp = evaluate(&req, &cfg);
    assert!(resp.allowed);
    assert!(
        !resp.warnings.iter().any(|w| w.contains("readinessProbe")),
        "readinessProbe warning should be suppressed when policy is disabled"
    );
}

// ---- bytes parsing ----

#[test]
fn bytes_parses_binary_suffixes() {
    assert_eq!(bytes(&Quantity("1Ki".to_string())), 1024);
    assert_eq!(bytes(&Quantity("1Mi".to_string())), 1024 * 1024);
    assert_eq!(bytes(&Quantity("2Gi".to_string())), 2 * 1024 * 1024 * 1024);
}

#[test]
fn bytes_parses_decimal_suffixes() {
    assert_eq!(bytes(&Quantity("1K".to_string())), 1000);
    assert_eq!(bytes(&Quantity("1M".to_string())), 1_000_000);
    assert_eq!(bytes(&Quantity("1G".to_string())), 1_000_000_000);
}

#[test]
fn bytes_parses_raw_number() {
    assert_eq!(bytes(&Quantity("1048576".to_string())), 1_048_576);
}

#[test]
fn bytes_returns_max_on_garbage() {
    assert_eq!(bytes(&Quantity("not_a_number".to_string())), u64::MAX);
}

// ---- is_deckwatch_actor ----

#[test]
fn is_deckwatch_actor_matches_service_account() {
    let user = UserInfo {
        username: "system:serviceaccount:deckwatch-ns:deckwatch-sa".to_string(),
        ..Default::default()
    };
    assert!(is_deckwatch_actor(&user));
}

#[test]
fn is_deckwatch_actor_matches_field_manager() {
    let user = UserInfo {
        username: "deckwatch".to_string(),
        ..Default::default()
    };
    assert!(is_deckwatch_actor(&user));
}

#[test]
fn is_deckwatch_actor_rejects_regular_user() {
    let user = UserInfo {
        username: "admin".to_string(),
        ..Default::default()
    };
    assert!(!is_deckwatch_actor(&user));
}

// ---- WebhookPolicies defaults ----

#[test]
fn webhook_policies_default_all_enabled() {
    let p = WebhookPolicies::default();
    assert!(p.memory_limit_lt_request);
    assert!(p.deployment_missing_readiness_probe);
    assert!(p.ingress_missing_class_name);
    assert!(p.protected_namespaces);
}

// ---- Ingress check ----

#[test]
fn check_ingress_warns_when_class_name_missing() {
    let ing = Ingress {
        metadata: ObjectMeta {
            name: Some("my-ingress".to_string()),
            namespace: Some("prod".to_string()),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::networking::v1::IngressSpec {
            ingress_class_name: None,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut warnings = Vec::new();
    check_ingress(&ing, &WebhookPolicies::default(), &mut warnings);
    assert!(warnings.iter().any(|w| w.contains("ingressClassName")));
}

#[test]
fn check_ingress_no_warning_when_class_name_set() {
    let ing = Ingress {
        metadata: ObjectMeta {
            name: Some("my-ingress".to_string()),
            namespace: Some("prod".to_string()),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::networking::v1::IngressSpec {
            ingress_class_name: Some("nginx".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut warnings = Vec::new();
    check_ingress(&ing, &WebhookPolicies::default(), &mut warnings);
    assert!(warnings.is_empty());
}
