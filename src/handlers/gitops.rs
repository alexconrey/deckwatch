use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kube::api::{ListParams, Patch, PatchParams};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::{self, K8sTimer};
use crate::state::AppState;
use crate::watcher::{ann, check_remote_head, get_ann};

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
    let dep_api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = dep_api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;

    let enabled = get_ann(&dep, "git-enabled") == Some("true");

    // Prefer new annotation; fall back to legacy `ecr-repository` for
    // deployments configured before the OCI-generic switch.
    let oci = get_ann(&dep, "oci-repository")
        .or_else(|| get_ann(&dep, "ecr-repository"))
        .unwrap_or("")
        .to_string();

    // "Configured" means the Secret exists — never return the value.
    let webhook_secret_configured = if enabled {
        let secrets_api = state.secrets_api(&ns)?;
        secrets_api.get(&webhook_secret_name(&name)).await.is_ok()
    } else {
        false
    };

    let config = if enabled {
        Some(GitOpsConfig {
            repo_url: get_ann(&dep, "git-repo").unwrap_or("").to_string(),
            branch: get_ann(&dep, "git-branch").unwrap_or("main").to_string(),
            token_secret: get_ann(&dep, "git-token-secret").map(|s| s.to_string()).filter(|s| !s.is_empty()),
            dockerfile_path: get_ann(&dep, "dockerfile-path")
                .unwrap_or("Dockerfile")
                .to_string(),
            docker_context: get_ann(&dep, "docker-context").unwrap_or(".").to_string(),
            oci_repository: oci.clone(),
            ecr_repository: oci,
            include_paths: get_ann(&dep, "include-paths")
                .filter(|s| !s.is_empty())
                .map(|s| s.split(',').map(|p| p.to_string()).collect())
                .unwrap_or_default(),
            exclude_paths: get_ann(&dep, "exclude-paths")
                .filter(|s| !s.is_empty())
                .map(|s| s.split(',').map(|p| p.to_string()).collect())
                .unwrap_or_default(),
            poll_interval_seconds: get_ann(&dep, "poll-interval")
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            webhook_enabled: get_ann(&dep, "webhook-enabled") == Some("true"),
            webhook_secret_configured,
        })
    } else {
        None
    };

    Ok(Json(GitOpsStatusResponse {
        enabled,
        config,
        last_commit_sha: get_ann(&dep, "last-commit-sha").map(|s| s.to_string()),
        last_build_status: get_ann(&dep, "last-build-status").map(|s| s.to_string()),
        last_build_job: get_ann(&dep, "last-build-job").map(|s| s.to_string()),
        last_build_time: get_ann(&dep, "last-build-time").map(|s| s.to_string()),
        last_build_error: get_ann(&dep, "last-build-error")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
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
        .ok_or_else(|| {
            AppError::BadRequest("oci_repository is required".to_string())
        })?;

    let dep_api = state.deployments_api(&ns)?;

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

    let mut annotations = BTreeMap::new();
    annotations.insert(ann("git-enabled"), "true".to_string());
    annotations.insert(ann("git-repo"), req.repo_url);
    annotations.insert(
        ann("git-branch"),
        req.branch.unwrap_or_else(|| "main".to_string()),
    );
    if let Some(ref ts) = req.token_secret {
        if !ts.is_empty() {
            annotations.insert(ann("git-token-secret"), ts.clone());
        }
    }
    annotations.insert(
        ann("dockerfile-path"),
        req.dockerfile_path
            .unwrap_or_else(|| "Dockerfile".to_string()),
    );
    annotations.insert(
        ann("docker-context"),
        req.docker_context.unwrap_or_else(|| ".".to_string()),
    );
    // Write the canonical annotation. `ecr-repository` is intentionally not
    // written on new configs; the reader falls back to it for legacy
    // deployments only.
    annotations.insert(ann("oci-repository"), oci_repository);
    annotations.insert(
        ann("include-paths"),
        req.include_paths
            .map(|p| p.join(","))
            .unwrap_or_default(),
    );
    annotations.insert(
        ann("exclude-paths"),
        req.exclude_paths
            .map(|p| p.join(","))
            .unwrap_or_default(),
    );
    annotations.insert(
        ann("poll-interval"),
        req.poll_interval_seconds.unwrap_or(60).to_string(),
    );
    annotations.insert(
        ann("webhook-enabled"),
        req.webhook_enabled.unwrap_or(false).to_string(),
    );

    let patch = serde_json::json!({
        "metadata": {
            "annotations": annotations
        }
    });
    let t = K8sTimer::new("deployments", "patch");
    let res = dep_api
        .patch(&name, &PatchParams::default(), &Patch::Merge(patch))
        .await;
    t.finish(res.is_ok());
    res?;

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
    labels.insert("deckwatch.io/webhook-secret".to_string(), "true".to_string());
    labels.insert("deckwatch.io/deployment".to_string(), deployment.to_string());

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

pub async fn delete_config(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = dep_api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;

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
        return Ok(StatusCode::NO_CONTENT);
    }

    let null_annotations: BTreeMap<String, Option<String>> =
        keys_to_remove.into_iter().map(|k| (k, None)).collect();

    let patch = serde_json::json!({
        "metadata": {
            "annotations": null_annotations
        }
    });
    let t = K8sTimer::new("deployments", "patch");
    let res = dep_api
        .patch(&name, &PatchParams::default(), &Patch::Merge(patch))
        .await;
    t.finish(res.is_ok());
    res?;

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
    let dep_api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = dep_api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;

    if get_ann(&dep, "git-enabled") != Some("true") {
        return Err(AppError::BadRequest(
            "gitops is not enabled on this deployment".to_string(),
        ));
    }

    let repo_url = get_ann(&dep, "git-repo")
        .ok_or_else(|| AppError::BadRequest("missing git-repo".to_string()))?;
    let branch = get_ann(&dep, "git-branch").unwrap_or("main");
    let token = match get_ann(&dep, "git-token-secret") {
        Some(token_secret) if !token_secret.is_empty() => {
            let secrets_api = state.secrets_api(&ns)?;
            let t = K8sTimer::new("secrets", "get");
            let secret = secrets_api.get(token_secret).await;
            t.finish(secret.is_ok());
            let secret = secret?;
            secret
                .data
                .as_ref()
                .and_then(|d| d.get("token"))
                .map(|v| String::from_utf8_lossy(&v.0).to_string())
                .unwrap_or_default()
        }
        _ => String::new(),
    };

    let http = reqwest::Client::new();
    let remote_sha = check_remote_head(&http, repo_url, branch, &token)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to check remote: {e}")))?;

    let short_sha = &remote_sha[..7.min(remote_sha.len())];
    let job_name = crate::watcher::trigger_build_public(&state, &ns, &dep, &remote_sha, &token)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to trigger build: {e}")))?;

    // Record that a build was kicked off for this namespace. The watcher
    // records "success" or "failure" once the Job completes.
    metrics::record_gitops_build(&ns, "started");

    let now = jiff::Timestamp::now().to_string();
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                ann("last-commit-sha"): remote_sha,
                ann("last-build-status"): "building",
                ann("last-build-job"): job_name,
                ann("last-build-time"): now,
                ann("last-build-error"): "",
            }
        }
    });
    let t = K8sTimer::new("deployments", "patch");
    let res = dep_api
        .patch(&name, &PatchParams::default(), &Patch::Merge(patch))
        .await;
    t.finish(res.is_ok());
    res?;

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
    let jobs_api = state.jobs_api(&ns)?;
    let lp =
        ListParams::default().labels(&format!("deckwatch.io/build=true,deckwatch.io/deployment={name}"));
    let t = K8sTimer::new("jobs", "list");
    let jobs = jobs_api.list(&lp).await;
    t.finish(jobs.is_ok());
    let jobs = jobs?;

    let builds: Vec<BuildSummary> = jobs
        .iter()
        .map(|job| {
            let job_name = job.name_any();
            let status = job.status.as_ref();
            let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
            let failed = status.and_then(|s| s.failed).unwrap_or(0);

            let status_str = if succeeded > 0 {
                "succeeded"
            } else if failed > 0 {
                "failed"
            } else {
                "running"
            };

            let commit_sha = job_name
                .strip_prefix(&format!("{name}-build-"))
                .unwrap_or("")
                .to_string();

            BuildSummary {
                job_name: job_name.clone(),
                commit_sha: commit_sha.clone(),
                status: status_str.to_string(),
                started_at: status
                    .and_then(|s| s.start_time.as_ref())
                    .map(|t| t.0.to_string()),
                completed_at: status
                    .and_then(|s| s.completion_time.as_ref())
                    .map(|t| t.0.to_string()),
                image_tag: commit_sha,
            }
        })
        .collect();

    Ok(Json(BuildListResponse { builds }))
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
    let summaries = pods.iter().map(|p| {
        let meta = &p.metadata;
        let phase = p.status.as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        JobPodSummary {
            name: meta.name.clone().unwrap_or_default(),
            phase,
        }
    }).collect();
    Ok(Json(JobPodListResponse { pods: summaries }))
}
