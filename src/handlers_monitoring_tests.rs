// Unit tests for src/handlers/monitoring.rs

use super::*;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{Container, ContainerPort, PodSpec, PodTemplateSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

fn dep_with_ports(ports: Vec<ContainerPort>) -> Deployment {
    Deployment {
        spec: Some(DeploymentSpec {
            selector: LabelSelector {
                match_labels: Some(std::collections::BTreeMap::from([(
                    "app".to_string(),
                    "foo".to_string(),
                )])),
                ..Default::default()
            },
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: "app".to_string(),
                        ports: Some(ports),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ---- pick_default_port (migrated from inline tests) ----

#[test]
fn default_port_prefers_metrics_named() {
    let dep = dep_with_ports(vec![
        ContainerPort {
            name: Some("http".into()),
            container_port: 8080,
            ..Default::default()
        },
        ContainerPort {
            name: Some("metrics".into()),
            container_port: 9090,
            ..Default::default()
        },
    ]);
    assert_eq!(pick_default_port(&dep), "metrics");
}

#[test]
fn default_port_falls_back_to_first_named() {
    let dep = dep_with_ports(vec![ContainerPort {
        name: Some("http".into()),
        container_port: 8080,
        ..Default::default()
    }]);
    assert_eq!(pick_default_port(&dep), "http");
}

#[test]
fn default_port_falls_back_to_numeric_when_unnamed() {
    let dep = dep_with_ports(vec![ContainerPort {
        name: None,
        container_port: 8080,
        ..Default::default()
    }]);
    assert_eq!(pick_default_port(&dep), "8080");
}

#[test]
fn default_port_placeholder_when_no_ports() {
    let dep = dep_with_ports(vec![]);
    assert_eq!(pick_default_port(&dep), "metrics");
}

// ---- podmonitor_name (migrated from inline tests) ----

#[test]
fn podmonitor_name_convention() {
    assert_eq!(podmonitor_name("web"), "web-deckwatch");
}

// ---- is_prom_duration (migrated from inline tests) ----

#[test]
fn valid_prom_durations() {
    for s in ["30s", "1m", "5m", "1h", "500ms", "7d", "1w", "1y"] {
        assert!(is_prom_duration(s), "expected {s} to be valid");
    }
}

#[test]
fn invalid_prom_durations() {
    for s in ["", "abc", "30", "30x", "1.5m", "-1s"] {
        assert!(!is_prom_duration(s), "expected {s} to be invalid");
    }
}

// ---- build_podmonitor_shape (migrated from inline tests) ----

#[test]
fn build_podmonitor_shape() {
    let labels = std::collections::BTreeMap::from([("app".to_string(), "foo".to_string())]);
    let pm = build_podmonitor(
        "foo-deckwatch",
        "ns",
        "foo",
        "uid-123",
        &labels,
        "metrics",
        "/metrics",
        "30s",
    );
    assert_eq!(pm.name_any(), "foo-deckwatch");
    assert_eq!(pm.metadata.namespace.as_deref(), Some("ns"));
    let owners = pm.metadata.owner_references.as_ref().expect("owner ref");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].kind, "Deployment");
    assert_eq!(owners[0].uid, "uid-123");
    assert_eq!(owners[0].controller, Some(false));

    let ep = pm.data["spec"]["podMetricsEndpoints"][0].clone();
    assert_eq!(ep["port"], "metrics");
    assert_eq!(ep["path"], "/metrics");
    assert_eq!(ep["interval"], "30s");
    assert_eq!(ep["honorLabels"], true);

    let sel = pm.data["spec"]["selector"]["matchLabels"].clone();
    assert_eq!(sel["app"], "foo");
}

// ---- MonitorConfigRequest deserialization ----

#[test]
fn monitor_config_request_deserializes_enabled_with_all_fields() {
    let json = serde_json::json!({
        "enabled": true,
        "port": "http-metrics",
        "path": "/custom/metrics",
        "interval": "15s"
    });
    let req: MonitorConfigRequest = serde_json::from_value(json).unwrap();
    assert!(req.enabled);
    assert_eq!(req.port.as_deref(), Some("http-metrics"));
    assert_eq!(req.path.as_deref(), Some("/custom/metrics"));
    assert_eq!(req.interval.as_deref(), Some("15s"));
}

#[test]
fn monitor_config_request_enabled_defaults_to_true() {
    let json = serde_json::json!({});
    let req: MonitorConfigRequest = serde_json::from_value(json).unwrap();
    assert!(req.enabled, "enabled should default to true");
    assert!(req.port.is_none());
    assert!(req.path.is_none());
    assert!(req.interval.is_none());
}

#[test]
fn monitor_config_request_disabled() {
    let json = serde_json::json!({ "enabled": false });
    let req: MonitorConfigRequest = serde_json::from_value(json).unwrap();
    assert!(!req.enabled);
}

#[test]
fn monitor_config_request_partial_optional_fields() {
    let json = serde_json::json!({
        "enabled": true,
        "port": "metrics"
    });
    let req: MonitorConfigRequest = serde_json::from_value(json).unwrap();
    assert!(req.enabled);
    assert_eq!(req.port.as_deref(), Some("metrics"));
    assert!(req.path.is_none());
    assert!(req.interval.is_none());
}

// ---- MonitorResponse serialization ----

#[test]
fn monitor_response_serializes_enabled() {
    let resp = MonitorResponse {
        enabled: true,
        name: "web-deckwatch".to_string(),
        namespace: "default".to_string(),
        port: "metrics".to_string(),
        path: "/metrics".to_string(),
        interval: "30s".to_string(),
        matching_pods: 3,
        unavailable_reason: None,
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["name"], "web-deckwatch");
    assert_eq!(json["namespace"], "default");
    assert_eq!(json["port"], "metrics");
    assert_eq!(json["path"], "/metrics");
    assert_eq!(json["interval"], "30s");
    assert_eq!(json["matching_pods"], 3);
    // unavailable_reason is None and skip_serializing_if applies
    assert!(json.get("unavailable_reason").is_none());
}

#[test]
fn monitor_response_serializes_with_unavailable_reason() {
    let resp = MonitorResponse {
        enabled: false,
        name: "web-deckwatch".to_string(),
        namespace: "kube-system".to_string(),
        port: "metrics".to_string(),
        path: "/metrics".to_string(),
        interval: "30s".to_string(),
        matching_pods: 0,
        unavailable_reason: Some("CRD not installed".to_string()),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["enabled"], false);
    assert_eq!(json["matching_pods"], 0);
    assert_eq!(json["unavailable_reason"], "CRD not installed");
}

#[test]
fn monitor_response_omits_unavailable_reason_when_none() {
    let resp = MonitorResponse {
        enabled: true,
        name: "api-deckwatch".to_string(),
        namespace: "prod".to_string(),
        port: "http-metrics".to_string(),
        path: "/metrics".to_string(),
        interval: "15s".to_string(),
        matching_pods: 5,
        unavailable_reason: None,
    };
    let json = serde_json::to_value(&resp).unwrap();
    // The field should not be present at all due to skip_serializing_if
    assert!(
        !json.as_object().unwrap().contains_key("unavailable_reason"),
        "unavailable_reason should be omitted when None"
    );
}

// ---- podmonitor_name helper ----

#[test]
fn podmonitor_name_appends_suffix() {
    assert_eq!(podmonitor_name("nginx"), "nginx-deckwatch");
    assert_eq!(podmonitor_name("my-app"), "my-app-deckwatch");
}

// ---- extract_endpoint ----

#[test]
fn extract_endpoint_from_valid_podmonitor() {
    let labels = std::collections::BTreeMap::from([("app".to_string(), "test".to_string())]);
    let pm = build_podmonitor(
        "test-deckwatch",
        "ns",
        "test",
        "uid-1",
        &labels,
        "custom-port",
        "/custom/path",
        "1m",
    );
    let (port, path, interval) = extract_endpoint(&pm).unwrap();
    assert_eq!(port, "custom-port");
    assert_eq!(path, "/custom/path");
    assert_eq!(interval, "1m");
}

#[test]
fn extract_endpoint_returns_none_for_empty_data() {
    let api_resource = kube::core::ApiResource {
        group: "monitoring.coreos.com".to_string(),
        version: "v1".to_string(),
        api_version: "monitoring.coreos.com/v1".to_string(),
        kind: "PodMonitor".to_string(),
        plural: "podmonitors".to_string(),
    };
    let obj = DynamicObject::new("empty", &api_resource);
    assert!(extract_endpoint(&obj).is_none());
}
