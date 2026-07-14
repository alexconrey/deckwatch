// Auto-rollback: watch deployments that opt in via the
// `deckwatch.io/auto-rollback: "true"` annotation and, when they've been
// unhealthy for > 5 minutes, roll them back to the previous revision.
//
// This module is intended to be appended to `src/watcher.rs`, or dropped
// beside it and re-exported. The public entry point is `auto_rollback_cycle`,
// which should be invoked from the existing poll loop in `run_poller`
// alongside `poll_cycle` and `monitor_builds`.
//
// Design rationale:
//
// * The "unhealthy since" timestamp is persisted as an annotation
//   (`deckwatch.io/unhealthy-since`), NOT held in memory. That lets an
//   operator restart deckwatch without resetting the 5-minute grace period
//   and re-arming the rollback trigger for still-broken deployments.
// * The annotation is cleared as soon as the deployment goes back to
//   `Available`, so a slow-but-healthy rollout does not accumulate a false
//   trigger.
// * After a rollback fires, we stamp `deckwatch.io/last-auto-rollback` with
//   the target revision and timestamp, and DELETE `unhealthy-since` — the
//   new revision starts with a clean grace period. If it also fails within
//   5 minutes, we'll roll back again (to whatever is now the previous
//   revision), which is what an operator would do manually.
// * We deliberately never roll back below revision 1. If the deployment has
//   only ever had one revision, `find_previous_revision` returns None and
//   we do nothing — there's nothing older to fall back to.

use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, ResourceExt};

use crate::kube_ext::{deployment_phase, DeploymentPhase};
use crate::state::AppState;
use crate::watcher::{ann, get_ann};

/// How long a deployment must be continuously unhealthy before we roll it
/// back. Matches the default `progressDeadlineSeconds` for a Kubernetes
/// Deployment (600s), halved — we want to react before the API server gives
/// up, not after.
const UNHEALTHY_GRACE_SECS: i64 = 300;

/// Annotation the deployment carries to opt into auto-rollback.
const AUTO_ROLLBACK_ANN: &str = "auto-rollback";
/// Annotation we write to record the first time we saw this deployment in a
/// bad state. Persisted so a deckwatch restart doesn't reset the timer.
const UNHEALTHY_SINCE_ANN: &str = "unhealthy-since";
/// Annotation we write after a successful auto-rollback for auditability.
const LAST_AUTO_ROLLBACK_ANN: &str = "last-auto-rollback";
/// Same revision annotation Kubernetes writes on each ReplicaSet.
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
/// Free-form change cause; consumed by the history endpoint / kubectl
/// rollout history.
const CHANGE_CAUSE_ANNOTATION: &str = "kubernetes.io/change-cause";

/// Runs one auto-rollback evaluation across every allowed namespace.
///
/// Call this from `run_poller` on every tick, right after `monitor_builds`.
/// Errors are logged but not propagated — one broken deployment must not
/// keep us from evaluating the rest.
pub async fn auto_rollback_cycle(state: &AppState) -> anyhow::Result<()> {
    let namespaces = if state.allowed_namespaces.is_empty() {
        let ns_api = state.namespaces_api();
        let ns_list = ns_api.list(&ListParams::default()).await?;
        ns_list.iter().map(|ns| ns.name_any()).collect::<Vec<_>>()
    } else {
        state.allowed_namespaces.clone()
    };

    for ns in &namespaces {
        let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
        let deps = match dep_api.list(&ListParams::default()).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(namespace = %ns, error = %e, "auto_rollback: list failed");
                continue;
            }
        };

        for dep in deps.items.iter() {
            if get_ann(dep, AUTO_ROLLBACK_ANN) != Some("true") {
                continue;
            }
            let name = dep.name_any();
            if let Err(e) = evaluate_deployment(state, ns, dep).await {
                tracing::warn!(
                    deployment = %name,
                    namespace = %ns,
                    error = %e,
                    "auto_rollback: evaluation failed"
                );
            }
        }
    }

    Ok(())
}

