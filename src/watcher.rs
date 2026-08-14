#![allow(dead_code, unused_imports)]
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, NodeAffinity, NodeSelector, NodeSelectorRequirement, NodeSelectorTerm,
    PodSpec, PodTemplateSpec, Secret,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, LogParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};

use sea_orm::entity::prelude::*;
use sea_orm::ActiveModelTrait;
use sea_orm::ActiveValue::Set;

use crate::entities::application_plugin_resources;
use crate::entities::builds;
use crate::entities::gitops_configs;
use crate::kube_ext::deployment_phase;
use crate::metrics;
use crate::plugins::SidecarSpec;
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
    let mut pending_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    let http_client = reqwest::Client::new();

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let cycle_start = Instant::now();

                if let Err(e) = poll_cycle(&state, &http_client).await {
                    tracing::error!(error = %e, "watcher poll cycle failed");
                }

                if let Err(e) = monitor_builds(&state, &http_client).await {
                    tracing::error!(error = %e, "watcher build monitor failed");
                }

                reconcile_application_plugins(&state).await;

                // Update resource gauges (best-effort; errors are logged, not fatal).
                update_resource_gauges(&state).await;

                metrics::record_gitops_poll_duration(cycle_start.elapsed().as_secs_f64());
            }
            _ = pending_interval.tick() => {
                reconcile_pending_resources(&state).await;
            }
        }
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
    // Resolve the git token. Shared token_secret takes priority over any
    // per-app encrypted_token so that switching from per-app to shared
    // tokens works even if a stale encrypted_token row remains.
    let token = if !config.token_secret.is_empty() {
        // Shared token from settings (looked up by name)
        let settings = crate::handlers::settings::load_settings_from_db(state).await;
        // resolve_token tries encrypted_token first, then falls back to
        // reading the k8s secret (secret_name/namespace) — same as plugins.rs.
        crate::plugins::resolve_git_token(&config.token_secret, &settings, state)
            .await
            .unwrap_or_default()
    } else if let Some(encrypted) = config.encrypted_token.as_deref() {
        // Per-app encrypted token (stored on the gitops config itself)
        crate::crypto::decrypt(&state.encryption_key, encrypted).unwrap_or_default()
    } else {
        String::new()
    };

    let auth_user = resolve_git_auth_user(&config.git_auth_user, repo_url);

    let remote_sha: String = check_remote_head(http, repo_url, branch, &token, &auth_user).await?;
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

    let job_name: String = trigger_build(state, ns, &dep, &remote_sha, &token, &auth_user).await?;
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
            build_log: Set(None),
            created_at: Set(now),
        };
        if let Err(e) = builds::Entity::insert(build_row).exec(&state.db).await {
            tracing::warn!(error = %e, "failed to insert build row into database");
        }
    }

    Ok(())
}

/// Resolve the HTTP Basic auth username for git operations. If the user
/// configured an explicit value, use it. Otherwise auto-detect from the
/// repo hostname: `oauth2` for GitLab, `x-access-token` for GitHub,
/// `x-token-auth` for Bitbucket, `oauth2` as the fallback.
pub fn resolve_git_auth_user(configured: &str, repo_url: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    let host = repo_url
        .strip_prefix("https://")
        .or_else(|| repo_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or("");
    if host.contains("github.com") || host.contains("github.") {
        "x-access-token".to_string()
    } else if host.contains("bitbucket.org") || host.contains("bitbucket.") {
        "x-token-auth".to_string()
    } else {
        "oauth2".to_string()
    }
}

pub async fn check_remote_head(
    http: &reqwest::Client,
    repo_url: &str,
    branch: &str,
    token: &str,
    auth_user: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/info/refs?service=git-upload-pack",
        repo_url.trim_end_matches('/')
    );

    let mut request = http.get(&url);
    if !token.is_empty() {
        let creds = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{auth_user}:{token}"),
        );
        request = request.header("Authorization", format!("Basic {creds}"));
    }

    let resp = request.send().await?.error_for_status()?.text().await?;

    parse_ref_sha(&resp, branch)
}

