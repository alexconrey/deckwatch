use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kube::api::{Patch, PatchParams};
use sea_orm::entity::prelude::DateTimeUtc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::entities::builds;
use crate::entities::gitops_configs;
use crate::error::AppError;
use crate::metrics::{self, K8sTimer};
use crate::state::AppState;
use crate::watcher::{check_remote_head, get_ann};

/// Return the current UTC time as a `DateTimeUtc` without requiring a direct
/// `chrono` dependency.
fn now_utc() -> DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

#[derive(Deserialize)]
pub struct GitOpsConfigRequest {
    pub repo_url: String,
    pub branch: Option<String>,
    pub token_secret: Option<String>,
    pub dockerfile_path: Option<String>,
    pub docker_context: Option<String>,
    /// Preferred field: OCI-generic registry destination
    /// (e.g. `docker.io/myorg/api`, `ghcr.io/myorg/api`,
    /// `591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/my-app`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_repository: Option<String>,
    /// Deprecated alias for `oci_repository`. Accepted for backwards
    /// compatibility with clients that still send the ECR-specific field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecr_repository: Option<String>,
    pub include_paths: Option<Vec<String>>,
    pub exclude_paths: Option<Vec<String>>,
    pub poll_interval_seconds: Option<i64>,
    pub webhook_enabled: Option<bool>,
    /// Optional shared secret used to sign incoming webhook deliveries.
    /// When set, the webhook receiver rejects deliveries whose signature
    /// header doesn't match. Stored in a per-deployment Kubernetes Secret;
    /// this field is only read on write and is never echoed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
}

#[derive(Serialize)]
pub struct GitOpsStatusResponse {
    pub enabled: bool,
    pub config: Option<GitOpsConfig>,
    pub last_commit_sha: Option<String>,
    pub last_build_status: Option<String>,
    pub last_build_job: Option<String>,
    pub last_build_time: Option<String>,
    pub last_build_error: Option<String>,
}

#[derive(Serialize)]
pub struct GitOpsConfig {
    pub repo_url: String,
    pub branch: String,
    pub token_secret: Option<String>,
    pub dockerfile_path: String,
    pub docker_context: String,
    /// Canonical field going forward.
    pub oci_repository: String,
    /// Mirror of `oci_repository` kept in the payload so older frontend
    /// bundles that still read `ecr_repository` continue to work.
    pub ecr_repository: String,
    pub include_paths: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub poll_interval_seconds: i64,
    pub webhook_enabled: bool,
    /// Whether a webhook signing secret is configured. The actual secret
    /// value is never returned; only its presence, so the UI can render
    /// "configured" vs "not configured" without leaking the value.
    pub webhook_secret_configured: bool,
}

#[derive(Serialize)]
pub struct BuildListResponse {
    pub builds: Vec<BuildSummary>,
}

#[derive(Serialize)]
pub struct BuildSummary {
    pub job_name: String,
    pub commit_sha: String,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub image_tag: String,
}

/// Name of the Kubernetes Secret that holds a per-deployment webhook signing
/// secret. Kept namespaced-by-deployment so operators can rotate one repo's
/// secret without touching another.
pub fn webhook_secret_name(deployment: &str) -> String {
    format!("{deployment}-gitops-webhook")
}

