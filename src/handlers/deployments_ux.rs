// Deployment UX endpoints — history, rollback, validate (dry-run), clone.
//
// These functions are designed to be appended to
// `src/handlers/deployments.rs` (or dropped in a sibling module). They reuse
// the existing `AppState`, error types, and the `deployment_detail` helper
// from `kube_ext`. The integration file
// `/tmp/deckwatch-staging/ux-features/backend/src/routes_additions.rs`
// shows the exact routes to register.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use k8s_openapi::api::core::v1::EnvVar;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::deployments::{
    CreateDeploymentRequest, DeploymentDetailResponse, UpdateDeploymentRequest,
};
use crate::kube_ext::deployment_detail;
use crate::state::AppState;

// ============================================================
// Deployment history — list ReplicaSets owned by the Deployment
// ============================================================

/// Annotation Kubernetes writes on each ReplicaSet with its numeric revision.
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
/// Optional free-form change cause written by `kubectl rollout ... --record`
/// and by our own /rollback endpoint.
const CHANGE_CAUSE_ANNOTATION: &str = "kubernetes.io/change-cause";

#[derive(Serialize)]
pub struct RevisionSummary {
    pub revision: i64,
    pub replica_set_name: String,
    pub image: String,
    pub replicas: i32,
    pub ready_replicas: i32,
    pub created_at: Option<String>,
    pub change_cause: Option<String>,
    /// True when this revision matches the Deployment's current spec — i.e.
    /// this is the ReplicaSet currently receiving traffic. Callers should
    /// hide the "roll back to this" button on the current revision.
    pub is_current: bool,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub revisions: Vec<RevisionSummary>,
}

pub async fn history(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<HistoryResponse>, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let dep = dep_api.get(&name).await?;

    // Match ReplicaSets by the deployment's selector. Filtering here (rather
    // than by ownerReference) matches what `kubectl rollout history` does and
    // lets us find revisions even if the RS metadata is missing an
    // ownerReference (e.g. after an offline restore).
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

    let rs_api: kube::Api<ReplicaSet> = kube::Api::namespaced(state.kube_client.clone(), &ns);
    let lp = ListParams::default().labels(&selector);
    let rs_list = rs_api.list(&lp).await?;

    let current_pod_template_hash = dep
        .spec
        .as_ref()
        .and_then(|s| s.template.metadata.as_ref())
        .and_then(|m| m.labels.as_ref())
        .and_then(|l| l.get("pod-template-hash"))
        .cloned();

    let mut revisions: Vec<RevisionSummary> = rs_list
        .items
        .iter()
        .filter_map(|rs| replica_set_to_revision(rs, current_pod_template_hash.as_deref()))
        .collect();

    // Newest revision first — this is what operators expect from
    // `kubectl rollout history`.
    revisions.sort_by_key(|r| std::cmp::Reverse(r.revision));

    Ok(Json(HistoryResponse { revisions }))
}

fn replica_set_to_revision(rs: &ReplicaSet, current_hash: Option<&str>) -> Option<RevisionSummary> {
    let annotations = rs.metadata.annotations.as_ref()?;
    let revision: i64 = annotations.get(REVISION_ANNOTATION)?.parse().ok()?;

    let image = rs
        .spec
        .as_ref()
        .and_then(|s| s.template.as_ref())
        .and_then(|t| t.spec.as_ref())
        .and_then(|s| s.containers.first())
        .map(|c| c.image.clone().unwrap_or_default())
        .unwrap_or_default();

    let is_current = match (current_hash, rs.metadata.labels.as_ref()) {
        (Some(hash), Some(labels)) => {
            labels.get("pod-template-hash").map(|s| s.as_str()) == Some(hash)
        }
        _ => false,
    };

    Some(RevisionSummary {
        revision,
        replica_set_name: rs.metadata.name.clone().unwrap_or_default(),
        image,
        replicas: rs.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0),
        ready_replicas: rs
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0),
        created_at: rs
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
        change_cause: annotations.get(CHANGE_CAUSE_ANNOTATION).cloned(),
        is_current,
    })
}

// ============================================================
// Rollback — patch the Deployment's pod template back to a prior RS
// ============================================================

