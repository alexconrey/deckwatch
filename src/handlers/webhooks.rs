//! `POST /api/webhooks/gitops` — GitHub / GitLab / Bitbucket delivery
//! receiver. Runs unauthenticated because the provider drives it: security
//! is entirely the shared-secret signature check that
//! [`GitProvider::verify_webhook`] enforces.
//!
//! Flow:
//!   1. Read the raw body bytes (needed verbatim for signature verification).
//!   2. Pick a provider by looking at the delivery headers: GitHub sends
//!      `X-Github-Event`, GitLab sends `X-Gitlab-Event`, Bitbucket sends
//!      `X-Event-Key`. Falls back to 400 if none matches — we never accept
//!      an unsigned delivery.
//!   3. Enumerate every deployment across allowed namespaces whose
//!      `deckwatch.io/git-repo` (normalized) + `deckwatch.io/git-branch`
//!      match the event.
//!   4. For each match, load its per-deployment webhook secret, re-verify
//!      the signature against *that* secret, and if it passes, kick off a
//!      build via the existing `trigger_build_public` path.
//!
//! We deliberately re-verify per deployment rather than trusting a single
//! successful check because the operator may configure the same repo in
//! multiple deployments with different signing secrets (e.g. one webhook
//! per environment). Any deployment whose secret doesn't match is silently
//! skipped, not rejected — otherwise a shared repo would fail globally on
//! the first deployment with a stale secret.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, ResourceExt};
use serde::Serialize;

use crate::error::AppError;
use crate::handlers::git_provider::{
    detect_provider, normalize_repo_url, GitHub, GitLab, Bitbucket, GitProvider, WebhookError,
    WebhookEvent,
};
use crate::handlers::gitops::webhook_secret_name;
use crate::metrics;
use crate::state::AppState;
use crate::watcher::{ann, get_ann};

#[derive(Serialize)]
pub struct WebhookResponse {
    pub triggered: Vec<TriggeredBuild>,
    pub skipped: Vec<SkippedDeployment>,
    pub provider: String,
    pub commit_sha: String,
}

#[derive(Serialize)]
pub struct TriggeredBuild {
    pub namespace: String,
    pub deployment: String,
    pub job_name: String,
}

