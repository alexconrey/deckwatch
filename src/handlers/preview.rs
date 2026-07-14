// Preview environments per PR/branch.
//
// When GitOps is enabled and an operator (or webhook) requests a preview
// for a branch, we spin up a *temporary* copy of the deployment named
// `{name}-preview-{sanitized-branch}`, kick off a Kaniko build from that
// branch, and (best-effort) create a preview Ingress at
// `{branch}.preview.{host}`.
//
// The clone is annotated with:
//
//   deckwatch.io/preview          = "true"          // marker for cleanup
//   deckwatch.io/preview-source   = "<source-name>" // deployment we cloned
//   deckwatch.io/preview-branch   = "<branch>"      // git branch tracked
//   deckwatch.io/preview-pr       = "<pr number>"   // optional PR ID
//   deckwatch.io/preview-expires  = "<RFC3339 ts>"  // auto-cleanup deadline
//   deckwatch.io/preview-host     = "<hostname>"    // preview Ingress host
//
// The watcher's `preview_cleanup` sweep runs on the same 10s cadence as
// the GitOps poller and deletes any preview whose `preview-expires` has
// passed. TTL defaults to 24h; callers can override up to a hard cap of
// 7 days so an accidental long TTL can't leak cluster resources forever.
//
// The clone reuses the source's GitOps config wholesale except for the
// branch — this way the preview keeps rebuilding when new commits land on
// the PR branch without any additional wiring, and rolls in a fresh image
// automatically via the same monitor_builds path production uses.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, ServiceBackendPort,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, PostParams};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::state::AppState;
use crate::watcher::{ann, get_ann};

// Marker + config annotations. Kept namespaced under `deckwatch.io/` so
// the existing `delete_config` scrubber and any operator-side selector
// (`deckwatch.io/preview=true`) can find them uniformly.
pub const PREVIEW_MARKER: &str = "preview";
pub const PREVIEW_SOURCE: &str = "preview-source";
pub const PREVIEW_BRANCH: &str = "preview-branch";
pub const PREVIEW_PR: &str = "preview-pr";
pub const PREVIEW_EXPIRES: &str = "preview-expires";
pub const PREVIEW_HOST: &str = "preview-host";

/// Default TTL for a preview env. Long enough that a PR review can slip
/// overnight; short enough that a forgotten preview does not accrue cost
/// forever.
const DEFAULT_TTL_HOURS: i64 = 24;

/// Absolute upper bound on caller-supplied TTL. Operators can extend a
/// preview by re-issuing the POST (which recomputes `preview-expires`
/// against `now + ttl`); nobody needs a two-week preview by accident.
const MAX_TTL_HOURS: i64 = 24 * 7;

/// Suffix stripped off a preview name to recover the source deployment
/// name when the annotation is missing (e.g. hand-created preview).
const PREVIEW_INFIX: &str = "-preview-";

#[derive(Debug, Deserialize)]
pub struct CreatePreviewRequest {
    /// Git branch to build. Also used to derive the preview name and host.
    pub branch: String,
    /// Optional PR number, recorded for display so operators can jump
    /// straight from the preview list back to the source PR.
    #[serde(default)]
    pub pr_number: Option<i64>,
    /// TTL in hours. Defaults to 24; capped at 168 (one week).
    #[serde(default)]
    pub ttl_hours: Option<i64>,
    /// Optional preview host suffix. Full host becomes
    /// `{sanitized-branch}.preview.{host_suffix}`. When unset, the
    /// preview Ingress is skipped and the operator can hit the preview
    /// via `kubectl port-forward` / the built-in port-forward UI.
    #[serde(default)]
    pub host_suffix: Option<String>,
    /// Ingress class to use for the preview Ingress. Defaults to
    /// whatever the source deployment's first Ingress carries, or the
    /// cluster default when the source has no Ingress.
    #[serde(default)]
    pub ingress_class: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewSummary {
    pub name: String,
    pub namespace: String,
    pub source_deployment: String,
    pub branch: String,
    pub pr_number: Option<i64>,
    pub expires_at: String,
    pub host: Option<String>,
    pub image: String,
    pub replicas_desired: i32,
    pub replicas_ready: i32,
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewListResponse {
    pub previews: Vec<PreviewSummary>,
}

/// `POST /api/namespaces/{ns}/deployments/{name}/preview`
///
/// Creates (or refreshes the TTL on) a preview environment for the given
/// branch. Idempotent: re-posting the same branch resets `preview-expires`
/// and re-triggers a build, so a PR sync webhook can call this every time
/// commits land without needing separate create/update logic.
pub async fn create_preview(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<CreatePreviewRequest>,
) -> Result<(StatusCode, Json<PreviewSummary>), AppError> {
    let branch = req.branch.trim();
    if branch.is_empty() {
        return Err(AppError::BadRequest("branch is required".to_string()));
    }
    let ttl_hours = req
        .ttl_hours
        .unwrap_or(DEFAULT_TTL_HOURS)
        .clamp(1, MAX_TTL_HOURS);

    let dep_api = state.deployments_api(&ns)?;
    let source = dep_api.get(&name).await?;

    if get_ann(&source, "git-enabled") != Some("true") {
        return Err(AppError::BadRequest(
            "preview environments require GitOps to be enabled on the source deployment"
                .to_string(),
        ));
    }

    let sanitized = sanitize_branch(branch);
    let preview_name = derive_preview_name(&name, &sanitized);

    // Host = `{sanitized-branch}.preview.{suffix}` when a suffix is
    // provided. Skipping this leaves the preview reachable only through
    // in-cluster addresses or port-forward, which is a legit choice for
    // teams that do not want DNS wildcards for previews.
    let preview_host = req
        .host_suffix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|suffix| format!("{sanitized}.preview.{}", suffix.trim_start_matches('.')));

    let expires_at = jiff::Timestamp::now()
        .checked_add(jiff::Span::new().hours(ttl_hours))
        .map_err(|e| AppError::BadRequest(format!("invalid TTL: {e}")))?
        .to_string();

    // Build the cloned Deployment. Selector, labels, and the git-branch
    // annotation are rewritten so the preview does not fight the source
    // for pods and the watcher polls the correct branch.
    let mut cloned = source.clone();
    cloned.status = None;

    let mut labels: BTreeMap<String, String> = source.metadata.labels.clone().unwrap_or_default();
    labels.insert("app".to_string(), preview_name.clone());
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );
    labels.insert("deckwatch.io/preview".to_string(), "true".to_string());