fn parse_ref_sha(resp: &str, branch: &str) -> anyhow::Result<String> {
    let target_ref = format!("refs/heads/{branch}");
    let mut search_from = 0;
    while let Some(pos) = resp[search_from..].find(&target_ref) {
        let ref_pos = search_from + pos;
        let before = &resp[..ref_pos];
        let before = before.trim_end_matches([' ', '\0']);
        if before.len() >= 40 {
            let sha = &before[before.len() - 40..];
            if sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(sha.to_string());
            }
        }
        search_from = ref_pos + target_ref.len();
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
    let config = {
        let app_id = format!("{ns}/{}", dep.name_any());
        gitops_configs::Entity::find()
            .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
            .one(&state.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no gitops config found for {app_id}"))?
    };
    let auth_user = resolve_git_auth_user(&config.git_auth_user, &config.repo_url);
    trigger_build(state, ns, dep, commit_sha, token, &auth_user).await
}

/// Compiled-in fallback used when no `build_architectures` are configured
/// in settings or when all entries are disabled.
const DEFAULT_BUILD_ARCHES: &[(&str, &str)] = &[("linux/amd64", "amd64"), ("linux/arm64", "arm64")];

/// Resolve the list of (platform, arch) pairs to build from settings.
/// Falls back to `DEFAULT_BUILD_ARCHES` when the list is empty or fully disabled.
fn resolve_build_arches(
    settings: &crate::handlers::settings::DeckwatchSettings,
) -> Vec<(String, String)> {
    let enabled: Vec<(String, String)> = settings
        .build_architectures
        .iter()
        .filter(|a| a.enabled)
        .map(|a| (a.platform.clone(), a.arch.clone()))
        .collect();
    if enabled.is_empty() {
        tracing::warn!("no build architectures enabled in settings; using compiled-in defaults");
        DEFAULT_BUILD_ARCHES
            .iter()
            .map(|&(p, a)| (p.to_string(), a.to_string()))
            .collect()
    } else {
        enabled
    }
}

/// Build a `NodeAffinity` that forces a pod onto a node with the given
/// architecture (e.g. `amd64` or `arm64`).
fn arch_node_affinity(arch: &str) -> k8s_openapi::api::core::v1::Affinity {
    k8s_openapi::api::core::v1::Affinity {
        node_affinity: Some(NodeAffinity {
            required_during_scheduling_ignored_during_execution: Some(NodeSelector {
                node_selector_terms: vec![NodeSelectorTerm {
                    match_expressions: Some(vec![NodeSelectorRequirement {
                        key: "kubernetes.io/arch".to_string(),
                        operator: "In".to_string(),
                        values: Some(vec![arch.to_string()]),
                    }]),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Resolve the Kaniko push destination, rewriting the public registry URL to
/// the in-cluster URL when the internal registry is configured.
fn resolve_kaniko_destination(state: &AppState, oci_repo: &str) -> String {
    rewrite_registry_url(
        state.registry_internal_url.as_deref(),
        state.registry_public_url.as_deref(),
        oci_repo,
    )
}

/// Pure helper: rewrite `oci_repo` from public to internal URL when both are
/// configured and `oci_repo` matches the public prefix.
fn rewrite_registry_url(
    internal_url: Option<&str>,
    public_url: Option<&str>,
    oci_repo: &str,
) -> String {
    match (internal_url, public_url) {
        (Some(internal), Some(public)) if oci_repo.starts_with(public) => {
            oci_repo.replacen(public, internal, 1)
        }
        _ => oci_repo.to_string(),
    }
}

/// Return `true` when the destination points at the embedded deckwatch
/// registry, which speaks plain HTTP and needs `--insecure-registry`.
fn is_internal_registry(state: &AppState, kaniko_destination: &str) -> bool {
    check_internal_registry(
        state.registry_public_url.as_deref(),
        state.registry_internal_url.as_deref(),
        kaniko_destination,
    )
}

/// Pure helper: returns true when `kaniko_destination` starts with the
/// public or internal registry URL prefix.
fn check_internal_registry(
    public_url: Option<&str>,
    internal_url: Option<&str>,
    kaniko_destination: &str,
) -> bool {
    public_url
        .or(internal_url)
        .map(|url| kaniko_destination.starts_with(url))
        .unwrap_or(false)
}

/// Create a Kubernetes Job and wait for any prior Job with the same name to
/// be cleaned up (retries up to 5 times on 409 AlreadyExists).
async fn create_job_with_cleanup(jobs_api: &Api<Job>, job: &Job) -> anyhow::Result<()> {
    let job_name = job.metadata.name.as_deref().unwrap_or("unknown");

    // Clean up any existing job with the same name. Use background
    // propagation and retry the create if the old object is still
    // finalizing (409 AlreadyExists).
    let dp = kube::api::DeleteParams {
        propagation_policy: Some(kube::api::PropagationPolicy::Background),
        ..Default::default()
    };
    let _ = jobs_api.delete(job_name, &dp).await;

    let mut attempts = 0;
    loop {
        match jobs_api.create(&PostParams::default(), job).await {
            Ok(_) => break,
            Err(kube::Error::Api(e)) if e.code == 409 && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Trigger a multi-architecture build.
///
/// Creates one Kaniko Job per architecture (amd64, arm64), each pinned to a
/// node of the matching architecture via node affinity. Each job pushes to
/// an arch-suffixed tag (e.g. `:abc1234-amd64`). When all arch jobs succeed,
/// `monitor_builds` creates a manifest-assembly Job that uses `crane` to
/// combine them into a single manifest list at the canonical tag (`:abc1234`).
///
/// Returns the "build group" job name prefix used to track all related jobs.
/// The individual per-arch jobs are named `{prefix}-{arch}` and the manifest
/// job is named `{prefix}-manifest`.
async fn trigger_build(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
    commit_sha: &str,
    token: &str,
    auth_user: &str,
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

    let kaniko_destination = resolve_kaniko_destination(state, oci_repo);

    let repo_no_scheme = repo_url
        .strip_prefix("https://")
        .or_else(|| repo_url.strip_prefix("http://"))
        .unwrap_or(repo_url);

    let kaniko_context = if token.is_empty() {
        format!("git://{repo_no_scheme}#refs/heads/{branch}")
    } else {
        format!("git://{auth_user}:{token}@{repo_no_scheme}#refs/heads/{branch}")
    };

    let internal_registry = is_internal_registry(state, &kaniko_destination);
    let registry_host = kaniko_destination.split('/').next().unwrap_or("");

    let jobs_api = state.jobs_api(ns)?;

    // Load settings to determine which architectures and build config to use.
    let settings = crate::handlers::settings::load_settings_from_db(state).await;
    let build_arches = resolve_build_arches(&settings);
    let bs = &settings.build_settings;
    let single_arch = build_arches.len() == 1;

    // Create one Kaniko Job per target architecture.
    for (platform, arch) in &build_arches {
        let arch_job_name = format!("{job_name}-{arch}");
        let arch_tag = if single_arch {
            short_sha.to_string()
        } else {
            format!("{short_sha}-{arch}")
        };

        let mut args = vec![
            format!("--dockerfile={dockerfile}"),
            format!("--context={kaniko_context}"),
            format!("--destination={kaniko_destination}:{arch_tag}"),
            format!("{}={platform}", bs.platform_flag),
            format!("--snapshot-mode={}", bs.snapshot_mode),
        ];

        if bs.cache_enabled {
            args.push("--cache=true".to_string());
        }

        if internal_registry && !registry_host.is_empty() {
            args.push(format!("--insecure-registry={registry_host}"));
        }

        if context != "." {
            args.push(format!("--context-sub-path={context}"));
        }

        for extra in &bs.extra_kaniko_args {
            args.push(extra.clone());
        }

        let mut labels = BTreeMap::new();
        labels.insert("deckwatch.io/build".to_string(), "true".to_string());
        labels.insert("deckwatch.io/deployment".to_string(), dep_name.clone());
        labels.insert("deckwatch.io/build-group".to_string(), job_name.clone());
        labels.insert("deckwatch.io/build-arch".to_string(), arch.to_string());

        let job = Job {
            metadata: ObjectMeta {
                name: Some(arch_job_name.clone()),
                namespace: Some(ns.to_string()),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(JobSpec {
                ttl_seconds_after_finished: Some(bs.job_ttl_seconds),
                backoff_limit: Some(bs.kaniko_backoff_limit),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(labels),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        restart_policy: Some("Never".to_string()),
                        affinity: Some(arch_node_affinity(arch)),
                        containers: vec![Container {
                            name: "kaniko".to_string(),
                            image: Some(bs.kaniko_image.clone()),
                            args: Some(args),
                            env: if token.is_empty() {
                                None
                            } else {
                                Some(vec![
                                    EnvVar {
                                        name: "GIT_USERNAME".to_string(),
                                        value: Some(auth_user.to_string()),
                                        ..Default::default()
                                    },
                                    EnvVar {
                                        name: "GIT_PASSWORD".to_string(),
                                        value: Some(token.to_string()),
                                        ..Default::default()
                                    },
                                ])
                            },
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        create_job_with_cleanup(&jobs_api, &job).await?;

        tracing::info!(
            deployment = %dep_name,
            arch = %arch,
            job = %arch_job_name,
            "created kaniko build job for architecture"
        );
    }

    Ok(job_name)
}

/// Create the manifest-assembly Job that combines per-architecture images
/// into a single OCI manifest list using `crane`. This runs after all
/// per-arch Kaniko jobs succeed.
///
/// The Job uses `gcr.io/go-containerregistry/crane:latest` which is the
/// official image for the `crane` CLI tool.
async fn create_manifest_job(
    state: &AppState,
    ns: &str,
    dep_name: &str,
    build_group: &str,
    short_sha: &str,
) -> anyhow::Result<String> {
    let app_id = format!("{ns}/{dep_name}");
    let config = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no gitops config found for {app_id}"))?;

    let oci_repo = &config.oci_repository;
    let kaniko_destination = resolve_kaniko_destination(state, oci_repo);
    let internal_registry = is_internal_registry(state, &kaniko_destination);

    let manifest_job_name = format!("{build_group}-manifest");

    // Build the list of arch-specific image refs to combine.
    let settings = crate::handlers::settings::load_settings_from_db(state).await;
    let build_arches = resolve_build_arches(&settings);
    let bs = &settings.build_settings;
    let arch_refs: Vec<String> = build_arches
        .iter()
        .map(|(_, arch)| format!("{kaniko_destination}:{short_sha}-{arch}"))
        .collect();

    let manifest_tag = format!("{kaniko_destination}:{short_sha}");

    let mut crane_args: Vec<String> = vec!["index".to_string(), "append".to_string()];

    crane_args.push("--flatten".to_string());

    for img_ref in &arch_refs {
        crane_args.push("--manifest".to_string());
        crane_args.push(img_ref.clone());
    }
    crane_args.push("--tag".to_string());
    crane_args.push(manifest_tag.clone());

    if internal_registry {
        crane_args.push("--insecure".to_string());
    }

    tracing::debug!(
        deployment = %dep_name,
        crane_args = ?crane_args,
        "crane manifest assembly command"
    );

    let mut labels = BTreeMap::new();
    labels.insert("deckwatch.io/build".to_string(), "true".to_string());
    labels.insert("deckwatch.io/deployment".to_string(), dep_name.to_string());
    labels.insert(
        "deckwatch.io/build-group".to_string(),
        build_group.to_string(),
    );
    labels.insert(
        "deckwatch.io/build-phase".to_string(),
        "manifest".to_string(),
    );

    let job = Job {
        metadata: ObjectMeta {
            name: Some(manifest_job_name.clone()),
            namespace: Some(ns.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(JobSpec {
            ttl_seconds_after_finished: Some(bs.job_ttl_seconds),
            backoff_limit: Some(bs.crane_backoff_limit),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    containers: vec![Container {
                        name: "crane".to_string(),
                        image: Some(bs.crane_image.clone()),
                        args: Some(crane_args),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let jobs_api = state.jobs_api(ns)?;
    create_job_with_cleanup(&jobs_api, &job).await?;

    tracing::info!(
        deployment = %dep_name,
        job = %manifest_job_name,
        "created manifest assembly job"
    );

    Ok(manifest_job_name)
}

/// Status of a multi-arch build group's constituent jobs.
#[derive(Debug)]
enum BuildGroupStatus {
    /// At least one arch-build is still running.
    InProgress,
    /// All arch-builds succeeded; manifest job has not been created yet.
    ArchesComplete,
    /// The manifest-assembly job is still running.
    ManifestInProgress,
    /// The manifest-assembly job succeeded — the build is done.
    ManifestComplete,
    /// At least one job (arch-build or manifest) has failed.
    Failed(String),
}

/// Examine all Jobs in `ns` that belong to the given `build_group` label
/// and determine the aggregate status.
async fn check_build_group_status(
    jobs_api: &Api<Job>,
    build_group: &str,
) -> anyhow::Result<BuildGroupStatus> {
    let lp = ListParams::default().labels(&format!("deckwatch.io/build-group={build_group}"));
    let jobs = jobs_api.list(&lp).await?;

    let mut arch_total = 0;
    let mut arch_succeeded = 0;
    let mut arch_failed = 0;
    let mut manifest_exists = false;
    let mut manifest_succeeded = false;
    let mut manifest_failed = false;

    for job in jobs.iter() {
        let labels = job.metadata.labels.as_ref();
        let is_manifest = labels
            .and_then(|l| l.get("deckwatch.io/build-phase"))
            .map(|v| v == "manifest")
            .unwrap_or(false);

        let status = job.status.as_ref();
        let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
        let failed = status.and_then(|s| s.failed).unwrap_or(0);

        if is_manifest {
            manifest_exists = true;
            if succeeded > 0 {
                manifest_succeeded = true;
            } else if failed > 0 {
                manifest_failed = true;
            }
        } else {
            arch_total += 1;
            if succeeded > 0 {
                arch_succeeded += 1;
            } else if failed > 0 {
                arch_failed += 1;
            }
        }
    }

    // Any failure is terminal.
    if arch_failed > 0 {
        return Ok(BuildGroupStatus::Failed(
            "one or more architecture build jobs failed".to_string(),
        ));
    }
    if manifest_failed {
        return Ok(BuildGroupStatus::Failed(
            "manifest assembly job failed".to_string(),
        ));
    }

    // Manifest completed — build is done.
    if manifest_succeeded {
        return Ok(BuildGroupStatus::ManifestComplete);
    }

    // Manifest exists but hasn't finished yet.
    if manifest_exists {
        return Ok(BuildGroupStatus::ManifestInProgress);
    }

    // All architectures done — time to create manifest job.
    if arch_total > 0 && arch_succeeded == arch_total {
        return Ok(BuildGroupStatus::ArchesComplete);
    }

    // Still waiting for arch builds.
    Ok(BuildGroupStatus::InProgress)
}

/// Collect build logs from all arch-build and manifest jobs in a group.
/// Returns a combined log string with per-job headers.
async fn capture_build_group_logs(state: &AppState, ns: &str, build_group: &str) -> Option<String> {
    let jobs_api: Api<Job> = Api::namespaced(state.kube_client.clone(), ns);
    let lp = ListParams::default().labels(&format!("deckwatch.io/build-group={build_group}"));
    let jobs = match jobs_api.list(&lp).await {
        Ok(j) => j,
        Err(_) => return None,
    };

    let mut combined = String::new();
    for job in jobs.iter() {
        let jn = job.metadata.name.as_deref().unwrap_or("unknown");
        if let Some(log) = capture_build_log(state, ns, jn).await {
            if !combined.is_empty() {
                combined.push_str("\n\n");
            }
            combined.push_str(&format!("=== Job: {jn} ===\n"));
            combined.push_str(&log);
        }
    }

    if combined.is_empty() {
        None
    } else {
        // Truncate to MAX_BUILD_LOG_BYTES to keep DB rows reasonable.
        if combined.len() > MAX_BUILD_LOG_BYTES {
            let tail = &combined[combined.len() - MAX_BUILD_LOG_BYTES..];
            let start = tail.find('\n').map(|i| i + 1).unwrap_or(0);
            Some(format!(
                "[truncated -- showing last ~{}KB]\n{}",
                MAX_BUILD_LOG_BYTES / 1024,
                &tail[start..]
            ))
        } else {
            Some(combined)
        }
    }
}

/// Clean up all Jobs belonging to a build group.
async fn cleanup_build_group_jobs(jobs_api: &Api<Job>, build_group: &str) {
    let lp = ListParams::default().labels(&format!("deckwatch.io/build-group={build_group}"));
    let dp = kube::api::DeleteParams {
        propagation_policy: Some(kube::api::PropagationPolicy::Background),
        ..Default::default()
    };
    if let Ok(jobs) = jobs_api.list(&lp).await {
        for job in jobs.iter() {
            if let Some(name) = job.metadata.name.as_deref() {
                let _ = jobs_api.delete(name, &dp).await;
            }
        }
    }
}

/// Verify that a newly built image is actually pullable from the registry
/// before patching the deployment. This prevents `ImagePullBackOff` errors
/// when the registry hasn't fully processed the manifest yet.
///
/// The check is only performed for images hosted on the deckwatch-managed
/// registry (internal or public URL). External registries (GHCR, ECR, etc.)
/// are trusted to be available immediately after a successful push.
///
/// Returns `true` if the image is confirmed available or if the check is not
/// applicable (external registry). Returns `false` if the registry returned
/// a non-200 status, indicating the image is not yet ready.
async fn verify_image_available(http: &reqwest::Client, state: &AppState, image_ref: &str) -> bool {
    // Parse the image reference into registry host and name:tag.
    // Expected format: "registry-host/repo/path:tag"
    let (registry_host, repo_and_tag) = match image_ref.split_once('/') {
        Some((host, rest)) if host.contains('.') || host.contains(':') => (host, rest),
        _ => {
            // No recognizable registry host (e.g. library images) -- skip check.
            return true;
        }
    };

    // Only check images on the deckwatch-managed registry.
    let is_managed = [
        state.registry_internal_url.as_deref(),
        state.registry_public_url.as_deref(),
    ]
    .iter()
    .flatten()
    .any(|url| {
        let normalized = url
            .trim_end_matches('/')
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);
        normalized.trim_end_matches('/') == registry_host
    });

    if !is_managed {
        // External registry -- trust the push succeeded.
        return true;
    }

    // Split repo_and_tag into name and tag.
    let (name, tag) = match repo_and_tag.rsplit_once(':') {
        Some((n, t)) => (n, t),
        None => {
            // No tag -- unusual, skip check.
            return true;
        }
    };

    // Prefer the internal URL for the check (avoids hairpin NAT), fall back
    // to the public URL.
    let base_url = state
        .registry_internal_url
        .as_deref()
        .or(state.registry_public_url.as_deref());

    let base_url = match base_url {
        Some(u) => u.trim_end_matches('/'),
        None => return true,
    };

    // Ensure the base URL has a scheme so reqwest can parse it.
    let base_url = if base_url.starts_with("http://") || base_url.starts_with("https://") {
        base_url.to_string()
    } else {
        format!("http://{base_url}")
    };

    let url = format!("{base_url}/v2/{name}/manifests/{tag}");

    match http.head(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(image = %image_ref, "image verified available in registry");
            true
        }
        Ok(resp) => {
            tracing::warn!(
                image = %image_ref,
                status = %resp.status(),
                "image not yet available in registry, deferring deployment update"
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                image = %image_ref,
                error = %e,
                "failed to verify image availability, deferring deployment update"
            );
            false
        }
    }
}

async fn monitor_builds(state: &AppState, http: &reqwest::Client) -> anyhow::Result<()> {
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

        let build_group = match config.last_build_job.as_deref() {
            Some(j) if !j.is_empty() => j,
            _ => continue,
        };

        let jobs_api: Api<Job> = Api::namespaced(state.kube_client.clone(), ns);

        let group_status = match check_build_group_status(&jobs_api, build_group).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    build_group = %build_group,
                    error = %e,
                    "failed to check build group status"
                );
                continue;
            }
        };

        let now = now_utc();
        let commit_sha = config.last_commit_sha.as_deref().unwrap_or("");
        let short_sha = &commit_sha[..7.min(commit_sha.len())];

        match group_status {
            BuildGroupStatus::InProgress | BuildGroupStatus::ManifestInProgress => {
                // Still waiting -- nothing to do.
                continue;
            }
            BuildGroupStatus::ArchesComplete => {
                let settings = crate::handlers::settings::load_settings_from_db(state).await;
                let build_arches = resolve_build_arches(&settings);

                if build_arches.len() == 1 {
                    // Single architecture -- no manifest assembly needed.
                    // The image was pushed directly to the canonical tag.
                    let oci_repo = &config.oci_repository;
                    let new_image = format!("{oci_repo}:{short_sha}");

                    // Verify the image is pullable before patching the deployment
                    // to avoid ImagePullBackOff errors.
                    if !verify_image_available(http, state, &new_image).await {
                        tracing::info!(
                            deployment = %dep_name,
                            image = %new_image,
                            "deferring deployment update until image is available"
                        );
                        continue;
                    }

                    tracing::info!(
                        deployment = %dep_name,
                        image = %new_image,
                        "single-arch build succeeded, updating deployment image"
                    );

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

                    // Ensure the deckwatch application label is present on the
                    // deployment metadata (it can get stripped by other controllers).
                    let label_patch = serde_json::json!({
                        "metadata": {
                            "labels": {
                                "deckwatch.io/application": dep_name
                            }
                        }
                    });
                    let _ = dep_api
                        .patch(
                            dep_name,
                            &PatchParams::default(),
                            &Patch::Strategic(label_patch),
                        )
                        .await;

                    update_gitops_config_field(&state.db, &config.application_id, |active| {
                        active.last_build_status = Set(Some("success".to_string()));
                        active.last_build_time = Set(Some(now));
                        active.last_build_error = Set(None);
                        active.updated_at = Set(now);
                    })
                    .await?;
                    metrics::record_gitops_build(ns, "success");

                    let build_log = capture_build_group_logs(state, ns, build_group).await;
                    update_build_status(&state.db, build_group, "succeeded", None, build_log).await;

                    cleanup_build_group_jobs(&jobs_api, build_group).await;
                } else {
                    // Multi-arch: create the manifest assembly job.
                    tracing::info!(
                        deployment = %dep_name,
                        build_group = %build_group,
                        "all arch builds complete, creating manifest assembly job"
                    );
                    if let Err(e) =
                        create_manifest_job(state, ns, dep_name, build_group, short_sha).await
                    {
                        tracing::error!(
                            deployment = %dep_name,
                            error = %e,
                            "failed to create manifest assembly job"
                        );
                        update_gitops_config_field(&state.db, &config.application_id, |active| {
                            active.last_build_status = Set(Some("failed".to_string()));
                            active.last_build_error =
                                Set(Some(format!("failed to create manifest assembly job: {e}")));
                            active.last_build_time = Set(Some(now));
                            active.updated_at = Set(now);
                        })
                        .await?;
                        metrics::record_gitops_build(ns, "failure");

                        let build_log = capture_build_group_logs(state, ns, build_group).await;
                        update_build_status(
                            &state.db,
                            build_group,
                            "failed",
                            Some("failed to create manifest assembly job"),
                            build_log,
                        )
                        .await;

                        cleanup_build_group_jobs(&jobs_api, build_group).await;
                    }
                }
            }
            BuildGroupStatus::ManifestComplete => {
                // The full multi-arch build is done. Update the deployment.
                let oci_repo = &config.oci_repository;
                let new_image = format!("{oci_repo}:{short_sha}");

                // Verify the image is pullable before patching the deployment
                // to avoid ImagePullBackOff errors.
                if !verify_image_available(http, state, &new_image).await {
                    tracing::info!(
                        deployment = %dep_name,
                        image = %new_image,
                        "deferring deployment update until image is available"
                    );
                    continue;
                }

                tracing::info!(
                    deployment = %dep_name,
                    image = %new_image,
                    "multi-arch build succeeded, updating deployment image"
                );

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

                // Ensure the deckwatch application label is present on the
                // deployment metadata (it can get stripped by other controllers).
                let label_patch = serde_json::json!({
                    "metadata": {
                        "labels": {
                            "deckwatch.io/application": dep_name
                        }
                    }
                });
                let _ = dep_api
                    .patch(
                        dep_name,
                        &PatchParams::default(),
                        &Patch::Strategic(label_patch),
                    )
                    .await;

                update_gitops_config_field(&state.db, &config.application_id, |active| {
                    active.last_build_status = Set(Some("success".to_string()));
                    active.last_build_time = Set(Some(now));
                    active.last_build_error = Set(None);
                    active.updated_at = Set(now);
                })
                .await?;
                metrics::record_gitops_build(ns, "success");

                let build_log = capture_build_group_logs(state, ns, build_group).await;
                update_build_status(&state.db, build_group, "succeeded", None, build_log).await;

                cleanup_build_group_jobs(&jobs_api, build_group).await;
            }
            BuildGroupStatus::Failed(reason) => {
                tracing::warn!(
                    deployment = %dep_name,
                    reason = %reason,
                    "multi-arch build failed"
                );

                update_gitops_config_field(&state.db, &config.application_id, |active| {
                    active.last_build_status = Set(Some("failed".to_string()));
                    active.last_build_error = Set(Some(reason.clone()));
                    active.last_build_time = Set(Some(now));
                    active.updated_at = Set(now);
                })
                .await?;
                metrics::record_gitops_build(ns, "failure");

                let build_log = capture_build_group_logs(state, ns, build_group).await;
                update_build_status(&state.db, build_group, "failed", Some(&reason), build_log)
                    .await;

                cleanup_build_group_jobs(&jobs_api, build_group).await;
            }
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

/// Maximum number of bytes to store for a build log. Logs larger than this
/// are truncated to the last `MAX_BUILD_LOG_BYTES` bytes so the most recent
/// (and usually most relevant) output is preserved.
const MAX_BUILD_LOG_BYTES: usize = 64 * 1024;

/// Capture build pod logs for the given job. Returns `None` (with a warning)
/// if the pod or its logs cannot be retrieved — log capture must never block
/// the cleanup path.
async fn capture_build_log(state: &AppState, ns: &str, job_name: &str) -> Option<String> {
    let pods_api = match state.pods_api(ns) {
        Ok(api) => api,
        Err(e) => {
            tracing::warn!(error = %e, job_name, "failed to get pods API for build log capture");
            return None;
        }
    };

    let lp = ListParams::default().labels(&format!("job-name={job_name}"));
    let pod_list = match pods_api.list(&lp).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, job_name, "failed to list pods for build log capture");
            return None;
        }
    };

    let pod_name = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())?;

    let log_params = LogParams {
        timestamps: true,
        ..Default::default()
    };

    match pods_api.logs(pod_name, &log_params).await {
        Ok(logs) => {
            if logs.len() > MAX_BUILD_LOG_BYTES {
                // Keep the tail — the end of the log is usually the most
                // informative (error messages, final build steps).
                let truncated = &logs[logs.len() - MAX_BUILD_LOG_BYTES..];
                // Find the first newline so we don't start mid-line.
                let start = truncated.find('\n').map(|i| i + 1).unwrap_or(0);
                Some(format!(
                    "[truncated — showing last ~{}KB]\n{}",
                    MAX_BUILD_LOG_BYTES / 1024,
                    &truncated[start..]
                ))
            } else {
                Some(logs)
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, job_name, pod_name, "failed to fetch build pod logs");
            None
        }
    }
}

/// Update a build row in the database by job name. Fire-and-forget — logs a
/// warning on failure so the watcher loop is not interrupted.
async fn update_build_status(
    db: &sea_orm::DatabaseConnection,
    job_name: &str,
    status: &str,
    error_message: Option<&str>,
    build_log: Option<String>,
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
    active.build_log = Set(build_log);

    if let Err(e) = active.update(db).await {
        tracing::warn!(error = %e, job_name, "failed to update build row in database");
    }
}

/// Hash a JSON patch value to a `u64` fingerprint using the standard library's
/// `DefaultHasher`. No external dependencies required. The hash is computed from
/// the compact JSON string so it captures every field and value.
fn fingerprint_patch(patch: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let s = patch.to_string();
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Build a JSON container object for a sidecar spec. Used when assembling the
/// SSA patch for deployments and cronjobs.
fn sidecar_to_json(s: &SidecarSpec) -> serde_json::Value {
    let mut c = serde_json::json!({
        "name": s.name,
        "image": s.image,
    });
    if let Some(port) = s.port {
        c["ports"] = serde_json::json!([{"containerPort": port}]);
    }
    if !s.env.is_empty() {
        c["env"] = serde_json::json!(s
            .env
            .iter()
            .map(|e| serde_json::json!({
                "name": e.name,
                "value": e.value,
            }))
            .collect::<Vec<_>>());
    }
    if s.cpu.is_some() || s.memory.is_some() {
        let mut req = serde_json::Map::new();
        let mut lim = serde_json::Map::new();
        if let Some(ref cpu) = s.cpu {
            req.insert("cpu".to_string(), serde_json::json!(cpu));
            lim.insert("cpu".to_string(), serde_json::json!(cpu));
        }
        if let Some(ref mem) = s.memory {
            req.insert("memory".to_string(), serde_json::json!(mem));
            lim.insert("memory".to_string(), serde_json::json!(mem));
        }
        c["resources"] = serde_json::json!({
            "requests": req,
            "limits": lim,
        });
    }
    c
}

async fn reconcile_application_plugins(state: &AppState) {
    // Snapshot the fingerprint cache once for this reconcile cycle.
    // Each key is `"namespace/name"` and the value is the hash of the last
    // patch JSON we sent to the API server for that workload.
    let fingerprints_arc = state.plugin_patch_fingerprints.clone();

    let rows = match application_plugin_resources::Entity::find()
        .all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "reconcile_application_plugins: failed to query db");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let mut app_state: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut app_annotations: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut app_sidecars: HashMap<String, Vec<SidecarSpec>> = HashMap::new();
    for row in rows {
        let state_map: HashMap<String, String> =
            serde_json::from_str(&row.state).unwrap_or_default();
        let ann_map: HashMap<String, String> =
            serde_json::from_str(&row.annotations).unwrap_or_default();
        let sidecars_raw: Vec<SidecarSpec> =
            serde_json::from_str(&row.sidecars).unwrap_or_default();
        let state_entry = app_state.entry(row.application_id.clone()).or_default();
        state_entry.extend(state_map);
        if !ann_map.is_empty() {
            let ann_entry = app_annotations
                .entry(row.application_id.clone())
                .or_default();
            ann_entry.extend(ann_map);
        }
        if !sidecars_raw.is_empty() {
            let sidecar_entry = app_sidecars.entry(row.application_id).or_default();
            sidecar_entry.extend(sidecars_raw);
        }
    }

    for (app_id, env_map) in &app_state {
        let sidecars = app_sidecars.get(app_id).cloned().unwrap_or_default();
        if env_map.is_empty() && sidecars.is_empty() {
            continue;
        }

        let (ns, app_name) = match app_id.split_once('/') {
            Some(pair) => pair,
            None => continue,
        };

        if !state.is_namespace_allowed(ns) {
            continue;
        }

        let mut env_vars: Vec<EnvVar> = env_map
            .iter()
            .map(|(k, v)| EnvVar {
                name: k.clone(),
                value: Some(v.clone()),
                ..Default::default()
            })
            .collect();
        env_vars.sort_by(|a, b| a.name.cmp(&b.name));

        let label_selector = format!("deckwatch.io/application={app_name}");
        let pp = PatchParams::apply("deckwatch-plugin-resources").force();

        let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
        let lp = ListParams::default().labels(&label_selector);
        match dep_api.list(&lp).await {
            Ok(deps) => {
                for dep in deps.items {
                    let dep_name = match dep.metadata.name.as_deref() {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    let ann_map = app_annotations.get(app_id).cloned().unwrap_or_default();

                    let primary_name = dep
                        .spec
                        .as_ref()
                        .and_then(|s| s.template.spec.as_ref())
                        .and_then(|ps| ps.containers.first())
                        .map(|c| c.name.clone())
                        .unwrap_or_default();

                    let mut containers_patch = vec![serde_json::json!({
                        "name": primary_name,
                        "env": env_vars.iter().map(|e| serde_json::json!({
                            "name": e.name,
                            "value": e.value.as_deref().unwrap_or(""),
                        })).collect::<Vec<_>>(),
                    })];
                    containers_patch.extend(sidecars.iter().map(sidecar_to_json));

                    let patch = serde_json::json!({
                        "apiVersion": "apps/v1",
                        "kind": "Deployment",
                        "metadata": {
                            "name": dep_name,
                            "namespace": ns,
                            "annotations": ann_map,
                        },
                        "spec": {
                            "template": {
                                "spec": {
                                    "containers": containers_patch,
                                }
                            }
                        }
                    });

                    let fp_key = format!("{ns}/{dep_name}");
                    let new_fp = fingerprint_patch(&patch);
                    let skip = {
                        let fps = fingerprints_arc.lock().await;
                        fps.get(&fp_key).copied() == Some(new_fp)
                    };
                    if skip {
                        tracing::debug!(
                            namespace = %ns,
                            deployment = %dep_name,
                            "reconcile_application_plugins: patch unchanged, skipping"
                        );
                    } else if let Err(e) =
                        dep_api.patch(&dep_name, &pp, &Patch::Apply(&patch)).await
                    {
                        tracing::warn!(
                            namespace = %ns,
                            deployment = %dep_name,
                            error = %e,
                            "reconcile_application_plugins: failed to patch deployment"
                        );
                    } else {
                        fingerprints_arc.lock().await.insert(fp_key, new_fp);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(namespace = %ns, error = %e, "reconcile_application_plugins: failed to list deployments");
            }
        }

        let cj_api = match state.cronjobs_api(ns) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(namespace = %ns, error = %e, "reconcile_application_plugins: failed to get cronjobs api");
                continue;
            }
        };

        match cj_api.list(&lp).await {
            Ok(cjs) => {
                use k8s_openapi::api::batch::v1::CronJob;
                for cj in cjs.items {
                    let cj_name = match cj.metadata.name.as_deref() {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    let container_name = cj
                        .spec
                        .as_ref()
                        .and_then(|s| s.job_template.spec.as_ref())
                        .and_then(|js| js.template.spec.as_ref())
                        .and_then(|ps| ps.containers.first())
                        .map(|c| c.name.clone())
                        .unwrap_or_default();

                    let mut cj_containers_patch = vec![serde_json::json!({
                        "name": container_name,
                        "env": env_vars.iter().map(|e| serde_json::json!({
                            "name": e.name,
                            "value": e.value.as_deref().unwrap_or(""),
                        })).collect::<Vec<_>>(),
                    })];
                    cj_containers_patch.extend(sidecars.iter().map(sidecar_to_json));

                    let patch = serde_json::json!({
                        "apiVersion": "batch/v1",
                        "kind": "CronJob",
                        "metadata": {
                            "name": cj_name,
                            "namespace": ns,
                        },
                        "spec": {
                            "jobTemplate": {
                                "spec": {
                                    "template": {
                                        "spec": {
                                            "containers": cj_containers_patch,
                                        }
                                    }
                                }
                            }
                        }
                    });

                    let cj_typed_api: Api<CronJob> = Api::namespaced(state.kube_client.clone(), ns);
                    let cj_fp_key = format!("cj:{ns}/{cj_name}");
                    let cj_new_fp = fingerprint_patch(&patch);
                    let cj_skip = {
                        let fps = fingerprints_arc.lock().await;
                        fps.get(&cj_fp_key).copied() == Some(cj_new_fp)
                    };
                    if cj_skip {
                        tracing::debug!(
                            namespace = %ns,
                            cronjob = %cj_name,
                            "reconcile_application_plugins: patch unchanged, skipping"
                        );
                    } else if let Err(e) = cj_typed_api
                        .patch(&cj_name, &pp, &Patch::Apply(&patch))
                        .await
                    {
                        tracing::warn!(
                            namespace = %ns,
                            cronjob = %cj_name,
                            error = %e,
                            "reconcile_application_plugins: failed to patch cronjob"
                        );
                    } else {
                        fingerprints_arc.lock().await.insert(cj_fp_key, cj_new_fp);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(namespace = %ns, error = %e, "reconcile_application_plugins: failed to list cronjobs");
            }
        }
    }
}

/// Pending status values for provisioned resources.
///
/// A resource is considered pending if any `*_STATUS` key in its state JSON
/// holds one of these values, or if it looks like a database resource
/// (contains `DB_ENGINE` or `DB_STATUS`) but has no `DB_HOST` yet.
const PENDING_STATUS_VALUES: &[&str] = &["provisioning", "creating", "modifying", "backing-up"];

/// Return `true` if the serialised state JSON indicates the resource is still
/// being provisioned (i.e. the watcher should keep polling).
fn is_pending(state_json: &str) -> bool {
    let state: HashMap<String, String> = match serde_json::from_str(state_json) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Any *_STATUS key whose value is a known in-progress string.
    for (key, value) in &state {
        if key.ends_with("_STATUS") && PENDING_STATUS_VALUES.contains(&value.as_str()) {
            return true;
        }
    }

    // Database resource with an empty or missing endpoint.
    if state.contains_key("DB_ENGINE") || state.contains_key("DB_STATUS") {
        let db_host = state.get("DB_HOST").map(String::as_str).unwrap_or("");
        if db_host.is_empty() {
            return true;
        }
    }

    false
}

/// Background reconciler for provisioned plugin resources that are still pending.
///
/// Iterates every row in `application_plugin_resources` whose state looks like
/// an in-progress provisioning (see [`is_pending`]). For each such row it
/// re-calls the plugin's `provision()` export; if the returned state has grown
/// or is no longer pending the DB row is updated so that the env-var reconciler
/// and the UI reflect the real infrastructure state.
///
/// Errors from individual `provision()` calls are logged at WARN and never
/// abort the loop — the row simply stays pending and is retried next cycle.
async fn reconcile_pending_resources(state: &AppState) {
    let rows = match application_plugin_resources::Entity::find()
        .all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "reconcile_pending_resources: failed to query db");
            return;
        }
    };

    let pending: Vec<_> = rows.into_iter().filter(|r| is_pending(&r.state)).collect();
    if pending.is_empty() {
        return;
    }

    // Snapshot the plugin list once so we hold the lock for the minimum time.
    let plugins = {
        let guard = state.plugins.read().await;
        guard.clone()
    };

    for row in pending {
        let plugin_name = row.plugin_name.clone();
        let resource_id = row.resource_id.clone();
        let current_state_json = row.state.clone();

        // Parse application_id as "{ns}/{name}".
        // Clone into owned strings so there is no live borrow on `row` when
        // we later call `row.into()` to construct the ActiveModel.
        let (ns, app_name) = match row.application_id.split_once('/') {
            Some((n, a)) => (n.to_string(), a.to_string()),
            None => continue,
        };

        // Skip if the plugin is not currently loaded.
        let plugin = match plugins.iter().find(|p| p.name == plugin_name) {
            Some(p) => p,
            None => {
                tracing::debug!(
                    plugin = %plugin_name,
                    namespace = %ns,
                    application = %app_name,
                    "reconcile_pending_resources: plugin not loaded, skipping"
                );
                continue;
            }
        };

        let fields: HashMap<String, String> = serde_json::from_str(&row.fields).unwrap_or_default();

        let req = crate::plugins::ResourceProvisionRequest {
            application_name: app_name.to_string(),
            namespace: ns.to_string(),
            resource_id: resource_id.clone(),
            fields,
        };

        let result = match crate::plugins::run_provision(plugin, &req) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    namespace = %ns,
                    application = %app_name,
                    plugin = %plugin_name,
                    resource_id = %resource_id,
                    error = %e,
                    "reconcile_pending_resources: provision() failed, will retry next cycle"
                );
                continue;
            }
        };

        let current_state: HashMap<String, String> =
            serde_json::from_str(&current_state_json).unwrap_or_default();
        let keys_before = current_state.len();
        let keys_after = result.state.len();

        let new_state_json =
            serde_json::to_string(&result.state).unwrap_or_else(|_| "{}".to_string());

        // Only write to the DB if the state grew or is no longer pending.
        let state_changed = keys_after > keys_before || !is_pending(&new_state_json);
        if !state_changed {
            continue;
        }

        let db_host = result.state.get("DB_HOST").cloned().unwrap_or_default();
        let new_annotations_json = serde_json::to_string(&result.deployment_annotations)
            .unwrap_or_else(|_| "{}".to_string());
        let new_sidecars_json =
            serde_json::to_string(&result.sidecars).unwrap_or_else(|_| "[]".to_string());
        let now = now_utc();

        let mut active: application_plugin_resources::ActiveModel = row.into();
        active.state = Set(new_state_json);
        active.annotations = Set(new_annotations_json);
        active.sidecars = Set(new_sidecars_json);
        active.updated_at = Set(now);

        if let Err(e) = active.update(&state.db).await {
            tracing::warn!(
                namespace = %ns,
                application = %app_name,
                plugin = %plugin_name,
                resource_id = %resource_id,
                error = %e,
                "reconcile_pending_resources: failed to update row"
            );
            continue;
        }

        tracing::info!(
            namespace = %ns,
            application = %app_name,
            plugin = %plugin_name,
            resource_id = %resource_id,
            state_keys_before = keys_before,
            state_keys_after = keys_after,
            db_host = %db_host,
            "resource status updated"
        );
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