#[derive(Deserialize)]
pub struct RollbackRequest {
    pub revision: i64,
}

pub async fn rollback(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let dep = dep_api.get(&name).await?;

    // Find the target ReplicaSet by revision annotation. We match against
    // the same selector as history() so we don't accidentally pick up a
    // stray RS that shares only some labels.
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

    let rs_api: kube::Api<ReplicaSet> = kube::Api::namespaced(state.kube_client.clone(), &ns);
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
                == Some(req.revision)
        })
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "revision {} not found for deployment '{name}'",
                req.revision
            ))
        })?;

    let target_pod_spec = target
        .spec
        .as_ref()
        .and_then(|s| s.template.as_ref())
        .and_then(|t| t.spec.as_ref())
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "revision {} has no pod spec to roll back to",
                req.revision
            ))
        })?;

    // Strategic-merge patch: replace the pod spec wholesale and stamp a
    // change-cause annotation so the new revision is legible in history.
    // We deliberately preserve the deployment's `spec.replicas` — a rollback
    // should not resurrect the replica count that was in effect at the time
    // of the target revision, only its container spec.
    let change_cause = format!("rollback to revision {} via deckwatch", req.revision);
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "spec": target_pod_spec,
            },
        },
        "metadata": {
            "annotations": {
                CHANGE_CAUSE_ANNOTATION: change_cause,
            },
        },
    });

    let updated = dep_api
        .patch(&name, &PatchParams::default(), &Patch::Strategic(patch))
        .await?;

    Ok(Json(build_detail_only(&updated)))
}

// ============================================================
// Dry-run validation
// ============================================================

#[derive(Serialize)]
pub struct ValidateResponse {
    pub ok: bool,
    pub errors: Vec<String>,
}

pub async fn validate(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateDeploymentRequest>,
) -> Result<Json<ValidateResponse>, AppError> {
    let dep = build_deployment_from_request(&ns, &req)?;
    let api = state.deployments_api(&ns)?;

    // `dry_run()` sets `?dryRun=All`, which runs full admission (including
    // webhooks) but does not persist. This surfaces most policy failures
    // (quota, PSA, OPA) at edit time rather than after the operator hits
    // Create.
    let pp = PostParams {
        dry_run: true,
        ..Default::default()
    };

    match api.create(&pp, &dep).await {
        Ok(_) => Ok(Json(ValidateResponse {
            ok: true,
            errors: vec![],
        })),
        Err(kube::Error::Api(err)) => Ok(Json(ValidateResponse {
            ok: false,
            errors: split_admission_errors(&err.message),
        })),
        Err(other) => Err(AppError::Kube(other)),
    }
}

