use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ConfigMapSummary {
    pub name: String,
    pub namespace: String,
    pub keys: Vec<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct ConfigMapListResponse {
    pub configmaps: Vec<ConfigMapSummary>,
}

#[derive(Serialize)]
pub struct ConfigMapDetail {
    pub name: String,
    pub namespace: String,
    pub keys: Vec<String>,
    pub data: BTreeMap<String, String>,
    pub binary_keys: Vec<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
pub struct CreateConfigMapRequest {
    pub name: String,
    pub data: BTreeMap<String, String>,
    pub labels: Option<BTreeMap<String, String>>,
    pub annotations: Option<BTreeMap<String, String>>,
}

fn configmap_keys(cm: &ConfigMap) -> Vec<String> {
    let mut keys: Vec<String> = cm
        .data
        .as_ref()
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default();
    if let Some(binary) = cm.binary_data.as_ref() {
        for k in binary.keys() {
            if !keys.iter().any(|existing| existing == k) {
                keys.push(k.clone());
            }
        }
    }
    keys.sort();
    keys
}

fn to_summary(cm: &ConfigMap) -> ConfigMapSummary {
    let meta = &cm.metadata;
    ConfigMapSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        keys: configmap_keys(cm),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
    }
}

fn to_detail(cm: &ConfigMap) -> ConfigMapDetail {
    let meta = &cm.metadata;
    let data = cm.data.clone().unwrap_or_default();
    let binary_keys = cm
        .binary_data
        .as_ref()
        .map(|b| b.keys().cloned().collect())
        .unwrap_or_default();
    ConfigMapDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        keys: configmap_keys(cm),
        data,
        binary_keys,
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        annotations: meta.annotations.clone().unwrap_or_default(),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<ConfigMapListResponse>, AppError> {
    let api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "list");
    let list = api.list(&ListParams::default()).await;
    t.finish(list.is_ok());
    let list = list?;
    let configmaps = list.iter().map(to_summary).collect();
    Ok(Json(ConfigMapListResponse { configmaps }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<ConfigMapDetail>, AppError> {
    let api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let cm = api.get(&name).await;
    t.finish(cm.is_ok());
    let cm = cm?;
    Ok(Json(to_detail(&cm)))
}

pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateConfigMapRequest>,
) -> Result<(StatusCode, Json<ConfigMapDetail>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }

    let api = state.configmaps_api(&ns)?;

    let mut labels = req.labels.unwrap_or_default();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.clone()),
            labels: Some(labels),
            annotations: req.annotations,
            ..Default::default()
        },
        data: Some(req.data),
        ..Default::default()
    };

    let t = K8sTimer::new("configmaps", "create");
    let created = api.create(&PostParams::default(), &cm).await;
    t.finish(created.is_ok());
    let created = created?;
    Ok((StatusCode::CREATED, Json(to_detail(&created))))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<CreateConfigMapRequest>,
) -> Result<Json<ConfigMapDetail>, AppError> {
    let api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let mut existing = existing?;

    existing.data = Some(req.data);
    if let Some(annotations) = req.annotations {
        existing.metadata.annotations = Some(annotations);
    }
    if let Some(mut labels) = req.labels {
        labels.insert(
            "app.kubernetes.io/managed-by".to_string(),
            "deckwatch".to_string(),
        );
        existing.metadata.labels = Some(labels);
    }

    let t = K8sTimer::new("configmaps", "replace");
    let updated = api.replace(&name, &PostParams::default(), &existing).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    Ok(Json(to_detail(&updated)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;
    Ok(StatusCode::NO_CONTENT)
}