pub async fn get_config(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<GitOpsStatusResponse>, AppError> {
    let app_id = format!("{ns}/{name}");

    // Try to load config from the database first.
    let row = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;

    // If no DB row exists, check for legacy annotations and migrate them.
    let row = match row {
        Some(r) => Some(r),
        None => {
            let dep_api = state.deployments_api(&ns)?;
            let t = K8sTimer::new("deployments", "get");
            let dep = dep_api.get(&name).await;
            t.finish(dep.is_ok());
            let dep = dep?;

            if get_ann(&dep, "git-enabled") == Some("true") {
                // Ensure application row exists before FK insert.
                crate::db::ensure_application(&state.db, &ns, &name)
                    .await
                    .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;
                // Migrate legacy annotations to DB.
                let oci = get_ann(&dep, "oci-repository")
                    .or_else(|| get_ann(&dep, "ecr-repository"))
                    .unwrap_or("")
                    .to_string();

                let now = now_utc();
                let model = gitops_configs::ActiveModel {
                    id: sea_orm::ActiveValue::Set(uuid::Uuid::new_v4().to_string()),
                    application_id: sea_orm::ActiveValue::Set(app_id.clone()),
                    repo_url: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "git-repo").unwrap_or("").to_string(),
                    ),
                    branch: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "git-branch").unwrap_or("main").to_string(),
                    ),
                    token_secret: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "git-token-secret").unwrap_or("").to_string(),
                    ),
                    dockerfile_path: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "dockerfile-path")
                            .unwrap_or("Dockerfile")
                            .to_string(),
                    ),
                    docker_context: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "docker-context").unwrap_or(".").to_string(),
                    ),
                    oci_repository: sea_orm::ActiveValue::Set(oci),
                    include_paths: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "include-paths").unwrap_or("").to_string(),
                    ),
                    exclude_paths: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "exclude-paths").unwrap_or("").to_string(),
                    ),
                    poll_interval_seconds: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "poll-interval")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(60),
                    ),
                    webhook_enabled: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "webhook-enabled") == Some("true"),
                    ),
                    last_commit_sha: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "last-commit-sha").map(|s| s.to_string()),
                    ),
                    last_build_status: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "last-build-status").map(|s| s.to_string()),
                    ),
                    last_build_job: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "last-build-job").map(|s| s.to_string()),
                    ),
                    last_build_time: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "last-build-time")
                            .and_then(|s| s.parse::<DateTimeUtc>().ok()),
                    ),
                    last_build_error: sea_orm::ActiveValue::Set(
                        get_ann(&dep, "last-build-error")
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string()),
                    ),
                    created_at: sea_orm::ActiveValue::Set(now),
                    updated_at: sea_orm::ActiveValue::Set(now),
                };
                let inserted = model
                    .insert(&state.db)
                    .await
                    .map_err(|e| AppError::BadRequest(format!("db insert error: {e}")))?;

                // Remove legacy annotations from the deployment.
                remove_legacy_annotations(&state, &ns, &name).await?;

                Some(inserted)
            } else {
                None
            }
        }
    };

    let (
        enabled,
        config,
        last_commit_sha,
        last_build_status,
        last_build_job,
        last_build_time,
        last_build_error,
    ) = match row {
        Some(ref r) => {
            // "Configured" means the Secret exists — never return the value.
            let webhook_secret_configured = {
                let secrets_api = state.secrets_api(&ns)?;
                secrets_api.get(&webhook_secret_name(&name)).await.is_ok()
            };

            let oci = r.oci_repository.clone();
            let config = GitOpsConfig {
                repo_url: r.repo_url.clone(),
                branch: r.branch.clone(),
                token_secret: if r.token_secret.is_empty() {
                    None
                } else {
                    Some(r.token_secret.clone())
                },
                dockerfile_path: r.dockerfile_path.clone(),
                docker_context: r.docker_context.clone(),
                oci_repository: oci.clone(),
                ecr_repository: oci,
                include_paths: if r.include_paths.is_empty() {
                    vec![]
                } else {
                    r.include_paths.split(',').map(|s| s.to_string()).collect()
                },
                exclude_paths: if r.exclude_paths.is_empty() {
                    vec![]
                } else {
                    r.exclude_paths.split(',').map(|s| s.to_string()).collect()
                },
                poll_interval_seconds: r.poll_interval_seconds as i64,
                webhook_enabled: r.webhook_enabled,
                webhook_secret_configured,
            };

            (
                true,
                Some(config),
                r.last_commit_sha.clone(),
                r.last_build_status.clone(),
                r.last_build_job.clone(),
                r.last_build_time.map(|t| t.to_string()),
                r.last_build_error.clone().filter(|s| !s.is_empty()),
            )
        }
        None => (false, None, None, None, None, None, None),
    };

    Ok(Json(GitOpsStatusResponse {
        enabled,
        config,
        last_commit_sha,
        last_build_status,
        last_build_job,
        last_build_time,
        last_build_error,
    }))
}