fn split_admission_errors(message: &str) -> Vec<String> {
    // Admission controllers often return newline- or semicolon-separated
    // strings inside a single message. Split so the frontend can render one
    // chip per failure without a bespoke parser.
    message
        .split(['\n', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ============================================================
// Clone — copy an existing deployment to another namespace / new name
// ============================================================

#[derive(Deserialize)]
pub struct CloneRequest {
    pub target_namespace: String,
    pub new_name: Option<String>,
    /// When true and a deployment with the target name already exists in the
    /// target namespace, it is deleted first. Defaults to false so the
    /// operator has to opt into clobbering.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Serialize)]
pub struct CloneResponse {
    #[serde(flatten)]
    pub detail: DeploymentDetailResponse,
    pub source_namespace: String,
    pub source_name: String,
    pub target_namespace: String,
    pub target_name: String,
}

pub async fn clone(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<CloneRequest>,
) -> Result<(StatusCode, Json<CloneResponse>), AppError> {
    let target_ns = req.target_namespace.trim();
    if target_ns.is_empty() {
        return Err(AppError::BadRequest(
            "target_namespace is required".to_string(),
        ));
    }
    let target_name = req
        .new_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&name)
        .to_string();

    if target_ns == ns && target_name == name {
        return Err(AppError::BadRequest(
            "clone target is identical to the source; specify a different namespace or new_name"
                .to_string(),
        ));
    }

    let src_api = state.deployments_api(&ns)?;
    let source = src_api.get(&name).await?;

    let mut cloned = source.clone();
    let selector_labels: BTreeMap<String, String> =
        BTreeMap::from([("app".to_string(), target_name.clone())]);

    // Rewrite identity + strip all cluster-managed metadata so `create()`
    // is accepted. We also rewrite the selector to key on the new name —
    // reusing the source's selector would let two Deployments fight over
    // the same pods if they land in the same namespace.
    cloned.metadata = ObjectMeta {
        name: Some(target_name.clone()),
        namespace: Some(target_ns.to_string()),
        labels: source.metadata.labels.clone(),
        annotations: source
            .metadata
            .annotations
            .clone()
            .map(strip_managed_annotations),
        ..Default::default()
    };
    cloned.status = None;

    if let Some(spec) = cloned.spec.as_mut() {
        spec.selector.match_labels = Some(selector_labels.clone());
        if let Some(meta) = spec.template.metadata.as_mut() {
            let labels = meta.labels.get_or_insert_with(BTreeMap::new);
            labels.insert("app".to_string(), target_name.clone());
        }
    }

    let dst_api = state.deployments_api(target_ns)?;

    if req.overwrite {
        // Best-effort delete; if it wasn't there, create still succeeds.
        let _ = dst_api.delete(&target_name, &DeleteParams::default()).await;
    }

    let created = dst_api.create(&PostParams::default(), &cloned).await?;

    Ok((
        StatusCode::CREATED,
        Json(CloneResponse {
            detail: build_detail_only(&created),
            source_namespace: ns,
            source_name: name,
            target_namespace: target_ns.to_string(),
            target_name,
        }),
    ))
}

/// Kubernetes writes a handful of `deployment.kubernetes.io/*` annotations
/// (revision, revision-history, desired-replicas) that only make sense in
/// the source cluster/deployment. Carrying them into the clone would confuse
/// the rollout controller into thinking the clone already has history.
fn strip_managed_annotations(mut a: BTreeMap<String, String>) -> BTreeMap<String, String> {
    a.retain(|k, _| !k.starts_with("deployment.kubernetes.io/"));
    a
}

// ============================================================
// Shared: build a minimal DeploymentDetailResponse without a second round-trip
// ============================================================

fn build_detail_only(dep: &Deployment) -> DeploymentDetailResponse {
    // The response envelope requires pods/ingresses, but for rollback/clone
    // the caller almost always re-fetches the detail after the mutation
    // completes (pods take a few seconds to reconcile anyway). Returning
    // empty lists keeps the endpoint fast; the caller triggers a refresh.
    DeploymentDetailResponse {
        detail: deployment_detail(dep),
        pods: vec![],
        ingresses: vec![],
    }
}

// ============================================================
// Deployment-builder used by validate()
// ============================================================
//
// This is intentionally a *copy* of the logic in `deployments::create`
// rather than a shared helper, because refactoring `create` to expose a
// pure builder is out of scope for this change. If you're moving this into
// `deployments.rs`, replace this fn with a call to the private builder
// and delete this section.

fn build_deployment_from_request(
    ns: &str,
    req: &CreateDeploymentRequest,
) -> Result<Deployment, AppError> {
    use k8s_openapi::api::apps::v1::DeploymentSpec;
    use k8s_openapi::api::core::v1::{
        Container, ContainerPort, EnvVar, ExecAction, HTTPGetAction, PodSpec, PodTemplateSpec,
        Probe, ResourceRequirements, TCPSocketAction,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.image.is_empty() {
        return Err(AppError::BadRequest("image is required".to_string()));
    }

    let mut labels = req.labels.clone().unwrap_or_default();
    labels.insert("app".to_string(), req.name.clone());
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let env_vars: Option<Vec<EnvVar>> = req.env.as_ref().map(|vars| {
        vars.iter()
            .map(|v| EnvVar {
                name: v.name.clone(),
                value: Some(v.value.clone()),
                ..Default::default()
            })
            .collect()
    });

    let ports: Option<Vec<ContainerPort>> = req.port.map(|p| {
        vec![ContainerPort {
            container_port: p,
            ..Default::default()
        }]
    });

    let build_resource_map = |spec: &crate::handlers::deployments::ResourceSpec| {
        let mut m = BTreeMap::new();
        if let Some(cpu) = spec.cpu.as_ref() {
            m.insert("cpu".to_string(), Quantity(cpu.clone()));
        }
        if let Some(mem) = spec.memory.as_ref() {
            m.insert("memory".to_string(), Quantity(mem.clone()));
        }
        m
    };
    let resources = if req.resource_requests.is_some() || req.resource_limits.is_some() {
        Some(ResourceRequirements {
            requests: req.resource_requests.as_ref().map(build_resource_map),
            limits: req.resource_limits.as_ref().map(build_resource_map),
            ..Default::default()
        })
    } else {
        None
    };

    let build_probe = |input: &crate::handlers::deployments::ProbeInput| -> Probe {
        let port = input.port.unwrap_or(80);
        let mut p = Probe {
            initial_delay_seconds: input.initial_delay_seconds,
            period_seconds: input.period_seconds,
            timeout_seconds: input.timeout_seconds,
            failure_threshold: input.failure_threshold,
            success_threshold: input.success_threshold,
            ..Default::default()
        };
        match input.probe_type.as_str() {
            "httpGet" => {
                p.http_get = Some(HTTPGetAction {
                    path: input.path.clone(),
                    port: IntOrString::Int(port),
                    ..Default::default()
                });
            }
            "tcpSocket" => {
                p.tcp_socket = Some(TCPSocketAction {
                    port: IntOrString::Int(port),
                    ..Default::default()
                });
            }
            "exec" => {
                p.exec = Some(ExecAction {
                    command: input.command.clone(),
                });
            }
            _ => {}
        }
        p
    };

    let container = Container {
        name: req.name.clone(),
        image: Some(req.image.clone()),
        ports,
        env: env_vars,
        command: req.command.clone(),
        args: req.args.clone(),
        resources,
        liveness_probe: req.liveness_probe.as_ref().map(build_probe),
        readiness_probe: req.readiness_probe.as_ref().map(build_probe),
        startup_probe: req.startup_probe.as_ref().map(build_probe),
        ..Default::default()
    };

    Ok(Deployment {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(req.replicas.unwrap_or(1)),
            selector: LabelSelector {
                match_labels: Some(BTreeMap::from([("app".to_string(), req.name.clone())])),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    })
}
// Append this to `src/handlers/deployments_ux.rs`.
//
// Also register the route in `src/routes.rs`:
//
//     .route(
//         "/api/namespaces/{ns}/deployments/{name}/history/{revision}/yaml",
//         get(deployments_ux::revision_yaml),
//     )
//
// The handler returns the pod template spec of the ReplicaSet at the given
// revision as YAML — not the whole ReplicaSet — because operators comparing
// revisions almost always want to see container/image/env/resource diffs,
// not the RS-level status/replicas that Kubernetes rewrites on every scale.

use axum::http::header;
use axum::response::{IntoResponse, Response};

pub async fn revision_yaml(
    State(state): State<AppState>,
    Path((ns, name, revision)): Path<(String, String, i64)>,
) -> Result<Response, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let dep = dep_api.get(&name).await?;

    // Reuse the same selector strategy as history() so an operator comparing
    // two revisions from the history table sees the exact same set of RSes
    // the table was built from — including RSes with a missing
    // ownerReference (offline restore) that ownerRef filtering would skip.
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

    let rs_api: kube::Api<ReplicaSet> = kube::Api::namespaced(state.kube_client.clone(), &ns);
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
                == Some(revision)
        })
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "revision {revision} not found for deployment '{name}'"
            ))
        })?;

    // Serialize the pod template only. This is what actually changes between
    // revisions — containers, images, env, resources, probes. Including the
    // RS metadata (name, uid, creationTimestamp, status.readyReplicas) would
    // drown real diffs in noise that flips on every scale event.
    let template = target
        .spec
        .as_ref()
        .and_then(|s| s.template.as_ref())
        .ok_or_else(|| AppError::BadRequest(format!("revision {revision} has no pod template")))?;

    let yaml = serde_yaml::to_string(template).map_err(|e| {
        AppError::BadRequest(format!(
            "failed to serialize revision {revision} as YAML: {e}"
        ))
    })?;

    Ok(([(header::CONTENT_TYPE, "text/yaml; charset=utf-8")], yaml).into_response())
}

