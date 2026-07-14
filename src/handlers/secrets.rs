use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;
use kube::api::{ListParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Serialize)]
pub struct SecretSummary {
    pub name: String,
    pub namespace: String,
    #[serde(rename = "type")]
    pub secret_type: String,
    pub keys: Vec<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct SecretListResponse {
    pub secrets: Vec<SecretSummary>,
}

#[derive(Serialize)]
pub struct SecretDetail {
    pub name: String,
    pub namespace: String,
    #[serde(rename = "type")]
    pub secret_type: String,
    pub keys: Vec<String>,
    pub data: Option<BTreeMap<String, String>>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub secret_type: Option<String>,
    pub data: BTreeMap<String, String>,
    pub labels: Option<BTreeMap<String, String>>,
    pub annotations: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
pub struct GetSecretQuery {
    #[serde(default)]
    pub reveal: bool,
}

fn secret_type(secret: &Secret) -> String {
    secret.type_.clone().unwrap_or_else(|| "Opaque".to_string())
}

fn secret_keys(secret: &Secret) -> Vec<String> {
    let mut keys: Vec<String> = secret
        .data
        .as_ref()
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default();
    if let Some(string_data) = secret.string_data.as_ref() {
        for k in string_data.keys() {
            if !keys.iter().any(|existing| existing == k) {
                keys.push(k.clone());
            }
        }
    }
    keys.sort();
    keys
}

fn to_summary(secret: &Secret) -> SecretSummary {
    let meta = &secret.metadata;
    SecretSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        secret_type: secret_type(secret),
        keys: secret_keys(secret),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
    }
}

fn to_detail(secret: &Secret, reveal: bool) -> SecretDetail {
    let meta = &secret.metadata;
    let data = if reveal {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        if let Some(d) = secret.data.as_ref() {
            for (k, v) in d.iter() {
                // v is a ByteString (already the raw bytes). Return as UTF-8
                // when possible, otherwise base64-encode.
                match std::str::from_utf8(&v.0) {
                    Ok(s) => {
                        out.insert(k.clone(), s.to_string());
                    }
                    Err(_) => {
                        out.insert(k.clone(), format!("base64:{}", B64.encode(&v.0)));
                    }
                }
            }
        }
        if let Some(sd) = secret.string_data.as_ref() {
            for (k, v) in sd.iter() {
                out.insert(k.clone(), v.clone());
            }
        }
        Some(out)
    } else {
        None
    };

    SecretDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        secret_type: secret_type(secret),
        keys: secret_keys(secret),
        data,
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        annotations: meta.annotations.clone().unwrap_or_default(),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<SecretListResponse>, AppError> {
    let api = state.secrets_api(&ns)?;
    let t = K8sTimer::new("secrets", "list");
    let list = api.list(&ListParams::default()).await;
    t.finish(list.is_ok());
    let list = list?;
    let secrets = list.iter().map(to_summary).collect();
    Ok(Json(SecretListResponse { secrets }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Query(query): Query<GetSecretQuery>,
) -> Result<Json<SecretDetail>, AppError> {
    let api = state.secrets_api(&ns)?;
    let t = K8sTimer::new("secrets", "get");
    let secret = api.get(&name).await;
    t.finish(secret.is_ok());
    let secret = secret?;
    Ok(Json(to_detail(&secret, query.reveal)))
}

pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretDetail>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.data.is_empty() {
        return Err(AppError::BadRequest(
            "at least one key/value pair is required".to_string(),
        ));
    }

    let api = state.secrets_api(&ns)?;

    let mut labels = req.labels.unwrap_or_default();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    // Base64-encode plaintext values into `data`. We accept plaintext from the
    // UI so keys can be typed in normally; the k8s API expects `data` to be
    // base64-encoded bytes.
    let data: BTreeMap<String, ByteString> = req
        .data
        .into_iter()
        .map(|(k, v)| (k, ByteString(v.into_bytes())))
        .collect();

    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.clone()),
            labels: Some(labels),
            annotations: req.annotations,
            ..Default::default()
        },
        type_: req.secret_type.or_else(|| Some("Opaque".to_string())),
        data: Some(data),
        ..Default::default()
    };

    let t = K8sTimer::new("secrets", "create");
    let created = api.create(&PostParams::default(), &secret).await;
    t.finish(created.is_ok());
    let created = created?;
    Ok((StatusCode::CREATED, Json(to_detail(&created, false))))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<Json<SecretDetail>, AppError> {
    let api = state.secrets_api(&ns)?;
    let t = K8sTimer::new("secrets", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let mut existing = existing?;

    let data: BTreeMap<String, ByteString> = req
        .data
        .into_iter()
        .map(|(k, v)| (k, ByteString(v.into_bytes())))
        .collect();

    existing.data = Some(data);
    existing.string_data = None;
    if let Some(t) = req.secret_type {
        existing.type_ = Some(t);
    }
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

    let t = K8sTimer::new("secrets", "replace");
    let updated = api.replace(&name, &PostParams::default(), &existing).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    Ok(Json(to_detail(&updated, false)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.secrets_api(&ns)?;
    let t = K8sTimer::new("secrets", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;
    Ok(StatusCode::NO_CONTENT)
}
