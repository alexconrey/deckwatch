// Unit tests for request/response types and helpers in src/handlers/autoscaling.rs

use super::*;
use k8s_openapi::api::autoscaling::v2::{
    HorizontalPodAutoscaler, HorizontalPodAutoscalerSpec, MetricValueStatus, ResourceMetricStatus,
};

// ---- HpaConfigRequest deserialization ----

#[test]
fn hpa_config_request_cpu_only() {
    let json = r#"{
        "min_replicas": 2,
        "max_replicas": 10,
        "target_cpu_utilization": 70
    }"#;
    let req: HpaConfigRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.min_replicas, 2);
    assert_eq!(req.max_replicas, 10);
    assert_eq!(req.target_cpu_utilization, Some(70));
    assert!(req.target_memory_utilization.is_none());
}

#[test]
fn hpa_config_request_memory_only() {
    let json = r#"{
        "min_replicas": 1,
        "max_replicas": 5,
        "target_memory_utilization": 80
    }"#;
    let req: HpaConfigRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.min_replicas, 1);
    assert_eq!(req.max_replicas, 5);
    assert!(req.target_cpu_utilization.is_none());
    assert_eq!(req.target_memory_utilization, Some(80));
}

#[test]
fn hpa_config_request_both_metrics() {
    let json = r#"{
        "min_replicas": 3,
        "max_replicas": 20,
        "target_cpu_utilization": 60,
        "target_memory_utilization": 75
    }"#;
    let req: HpaConfigRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.min_replicas, 3);
    assert_eq!(req.max_replicas, 20);
    assert_eq!(req.target_cpu_utilization, Some(60));
    assert_eq!(req.target_memory_utilization, Some(75));
}

#[test]
fn hpa_config_request_no_metrics() {
    let json = r#"{
        "min_replicas": 1,
        "max_replicas": 3
    }"#;
    let req: HpaConfigRequest = serde_json::from_str(json).unwrap();
    assert!(req.target_cpu_utilization.is_none());
    assert!(req.target_memory_utilization.is_none());
}

// ---- HpaResponse serialization ----

#[test]
fn hpa_response_serializes() {
    let resp = HpaResponse {
        name: "my-deploy".to_string(),
        namespace: "default".to_string(),
        min_replicas: Some(2),
        max_replicas: 10,
        target_cpu_utilization: Some(70),
        target_memory_utilization: None,
        current_replicas: Some(3),
        desired_replicas: Some(3),
        current_cpu_utilization: Some(45),
        current_memory_utilization: None,
        conditions: vec![HpaCondition {
            condition_type: "ScalingActive".to_string(),
            status: "True".to_string(),
            reason: Some("ValidMetricFound".to_string()),
            message: Some("the HPA was able to compute the replica count".to_string()),
        }],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["name"], "my-deploy");
    assert_eq!(json["namespace"], "default");
    assert_eq!(json["min_replicas"], 2);
    assert_eq!(json["max_replicas"], 10);
    assert_eq!(json["target_cpu_utilization"], 70);
    assert!(json["target_memory_utilization"].is_null());
    assert_eq!(json["current_replicas"], 3);
    assert_eq!(json["conditions"].as_array().unwrap().len(), 1);
    assert_eq!(json["conditions"][0]["condition_type"], "ScalingActive");
}

#[test]
fn hpa_response_empty_conditions() {
    let resp = HpaResponse {
        name: "test".to_string(),
        namespace: "ns".to_string(),
        min_replicas: None,
        max_replicas: 5,
        target_cpu_utilization: None,
        target_memory_utilization: None,
        current_replicas: None,
        desired_replicas: None,
        current_cpu_utilization: None,
        current_memory_utilization: None,
        conditions: vec![],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["conditions"].as_array().unwrap().is_empty());
    assert!(json["min_replicas"].is_null());
}

// ---- resource_metric helper ----