/// Evaluate one deployment. There are four cases:
///
/// 1. Healthy (Available): clear any `unhealthy-since` marker and return.
/// 2. Unhealthy for the first time: stamp `unhealthy-since=now`, return.
/// 3. Unhealthy but under the grace window: nothing to do.
/// 4. Unhealthy for > grace window: roll back and clear the marker.
async fn evaluate_deployment(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
) -> anyhow::Result<()> {
    let phase = deployment_phase(dep);
    let name = dep.name_any();

    let is_unhealthy = matches!(
        phase,
        DeploymentPhase::Progressing | DeploymentPhase::Failed | DeploymentPhase::Degraded
    );

    // Case 1: back to healthy. Wipe the marker if present so a future
    // failure gets its own fresh grace window.
    if !is_unhealthy {
        if get_ann(dep, UNHEALTHY_SINCE_ANN).is_some_and(|s| !s.is_empty()) {
            clear_unhealthy_marker(state, ns, &name).await?;
        }
        return Ok(());
    }

    let now = jiff::Timestamp::now();

    // Case 2: first sighting of an unhealthy state.
    let since_str = match get_ann(dep, UNHEALTHY_SINCE_ANN) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            stamp_unhealthy_since(state, ns, &name, &now.to_string()).await?;
            return Ok(());
        }
    };

    // Parse the persisted timestamp. If it's malformed (e.g. someone edited
    // it by hand) reset it to now rather than firing a rollback on the
    // strength of a bogus timestamp.
    let since: jiff::Timestamp = match since_str.parse() {
        Ok(t) => t,
        Err(_) => {
            tracing::warn!(
                deployment = %name,
                stamp = %since_str,
                "auto_rollback: unparseable unhealthy-since; resetting"
            );
            stamp_unhealthy_since(state, ns, &name, &now.to_string()).await?;
            return Ok(());
        }
    };

    let elapsed = now.duration_since(since).as_secs();
    if elapsed < UNHEALTHY_GRACE_SECS {
        // Case 3: still within grace window.
        return Ok(());
    }

    // Case 4: fire the rollback. Skip silently if there's no earlier
    // revision — nothing to fall back to.
    let Some(target_revision) = find_previous_revision(state, ns, dep).await? else {
        tracing::info!(
            deployment = %name,
            "auto_rollback: no previous revision available; skipping"
        );
        // Clear the marker so we're not re-evaluating a permanently
        // un-rollbackable deployment every tick. If it goes healthy and
        // fails again, we'll re-arm.
        clear_unhealthy_marker(state, ns, &name).await?;
        return Ok(());
    };

    tracing::warn!(
        deployment = %name,
        namespace = %ns,
        target_revision,
        elapsed_secs = elapsed,
        phase = ?phase,
        "auto_rollback: firing rollback"
    );

    perform_rollback(state, ns, dep, target_revision).await?;
    Ok(())
}

/// Find the revision one below the deployment's current revision. Returns
/// None if the deployment has only ever had one revision (nothing to fall
/// back to) or if the current revision annotation is missing.
async fn find_previous_revision(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
) -> anyhow::Result<Option<i64>> {
    let current: i64 = dep
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(REVISION_ANNOTATION))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if current <= 1 {
        return Ok(None);
    }

    // Match by the deployment's selector, same as `deployments_ux::history`,
    // so we pick up RSes even when their ownerReference is missing (offline
    // restore case).
    let selector = dep
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();

    let rs_api: Api<ReplicaSet> = Api::namespaced(state.kube_client.clone(), ns);
    let rs_list = rs_api
        .list(&ListParams::default().labels(&selector))
        .await?;

    // Collect revisions strictly less than current, then take the max.
    let mut candidates: Vec<i64> = rs_list
        .items
        .iter()
        .filter_map(|rs| {
            let rev: i64 = rs
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(REVISION_ANNOTATION))
                .and_then(|s| s.parse().ok())?;
            // Skip empty (scaled-to-zero, no pod-template) RSes so we don't
            // roll back to a revision that has no containers to run.
            let has_template = rs
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.spec.as_ref())
                .is_some_and(|s| !s.containers.is_empty());
            if !has_template {
                return None;
            }
            if rev < current {
                Some(rev)
            } else {
                None
            }
        })
        .collect();
    candidates.sort_unstable();
    Ok(candidates.pop())
}

