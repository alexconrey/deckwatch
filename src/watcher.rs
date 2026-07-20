#![allow(dead_code, unused_imports)]
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, EnvVarSource, PodSpec, PodTemplateSpec, SecretKeySelector,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};

use sea_orm::entity::prelude::*;
use sea_orm::ActiveModelTrait;
use sea_orm::ActiveValue::Set;

use crate::entities::builds;
use crate::entities::gitops_configs;
use crate::kube_ext::deployment_phase;
use crate::metrics;
use crate::state::AppState;

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

const ANN_PREFIX: &str = "deckwatch.io";

pub fn ann(key: &str) -> String {
    format!("{ANN_PREFIX}/{key}")
}

pub fn get_ann<'a>(dep: &'a Deployment, key: &str) -> Option<&'a str> {
    dep.metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(&ann(key)))
        .map(|s| s.as_str())
}

/// OCI destination for the built image. Reads the new `oci-repository`
/// annotation, falling back to the legacy `ecr-repository` for deployments
/// configured before the OCI-generic switch.
pub fn get_oci_repository(dep: &Deployment) -> Option<&str> {
    get_ann(dep, "oci-repository").or_else(|| get_ann(dep, "ecr-repository"))
}

pub async fn run_poller(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    let http_client = reqwest::Client::new();

    loop {
        interval.tick().await;

        let cycle_start = Instant::now();

        if let Err(e) = poll_cycle(&state, &http_client).await {
            tracing::error!(error = %e, "watcher poll cycle failed");
        }

        if let Err(e) = monitor_builds(&state).await {
            tracing::error!(error = %e, "watcher build monitor failed");
        }

        // Update resource gauges (best-effort; errors are logged, not fatal).
        update_resource_gauges(&state).await;

        metrics::record_gitops_poll_duration(cycle_start.elapsed().as_secs_f64());
    }
}

async fn poll_cycle(state: &AppState, http: &reqwest::Client) -> anyhow::Result<()> {
    // Query all gitops configs from the database instead of scanning
    // deployment annotations.
    let configs = gitops_configs::Entity::find().all(&state.db).await?;

    for config in configs.iter() {
        // Parse application_id as "{ns}/{name}".
        let (ns, dep_name) = match config.application_id.split_once('/') {
            Some(pair) => pair,
            None => {
                tracing::warn!(
                    application_id = %config.application_id,
                    "invalid application_id format, expected 'namespace/name'"
                );
                continue;
            }
        };

        // Respect namespace restrictions.
        if !state.is_namespace_allowed(ns) {
            continue;
        }

        // Skip if already building.
        if config.last_build_status.as_deref() == Some("building") {
            continue;
        }

        if let Err(e) = check_and_build(state, http, ns, dep_name, config).await {
            tracing::warn!(
                deployment = %dep_name,
                namespace = %ns,
                error = %e,
                "git check failed"
            );
        }
    }

    Ok(())
}

