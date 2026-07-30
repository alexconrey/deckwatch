use axum::extract::State;
use axum::Json;
use kube::api::ListParams;
use serde::Serialize;

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
}

#[derive(Serialize)]
pub struct StorageClassListResponse {
    pub storage_classes: Vec<StorageClassSummary>,
}

pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<StorageClassListResponse>, AppError> {
    let api = state.storageclasses_api();
    let t = K8sTimer::new("storageclasses", "list");
    let result = api.list(&ListParams::default()).await;
    t.finish(result.is_ok());
    let list = result?;

    let storage_classes = list
        .iter()
        .map(|sc| {
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
            }
        })
        .collect();

    Ok(Json(StorageClassListResponse { storage_classes }))
}
