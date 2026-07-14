use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, PostParams};
use kube::ResourceExt;
use serde::Deserialize;

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct NamespaceListResponse {
    pub namespaces: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(serde::Serialize)]
pub struct CreateNamespaceResponse {
    pub name: String,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
}

pub async fn list_namespaces(
    State(state): State<AppState>,
) -> Result<Json<NamespaceListResponse>, AppError> {
    let namespaces = if state.allowed_namespaces.is_empty() {
        let ns_api = state.namespaces_api();
        let t = K8sTimer::new("namespaces", "list");
        let ns_list = ns_api.list(&ListParams::default()).await;
        t.finish(ns_list.is_ok());
        let ns_list = ns_list?;
        ns_list.iter().map(|ns| ns.name_any()).collect()
    } else {
        state.allowed_namespaces.clone()
    };

    Ok(Json(NamespaceListResponse { namespaces }))
}

pub async fn create_namespace(
    State(state): State<AppState>,
    Json(req): Json<CreateNamespaceRequest>,
) -> Result<(StatusCode, Json<CreateNamespaceResponse>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }

    if !state.is_namespace_allowed(&req.name) {
        return Err(AppError::NamespaceNotAllowed(req.name));
    }

    let mut labels = req.labels.unwrap_or_default();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let ns = Namespace {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        ..Default::default()
    };

    let api = state.namespaces_api();
    let t = K8sTimer::new("namespaces", "create");
    let created = api.create(&PostParams::default(), &ns).await;
    t.finish(created.is_ok());
    let created = created?;

    Ok((
        StatusCode::CREATED,
        Json(CreateNamespaceResponse {
            name: created.name_any(),
            created_at: created
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0.to_string()),
            labels: created.metadata.labels.clone().unwrap_or_default(),
        }),
    ))
}