/// Patch the deployment's pod template back to the ReplicaSet at the given
/// revision. Mirrors `deployments_ux::rollback` but is invoked from the
/// watcher instead of an HTTP request, so we do all the work inline rather
/// than reusing that function (which returns an axum response type).
async fn perform_rollback(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
    target_revision: i64,
) -> anyhow::Result<()> {
    let name = dep.name_any();

    let selector = dep
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();

    let rs_api: Api<ReplicaSet> = Api::namespaced(state.kube_client.clone(), ns);
    let rs_list = rs_api
        .list(&ListParams::default().labels(&selector))
        .await?;

    let target = rs_list
        .items
        .into_iter()
        .find(|rs| {
            rs.metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(REVISION_ANNOTATION))
                .and_then(|s| s.parse::<i64>().ok())
                == Some(target_revision)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "auto_rollback: target revision {target_revision} disappeared between selection and patch"
            )
        })?;

    let target_pod_spec = target
        .spec
        .as_ref()
        .and_then(|s| s.template.as_ref())
        .and_then(|t| t.spec.as_ref())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "auto_rollback: target revision {target_revision} has no pod spec"
            )
        })?;

    let now = jiff::Timestamp::now().to_string();
    let change_cause = format!(
        "auto-rollback to revision {target_revision} via deckwatch (unhealthy > {UNHEALTHY_GRACE_SECS}s)"
    );

    // Strategic-merge patch: same shape as the manual rollback path. We also
    // stamp deckwatch bookkeeping annotations in the same call so the API
    // server sees a single write.
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "spec": target_pod_spec,
            },
        },
        "metadata": {
            "annotations": {
                CHANGE_CAUSE_ANNOTATION: change_cause,
                ann(LAST_AUTO_ROLLBACK_ANN): format!("{now}: revision {target_revision}"),
                // Clear the unhealthy-since marker so the new revision
                // starts with a fresh grace window rather than immediately
                // tripping the rollback again on the next tick.
                ann(UNHEALTHY_SINCE_ANN): "",
            },
        },
    });

    let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
    dep_api
        .patch(&name, &PatchParams::default(), &Patch::Strategic(patch))
        .await?;

    Ok(())
}

async fn stamp_unhealthy_since(
    state: &AppState,
    ns: &str,
    dep_name: &str,
    ts: &str,
) -> anyhow::Result<()> {
    let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                ann(UNHEALTHY_SINCE_ANN): ts,
            }
        }
    });
    dep_api
        .patch(dep_name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;
    Ok(())
}

async fn clear_unhealthy_marker(
    state: &AppState,
    ns: &str,
    dep_name: &str,
) -> anyhow::Result<()> {
    let dep_api: Api<Deployment> = Api::namespaced(state.kube_client.clone(), ns);
    // Empty string on a merge patch is Kubernetes' idiomatic way to remove
    // an annotation. `null` would also work but requires a strategic-merge
    // and a more invasive patch shape.
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                ann(UNHEALTHY_SINCE_ANN): "",
            }
        }
    });
    dep_api
        .patch(dep_name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;
    Ok(())
}

// ============================================================
// Integration notes for src/watcher.rs
// ============================================================
//
// In `run_poller`, after `monitor_builds(&state).await`, add:
//
//     if let Err(e) = auto_rollback_cycle(&state).await {
//         tracing::error!(error = %e, "watcher auto-rollback cycle failed");
//     }
//
// Existing helpers `ann` and `get_ann` are already `pub` in `watcher.rs`,
// so this module can call them without further visibility changes. The
// module name in the file it's dropped into should be `auto_rollback` so
// the caller sees `auto_rollback::auto_rollback_cycle`.