// PUT-path (edit) dry-run validation for existing deployments.
//
// Companion to `deployments_ux::validate`, which dry-runs a *create*. This
// one merges the update request into the live deployment and dry-runs a
// *replace*, so operators editing an existing deployment can catch admission
// failures (quota, PSA, OPA) before hitting Save.
//
// This file is intended to be appended to
// `src/handlers/deployments_ux.rs`. It re-uses `ValidateResponse` and
// `split_admission_errors` from that module.

/// `POST /api/namespaces/{ns}/deployments/{name}/validate`
///
/// Fetches the current Deployment, applies the fields present in the request
/// on top of its spec (matching the semantics of `deployments::update`), and
/// asks the API server to validate the result via `?dryRun=All`. Nothing is
/// persisted. Returns the same `ValidateResponse` shape as the create-path
/// validator so the frontend can render it uniformly.
pub async fn validate_update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<UpdateDeploymentRequest>,
) -> Result<Json<ValidateResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let existing = api.get(&name).await?;
    let candidate = apply_update(existing, req);

    // `replace()` with dry_run=true runs the full admission chain (validating
    // webhooks, PSA, quota) without persisting. We deliberately use replace
    // rather than patch here so the dry-run models what `deployments::update`
    // will actually send at Save time.
    let pp = PostParams {
        dry_run: true,
        ..Default::default()
    };

    match api.replace(&name, &pp, &candidate).await {
        Ok(_) => Ok(Json(ValidateResponse {
            ok: true,
            errors: vec![],
        })),
        Err(kube::Error::Api(err)) => Ok(Json(ValidateResponse {
            ok: false,
            errors: split_admission_errors(&err.message),
        })),
        Err(other) => Err(AppError::Kube(other)),
    }
}