#[derive(Serialize)]
pub struct SkippedDeployment {
    pub namespace: String,
    pub deployment: String,
    pub reason: String,
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<WebhookResponse>), AppError> {
    // Pick provider by inspecting delivery headers. We do this before
    // signature check because we can't verify without knowing whose scheme
    // to run.
    let provider: Box<dyn GitProvider> = if headers.contains_key("x-github-event") {
        Box::new(GitHub)
    } else if headers.contains_key("x-gitlab-event") {
        Box::new(GitLab)
    } else if headers.contains_key("x-event-key") {
        Box::new(Bitbucket)
    } else {
        return Err(AppError::BadRequest(
            "unrecognized webhook: no provider header found".to_string(),
        ));
    };

    // Parse the event body once to learn (repo, branch, sha). We reject
    // signature failures here using an empty secret — this call succeeds
    // for GitLab (which just needs *some* token) if the secret is empty,
    // and for GitHub/Bitbucket it will always fail. That's fine: we only
    // use this pass to extract the routing info. We then RE-verify per
    // matching deployment with the deployment's actual secret before
    // triggering a build.
    //
    // For GitHub/Bitbucket we can't extract the payload without a valid
    // signature (verify_webhook checks sig before parsing), so we fall
    // back to parsing the JSON body directly here — provider-specific but
    // unavoidable given the "which deployment does this go to?" chicken-
    // and-egg problem.
    let event = extract_event_unverified(provider.name(), &headers, &body)
        .map_err(|e| AppError::BadRequest(format!("failed to parse webhook body: {e}")))?;

    tracing::info!(
        provider = provider.name(),
        repo = %event.repo_url,
        branch = %event.branch,
        commit = %event.commit_sha,
        "webhook received"
    );

    // Enumerate candidate deployments. Uses the same namespace-scoping
    // logic as the poller so operators aren't surprised by webhooks
    // reaching namespaces the poller wouldn't touch.
    let namespaces = if state.allowed_namespaces.is_empty() {
        let ns_api = state.namespaces_api();
        ns_api
            .list(&ListParams::default())
            .await?
            .iter()
            .map(|ns| ns.name_any())
            .collect::<Vec<_>>()
    } else {
        state.allowed_namespaces.clone()
    };

    let mut triggered = Vec::new();
    let mut skipped = Vec::new();

    for ns in &namespaces {
        let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
        let deps = dep_api.list(&ListParams::default()).await?;

        for dep in deps.items.iter() {
            let dep_name = dep.name_any();
            if get_ann(dep, "git-enabled") != Some("true") {
                continue;
            }
            if get_ann(dep, "webhook-enabled") != Some("true") {
                continue;
            }
            let dep_repo = match get_ann(dep, "git-repo") {
                Some(r) => normalize_repo_url(r),
                None => continue,
            };
            let dep_branch = get_ann(dep, "git-branch").unwrap_or("main");
            if dep_repo != event.repo_url || dep_branch != event.branch {
                continue;
            }

            // Load the deployment's own signing secret. If none is
            // configured, refuse — accepting an unsigned webhook that
            // triggers a build would let anyone kick a deploy just by
            // knowing the repo URL.
            let secret_value =
                match load_webhook_secret(&state, ns, &dep_name).await {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        skipped.push(SkippedDeployment {
                            namespace: ns.clone(),
                            deployment: dep_name.clone(),
                            reason: "no webhook secret configured".to_string(),
                        });
                        continue;
                    }
                    Err(e) => {
                        skipped.push(SkippedDeployment {
                            namespace: ns.clone(),
                            deployment: dep_name.clone(),
                            reason: format!("failed to load secret: {e}"),
                        });
                        continue;
                    }
                };

            // Now do the *authoritative* signature check against this
            // deployment's secret. This is the only check that gates a
            // build kickoff — the earlier body-parse was routing-only.
            if let Err(e) = provider.verify_webhook(&headers, &body, &secret_value) {
                match e {
                    WebhookError::UnsupportedEvent(reason) => {
                        skipped.push(SkippedDeployment {
                            namespace: ns.clone(),
                            deployment: dep_name.clone(),
                            reason: format!("event not actionable: {reason}"),
                        });
                    }
                    _ => {
                        // Signature/missing-header errors are logged at
                        // debug because multiple deployments sharing a
                        // repo will naturally see mismatches for the ones
                        // whose secrets don't match this delivery.
                        tracing::debug!(
                            namespace = %ns,
                            deployment = %dep_name,
                            error = %e,
                            "webhook signature mismatch"
                        );
                        skipped.push(SkippedDeployment {
                            namespace: ns.clone(),
                            deployment: dep_name.clone(),
                            reason: "signature mismatch".to_string(),
                        });
                    }
                }
                continue;
            }

            // Signature OK. Kick off a build using the same helper the
            // manual trigger endpoint uses, so bookkeeping stays uniform.
            let token = load_git_token(&state, ns, dep).await;
            match crate::watcher::trigger_build_public(
                &state,
                ns,
                dep,
                &event.commit_sha,
                &token,
            )
            .await
            {
                Ok(job_name) => {
                    metrics::record_gitops_build(ns, "started");
                    // Mirror the poller's post-build patch so the UI sees
                    // the same "building" indicator regardless of source.
                    let now = jiff::Timestamp::now().to_string();
                    let patch = serde_json::json!({
                        "metadata": {
                            "annotations": {
                                ann("last-commit-sha"): event.commit_sha,
                                ann("last-build-status"): "building",
                                ann("last-build-job"): job_name,
                                ann("last-build-time"): now,
                                ann("last-build-error"): "",
                            }
                        }
                    });
                    let _ = dep_api
                        .patch(&dep_name, &PatchParams::default(), &Patch::Merge(patch))
                        .await;
                    triggered.push(TriggeredBuild {
                        namespace: ns.clone(),
                        deployment: dep_name,
                        job_name,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        namespace = %ns,
                        deployment = %dep_name,
                        error = %e,
                        "failed to trigger webhook build"
                    );
                    skipped.push(SkippedDeployment {
                        namespace: ns.clone(),
                        deployment: dep_name,
                        reason: format!("trigger failed: {e}"),
                    });
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        Json(WebhookResponse {
            triggered,
            skipped,
            provider: provider.name().to_string(),
            commit_sha: event.commit_sha,
        }),
    ))
}

/// Parse the webhook body without checking the signature. Only used to
/// figure out which deployments to fan out to; the authoritative
/// signature check happens per-deployment with each deployment's own
/// secret before any build is triggered.
fn extract_event_unverified(
    provider: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> anyhow::Result<WebhookEvent> {
    match provider {
        "github" => {
            let event = headers
                .get("x-github-event")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if event != "push" {
                anyhow::bail!("unsupported github event: {event}");
            }
            #[derive(serde::Deserialize)]
            struct GhPush {
                #[serde(rename = "ref")]
                ref_: String,
                after: String,
                repository: GhRepo,
            }
            #[derive(serde::Deserialize)]
            struct GhRepo {
                clone_url: String,
            }
            let p: GhPush = serde_json::from_slice(body)?;
            let branch = p
                .ref_
                .strip_prefix("refs/heads/")
                .ok_or_else(|| anyhow::anyhow!("non-branch ref: {}", p.ref_))?
                .to_string();
            Ok(WebhookEvent {
                repo_url: normalize_repo_url(&p.repository.clone_url),
                branch,
                commit_sha: p.after,
            })
        }
        "gitlab" => {
            let event = headers
                .get("x-gitlab-event")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if event != "Push Hook" {
                anyhow::bail!("unsupported gitlab event: {event}");
            }
            #[derive(serde::Deserialize)]
            struct GlPush {
                #[serde(rename = "ref")]
                ref_: String,
                after: String,
                project: GlProject,
            }
            #[derive(serde::Deserialize)]
            struct GlProject {
                git_http_url: String,
            }
            let p: GlPush = serde_json::from_slice(body)?;
            let branch = p
                .ref_
                .strip_prefix("refs/heads/")
                .ok_or_else(|| anyhow::anyhow!("non-branch ref: {}", p.ref_))?
                .to_string();
            Ok(WebhookEvent {
                repo_url: normalize_repo_url(&p.project.git_http_url),
                branch,
                commit_sha: p.after,
            })
        }
        "bitbucket" => {
            let event = headers
                .get("x-event-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if event != "repo:push" {
                anyhow::bail!("unsupported bitbucket event: {event}");
            }
            #[derive(serde::Deserialize)]
            struct BbPush {
                repository: BbRepo,
                push: BbPushChanges,
            }
            #[derive(serde::Deserialize)]
            struct BbRepo {
                links: BbLinks,
            }
            #[derive(serde::Deserialize)]
            struct BbLinks {
                html: BbHref,
            }
            #[derive(serde::Deserialize)]
            struct BbHref {
                href: String,
            }
            #[derive(serde::Deserialize)]
            struct BbPushChanges {
                changes: Vec<BbChange>,
            }
            #[derive(serde::Deserialize)]
            struct BbChange {
                new: Option<BbRef>,
            }
            #[derive(serde::Deserialize)]
            struct BbRef {
                name: String,
                #[serde(rename = "type")]
                kind: String,
                target: BbTarget,
            }
            #[derive(serde::Deserialize)]
            struct BbTarget {
                hash: String,
            }
            let p: BbPush = serde_json::from_slice(body)?;
            let change = p
                .push
                .changes
                .into_iter()
                .find(|c| c.new.as_ref().map(|r| r.kind == "branch").unwrap_or(false))
                .and_then(|c| c.new)
                .ok_or_else(|| anyhow::anyhow!("no branch change in push"))?;
            Ok(WebhookEvent {
                repo_url: normalize_repo_url(&p.repository.links.html.href),
                branch: change.name,
                commit_sha: change.target.hash,
            })
        }
        _ => anyhow::bail!("unknown provider: {provider}"),
    }
}

/// Read the shared signing secret for a deployment from its dedicated
/// Kubernetes Secret. Returns `Ok(None)` when the Secret doesn't exist so
/// callers can treat "no secret configured" separately from an API error.
async fn load_webhook_secret(
    state: &AppState,
    ns: &str,
    dep_name: &str,
) -> Result<Option<String>, AppError> {
    let secrets_api = state.secrets_api(ns)?;
    match secrets_api.get(&webhook_secret_name(dep_name)).await {
        Ok(secret) => Ok(secret
            .data
            .as_ref()
            .and_then(|d| d.get("secret"))
            .map(|v| String::from_utf8_lossy(&v.0).to_string())),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Best-effort token loader for the eventual `trigger_build_public` call.
/// Returns "" on any failure — the smart-HTTP build path already handles
/// unauthenticated git contexts, and we don't want to abort a webhook build
/// because a token secret got deleted (the caller will just see an auth
/// failure in the build logs, which is a clearer signal than 500 from the
/// webhook endpoint).
async fn load_git_token(state: &AppState, ns: &str, dep: &Deployment) -> String {
    let Some(token_secret) = get_ann(dep, "git-token-secret") else {
        return String::new();
    };
    if token_secret.is_empty() {
        return String::new();
    }
    let Ok(api) = state.secrets_api(ns) else {
        return String::new();
    };
    match api.get(token_secret).await {
        Ok(secret) => secret
            .data
            .as_ref()
            .and_then(|d| d.get("token"))
            .map(|v| String::from_utf8_lossy(&v.0).to_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}