pub async fn set_config(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<GitOpsConfigRequest>,
) -> Result<Json<GitOpsStatusResponse>, AppError> {
    if req.repo_url.is_empty() {
        return Err(AppError::BadRequest("repo_url is required".to_string()));
    }
    // token_secret is optional — public repos don't need one

    let oci_repository = req
        .oci_repository
        .clone()
        .or(req.ecr_repository.clone())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("oci_repository is required".to_string()))?;

    // Verify the deployment exists and namespace is allowed.
    let _dep_api = state.deployments_api(&ns)?;

    // If a webhook signing secret was provided (non-empty), upsert it into a
    // per-deployment Kubernetes Secret. An empty/missing string is treated
    // as "don't touch" so the UI can save the rest of the form without
    // clobbering an existing secret. Callers rotate the secret by sending
    // the new value; deleting the whole gitops config removes the Secret.
    if let Some(webhook_secret) = req.webhook_secret.as_deref() {
        if !webhook_secret.is_empty() {
            upsert_webhook_secret(&state, &ns, &name, webhook_secret).await?;
        }
    }

    let app_id = crate::db::ensure_application(&state.db, &ns, &name)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;
    let now = now_utc();

    // Check for existing row to decide insert vs update.
    let existing = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;

    let branch = req.branch.unwrap_or_else(|| "main".to_string());
    let token_secret = req.token_secret.unwrap_or_default();
    let dockerfile_path = req
        .dockerfile_path
        .unwrap_or_else(|| "Dockerfile".to_string());
    let docker_context = req.docker_context.unwrap_or_else(|| ".".to_string());
    let include_paths = req.include_paths.map(|p| p.join(",")).unwrap_or_default();
    let exclude_paths = req.exclude_paths.map(|p| p.join(",")).unwrap_or_default();
    let poll_interval_seconds = req.poll_interval_seconds.unwrap_or(60) as i32;
    let webhook_enabled = req.webhook_enabled.unwrap_or(false);

    match existing {
        Some(row) => {
            // Update existing row.
            let mut active: gitops_configs::ActiveModel = row.into();
            active.repo_url = sea_orm::ActiveValue::Set(req.repo_url);
            active.branch = sea_orm::ActiveValue::Set(branch);
            active.token_secret = sea_orm::ActiveValue::Set(token_secret);
            active.dockerfile_path = sea_orm::ActiveValue::Set(dockerfile_path);
            active.docker_context = sea_orm::ActiveValue::Set(docker_context);
            active.oci_repository = sea_orm::ActiveValue::Set(oci_repository);
            active.include_paths = sea_orm::ActiveValue::Set(include_paths);
            active.exclude_paths = sea_orm::ActiveValue::Set(exclude_paths);
            active.poll_interval_seconds = sea_orm::ActiveValue::Set(poll_interval_seconds);
            active.webhook_enabled = sea_orm::ActiveValue::Set(webhook_enabled);
            active.updated_at = sea_orm::ActiveValue::Set(now);
            active
                .update(&state.db)
                .await
                .map_err(|e| AppError::BadRequest(format!("db update error: {e}")))?;
        }
        None => {
            // Insert new row.
            let model = gitops_configs::ActiveModel {
                id: sea_orm::ActiveValue::Set(uuid::Uuid::new_v4().to_string()),
                application_id: sea_orm::ActiveValue::Set(app_id),
                repo_url: sea_orm::ActiveValue::Set(req.repo_url),
                branch: sea_orm::ActiveValue::Set(branch),
                token_secret: sea_orm::ActiveValue::Set(token_secret),
                dockerfile_path: sea_orm::ActiveValue::Set(dockerfile_path),
                docker_context: sea_orm::ActiveValue::Set(docker_context),
                oci_repository: sea_orm::ActiveValue::Set(oci_repository),
                include_paths: sea_orm::ActiveValue::Set(include_paths),
                exclude_paths: sea_orm::ActiveValue::Set(exclude_paths),
                poll_interval_seconds: sea_orm::ActiveValue::Set(poll_interval_seconds),
                webhook_enabled: sea_orm::ActiveValue::Set(webhook_enabled),
                last_commit_sha: sea_orm::ActiveValue::Set(None),
                last_build_status: sea_orm::ActiveValue::Set(None),
                last_build_job: sea_orm::ActiveValue::Set(None),
                last_build_time: sea_orm::ActiveValue::Set(None),
                last_build_error: sea_orm::ActiveValue::Set(None),
                created_at: sea_orm::ActiveValue::Set(now),
                updated_at: sea_orm::ActiveValue::Set(now),
            };
            model
                .insert(&state.db)
                .await
                .map_err(|e| AppError::BadRequest(format!("db insert error: {e}")))?;
        }
    }

    // Remove legacy annotations from the deployment (clean migration).
    remove_legacy_annotations(&state, &ns, &name).await?;

    get_config(State(state), Path((ns, name))).await
}

