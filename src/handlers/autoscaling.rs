use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::autoscaling::v2::{
    CrossVersionObjectReference, HorizontalPodAutoscaler, HorizontalPodAutoscalerSpec, MetricSpec,
    MetricStatus, MetricTarget, ResourceMetricSource,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct HpaConfigRequest {
    pub min_replicas: i32,
    pub max_replicas: i32,
    pub target_cpu_utilization: Option<i32>,
    pub target_memory_utilization: Option<i32>,
}

#[derive(Serialize)]
pub struct HpaResponse {
    pub name: String,
    pub namespace: String,
    pub min_replicas: Option<i32>,
    pub max_replicas: i32,
    pub target_cpu_utilization: Option<i32>,
    pub target_memory_utilization: Option<i32>,
    pub current_replicas: Option<i32>,
    pub desired_replicas: Option<i32>,
    pub current_cpu_utilization: Option<i32>,
    pub current_memory_utilization: Option<i32>,
    pub conditions: Vec<HpaCondition>,
}

#[derive(Serialize)]
pub struct HpaCondition {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let api = state.hpa_api(&ns)?;
    match api.get(&name).await {
        Ok(hpa) => Ok(Json(serde_json::to_value(to_response(&hpa, &ns, &name)).unwrap())),
        Err(kube::Error::Api(ref e)) if e.code == 404 => {
            Ok(Json(serde_json::json!(null)))
        }
        Err(e) => Err(AppError::Kube(e)),
    }
}

pub async fn upsert(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<HpaConfigRequest>,
) -> Result<Json<HpaResponse>, AppError> {
    if req.max_replicas < 1 {
        return Err(AppError::BadRequest("max_replicas must be >= 1".to_string()));
    }
    if req.min_replicas < 1 || req.min_replicas > req.max_replicas {
        return Err(AppError::BadRequest(
            "min_replicas must be >= 1 and <= max_replicas".to_string(),
        ));
    }
    if req.target_cpu_utilization.is_none() && req.target_memory_utilization.is_none() {
        return Err(AppError::BadRequest(
            "at least one of target_cpu_utilization or target_memory_utilization is required".to_string(),
        ));
    }
    if let Some(cpu) = req.target_cpu_utilization {
        if !(1..=100).contains(&cpu) {
            return Err(AppError::BadRequest(
                "target_cpu_utilization must be between 1 and 100".to_string(),
            ));
        }
    }
    if let Some(mem) = req.target_memory_utilization {
        if !(1..=100).contains(&mem) {
            return Err(AppError::BadRequest(
                "target_memory_utilization must be between 1 and 100".to_string(),
            ));
        }
    }

    // Fail fast if the target Deployment doesn't exist -- otherwise the HPA
    // would land in a permanent ScalingActive=False state until someone
    // notices.
    let dep_api = state.deployments_api(&ns)?;
    dep_api.get(&name).await?;

    let mut metrics: Vec<MetricSpec> = Vec::new();
    if let Some(cpu) = req.target_cpu_utilization {
        metrics.push(resource_metric("cpu", cpu));
    }
    if let Some(mem) = req.target_memory_utilization {
        metrics.push(resource_metric("memory", mem));
    }

    let spec = HorizontalPodAutoscalerSpec {
        scale_target_ref: CrossVersionObjectReference {
            api_version: Some("apps/v1".to_string()),
            kind: "Deployment".to_string(),
            name: name.clone(),
        },
        min_replicas: Some(req.min_replicas),
        max_replicas: req.max_replicas,
        metrics: Some(metrics),
        ..Default::default()
    };

    let api = state.hpa_api(&ns)?;
    let hpa = match api.get(&name).await {
        Ok(mut existing) => {
            existing.spec = Some(spec);
            api.replace(&name, &PostParams::default(), &existing).await?
        }
        Err(kube::Error::Api(e)) if e.code == 404 => {
            let hpa = HorizontalPodAutoscaler {
                metadata: ObjectMeta {
                    name: Some(name.clone()),
                    namespace: Some(ns.clone()),
                    ..Default::default()
                },
                spec: Some(spec),
                status: None,
            };
            api.create(&PostParams::default(), &hpa).await?
        }
        Err(e) => return Err(AppError::Kube(e)),
    };

    Ok(Json(to_response(&hpa, &ns, &name)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.hpa_api(&ns)?;
    api.delete(&name, &DeleteParams::default()).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn resource_metric(name: &str, average_utilization: i32) -> MetricSpec {
    MetricSpec {
        type_: "Resource".to_string(),
        resource: Some(ResourceMetricSource {
            name: name.to_string(),
            target: MetricTarget {
                type_: "Utilization".to_string(),
                average_utilization: Some(average_utilization),
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

fn to_response(hpa: &HorizontalPodAutoscaler, ns: &str, name: &str) -> HpaResponse {
    let spec = hpa.spec.as_ref();
    let status = hpa.status.as_ref();

    let (target_cpu, target_mem) = spec
        .and_then(|s| s.metrics.as_ref())
        .map(|metrics| extract_resource_targets(metrics))
        .unwrap_or((None, None));

    let (current_cpu, current_mem) = status
        .and_then(|s| s.current_metrics.as_ref())
        .map(|metrics| extract_current_utilization(metrics))
        .unwrap_or((None, None));

    HpaResponse {
        name: hpa.metadata.name.clone().unwrap_or_else(|| name.to_string()),
        namespace: hpa.metadata.namespace.clone().unwrap_or_else(|| ns.to_string()),
        min_replicas: spec.and_then(|s| s.min_replicas),
        max_replicas: spec.map(|s| s.max_replicas).unwrap_or(0),
        target_cpu_utilization: target_cpu,
        target_memory_utilization: target_mem,
        current_replicas: status.map(|s| s.current_replicas.unwrap_or(0)),
        desired_replicas: status.map(|s| s.desired_replicas),
        current_cpu_utilization: current_cpu,
        current_memory_utilization: current_mem,
        conditions: status
            .and_then(|s| s.conditions.as_ref())
            .map(|conds| {
                conds
                    .iter()
                    .map(|c| HpaCondition {
                        condition_type: c.type_.clone(),
                        status: c.status.clone(),
                        reason: c.reason.clone(),
                        message: c.message.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn extract_resource_targets(metrics: &[MetricSpec]) -> (Option<i32>, Option<i32>) {
    let mut cpu = None;
    let mut mem = None;
    for m in metrics {
        if let Some(r) = &m.resource {
            let value = r.target.average_utilization;
            match r.name.as_str() {
                "cpu" => cpu = value,
                "memory" => mem = value,
                _ => {}
            }
        }
    }
    (cpu, mem)
}

fn extract_current_utilization(metrics: &[MetricStatus]) -> (Option<i32>, Option<i32>) {
    let mut cpu = None;
    let mut mem = None;
    for m in metrics {
        if let Some(r) = &m.resource {
            let value = r.current.average_utilization;
            match r.name.as_str() {
                "cpu" => cpu = value,
                "memory" => mem = value,
                _ => {}
            }
        }
    }
    (cpu, mem)
}