async fn check_and_build(
    state: &AppState,
    http: &reqwest::Client,
    ns: &str,
    dep_name: &str,
    config: &gitops_configs::Model,
) -> anyhow::Result<()> {
    let repo_url = &config.repo_url;
    let branch = &config.branch;
    let token = if !config.token_secret.is_empty() {
        let secrets_api = state.secrets_api(ns)?;
        match secrets_api.get(&config.token_secret).await {
            Ok(secret) => secret
                .data
                .as_ref()
                .and_then(|d| d.get("token"))
                .map(|v| String::from_utf8_lossy(&v.0).to_string())
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    let remote_sha: String = check_remote_head(http, repo_url, branch, &token).await?;
    let last_sha = config.last_commit_sha.as_deref().unwrap_or("");

    if remote_sha == last_sha {
        return Ok(());
    }

    let short_sha = &remote_sha[..7.min(remote_sha.len())];

    let include_paths: Vec<&str> = if config.include_paths.is_empty() {
        vec![]
    } else {
        config.include_paths.split(',').collect()
    };
    let exclude_paths: Vec<&str> = if config.exclude_paths.is_empty() {
        vec![]
    } else {
        config.exclude_paths.split(',').collect()
    };

    if (!include_paths.is_empty() || !exclude_paths.is_empty()) && !last_sha.is_empty() {
        if let Some(changed) =
            check_paths_github(http, repo_url, &token, last_sha, &remote_sha).await
        {
            let dominated_by_excludes = !changed.iter().any(|f| {
                let included =
                    include_paths.is_empty() || include_paths.iter().any(|p| f.starts_with(p));
                let excluded = exclude_paths.iter().any(|p| f.starts_with(p));
                included && !excluded
            });
            if dominated_by_excludes {
                tracing::info!(
                    deployment = %dep_name,
                    commit = %short_sha,
                    "skipping build: no included paths changed"
                );
                // Update the DB row with the new commit SHA (skip build).
                update_gitops_config_field(&state.db, &config.application_id, |active| {
                    active.last_commit_sha = Set(Some(remote_sha.clone()));
                    active.updated_at = Set(now_utc());
                })
                .await?;
                return Ok(());
            }
        }
    }

    tracing::info!(
        deployment = %dep_name,
        namespace = %ns,
        commit = %short_sha,
        "new commit detected, triggering build"
    );

    // We still need the Deployment object for trigger_build (Kaniko Job
    // creation needs the dep name). Fetch it from K8s.
    let dep_api = state.deployments_api(ns)?;
    let dep = dep_api.get(dep_name).await?;

    let job_name: String = trigger_build(state, ns, &dep, &remote_sha, &token).await?;
    // Counter incremented once per build kickoff; success/failure is recorded
    // later in monitor_builds when the Job completes.
    metrics::record_gitops_build(ns, "started");

    // Update the gitops_configs row with build status.
    let now = now_utc();
    update_gitops_config_field(&state.db, &config.application_id, |active| {
        active.last_commit_sha = Set(Some(remote_sha.clone()));
        active.last_build_status = Set(Some("building".to_string()));
        active.last_build_job = Set(Some(job_name.clone()));
        active.last_build_time = Set(Some(now));
        active.last_build_error = Set(None);
        active.updated_at = Set(now);
    })
    .await?;

    // Ensure the application row exists before FK insert.
    if let Err(e) = crate::db::ensure_application(&state.db, ns, dep_name).await {
        tracing::warn!(error = %e, "failed to ensure application row");
    }

    // Persist the build in the builds table so history survives Job TTL cleanup.
    {
        let build_row = builds::ActiveModel {
            id: Set(uuid::Uuid::new_v4().to_string()),
            application_id: Set(config.application_id.clone()),
            job_name: Set(job_name.clone()),
            commit_sha: Set(remote_sha.clone()),
            image_tag: Set(short_sha.to_string()),
            status: Set("building".to_string()),
            started_at: Set(Some(now)),
            completed_at: Set(None),
            error_message: Set(None),
            created_at: Set(now),
        };
        if let Err(e) = builds::Entity::insert(build_row).exec(&state.db).await {
            tracing::warn!(error = %e, "failed to insert build row into database");
        }
    }

    Ok(())
}

pub async fn check_remote_head(
    http: &reqwest::Client,
    repo_url: &str,
    branch: &str,
    token: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/info/refs?service=git-upload-pack",
        repo_url.trim_end_matches('/')
    );

    let mut request = http.get(&url);
    if !token.is_empty() {
        let creds = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("x-token:{token}"),
        );
        request = request.header("Authorization", format!("Basic {creds}"));
    }

    let resp = request.send().await?.error_for_status()?.text().await?;

    let target_ref = format!("refs/heads/{branch}");
    for line in resp.lines() {
        if line.contains(&target_ref) {
            let sha = line
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_start_matches(|c: char| !c.is_ascii_hexdigit());
            if sha.len() >= 40 {
                return Ok(sha.to_string());
            }
        }
    }

    anyhow::bail!("branch '{branch}' not found in remote refs")
}

async fn check_paths_github(
    http: &reqwest::Client,
    repo_url: &str,
    token: &str,
    base: &str,
    head: &str,
) -> Option<Vec<String>> {
    if !repo_url.contains("github.com") {
        return None;
    }

    let parts: Vec<&str> = repo_url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplitn(3, '/')
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let repo = parts[0];
    let owner = parts[1];

    let url = format!("https://api.github.com/repos/{owner}/{repo}/compare/{base}...{head}");

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "deckwatch")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?;

    let files = resp
        .get("files")?
        .as_array()?
        .iter()
        .filter_map(|f| f.get("filename")?.as_str().map(|s| s.to_string()))
        .collect();

    Some(files)
}

pub async fn trigger_build_public(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
    commit_sha: &str,
    token: &str,
) -> anyhow::Result<String> {
    trigger_build(state, ns, dep, commit_sha, token).await
}