/// Create or replace the per-deployment webhook secret Secret. The signing
/// key is stored under a single `secret` data key so the webhook receiver
/// can pull it uniformly regardless of provider.
async fn upsert_webhook_secret(
    state: &AppState,
    ns: &str,
    deployment: &str,
    value: &str,
) -> Result<(), AppError> {
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;

    let secret_name = webhook_secret_name(deployment);
    let secrets_api = state.secrets_api(ns)?;

    let mut data = BTreeMap::new();
    data.insert("secret".to_string(), ByteString(value.as_bytes().to_vec()));

    let mut labels = BTreeMap::new();
    labels.insert(
        "deckwatch.io/webhook-secret".to_string(),
        "true".to_string(),
    );
    labels.insert(
        "deckwatch.io/deployment".to_string(),
        deployment.to_string(),
    );

    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(secret_name.clone()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    };

    // Try create first; on 409 (already exists) fall back to a Merge patch
    // that only rewrites the `data` field. Avoids clobbering unrelated
    // metadata a cluster admin may have added (e.g. ExternalSecrets
    // annotations pointing at a secret manager).
    match secrets_api
        .create(&kube::api::PostParams::default(), &secret)
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(e)) if e.code == 409 => {
            let patch = serde_json::json!({
                "data": {
                    "secret": base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        value.as_bytes(),
                    )
                }
            });
            secrets_api
                .patch(&secret_name, &PatchParams::default(), &Patch::Merge(patch))
                .await?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Remove all `deckwatch.io/*` annotations from a deployment. Used during
