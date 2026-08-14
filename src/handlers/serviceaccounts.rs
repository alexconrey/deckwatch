//! ServiceAccount CRUD handlers.
//!
//! Exposes REST endpoints for listing, fetching, creating, patching, and
//! deleting Kubernetes ServiceAccounts in a namespace. Includes first-class
//! support for the EKS IRSA annotation (`eks.amazonaws.com/role-arn`) so
//! operators can inspect and manage IRSA-bound service accounts without kubectl.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::core::v1::ServiceAccount;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

/// EKS IRSA annotation key for IAM role binding.
pub const IRSA_ANNOTATION: &str = "eks.amazonaws.com/role-arn";

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ServiceAccountSummary {
    pub name: String,
    pub namespace: String,
    /// Populated when `eks.amazonaws.com/role-arn` annotation is present.
    pub irsa_role_arn: Option<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct ServiceAccountDetail {
    pub name: String,
    pub namespace: String,
    /// Populated when `eks.amazonaws.com/role-arn` annotation is present.
    pub irsa_role_arn: Option<String>,
    pub annotations: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<String>,
}

#[derive(Serialize)]
pub struct ServiceAccountListResponse {
    pub service_accounts: Vec<ServiceAccountSummary>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateServiceAccountRequest {
    pub name: String,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Patch request: merges supplied annotations and labels onto existing SA.
/// Only non-null keys are updated — omit a key to leave it unchanged.
#[derive(Deserialize)]
pub struct PatchServiceAccountRequest {
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn irsa_from_annotations(annotations: &BTreeMap<String, String>) -> Option<String> {
    annotations.get(IRSA_ANNOTATION).cloned()
}

fn to_summary(sa: &ServiceAccount) -> ServiceAccountSummary {
    let meta = &sa.metadata;
    let annotations = meta.annotations.clone().unwrap_or_default();
    ServiceAccountSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        irsa_role_arn: irsa_from_annotations(&annotations),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
    }
}

fn to_detail(sa: &ServiceAccount) -> ServiceAccountDetail {
    let meta = &sa.metadata;
    let annotations = meta.annotations.clone().unwrap_or_default();
    ServiceAccountDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        irsa_role_arn: irsa_from_annotations(&annotations),
        annotations,
        labels: meta.labels.clone().unwrap_or_default(),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/namespaces/{ns}/serviceaccounts`
pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<ServiceAccountListResponse>, AppError> {
    let api = state.serviceaccounts_api(&ns)?;
    let t = K8sTimer::new("serviceaccounts", "list");
    let result = api.list(&ListParams::default()).await;
    t.finish(result.is_ok());
    let list = result?;
    let service_accounts = list.iter().map(to_summary).collect();
    Ok(Json(ServiceAccountListResponse { service_accounts }))
}

/// `GET /api/namespaces/{ns}/serviceaccounts/{name}`
pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<ServiceAccountDetail>, AppError> {
    let api = state.serviceaccounts_api(&ns)?;
    let t = K8sTimer::new("serviceaccounts", "get");
    let result = api.get(&name).await;
    t.finish(result.is_ok());
    let sa = result?;
    Ok(Json(to_detail(&sa)))
}

/// `POST /api/namespaces/{ns}/serviceaccounts`
pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateServiceAccountRequest>,
) -> Result<(StatusCode, Json<ServiceAccountDetail>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }

    let api = state.serviceaccounts_api(&ns)?;

    let mut labels = req.labels;
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let sa = ServiceAccount {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.clone()),
            labels: Some(labels),
            annotations: if req.annotations.is_empty() {
                None
            } else {
                Some(req.annotations)
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let t = K8sTimer::new("serviceaccounts", "create");
    let result = api.create(&PostParams::default(), &sa).await;
    t.finish(result.is_ok());
    let created = result?;

    if let Err(e) = audit::log_action(
        &state.db,
        "create",
        "serviceaccount",
        &req.name,
        &ns,
        "created service account",
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok((StatusCode::CREATED, Json(to_detail(&created))))
}

/// `PATCH /api/namespaces/{ns}/serviceaccounts/{name}`
///
/// Merges `annotations` and `labels` from the request onto the existing SA.
/// Existing keys not present in the request are preserved.
pub async fn patch(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<PatchServiceAccountRequest>,
) -> Result<Json<ServiceAccountDetail>, AppError> {
    let api = state.serviceaccounts_api(&ns)?;

    // Fetch current to merge rather than replace.
    let t = K8sTimer::new("serviceaccounts", "get");
    let result = api.get(&name).await;
    t.finish(result.is_ok());
    let existing = result?;

    let mut annotations = existing.metadata.annotations.clone().unwrap_or_default();
    annotations.extend(req.annotations);

    let mut labels = existing.metadata.labels.clone().unwrap_or_default();
    labels.extend(req.labels);

    let patch_val = serde_json::json!({
        "metadata": {
            "annotations": annotations,
            "labels": labels,
        }
    });

    let t = K8sTimer::new("serviceaccounts", "patch");
    let result = api
        .patch(&name, &PatchParams::default(), &Patch::Merge(patch_val))
        .await;
    t.finish(result.is_ok());
    let updated = result?;

    if let Err(e) = audit::log_action(
        &state.db,
        "patch",
        "serviceaccount",
        &name,
        &ns,
        "patched service account annotations/labels",
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(to_detail(&updated)))
}

/// `DELETE /api/namespaces/{ns}/serviceaccounts/{name}`
pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.serviceaccounts_api(&ns)?;
    let t = K8sTimer::new("serviceaccounts", "delete");
    let result = api.delete(&name, &Default::default()).await;
    t.finish(result.is_ok());
    result?;

    if let Err(e) = audit::log_action(
        &state.db,
        "delete",
        "serviceaccount",
        &name,
        &ns,
        "deleted service account",
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Plugin SA lifecycle ───────────────────────────────────────────────────────

/// Ensure a ServiceAccount exists in `ns` with the given `name` and optional
/// IRSA role ARN annotation. Used by the plugin lifecycle after `apply_plugins`
/// returns a `WantsServiceAccount` — creates if absent, patches if the IRSA
/// annotation differs. Logs but does not propagate errors so a failing SA
/// creation never blocks the deployment itself.
pub async fn ensure_service_account(
    kube_client: &kube::Client,
    ns: &str,
    name: &str,
    irsa_role_arn: &str,
) {
    let api: kube::Api<ServiceAccount> = kube::Api::namespaced(kube_client.clone(), ns);

    let mut annotations: BTreeMap<String, String> = BTreeMap::new();
    if !irsa_role_arn.is_empty() {
        annotations.insert(IRSA_ANNOTATION.to_string(), irsa_role_arn.to_string());
    }

    let sa = ServiceAccount {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations.clone())
            },
            labels: Some(BTreeMap::from([(
                "app.kubernetes.io/managed-by".to_string(),
                "deckwatch".to_string(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    };

    // Attempt create first; on 409 (AlreadyExists) patch to reconcile annotation.
    match api.create(&PostParams::default(), &sa).await {
        Ok(_) => {
            tracing::info!(namespace = ns, name, irsa_role_arn, "plugin SA created");
        }
        Err(kube::Error::Api(e)) if e.code == 409 => {
            // SA exists — patch the IRSA annotation if an ARN was requested.
            if !irsa_role_arn.is_empty() {
                let patch_val = serde_json::json!({
                    "metadata": {
                        "annotations": { IRSA_ANNOTATION: irsa_role_arn }
                    }
                });
                match api
                    .patch(name, &PatchParams::default(), &Patch::Merge(patch_val))
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            namespace = ns,
                            name,
                            irsa_role_arn,
                            "plugin SA IRSA annotation patched"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            namespace = ns,
                            name,
                            error = %e,
                            "plugin SA patch failed"
                        );
                    }
                }
            } else {
                tracing::info!(
                    namespace = ns,
                    name,
                    "plugin SA already exists, no patch needed"
                );
            }
        }
        Err(e) => {
            tracing::error!(namespace = ns, name, error = %e, "plugin SA create failed");
        }
    }
}