async fn trigger_build(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
    commit_sha: &str,
    token: &str,
) -> anyhow::Result<String> {
    let dep_name = dep.name_any();
    let short_sha = &commit_sha[..7.min(commit_sha.len())];
    let job_name = format!("{dep_name}-build-{short_sha}");

    // Read config from the database.
    let app_id = format!("{ns}/{dep_name}");
    let config = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no gitops config found for {app_id}"))?;

    let repo_url = &config.repo_url;
    let branch = &config.branch;
    let dockerfile = &config.dockerfile_path;
    let context = &config.docker_context;
    let oci_repo = &config.oci_repository;
    let token_secret = &config.token_secret;

    let repo_no_scheme = repo_url
        .strip_prefix("https://")
        .or_else(|| repo_url.strip_prefix("http://"))
        .unwrap_or(repo_url);

    let kaniko_context = if token.is_empty() {
        format!("git://{repo_no_scheme}#refs/heads/{branch}")
    } else {
        format!("git://x-token:{token}@{repo_no_scheme}#refs/heads/{branch}")
    };

    let mut args = vec![
        format!("--dockerfile={dockerfile}"),
        format!("--context={kaniko_context}"),
        format!("--destination={oci_repo}:{short_sha}"),
        "--cache=true".to_string(),
        "--snapshot-mode=redo".to_string(),
    ];

    if context != "." {
        args.push(format!("--context-sub-path={context}"));
    }

    let mut labels = BTreeMap::new();
    labels.insert("deckwatch.io/build".to_string(), "true".to_string());
    labels.insert("deckwatch.io/deployment".to_string(), dep_name.clone());

    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.clone()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(JobSpec {
            ttl_seconds_after_finished: Some(3600),
            backoff_limit: Some(0),
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    containers: vec![Container {
                        name: "kaniko".to_string(),
                        image: Some("gcr.io/kaniko-project/executor:latest".to_string()),
                        args: Some(args),
                        env: if token_secret.is_empty() {
                            None
                        } else {
                            Some(vec![EnvVar {
                                name: "GIT_TOKEN".to_string(),
                                value_from: Some(EnvVarSource {
                                    secret_key_ref: Some(SecretKeySelector {
                                        name: token_secret.to_string(),
                                        key: "token".to_string(),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }])
                        },
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let jobs_api = state.jobs_api(ns)?;

    // Clean up any existing job with the same name (e.g. from a previous failed build)
    let _ = jobs_api.delete(&job_name, &Default::default()).await;
    jobs_api.create(&PostParams::default(), &job).await?;

    Ok(job_name)
}

async fn monitor_builds(state: &AppState) -> anyhow::Result<()> {
    // Find all gitops configs that have an active build ("building" status).
    let building_configs = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::LastBuildStatus.eq("building"))
        .all(&state.db)
        .await?;

    for config in building_configs.iter() {
        let (ns, dep_name) = match config.application_id.split_once('/') {
            Some(pair) => pair,
            None => continue,
        };

        if !state.is_namespace_allowed(ns) {
            continue;
        }

        let job_name = match config.last_build_job.as_deref() {
            Some(j) if !j.is_empty() => j,
            _ => continue,
        };

        // Check the Job status in Kubernetes.
        let jobs_api: Api<Job> = Api::namespaced(state.kube_client.clone(), ns);
        let job = match jobs_api.get(job_name).await {
            Ok(j) => j,
            Err(_) => continue,
        };

        let status = job.status.as_ref();
        let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
        let failed = status.and_then(|s| s.failed).unwrap_or(0);

        if succeeded == 0 && failed == 0 {
            continue;
        }

        let now = now_utc();

        if succeeded > 0 {
            let oci_repo = &config.oci_repository;
            let commit_sha = config.last_commit_sha.as_deref().unwrap_or("");
            let short_sha = &commit_sha[..7.min(commit_sha.len())];
            let new_image = format!("{oci_repo}:{short_sha}");

            tracing::info!(
                deployment = %dep_name,
                image = %new_image,
                "build succeeded, updating deployment image"
            );

            // Update the Deployment's image in K8s (this stays in K8s).
            let dep_api = state.deployments_api(ns)?;
            let image_patch = serde_json::json!({
                "spec": {
                    "template": {
                        "spec": {
                            "containers": [{
                                "name": dep_name,
                                "image": new_image,
                            }]
                        }
                    }
                }
            });
            let _ = dep_api
                .patch(
                    dep_name,
                    &PatchParams::default(),
                    &Patch::Strategic(image_patch),
                )
                .await;

            // Update the gitops_configs row.
            update_gitops_config_field(&state.db, &config.application_id, |active| {
                active.last_build_status = Set(Some("success".to_string()));
                active.last_build_time = Set(Some(now));
                active.last_build_error = Set(None);
                active.updated_at = Set(now);
            })
            .await?;
            metrics::record_gitops_build(ns, "success");

            // Update the builds DB row.
            update_build_status(&state.db, job_name, "succeeded", None).await;
        } else {
            tracing::warn!(deployment = %dep_name, "build failed");

            // Update the gitops_configs row.
            update_gitops_config_field(&state.db, &config.application_id, |active| {
                active.last_build_status = Set(Some("failed".to_string()));
                active.last_build_error = Set(Some("Kaniko build job failed".to_string()));
                active.last_build_time = Set(Some(now));
                active.updated_at = Set(now);
            })
            .await?;
            metrics::record_gitops_build(ns, "failure");

            // Update the builds DB row.
            update_build_status(
                &state.db,
                job_name,
                "failed",
                Some("Kaniko build job failed"),
            )
            .await;
        }
    }

    Ok(())
}

/// Load the gitops_configs row for the given application_id, apply the
/// provided mutations to the active model, and save it back. Returns an
/// error if the row is not found.
async fn update_gitops_config_field<F>(
    db: &sea_orm::DatabaseConnection,
    application_id: &str,
    mutate: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut gitops_configs::ActiveModel),
{
    use sea_orm::QueryFilter;

    let row = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(application_id))
        .one(db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no gitops config found for {application_id}"))?;

    let mut active: gitops_configs::ActiveModel = row.into();
    mutate(&mut active);
    active.update(db).await?;

    Ok(())
}

/// Update a build row in the database by job name. Fire-and-forget — logs a
/// warning on failure so the watcher loop is not interrupted.
async fn update_build_status(
    db: &sea_orm::DatabaseConnection,
    job_name: &str,
    status: &str,
    error_message: Option<&str>,
) {
    use sea_orm::QueryFilter;

    let row = match builds::Entity::find()
        .filter(builds::Column::JobName.eq(job_name))
        .one(db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::debug!(job_name, "no builds DB row found for job; skipping update");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, job_name, "failed to query builds table");
            return;
        }
    };

    let now_utc = now_utc();
    let mut active: builds::ActiveModel = row.into();
    active.status = Set(status.to_string());
    active.completed_at = Set(Some(now_utc));
    active.error_message = Set(error_message.map(|s| s.to_string()));

    if let Err(e) = active.update(db).await {
        tracing::warn!(error = %e, job_name, "failed to update build row in database");
    }
}

/// Refresh all resource-count gauges. Runs once per poll cycle.
///
/// For deployments we break down by (namespace, status) so Prometheus can
/// alert on degraded/failed counts. For ingresses we break down by namespace.
/// The gitops watcher count is a simple scalar.
async fn update_resource_gauges(state: &AppState) {
    // ---- gitops watchers gauge ----
    match gitops_configs::Entity::find().count(&state.db).await {
        Ok(count) => metrics::set_gitops_watchers(count as f64),
        Err(e) => tracing::debug!(error = %e, "failed to count gitops configs"),
    }

    // Determine which namespaces to scan.
    let namespaces: Vec<String> = if state.allowed_namespaces.is_empty() {
        // All namespaces mode: list namespaces from the cluster.
        let ns_api = state.namespaces_api();
        match ns_api.list(&ListParams::default()).await {
            Ok(ns_list) => ns_list
                .iter()
                .filter_map(|ns| ns.metadata.name.clone())
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "failed to list namespaces for gauge update");
                return;
            }
        }
    } else {
        state.allowed_namespaces.clone()
    };

    // Phase labels used for the deployment gauge. Order matches DeploymentPhase.
    let phase_labels = [
        "available",
        "progressing",
        "degraded",
        "failed",
        "scaled_to_zero",
    ];

    for ns in &namespaces {
        // ---- deployments gauge ----
        let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
        match dep_api.list(&ListParams::default()).await {
            Ok(deps) => {
                let mut counts: HashMap<&str, f64> = HashMap::new();
                for label in &phase_labels {
                    counts.insert(label, 0.0);
                }
                for dep in deps.iter() {
                    let phase = deployment_phase(dep);
                    let label = match phase {
                        crate::kube_ext::DeploymentPhase::Available => "available",
                        crate::kube_ext::DeploymentPhase::Progressing => "progressing",
                        crate::kube_ext::DeploymentPhase::Degraded => "degraded",
                        crate::kube_ext::DeploymentPhase::Failed => "failed",
                        crate::kube_ext::DeploymentPhase::ScaledToZero => "scaled_to_zero",
                    };
                    *counts.entry(label).or_insert(0.0) += 1.0;
                }
                for (status, count) in &counts {
                    metrics::set_deployments_managed(ns, status, *count);
                }
            }
            Err(e) => {
                tracing::debug!(namespace = %ns, error = %e, "failed to list deployments for gauge");
            }
        }

        // ---- ingresses gauge ----
        let ing_api: Api<k8s_openapi::api::networking::v1::Ingress> =
            Api::namespaced(state.kube_client.clone(), ns);
        match ing_api.list(&ListParams::default()).await {
            Ok(ingresses) => {
                metrics::set_ingresses_managed(ns, ingresses.items.len() as f64);
            }
            Err(e) => {
                tracing::debug!(namespace = %ns, error = %e, "failed to list ingresses for gauge");
            }
        }
    }
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod watcher_tests;