    let mut annotations: BTreeMap<String, String> = source
        .metadata
        .annotations
        .clone()
        .map(strip_managed_annotations)
        .unwrap_or_default();

    // Rewrite the git branch so the poller tracks the preview branch, not
    // whatever the source was following.
    annotations.insert(ann("git-branch"), branch.to_string());
    // Clear the last-commit-sha so the watcher treats the first branch
    // check as "new commit" and kicks a build immediately.
    annotations.insert(ann("last-commit-sha"), String::new());

    annotations.insert(ann(PREVIEW_MARKER), "true".to_string());
    annotations.insert(ann(PREVIEW_SOURCE), name.clone());
    annotations.insert(ann(PREVIEW_BRANCH), branch.to_string());
    annotations.insert(ann(PREVIEW_EXPIRES), expires_at.clone());
    if let Some(pr) = req.pr_number {
        annotations.insert(ann(PREVIEW_PR), pr.to_string());
    }
    if let Some(host) = preview_host.as_ref() {
        annotations.insert(ann(PREVIEW_HOST), host.clone());
    }

    cloned.metadata = ObjectMeta {
        name: Some(preview_name.clone()),
        namespace: Some(ns.clone()),
        labels: Some(labels.clone()),
        annotations: Some(annotations),
        ..Default::default()
    };

    let selector_labels: BTreeMap<String, String> =
        BTreeMap::from([("app".to_string(), preview_name.clone())]);

    if let Some(spec) = cloned.spec.as_mut() {
        spec.selector.match_labels = Some(selector_labels.clone());
        if let Some(meta) = spec.template.metadata.as_mut() {
            let l = meta.labels.get_or_insert_with(BTreeMap::new);
            l.insert("app".to_string(), preview_name.clone());
        }
    }

    // Idempotent apply: try create; on 409 delete-and-recreate so the
    // caller (typically a webhook fired on `pull_request.synchronize`)
    // does not need to know whether this is the first sync or the fifth.
    match dep_api.create(&PostParams::default(), &cloned).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 409 => {
            let _ = dep_api
                .delete(&preview_name, &DeleteParams::default())
                .await;
            dep_api.create(&PostParams::default(), &cloned).await?;
        }
        Err(e) => return Err(e.into()),
    }

    // Best-effort preview Ingress. Failing to create the Ingress does not
    // fail the whole preview — the Deployment is already up, and the
    // operator can add an ingress manually if the class is unavailable.
    if let Some(host) = preview_host.as_ref() {
        let ingress_class = match req.ingress_class.clone() {
            Some(c) => Some(c),
            None => infer_ingress_class(&state, &ns, &name).await.ok().flatten(),
        };
        if let Err(e) = create_preview_ingress(
            &state,
            &ns,
            &preview_name,
            host,
            ingress_class.as_deref(),
            source_service_port(&source),
        )
        .await
        {
            tracing::warn!(
                deployment = %preview_name,
                host = %host,
                error = %e,
                "preview ingress creation failed; deployment is up but not routed"
            );
        }
    }

    let created = dep_api.get(&preview_name).await?;
    Ok((StatusCode::CREATED, Json(summarize_preview(&created, &ns))))
}