/// migration from annotation-based config to database-backed config and
/// on delete to clean up any leftover annotations.
async fn remove_legacy_annotations(state: &AppState, ns: &str, name: &str) -> Result<(), AppError> {
    let dep_api = state.deployments_api(ns)?;

    // Best-effort: if the deployment doesn't exist, skip cleanup.
    let dep = match dep_api.get(name).await {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };

    let keys_to_remove: Vec<String> = dep
        .metadata
        .annotations
        .as_ref()
        .map(|a| {
            a.keys()
                .filter(|k| k.starts_with("deckwatch.io/"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    if keys_to_remove.is_empty() {
        return Ok(());
    }

    let null_annotations: BTreeMap<String, Option<String>> =
        keys_to_remove.into_iter().map(|k| (k, None)).collect();

    let patch = serde_json::json!({
        "metadata": {
            "annotations": null_annotations
        }
    });
    let _ = dep_api
        .patch(name, &PatchParams::default(), &Patch::Merge(patch))
        .await;

    Ok(())
}

pub async fn delete_config(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    // Verify namespace is allowed.
    let _ = state.deployments_api(&ns)?;

    let app_id = format!("{ns}/{name}");

    // Delete the DB row.
    gitops_configs::Entity::delete_many()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .exec(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db delete error: {e}")))?;

    // Remove legacy annotations from the deployment.
    remove_legacy_annotations(&state, &ns, &name).await?;

    // Best-effort cleanup of the webhook signing Secret. Ignore any error
    // (usually NotFound if webhooks were never enabled) so callers hitting
    // delete twice don't see spurious failures.
    let secrets_api = state.secrets_api(&ns)?;
    let _ = secrets_api
        .delete(&webhook_secret_name(&name), &Default::default())
        .await;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn trigger_build(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let app_id = crate::db::ensure_application(&state.db, &ns, &name)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;

    // Read gitops config from the DB.
    let config_row = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?
        .ok_or_else(|| {
            AppError::BadRequest("gitops is not enabled on this deployment".to_string())
        })?;

    // We still need the Deployment object for trigger_build_public (it reads
    // annotations for Kaniko args). Fetch it.
    let dep_api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = dep_api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;

    let token = if !config_row.token_secret.is_empty() {
        let secrets_api = state.secrets_api(&ns)?;
        let t = K8sTimer::new("secrets", "get");
        let secret = secrets_api.get(&config_row.token_secret).await;
        t.finish(secret.is_ok());
        let secret = secret?;
        secret
            .data
            .as_ref()
            .and_then(|d| d.get("token"))
            .map(|v| String::from_utf8_lossy(&v.0).to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let http = reqwest::Client::new();
    let remote_sha = check_remote_head(&http, &config_row.repo_url, &config_row.branch, &token)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to check remote: {e}")))?;

    let short_sha = &remote_sha[..7.min(remote_sha.len())];
    let job_name = crate::watcher::trigger_build_public(&state, &ns, &dep, &remote_sha, &token)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to trigger build: {e}")))?;

    // Record that a build was kicked off for this namespace. The watcher
    // records "success" or "failure" once the Job completes.
    metrics::record_gitops_build(&ns, "started");

    // Update the gitops_configs row with build status.
    let now = now_utc();
    let mut active: gitops_configs::ActiveModel = config_row.into();
    active.last_commit_sha = sea_orm::ActiveValue::Set(Some(remote_sha.clone()));
    active.last_build_status = sea_orm::ActiveValue::Set(Some("building".to_string()));
    active.last_build_job = sea_orm::ActiveValue::Set(Some(job_name.clone()));
    active.last_build_time = sea_orm::ActiveValue::Set(Some(now));
    active.last_build_error = sea_orm::ActiveValue::Set(None);
    active.updated_at = sea_orm::ActiveValue::Set(now);
    active
        .update(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db update error: {e}")))?;

    // Persist the build in the builds table so history survives Job TTL cleanup.
    {
        let build_row = builds::ActiveModel {
            id: sea_orm::ActiveValue::Set(uuid::Uuid::new_v4().to_string()),
            application_id: sea_orm::ActiveValue::Set(app_id),
            job_name: sea_orm::ActiveValue::Set(job_name.clone()),
            commit_sha: sea_orm::ActiveValue::Set(remote_sha.clone()),
            image_tag: sea_orm::ActiveValue::Set(short_sha.to_string()),
            status: sea_orm::ActiveValue::Set("building".to_string()),
            started_at: sea_orm::ActiveValue::Set(Some(now)),
            completed_at: sea_orm::ActiveValue::Set(None),
            error_message: sea_orm::ActiveValue::Set(None),
            created_at: sea_orm::ActiveValue::Set(now),
        };
        if let Err(e) = builds::Entity::insert(build_row).exec(&state.db).await {
            tracing::warn!(error = %e, "failed to insert build row into database");
        }
    }

    Ok(Json(serde_json::json!({
        "message": "build triggered",
        "job_name": job_name,
        "commit_sha": short_sha,
    })))
}

pub async fn list_builds(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<BuildListResponse>, AppError> {
    // Ensure the namespace is allowed.
    let _ = state.deployments_api(&ns)?;

    let app_id = format!("{ns}/{name}");
    let rows = builds::Entity::find()
        .filter(builds::Column::ApplicationId.eq(&app_id))
        .order_by_desc(builds::Column::CreatedAt)
        .limit(50)
        .all(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to query builds: {e}")))?;

    let build_list: Vec<BuildSummary> = rows
        .into_iter()
        .map(|row| BuildSummary {
            job_name: row.job_name,
            commit_sha: row.commit_sha,
            status: row.status,
            started_at: row.started_at.map(|t| t.to_rfc3339()),
            completed_at: row.completed_at.map(|t| t.to_rfc3339()),
            image_tag: row.image_tag,
        })
        .collect();

    Ok(Json(BuildListResponse { builds: build_list }))
}

#[derive(serde::Serialize)]
pub struct JobPodSummary {
    pub name: String,
    pub phase: String,
}

#[derive(serde::Serialize)]
pub struct JobPodListResponse {
    pub pods: Vec<JobPodSummary>,
}

pub async fn list_job_pods(
    State(state): State<AppState>,
    Path((ns, job_name)): Path<(String, String)>,
) -> Result<Json<JobPodListResponse>, AppError> {
    let pods_api = state.pods_api(&ns)?;
    let lp = kube::api::ListParams::default().labels(&format!("job-name={job_name}"));
    let pods = pods_api.list(&lp).await?;
    let summaries = pods
        .iter()
        .map(|p| {
            let meta = &p.metadata;
            let phase = p
                .status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            JobPodSummary {
                name: meta.name.clone().unwrap_or_default(),
                phase,
            }
        })
        .collect();
    Ok(Json(JobPodListResponse { pods: summaries }))
}

#[cfg(test)]
#[path = "../handlers_gitops_tests.rs"]
mod tests;
