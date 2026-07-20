use super::*;
use serde_json;
use std::collections::BTreeMap;

// ---- CreateIngressRequest deserialization ----

#[test]
fn create_ingress_request_deserializes_full() {
    let json = serde_json::json!({
        "name": "my-ingress",
        "host": "app.example.com",
        "paths": [
            {
                "path": "/api",
                "path_type": "Prefix",
                "service_name": "api-svc",
                "service_port": 8080
            },
            {
                "path": "/health",
                "path_type": "Exact",
                "service_name": "health-svc",
                "service_port": 9090
            }
        ],
        "ingress_class": "alb",
        "annotations": {
            "alb.ingress.kubernetes.io/scheme": "internal",
            "alb.ingress.kubernetes.io/target-type": "ip"
        },
        "tls": [
            {
                "hosts": ["app.example.com", "api.example.com"],
                "secret_name": "tls-secret"
            }
        ]
    });

    let req: CreateIngressRequest = serde_json::from_value(json).expect("deserialize");
    assert_eq!(req.name, "my-ingress");
    assert_eq!(req.host.as_deref(), Some("app.example.com"));
    assert_eq!(req.paths.len(), 2);
    assert_eq!(req.paths[0].path, "/api");
    assert_eq!(req.paths[0].path_type.as_deref(), Some("Prefix"));
    assert_eq!(req.paths[0].service_name, "api-svc");
    assert_eq!(req.paths[0].service_port, 8080);
    assert_eq!(req.paths[1].path, "/health");
    assert_eq!(req.paths[1].path_type.as_deref(), Some("Exact"));
    assert_eq!(req.paths[1].service_name, "health-svc");
    assert_eq!(req.paths[1].service_port, 9090);
    assert_eq!(req.ingress_class.as_deref(), Some("alb"));

    let annotations = req.annotations.expect("annotations present");
    assert_eq!(
        annotations.get("alb.ingress.kubernetes.io/scheme").unwrap(),
        "internal"
    );
    assert_eq!(
        annotations
            .get("alb.ingress.kubernetes.io/target-type")
            .unwrap(),
        "ip"
    );

    let tls = req.tls.expect("tls present");
    assert_eq!(tls.len(), 1);
    assert_eq!(tls[0].hosts, vec!["app.example.com", "api.example.com"]);
    assert_eq!(tls[0].secret_name.as_deref(), Some("tls-secret"));
}

#[test]
fn create_ingress_request_deserializes_minimal() {
    let json = serde_json::json!({
        "name": "minimal-ingress",
        "paths": [
            {
                "path": "/",
                "service_name": "default-svc",
                "service_port": 80
            }
        ]
    });

    let req: CreateIngressRequest = serde_json::from_value(json).expect("deserialize");
    assert_eq!(req.name, "minimal-ingress");
    assert!(req.host.is_none());
    assert_eq!(req.paths.len(), 1);
    assert_eq!(req.paths[0].path, "/");
    assert!(req.paths[0].path_type.is_none());
    assert_eq!(req.paths[0].service_name, "default-svc");
    assert_eq!(req.paths[0].service_port, 80);
    assert!(req.ingress_class.is_none());
    assert!(req.annotations.is_none());
    assert!(req.tls.is_none());
}

#[test]
fn create_ingress_request_rejects_missing_name() {
    let json = serde_json::json!({
        "paths": [{"path": "/", "service_name": "svc", "service_port": 80}]
    });
    let result = serde_json::from_value::<CreateIngressRequest>(json);
    assert!(result.is_err(), "name is required");
}

#[test]
fn create_ingress_request_rejects_missing_paths() {
    let json = serde_json::json!({
        "name": "bad-ingress"
    });
    let result = serde_json::from_value::<CreateIngressRequest>(json);
    assert!(result.is_err(), "paths is required");
}

#[test]
fn ingress_path_input_rejects_missing_service_port() {
    let json = serde_json::json!({
        "name": "bad-ingress",
        "paths": [{"path": "/", "service_name": "svc"}]
    });
    let result = serde_json::from_value::<CreateIngressRequest>(json);
    assert!(result.is_err(), "service_port is required on each path");
}

