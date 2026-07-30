use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::storage::v1::StorageClass;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StorageClassSummary {
    pub name: String,
    pub provisioner: String,
    pub reclaim_policy: Option<String>,
    pub volume_binding_mode: Option<String>,
    pub allow_volume_expansion: bool,
    pub is_default: bool,
    pub mount_options: Option<Vec<String>>,
    pub parameters: Option<BTreeMap<String, String>>,
}

#[derive(Serialize)]
pub struct StorageClassListResponse {
    pub storage_classes: Vec<StorageClassSummary>,
}

#[derive(Deserialize)]
pub struct CreateStorageClassRequest {
    pub name: String,
    pub provisioner: String,
    pub reclaim_policy: Option<String>,
    pub volume_binding_mode: Option<String>,
    pub allow_volume_expansion: Option<bool>,
    pub mount_options: Option<Vec<String>>,
    pub parameters: Option<BTreeMap<String, String>>,
    pub is_default: Option<bool>,
}

fn summarize(sc: &StorageClass) -> StorageClassSummary {
    let name = sc.metadata.name.clone().unwrap_or_default();
    let is_default = sc
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("storageclass.kubernetes.io/is-default-class"))
        .map(|v| v == "true")
        .unwrap_or(false);

    StorageClassSummary {
        name,
        provisioner: sc.provisioner.clone(),
        reclaim_policy: sc.reclaim_policy.clone(),
        volume_binding_mode: sc.volume_binding_mode.clone(),
        allow_volume_expansion: sc.allow_volume_expansion.unwrap_or(false),
        is_default,
        mount_options: sc.mount_options.clone(),
        parameters: sc.parameters.clone(),
    }
}

pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<StorageClassListResponse>, AppError> {
    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "list");
    let result = api.list(&ListParams::default()).await;
    t.finish(result.is_ok());
    let list = result?;

    let storage_classes = list.iter().map(summarize).collect();

    Ok(Json(StorageClassListResponse { storage_classes }))
}

pub async fn get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<StorageClassSummary>, AppError> {
    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "get");
    let result = api.get(&name).await;
    t.finish(result.is_ok());
    let sc = result?;
    Ok(Json(summarize(&sc)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateStorageClassRequest>,
) -> Result<(StatusCode, Json<StorageClassSummary>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.provisioner.is_empty() {
        return Err(AppError::BadRequest("provisioner is required".to_string()));
    }

    let mut annotations = BTreeMap::new();
    if req.is_default.unwrap_or(false) {
        annotations.insert(
            "storageclass.kubernetes.io/is-default-class".to_string(),
            "true".to_string(),
        );
    }

    let mut labels = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let sc = StorageClass {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            labels: Some(labels),
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations)
            },
            ..Default::default()
        },
        provisioner: req.provisioner,
        reclaim_policy: req.reclaim_policy,
        volume_binding_mode: req.volume_binding_mode,
        allow_volume_expansion: req.allow_volume_expansion,
        mount_options: req.mount_options,
        parameters: req.parameters,
        ..Default::default()
    };

    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "create");
    let created = api.create(&PostParams::default(), &sc).await;
    t.finish(created.is_ok());
    let created = created?;

    if let Err(e) = audit::log_action(
        &state.db,
        "create",
        "storageclass",
        &req.name,
        "",
        &format!("created storage class {}", req.name),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok((StatusCode::CREATED, Json(summarize(&created))))
}

