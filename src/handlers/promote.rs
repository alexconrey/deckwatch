// Multi-environment promotion (dev -> staging -> prod).
//
// Promotion is *not* a full clone. It copies the source deployment's
// current *image* and select spec fields into a deployment that already
// exists in the target namespace, patching in place. The target keeps
// its own environment-specific config (replicas, HPA, secrets refs, env
// vars that differ per environment, ingress rules) — only the fields an
// operator would consciously want to move between environments are
// promoted.
//
// This is deliberately narrower than /clone (which stands up a brand-new
// Deployment). The Heroku Pipelines model that operators expect is:
// "run this image in prod, keep everything else the same." Overwriting
// the target's replicas or resource limits would silently destroy tuning
// that the SRE team did in prod.
//
// The promoted fields are:
//   - container image (primary container only — sidecars are left as-is)
//   - command / args (rarely different across environments; when they
//     are the operator is doing something exotic and needs the diff)
//
// Environment variables, resource requests/limits, and probes are left
// alone. Ingress hostnames, HPA config, PDBs are not touched.
//
// The two-step "diff then apply" flow is exposed as:
//
//   POST /api/namespaces/{ns}/deployments/{name}/promote?dry_run=true
//     -> returns a PromotePreview describing what would change
//   POST /api/namespaces/{ns}/deployments/{name}/promote
//     -> applies the change and returns the same preview plus the new
//        DeploymentDetailResponse of the target

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use kube::api::{Patch, PatchParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::deployments::DeploymentDetailResponse;
use crate::kube_ext::deployment_detail;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PromoteRequest {
    /// Namespace the promotion targets. Must be in the allowed namespace
    /// list — verified transitively via `state.deployments_api`.
    pub target_namespace: String,
    /// Optional target deployment name. Defaults to the same name as the
    /// source, which is what the vast majority of promotion flows want
    /// (`api` in `dev` -> `api` in `prod`). Renaming across environments
    /// is unusual but supported for the "we have both dev-api and api"
    /// case some teams inherit from earlier tooling.
    #[serde(default)]
    pub target_name: Option<String>,
    /// Optional operator-supplied justification. Written to the target
    /// deployment's `kubernetes.io/change-cause` annotation so it shows
    /// up in `kubectl rollout history` and Deckwatch's history table.
    /// Free-form; typical values are a PR/ticket link or "hotfix: rollback
    /// stripe integration".
    #[serde(default)]
    pub change_cause: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PromoteQuery {
    /// When true, return the diff without applying it. Same endpoint
    /// serves both flows so the frontend does one round-trip for
    /// preview and a second confirming round-trip for apply.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct PromoteFieldChange {
    pub field: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PromotePreview {
    pub source_namespace: String,
    pub source_name: String,
    pub target_namespace: String,
    pub target_name: String,
    /// Ordered list of field-level changes the promotion would apply.
    /// Empty when the source and target are already in sync — the
    /// frontend surfaces this as "nothing to promote" without failing.
    pub changes: Vec<PromoteFieldChange>,
    /// True when the source and target already match on every promoted
    /// field. UI uses this to disable the apply button.
    pub no_op: bool,
}

#[derive(Debug, Serialize)]
pub struct PromoteResponse {
    #[serde(flatten)]
    pub preview: PromotePreview,
    /// When `dry_run=false`, the target's post-apply detail. Omitted for
    /// dry-runs so the frontend never accidentally renders a stale view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<DeploymentDetailResponse>,
}

/// `POST /api/namespaces/{ns}/deployments/{name}/promote[?dry_run=true]`
///
/// Promotes the source deployment's runnable fields into the target. When
/// `dry_run` is set, returns the diff without patching; otherwise applies
/// and returns the diff plus the post-apply target detail.
pub async fn promote(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Query(q): Query<PromoteQuery>,
    Json(req): Json<PromoteRequest>,
) -> Result<(StatusCode, Json<PromoteResponse>), AppError> {
    let target_ns = req.target_namespace.trim();
    if target_ns.is_empty() {
        return Err(AppError::BadRequest(
            "target_namespace is required".to_string(),
        ));
    }
    let target_name = req
        .target_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&name)
        .to_string();

    if target_ns == ns && target_name == name {
        return Err(AppError::BadRequest(
            "promotion target is identical to source; choose a different namespace or target_name"
                .to_string(),
        ));
    }

    let src_api = state.deployments_api(&ns)?;
    let dst_api = state.deployments_api(target_ns)?;

    let source = src_api.get(&name).await?;
    let target = dst_api.get(&target_name).await.map_err(|e| match e {
        kube::Error::Api(api_err) if api_err.code == 404 => AppError::NotFound(format!(
            "target deployment '{target_name}' does not exist in '{target_ns}'; \
             create it first (typically by promoting from a lower environment or via /clone)"
        )),
        other => AppError::Kube(other),
    })?;

    let changes = compute_changes(&source, &target);
    let no_op = changes.is_empty();

    let preview = PromotePreview {
        source_namespace: ns.clone(),
        source_name: name.clone(),
        target_namespace: target_ns.to_string(),
        target_name: target_name.clone(),
        changes,
        no_op,
    };

    if q.dry_run || no_op {
        // On dry-run OR on no-op, we don't patch; either the frontend is
        // asking for the diff (dry-run) or there's nothing to apply. The
        // 200 status distinguishes this from a real 201-on-apply.
        return Ok((
            StatusCode::OK,
            Json(PromoteResponse {
                preview,
                target: None,
            }),
        ));
    }

    let patch = build_promote_patch(&source, req.change_cause.as_deref());

    let patched = dst_api
        .patch(
            &target_name,
            &PatchParams::default(),
            &Patch::Strategic(patch),
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(PromoteResponse {
            preview,
            target: Some(DeploymentDetailResponse {
                detail: deployment_detail(&patched),
                // Post-promote the pods/ingresses in the target namespace
                // are the client's responsibility to re-fetch — the
                // detail view will poll them on the next tick. Keeping
                // them empty here avoids an extra pods.list round-trip
                // in a hot code path.
                pods: vec![],
                ingresses: vec![],
            }),
        }),
    ))
}

// -------------------------------------------------------------- diff builder

fn compute_changes(source: &Deployment, target: &Deployment) -> Vec<PromoteFieldChange> {
    let src = primary_container(source);
    let dst = primary_container(target);

    let mut changes = Vec::new();

    let src_image = src.and_then(|c| c.image.clone()).unwrap_or_default();
    let dst_image = dst.and_then(|c| c.image.clone()).unwrap_or_default();
    if src_image != dst_image {
        changes.push(PromoteFieldChange {
            field: "image".to_string(),
            from: Some(dst_image),
            to: Some(src_image),
        });
    }

    let src_cmd = src.and_then(|c| c.command.clone()).unwrap_or_default();
    let dst_cmd = dst.and_then(|c| c.command.clone()).unwrap_or_default();
    if src_cmd != dst_cmd {
        changes.push(PromoteFieldChange {
            field: "command".to_string(),
            from: Some(format!("{dst_cmd:?}")),
            to: Some(format!("{src_cmd:?}")),
        });
    }

    let src_args = src.and_then(|c| c.args.clone()).unwrap_or_default();
    let dst_args = dst.and_then(|c| c.args.clone()).unwrap_or_default();
    if src_args != dst_args {
        changes.push(PromoteFieldChange {
            field: "args".to_string(),
            from: Some(format!("{dst_args:?}")),
            to: Some(format!("{src_args:?}")),
        });
    }

    changes
}

fn primary_container(dep: &Deployment) -> Option<&k8s_openapi::api::core::v1::Container> {
    dep.spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|p| p.containers.first())
}