#[test]
fn resource_metric_cpu() {
    let m = resource_metric("cpu", 70);
    assert_eq!(m.type_, "Resource");
    let r = m.resource.unwrap();
    assert_eq!(r.name, "cpu");
    assert_eq!(r.target.type_, "Utilization");
    assert_eq!(r.target.average_utilization, Some(70));
}

#[test]
fn resource_metric_memory() {
    let m = resource_metric("memory", 85);
    let r = m.resource.unwrap();
    assert_eq!(r.name, "memory");
    assert_eq!(r.target.average_utilization, Some(85));
}

// ---- extract_resource_targets ----

#[test]
fn extract_resource_targets_cpu_and_memory() {
    let metrics = vec![resource_metric("cpu", 60), resource_metric("memory", 80)];
    let (cpu, mem) = extract_resource_targets(&metrics);
    assert_eq!(cpu, Some(60));
    assert_eq!(mem, Some(80));
}

#[test]
fn extract_resource_targets_cpu_only() {
    let metrics = vec![resource_metric("cpu", 50)];
    let (cpu, mem) = extract_resource_targets(&metrics);
    assert_eq!(cpu, Some(50));
    assert_eq!(mem, None);
}

#[test]
fn extract_resource_targets_empty() {
    let (cpu, mem) = extract_resource_targets(&[]);
    assert_eq!(cpu, None);
    assert_eq!(mem, None);
}

// ---- extract_current_utilization ----

#[test]
fn extract_current_utilization_both() {
    let metrics = vec![
        MetricStatus {
            type_: "Resource".to_string(),
            resource: Some(ResourceMetricStatus {
                name: "cpu".to_string(),
                current: MetricValueStatus {
                    average_utilization: Some(42),
                    ..Default::default()
                },
            }),
            ..Default::default()
        },
        MetricStatus {
            type_: "Resource".to_string(),
            resource: Some(ResourceMetricStatus {
                name: "memory".to_string(),
                current: MetricValueStatus {
                    average_utilization: Some(67),
                    ..Default::default()
                },
            }),
            ..Default::default()
        },
    ];
    let (cpu, mem) = extract_current_utilization(&metrics);
    assert_eq!(cpu, Some(42));
    assert_eq!(mem, Some(67));
}

#[test]
fn extract_current_utilization_empty() {
    let (cpu, mem) = extract_current_utilization(&[]);
    assert_eq!(cpu, None);
    assert_eq!(mem, None);
}

// ---- to_response ----

#[test]
fn to_response_minimal_hpa() {
    let hpa = HorizontalPodAutoscaler {
        metadata: ObjectMeta {
            name: Some("my-hpa".to_string()),
            namespace: Some("production".to_string()),
            ..Default::default()
        },
        spec: Some(HorizontalPodAutoscalerSpec {
            max_replicas: 10,
            min_replicas: Some(2),
            metrics: Some(vec![resource_metric("cpu", 70)]),
            ..Default::default()
        }),
        status: None,
    };
    let resp = to_response(&hpa, "production", "my-hpa");
    assert_eq!(resp.name, "my-hpa");
    assert_eq!(resp.namespace, "production");
    assert_eq!(resp.min_replicas, Some(2));
    assert_eq!(resp.max_replicas, 10);
    assert_eq!(resp.target_cpu_utilization, Some(70));
    assert!(resp.target_memory_utilization.is_none());
    assert!(resp.current_replicas.is_none());
    assert!(resp.conditions.is_empty());
}

#[test]
fn to_response_uses_fallback_name_and_ns() {
    let hpa = HorizontalPodAutoscaler {
        metadata: ObjectMeta::default(),
        spec: None,
        status: None,
    };
    let resp = to_response(&hpa, "fallback-ns", "fallback-name");
    assert_eq!(resp.name, "fallback-name");
    assert_eq!(resp.namespace, "fallback-ns");
    assert_eq!(resp.max_replicas, 0);
}
