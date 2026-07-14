use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, EnvVarSource, PodSpec, PodTemplateSpec, SecretKeySelector,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};

use crate::metrics;
use crate::state::AppState;

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
pub fn get_oci_repository<'a>(dep: &'a Deployment) -> Option<&'a str> {
    get_ann(dep, "oci-repository").or_else(|| get_ann(dep, "ecr-repository"))
}

pub async fn run_poller(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    let http_client = reqwest::Client::new();

    loop {
        interval.tick().await;

        if let Err(e) = poll_cycle(&state, &http_client).await {
            tracing::error!(error = %e, "watcher poll cycle failed");
        }

        if let Err(e) = monitor_builds(&state).await {
            tracing::error!(error = %e, "watcher build monitor failed");
        }
    }
}

async fn poll_cycle(state: &AppState, http: &reqwest::Client) -> anyhow::Result<()> {
    let namespaces = if state.allowed_namespaces.is_empty() {
        let ns_api = state.namespaces_api();
        let ns_list = ns_api.list(&ListParams::default()).await?;
        ns_list.iter().map(|ns| ns.name_any()).collect::<Vec<_>>()
    } else {
        state.allowed_namespaces.clone()
    };

    for ns in &namespaces {
        let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
        let deps = dep_api.list(&ListParams::default()).await?;

        for dep in deps.items.iter() {
            if get_ann(dep, "git-enabled") != Some("true") {
                continue;
            }

            if get_ann(dep, "last-build-status") == Some("building") {
                continue;
            }

            let name = dep.name_any();
            if let Err(e) = check_and_build(state, http, ns, dep).await {
                tracing::warn!(deployment = %name, namespace = %ns, error = %e, "git check failed");
            }
        }
    }

    Ok(())
}