/// Mirror of the mutation `deployments::update` performs, but pure —
/// takes the live Deployment and the requested changes and returns the
/// candidate object that would be sent to the API server.
///
/// Kept in sync with `deployments::update` by only touching fields the
/// update handler also touches. If a new field is added there, add it here
/// too or the dry-run will silently diverge from the real save.
fn apply_update(mut dep: Deployment, req: UpdateDeploymentRequest) -> Deployment {
    let ports_specified = req.port.is_some() || req.ports.is_some();
    let resolved_ports = if ports_specified {
        crate::handlers::deployments::resolve_ports(req.port, req.ports)
    } else {
        None
    };

    if let Some(spec) = dep.spec.as_mut() {
        if let Some(replicas) = req.replicas {
            spec.replicas = Some(replicas);
        }

        if let Some(pod_spec) = spec.template.spec.as_mut() {
            if let Some(container) = pod_spec.containers.first_mut() {
                if let Some(image) = req.image {
                    container.image = Some(image);
                }
                if let Some(env) = req.env {
                    container.env = Some(
                        env.into_iter()
                            .map(|v| EnvVar {
                                name: v.name,
                                value: Some(v.value),
                                ..Default::default()
                            })
                            .collect(),
                    );
                }
                if let Some(command) = req.command {
                    container.command = Some(command);
                }
                if let Some(args) = req.args {
                    container.args = Some(args);
                }
                if ports_specified {
                    container.ports = resolved_ports;
                }
                container.resources = crate::handlers::deployments::build_resources(
                    req.resource_requests,
                    req.resource_limits,
                );
                if let Some(probe) = req.liveness_probe {
                    container.liveness_probe =
                        Some(crate::handlers::deployments::build_probe(probe));
                }
                if let Some(probe) = req.readiness_probe {
                    container.readiness_probe =
                        Some(crate::handlers::deployments::build_probe(probe));
                }
                if let Some(probe) = req.startup_probe {
                    container.startup_probe =
                        Some(crate::handlers::deployments::build_probe(probe));
                }
            }
        }
    }

    dep
}