/// `GET /api/namespaces/{ns}/previews` — list all preview envs in the ns.
///
/// Uses the `deckwatch.io/preview=true` label so an operator running
/// `kubectl get deploy -l deckwatch.io/preview=true` sees the same view.
pub async fn list_previews(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<PreviewListResponse>, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let lp = ListParams::default().labels("deckwatch.io/preview=true");
    let list = dep_api.list(&lp).await?;
    let previews = list
        .items
        .iter()
        .map(|d| summarize_preview(d, &ns))
        .collect();
    Ok(Json(PreviewListResponse { previews }))
}

/// `GET /api/namespaces/{ns}/deployments/{name}/previews`
///
/// Same as list_previews but filtered to previews cloned from a specific
/// source deployment. Filtering on the client would be fine but keeping
/// the filter server-side lets the frontend paginate/poll cheaply.
pub async fn list_previews_for_source(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<PreviewListResponse>, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let lp = ListParams::default().labels("deckwatch.io/preview=true");
    let list = dep_api.list(&lp).await?;
    let previews = list
        .items
        .iter()
        .filter(|d| {
            d.metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(&ann(PREVIEW_SOURCE)))
                .map(|s| s.as_str())
                == Some(name.as_str())
        })
        .map(|d| summarize_preview(d, &ns))
        .collect();
    Ok(Json(PreviewListResponse { previews }))
}

/// `DELETE /api/namespaces/{ns}/deployments/{preview_name}/preview`
///
/// Manually tears down a preview (deletes Deployment + preview Ingress).
/// The cleanup sweep does the same on TTL expiry; this is the "I am done
/// with this preview" button.
pub async fn delete_preview(
    State(state): State<AppState>,
    Path((ns, preview_name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let dep = dep_api.get(&preview_name).await?;
    if get_ann(&dep, PREVIEW_MARKER) != Some("true") {
        return Err(AppError::BadRequest(format!(
            "deployment '{preview_name}' is not a preview environment"
        )));
    }
    delete_preview_resources(&state, &ns, &preview_name).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- helpers

fn summarize_preview(dep: &Deployment, ns: &str) -> PreviewSummary {
    let anns = dep.metadata.annotations.as_ref();
    let get = |k: &str| -> Option<String> { anns.and_then(|a| a.get(&ann(k))).cloned() };

    // If the annotation was scrubbed but the naming convention holds,
    // fall back to the `{source}-preview-{branch}` prefix so preview
    // rows in the UI never render with an empty "source" column.
    let source_deployment = get(PREVIEW_SOURCE).unwrap_or_else(|| {
        dep.name_any()
            .split_once(PREVIEW_INFIX)
            .map(|(src, _)| src.to_string())
            .unwrap_or_default()
    });

    let image = dep
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first())
        .and_then(|c| c.image.clone())
        .unwrap_or_default();

    PreviewSummary {
        name: dep.name_any(),
        namespace: ns.to_string(),
        source_deployment,
        branch: get(PREVIEW_BRANCH).unwrap_or_default(),
        pr_number: get(PREVIEW_PR).and_then(|s| s.parse().ok()),
        expires_at: get(PREVIEW_EXPIRES).unwrap_or_default(),
        host: get(PREVIEW_HOST).filter(|s| !s.is_empty()),
        image,
        replicas_desired: dep.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0),
        replicas_ready: dep
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0),
        created_at: dep
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
    }
}