async fn check_and_build(
    state: &AppState,
    http: &reqwest::Client,
    ns: &str,
    dep: &Deployment,
) -> anyhow::Result<()> {
    let repo_url = get_ann(dep, "git-repo")
        .ok_or_else(|| anyhow::anyhow!("missing git-repo annotation"))?;
    let branch = get_ann(dep, "git-branch").unwrap_or("main");
    let token = match get_ann(dep, "git-token-secret") {
        Some(token_secret) if !token_secret.is_empty() => {
            let secrets_api = state.secrets_api(ns)?;
            match secrets_api.get(token_secret).await {
                Ok(secret) => secret.data.as_ref()
                    .and_then(|d| d.get("token"))
                    .map(|v| String::from_utf8_lossy(&v.0).to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            }
        }
        _ => String::new(),
    };

    let remote_sha: String = check_remote_head(http, repo_url, branch, &token).await?;
    let last_sha = get_ann(dep, "last-commit-sha").unwrap_or("");

    if remote_sha == last_sha {
        return Ok(());
    }

    let short_sha = &remote_sha[..7.min(remote_sha.len())];
    let dep_name = dep.name_any();

    let include_paths: Vec<&str> = get_ann(dep, "include-paths")
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').collect())
        .unwrap_or_default();
    let exclude_paths: Vec<&str> = get_ann(dep, "exclude-paths")
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').collect())
        .unwrap_or_default();

    if !include_paths.is_empty() || !exclude_paths.is_empty() {
        if !last_sha.is_empty() {
            if let Some(changed) =
                check_paths_github(http, repo_url, &token, last_sha, &remote_sha).await
            {
                let dominated_by_excludes = !changed.iter().any(|f| {
                    let included = include_paths.is_empty()
                        || include_paths.iter().any(|p| f.starts_with(p));
                    let excluded = exclude_paths.iter().any(|p| f.starts_with(p));
                    included && !excluded
                });
                if dominated_by_excludes {
                    tracing::info!(
                        deployment = %dep_name,
                        commit = %short_sha,
                        "skipping build: no included paths changed"
                    );
                    update_annotation(state, ns, &dep_name, "last-commit-sha", &remote_sha)
                        .await?;
                    return Ok(());
                }
            }
        }
    }

    tracing::info!(
        deployment = %dep_name,
        namespace = %ns,
        commit = %short_sha,
        "new commit detected, triggering build"
    );

    let job_name: String = trigger_build(state, ns, dep, &remote_sha, &token).await?;
    // Counter incremented once per build kickoff; success/failure is recorded
    // later in monitor_builds when the Job completes.
    metrics::record_gitops_build(ns, "started");

    let dep_api = state.deployments_api(ns)?;
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
    dep_api
        .patch(&dep_name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;

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

    let resp = request
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

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

    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/compare/{base}...{head}"
    );

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

    let repo_url = get_ann(dep, "git-repo").unwrap_or("");
    let branch = get_ann(dep, "git-branch").unwrap_or("main");
    let dockerfile = get_ann(dep, "dockerfile-path").unwrap_or("Dockerfile");
    let context = get_ann(dep, "docker-context").unwrap_or(".");
    let oci_repo = get_oci_repository(dep).unwrap_or("");
    let token_secret = get_ann(dep, "git-token-secret").unwrap_or("");

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
                        image: Some(
                            "gcr.io/kaniko-project/executor:latest".to_string(),
                        ),
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
    let namespaces = if state.allowed_namespaces.is_empty() {
        let ns_api = state.namespaces_api();
        let ns_list = ns_api.list(&ListParams::default()).await?;
        ns_list.iter().map(|ns| ns.name_any()).collect::<Vec<_>>()
    } else {
        state.allowed_namespaces.clone()
    };

    for ns in &namespaces {
        let jobs_api: Api<Job> = Api::namespaced(state.kube_client.clone(), ns);
        let jobs = jobs_api
            .list(&ListParams::default().labels("deckwatch.io/build=true"))
            .await?;

        for job in jobs.items.iter() {
            let dep_name = job
                .metadata
                .labels
                .as_ref()
                .and_then(|l| l.get("deckwatch.io/deployment"))
                .cloned()
                .unwrap_or_default();

            if dep_name.is_empty() {
                continue;
            }

            let status = job.status.as_ref();
            let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
            let failed = status.and_then(|s| s.failed).unwrap_or(0);

            if succeeded == 0 && failed == 0 {
                continue;
            }

            let dep_api = state.deployments_api(ns)?;
            let dep = match dep_api.get(&dep_name).await {
                Ok(d) => d,
                Err(_) => continue,
            };

            let current_status = get_ann(&dep, "last-build-status").unwrap_or("");
            if current_status != "building" {
                continue;
            }

            if succeeded > 0 {
                let oci_repo = get_oci_repository(&dep).unwrap_or("");
                let commit_sha = get_ann(&dep, "last-commit-sha").unwrap_or("");
                let short_sha = &commit_sha[..7.min(commit_sha.len())];
                let new_image = format!("{oci_repo}:{short_sha}");

                tracing::info!(
                    deployment = %dep_name,
                    image = %new_image,
                    "build succeeded, updating deployment image"
                );

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
                        &dep_name,
                        &PatchParams::default(),
                        &Patch::Strategic(image_patch),
                    )
                    .await;

                update_annotation(state, ns, &dep_name, "last-build-status", "success").await?;
                metrics::record_gitops_build(ns, "success");
            } else {
                tracing::warn!(deployment = %dep_name, "build failed");
                update_annotation(state, ns, &dep_name, "last-build-status", "failed").await?;
                update_annotation(
                    state,
                    ns,
                    &dep_name,
                    "last-build-error",
                    "Kaniko build job failed",
                )
                .await?;
                metrics::record_gitops_build(ns, "failure");
            }
        }
    }

    Ok(())
}

async fn update_annotation(
    state: &AppState,
    ns: &str,
    dep_name: &str,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    let dep_api = state.deployments_api(ns)?;
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                ann(key): value
            }
        }
    });
    dep_api
        .patch(dep_name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;
    Ok(())
}