#[test]
fn tls_input_deserializes_without_secret_name() {
    let json = serde_json::json!({
        "name": "tls-ingress",
        "paths": [{"path": "/", "service_name": "svc", "service_port": 443}],
        "tls": [{"hosts": ["example.com"]}]
    });
    let req: CreateIngressRequest = serde_json::from_value(json).expect("deserialize");
    let tls = req.tls.expect("tls present");
    assert_eq!(tls[0].hosts, vec!["example.com"]);
    assert!(tls[0].secret_name.is_none());
}

// ---- IngressClassInfo serialization ----

#[test]
fn ingress_class_info_serializes_default() {
    let info = IngressClassInfo {
        name: "alb".to_string(),
        is_default: true,
    };
    let value = serde_json::to_value(&info).expect("serialize");
    assert_eq!(value["name"], "alb");
    assert_eq!(value["is_default"], true);
}

#[test]
fn ingress_class_info_serializes_non_default() {
    let info = IngressClassInfo {
        name: "nginx".to_string(),
        is_default: false,
    };
    let value = serde_json::to_value(&info).expect("serialize");
    assert_eq!(value["name"], "nginx");
    assert_eq!(value["is_default"], false);
}

// ---- IngressClassListResponse serialization ----

#[test]
fn ingress_class_list_response_serializes_empty() {
    let resp = IngressClassListResponse { classes: vec![] };
    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["classes"], serde_json::json!([]));
}

#[test]
fn ingress_class_list_response_serializes_multiple() {
    let resp = IngressClassListResponse {
        classes: vec![
            IngressClassInfo {
                name: "alb".to_string(),
                is_default: true,
            },
            IngressClassInfo {
                name: "nginx".to_string(),
                is_default: false,
            },
        ],
    };
    let value = serde_json::to_value(&resp).expect("serialize");
    let classes = value["classes"].as_array().expect("classes array");
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0]["name"], "alb");
    assert_eq!(classes[0]["is_default"], true);
    assert_eq!(classes[1]["name"], "nginx");
    assert_eq!(classes[1]["is_default"], false);
}

// ---- IngressListResponse serialization ----

#[test]
fn ingress_list_response_serializes_empty() {
    let resp = IngressListResponse { ingresses: vec![] };
    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["ingresses"], serde_json::json!([]));
}

#[test]
fn ingress_list_response_serializes_with_mock_data() {
    let resp = IngressListResponse {
        ingresses: vec![
            crate::kube_ext::IngressSummary {
                name: "web-ingress".to_string(),
                namespace: "production".to_string(),
                hosts: vec!["web.example.com".to_string()],
                ingress_class: Some("alb".to_string()),
                created_at: Some("2025-06-01T00:00:00Z".to_string()),
                labels: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "app.kubernetes.io/managed-by".to_string(),
                        "deckwatch".to_string(),
                    );
                    m
                },
                addresses: vec!["10.0.0.1".to_string()],
            },
            crate::kube_ext::IngressSummary {
                name: "api-ingress".to_string(),
                namespace: "production".to_string(),
                hosts: vec![],
                ingress_class: None,
                created_at: None,
                labels: BTreeMap::new(),
                addresses: vec![],
            },
        ],
    };

    let value = serde_json::to_value(&resp).expect("serialize");
    let ingresses = value["ingresses"].as_array().expect("ingresses array");
    assert_eq!(ingresses.len(), 2);

    // First ingress: fully populated
    assert_eq!(ingresses[0]["name"], "web-ingress");
    assert_eq!(ingresses[0]["namespace"], "production");
    assert_eq!(
        ingresses[0]["hosts"],
        serde_json::json!(["web.example.com"])
    );
    assert_eq!(ingresses[0]["ingress_class"], "alb");
    assert_eq!(ingresses[0]["created_at"], "2025-06-01T00:00:00Z");
    assert_eq!(
        ingresses[0]["labels"]["app.kubernetes.io/managed-by"],
        "deckwatch"
    );
    assert_eq!(ingresses[0]["addresses"], serde_json::json!(["10.0.0.1"]));

    // Second ingress: minimal
    assert_eq!(ingresses[1]["name"], "api-ingress");
    assert!(ingresses[1]["hosts"].as_array().unwrap().is_empty());
    assert!(ingresses[1]["ingress_class"].is_null());
    assert!(ingresses[1]["created_at"].is_null());
    assert!(ingresses[1]["addresses"].as_array().unwrap().is_empty());
}
