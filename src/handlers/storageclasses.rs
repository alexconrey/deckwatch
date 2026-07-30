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
                },
                StorageClassSummary {
                    name: "gp2".to_string(),
                    provisioner: "kubernetes.io/aws-ebs".to_string(),
                    reclaim_policy: Some("Delete".to_string()),
                    volume_binding_mode: Some("Immediate".to_string()),
                    allow_volume_expansion: false,
                    is_default: false,
                },
            ],
        };
        let json = serde_json::to_value(&resp).unwrap();
        let classes = json["storage_classes"].as_array().unwrap();
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0]["name"], "gp3");
        assert_eq!(classes[1]["name"], "gp2");
    }
}