// ============================================================
// Notes for integrators
// ============================================================
//
// 1. The helpers `resolve_ports`, `build_resources`, and `build_probe` in
//    `src/handlers/deployments.rs` are currently private (no `pub`). Change
//    them to `pub(crate)` so this file can call them without duplication.
//
// 2. `split_admission_errors` and `ValidateResponse` are currently private
//    inside `deployments_ux.rs`. Mark them `pub(crate)` too. `ValidateResponse`
//    is also referenced by the create-path validator, so this is a minimal
//    visibility widening.
//
// 3. Register the route in `src/routes.rs`:
//
//        .route(
//            "/api/namespaces/{ns}/deployments/{name}/validate",
//            post(deployments_ux::validate_update),
//        )
//
//    Note the URL includes `{name}` — this is what distinguishes it from
//    the existing create-path validator at
//    `/api/namespaces/{ns}/deployments/validate`. Axum can route both since
//    the create route has no `{name}` segment.

// Auto-rollback toggle endpoint.
//
// The auto-rollback behaviour is driven by the `deckwatch.io/auto-rollback`
// annotation on the Deployment object. That annotation could be set via
// `kubectl annotate`, the YAML editor, or by the frontend toggle wired to
// this endpoint. Existing edit-form fields (image, replicas, probes) never
// touch annotations, so we expose the toggle as its own small route rather
// than folding it into `deployments::update`.
//
// This file is intended to be appended to `src/handlers/deployments_ux.rs`.

/// Same annotation key the watcher's `auto_rollback` module reads.
/// Duplicated as a string here (rather than importing) so the handler stays
/// self-contained; the watcher is the source of truth for the *behaviour*,
/// this endpoint just flips the switch.
const AUTO_ROLLBACK_ANNOTATION: &str = "deckwatch.io/auto-rollback";
const UNHEALTHY_SINCE_ANNOTATION: &str = "deckwatch.io/unhealthy-since";

#[derive(Deserialize)]
pub struct AutoRollbackRequest {
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct AutoRollbackResponse {
    pub enabled: bool,
}

/// `POST /api/namespaces/{ns}/deployments/{name}/auto-rollback`
///
/// Flips the `deckwatch.io/auto-rollback` annotation. Turning it off also
/// clears any in-flight `unhealthy-since` marker so re-enabling doesn't
/// resume a stale grace-window countdown.
pub async fn set_auto_rollback(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<AutoRollbackRequest>,
) -> Result<Json<AutoRollbackResponse>, AppError> {
    let api = state.deployments_api(&ns)?;

    // Empty string is Kubernetes' idiomatic delete-via-merge-patch. When
    // disabling we scrub both annotations at once so the watcher won't
    // find a live `unhealthy-since` on the next re-enable.
    let (auto_val, unhealthy_val) = if req.enabled {
        ("true", None)
    } else {
        ("", Some(""))
    };

    let mut annotations = serde_json::Map::new();
    annotations.insert(
        AUTO_ROLLBACK_ANNOTATION.to_string(),
        serde_json::Value::String(auto_val.to_string()),
    );
    if let Some(v) = unhealthy_val {
        annotations.insert(
            UNHEALTHY_SINCE_ANNOTATION.to_string(),
            serde_json::Value::String(v.to_string()),
        );
    }

    let patch = serde_json::json!({
        "metadata": {
            "annotations": annotations,
        }
    });

    api.patch(&name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;

    Ok(Json(AutoRollbackResponse {
        enabled: req.enabled,
    }))
}

// ============================================================
// Integration notes
// ============================================================
//
// Register in `src/routes.rs` alongside the other deployments_ux routes:
//
//     .route(
//         "/api/namespaces/{ns}/deployments/{name}/auto-rollback",
//         post(deployments_ux::set_auto_rollback),
//     )
//
// The frontend reads the current state directly from the Deployment's
// `annotations` map (already surfaced in DeploymentDetail via
// `kube_ext::deployment_detail`) — no separate GET endpoint is needed.

#[cfg(test)]
#[path = "../handlers_deployments_ux_tests.rs"]
mod handlers_deployments_ux_tests;