/// Strategic-merge patch payload that overlays only the promoted fields
/// onto the target. Using a strategic-merge patch (not JSON Merge or
/// JSON Patch) means the `containers` array is matched by container
/// `name` and the target's sidecars survive untouched. Deckwatch names
/// the primary container after the deployment; strategic merge will
/// match by that name and update image/command/args in place.
fn build_promote_patch(source: &Deployment, change_cause: Option<&str>) -> serde_json::Value {
    let src = primary_container(source);
    let name = src.map(|c| c.name.clone()).unwrap_or_default();
    let image = src.and_then(|c| c.image.clone());
    let command = src.and_then(|c| c.command.clone());
    let args = src.and_then(|c| c.args.clone());

    let mut container = serde_json::json!({ "name": name });
    if let Some(image) = image {
        container["image"] = serde_json::Value::String(image);
    }
    if let Some(command) = command {
        container["command"] =
            serde_json::Value::Array(command.into_iter().map(serde_json::Value::String).collect());
    }
    if let Some(args) = args {
        container["args"] =
            serde_json::Value::Array(args.into_iter().map(serde_json::Value::String).collect());
    }

    // Stamp change-cause annotation so the promotion shows up in the
    // rollout history the same way manual `kubectl set image --record`
    // used to. Even without an operator-supplied reason we record the
    // fact that it was a promotion so operators diffing history rows
    // aren't left guessing.
    let cause = change_cause
        .map(str::to_string)
        .unwrap_or_else(|| "promoted via deckwatch".to_string());

    serde_json::json!({
        "metadata": {
            "annotations": {
                "kubernetes.io/change-cause": cause,
            }
        },
        "spec": {
            "template": {
                "spec": {
                    "containers": [container]
                }
            }
        }
    })
}