/// Delete the preview Deployment and the paired preview Ingress. Both
/// deletes are best-effort NotFound-tolerant so a partially-created
/// preview (e.g. Deployment created, Ingress failed) can still be cleaned
/// up without a stuck-forever state.
pub async fn delete_preview_resources(
    state: &AppState,
    ns: &str,
    preview_name: &str,
) -> Result<(), AppError> {
    let dep_api = state.deployments_api(ns)?;
    match dep_api.delete(preview_name, &DeleteParams::default()).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 404 => {}
        Err(e) => return Err(e.into()),
    }

    let ing_api = state.ingresses_api(ns)?;
    let ingress_name = preview_ingress_name(preview_name);
    match ing_api
        .delete(&ingress_name, &DeleteParams::default())
        .await
    {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 404 => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

async fn create_preview_ingress(
    state: &AppState,
    ns: &str,
    preview_name: &str,
    host: &str,
    ingress_class: Option<&str>,
    service_port: i32,
) -> Result<(), AppError> {
    let api = state.ingresses_api(ns)?;
    let name = preview_ingress_name(preview_name);

    let mut labels = BTreeMap::new();
    labels.insert("deckwatch.io/preview".to_string(), "true".to_string());
    labels.insert(
        "deckwatch.io/preview-for".to_string(),
        preview_name.to_string(),
    );

    let ing = Ingress {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(IngressSpec {
            ingress_class_name: ingress_class.map(str::to_string),
            rules: Some(vec![IngressRule {
                host: Some(host.to_string()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        path: Some("/".to_string()),
                        path_type: "Prefix".to_string(),
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: preview_name.to_string(),
                                port: Some(ServiceBackendPort {
                                    number: Some(service_port),
                                    ..Default::default()
                                }),
                            }),
                            ..Default::default()
                        },
                    }],
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    match api.create(&PostParams::default(), &ing).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(e)) if e.code == 409 => {
            // Ingress already exists from an earlier preview refresh —
            // leave it (host + backend name are stable across refreshes).
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn preview_ingress_name(preview_name: &str) -> String {
    format!("{preview_name}-preview")
}

/// Kubernetes annotations Kubernetes itself owns (revision history,
/// pod-template-hash) do not survive being copied to a new Deployment —
/// carrying them into a preview confuses the rollout controller.
fn strip_managed_annotations(mut a: BTreeMap<String, String>) -> BTreeMap<String, String> {
    a.retain(|k, _| {
        !k.starts_with("deployment.kubernetes.io/") && !k.starts_with("kubectl.kubernetes.io/")
    });
    a
}

/// Look at the source deployment's existing Ingresses and steal the class
/// of the first one that routes to the source Service. Falls back to None
/// so `create_preview_ingress` leaves the class unset and the cluster's
/// default IngressClass wins.
async fn infer_ingress_class(
    state: &AppState,
    ns: &str,
    source_deployment: &str,
) -> Result<Option<String>, AppError> {
    let api = state.ingresses_api(ns)?;
    let list = api.list(&ListParams::default()).await?;
    let matched = list
        .items
        .into_iter()
        .find(|i| {
            i.spec
                .as_ref()
                .and_then(|s| s.rules.as_ref())
                .map(|rules| {
                    rules.iter().any(|r| {
                        r.http
                            .as_ref()
                            .map(|http| {
                                http.paths.iter().any(|p| {
                                    p.backend
                                        .service
                                        .as_ref()
                                        .map(|svc| svc.name == source_deployment)
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .and_then(|i| i.spec.and_then(|s| s.ingress_class_name));
    Ok(matched)
}

/// Sniff the source's first container port so the preview Ingress does
/// not point at a nonexistent backend port. Defaults to 80 when the
/// source doesn't declare one — same fallback used by the ingresses
/// handler.
fn source_service_port(dep: &Deployment) -> i32 {
    dep.spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first())
        .and_then(|c| c.ports.as_ref())
        .and_then(|ports| ports.first())
        .map(|p| p.container_port)
        .unwrap_or(80)
}

/// Preview name = `{source}-preview-{sanitized-branch}`, truncated to
/// fit K8s' 63-char resource-name limit. When truncation happens we
/// keep the source prefix intact and cut from the branch tail so two
/// previews from different branches don't collide.
pub fn derive_preview_name(source: &str, sanitized_branch: &str) -> String {
    let base = format!("{source}{PREVIEW_INFIX}{sanitized_branch}");
    if base.len() <= 63 {
        return base;
    }
    let overflow = base.len() - 63;
    let cut = sanitized_branch.len().saturating_sub(overflow);
    format!("{source}{PREVIEW_INFIX}{}", &sanitized_branch[..cut])
}

/// Lowercase, replace anything outside `[a-z0-9-]` with `-`, collapse
/// runs, and trim to something Kubernetes will accept as a DNS-1123
/// label component. Branch names like `feature/CAT-123_Foo` become
/// `feature-cat-123-foo`.
pub fn sanitize_branch(branch: &str) -> String {
    let lower: String = branch
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut out = String::with_capacity(lower.len());
    let mut prev_dash = false;
    for c in lower.chars() {
        if c == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("branch");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_common_branch_shapes() {
        assert_eq!(sanitize_branch("main"), "main");
        assert_eq!(sanitize_branch("feature/CAT-123"), "feature-cat-123");
        assert_eq!(sanitize_branch("release/2026.07"), "release-2026-07");
        assert_eq!(sanitize_branch("---weird---"), "weird");
        assert_eq!(sanitize_branch(""), "branch");
    }

    #[test]
    fn preview_name_truncates_within_63_chars() {
        let long = "x".repeat(80);
        let name = derive_preview_name("api", &long);
        assert!(name.len() <= 63);
        assert!(name.starts_with("api-preview-"));
    }

    #[test]
    fn preview_name_common_case() {
        assert_eq!(
            derive_preview_name("api", "feature-cat-123"),
            "api-preview-feature-cat-123"
        );
    }
}