pub async fn update(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CreateStorageClassRequest>,
) -> Result<Json<StorageClassSummary>, AppError> {
    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let mut sc = existing?;

    sc.provisioner = req.provisioner;
    sc.reclaim_policy = req.reclaim_policy;
    sc.volume_binding_mode = req.volume_binding_mode;
    sc.allow_volume_expansion = req.allow_volume_expansion;
    sc.mount_options = req.mount_options;
    sc.parameters = req.parameters;

    let annotations = sc.metadata.annotations.get_or_insert_with(BTreeMap::new);
    if req.is_default.unwrap_or(false) {
        annotations.insert(
            "storageclass.kubernetes.io/is-default-class".to_string(),
            "true".to_string(),
        );
    } else {
        annotations.remove("storageclass.kubernetes.io/is-default-class");
    }

    let t = K8sTimer::new("storageclasses", "replace");
    let updated = api.replace(&name, &PostParams::default(), &sc).await;
    t.finish(updated.is_ok());
    let updated = updated?;

    if let Err(e) = audit::log_action(
        &state.db,
        "update",
        "storageclass",
        &name,
        "",
        "updated storage class",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(summarize(&updated)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;

    if let Err(e) = audit::log_action(
        &state.db,
        "delete",
        "storageclass",
        &name,
        "",
        "deleted storage class",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_class_summary_serializes() {
        let sc = StorageClassSummary {
            name: "gp3".to_string(),
            provisioner: "ebs.csi.aws.com".to_string(),
            reclaim_policy: Some("Delete".to_string()),
            volume_binding_mode: Some("WaitForFirstConsumer".to_string()),
            allow_volume_expansion: true,
            is_default: true,
            mount_options: None,
            parameters: None,
        };
        let json = serde_json::to_value(&sc).unwrap();
        assert_eq!(json["name"], "gp3");
        assert_eq!(json["provisioner"], "ebs.csi.aws.com");
        assert_eq!(json["reclaim_policy"], "Delete");
        assert_eq!(json["volume_binding_mode"], "WaitForFirstConsumer");
        assert_eq!(json["allow_volume_expansion"], true);
        assert_eq!(json["is_default"], true);
    }

    #[test]
    fn storage_class_summary_null_fields() {
        let sc = StorageClassSummary {
            name: "local-path".to_string(),
            provisioner: "rancher.io/local-path".to_string(),
            reclaim_policy: None,
            volume_binding_mode: None,
            allow_volume_expansion: false,
            is_default: false,
            mount_options: None,
            parameters: None,
        };
        let json = serde_json::to_value(&sc).unwrap();
        assert_eq!(json["name"], "local-path");
        assert!(json["reclaim_policy"].is_null());
        assert!(json["volume_binding_mode"].is_null());
        assert_eq!(json["allow_volume_expansion"], false);
        assert_eq!(json["is_default"], false);
    }

    #[test]
    fn list_response_serializes() {
        let resp = StorageClassListResponse {
            storage_classes: vec![
                StorageClassSummary {
                    name: "gp3".to_string(),
                    provisioner: "ebs.csi.aws.com".to_string(),
                    reclaim_policy: Some("Delete".to_string()),
                    volume_binding_mode: Some("WaitForFirstConsumer".to_string()),
                    allow_volume_expansion: true,
                    is_default: true,
                    mount_options: None,
                    parameters: None,
                },
                StorageClassSummary {
                    name: "gp2".to_string(),
                    provisioner: "kubernetes.io/aws-ebs".to_string(),
                    reclaim_policy: Some("Delete".to_string()),
                    volume_binding_mode: Some("Immediate".to_string()),
                    allow_volume_expansion: false,
                    is_default: false,
                    mount_options: None,
                    parameters: None,
                },
            ],
        };
        let json = serde_json::to_value(&resp).unwrap();
        let classes = json["storage_classes"].as_array().unwrap();
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0]["name"], "gp3");
        assert_eq!(classes[1]["name"], "gp2");
    }

    #[test]
    fn create_request_deserializes() {
        let json = serde_json::json!({
            "name": "gp3",
            "provisioner": "ebs.csi.aws.com",
            "reclaim_policy": "Delete",
            "volume_binding_mode": "WaitForFirstConsumer",
            "allow_volume_expansion": true,
            "is_default": true,
            "mount_options": ["debug"],
            "parameters": {"type": "gp3"}
        });
        let req: CreateStorageClassRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "gp3");
        assert_eq!(req.provisioner, "ebs.csi.aws.com");
        assert_eq!(req.is_default, Some(true));
        assert_eq!(req.mount_options.unwrap(), vec!["debug"]);
        assert_eq!(req.parameters.unwrap()["type"], "gp3");
    }
}
