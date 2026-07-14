use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, ExecAction, HTTPGetAction, Probe, ResourceRequirements,
    TCPSocketAction,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use serde::Deserialize;

use crate::audit;
use crate::error::AppError;
use crate::kube_ext::{
    deployment_detail, deployment_summary, ingress_summary, pod_summary, DeploymentDetail,
    DeploymentSummary, IngressSummary, PodSummary,
};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub label_selector: Option<String>,
}

#[derive(serde::Serialize)]
pub struct DeploymentListResponse {
    pub deployments: Vec<DeploymentSummary>,
}

#[derive(serde::Serialize, Debug)]
pub struct DeploymentDetailResponse {
    #[serde(flatten)]
    pub detail: DeploymentDetail,
    pub pods: Vec<PodSummary>,
    pub ingresses: Vec<IngressSummary>,
}

#[derive(Deserialize)]
pub struct CreateDeploymentRequest {
    pub name: String,
    pub image: String,
    pub replicas: Option<i32>,
    // Backward compat: older clients send a single `port`; newer clients send
    // `ports`. Both are accepted; `ports` wins if both are present.
    pub port: Option<i32>,
    pub ports: Option<Vec<PortInput>>,
    pub env: Option<Vec<EnvVarInput>>,
    pub labels: Option<BTreeMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub resource_limits: Option<ResourceSpec>,
    pub resource_requests: Option<ResourceSpec>,
    pub liveness_probe: Option<ProbeInput>,
    pub readiness_probe: Option<ProbeInput>,
    pub startup_probe: Option<ProbeInput>,
}

#[derive(Deserialize)]
pub struct UpdateDeploymentRequest {
    pub image: Option<String>,
    pub replicas: Option<i32>,
    pub port: Option<i32>,
    pub ports: Option<Vec<PortInput>>,
    pub env: Option<Vec<EnvVarInput>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub resource_limits: Option<ResourceSpec>,
    pub resource_requests: Option<ResourceSpec>,
    pub liveness_probe: Option<ProbeInput>,
    pub readiness_probe: Option<ProbeInput>,
    pub startup_probe: Option<ProbeInput>,
}

#[derive(Deserialize)]
pub struct PortInput {
    pub port: i32,
    pub name: Option<String>,
    pub protocol: Option<String>,
}

#[derive(Deserialize)]
pub struct EnvVarInput {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize)]
pub struct ResourceSpec {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Deserialize)]
pub struct ProbeInput {
    pub probe_type: String,
    pub path: Option<String>,
    pub port: Option<i32>,
    pub command: Option<Vec<String>>,
    pub initial_delay_seconds: Option<i32>,
    pub period_seconds: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub failure_threshold: Option<i32>,
    pub success_threshold: Option<i32>,
}

#[derive(Deserialize)]
pub struct ScaleRequest {
    pub replicas: i32,
}

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<DeploymentListResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let mut lp = ListParams::default();
    if let Some(selector) = query.label_selector {
        lp = lp.labels(&selector);
    }
    let t = K8sTimer::new("deployments", "list");
    let deps = api.list(&lp).await;
    t.finish(deps.is_ok());
    let deps = deps?;
    let deployments = deps.iter().map(deployment_summary).collect();
    Ok(Json(DeploymentListResponse { deployments }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let dep_api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = dep_api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;
    let detail = deployment_detail(&dep);

    let pods = list_pods_for_deployment(&state, &ns, &dep).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;

    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

pub async fn get_yaml(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let dep = api.get(&name).await;
    t.finish(dep.is_ok());
    let dep = dep?;
    let yaml = serde_yaml::to_string(&dep).map_err(|e| {
        AppError::BadRequest(format!("failed to serialize deployment as YAML: {e}"))
    })?;
    Ok(([(header::CONTENT_TYPE, "text/yaml; charset=utf-8")], yaml).into_response())
}

pub async fn update_yaml(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    body: String,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let mut dep: Deployment = serde_yaml::from_str(&body).map_err(|e| {
        let friendly = if let Some(loc) = e.location() {
            format!(
                "YAML parse error at line {}, column {}: {}",
                loc.line(),
                loc.column(),
                e
            )
        } else {
            format!("invalid YAML: {e}")
        };
        AppError::BadRequest(friendly)
    })?;

    // Ensure the resource identity matches the URL path — refuse mismatches so users
    // don't accidentally rename or move a Deployment via the YAML editor.
    if let Some(meta_name) = dep.metadata.name.as_deref() {
        if meta_name != name {
            return Err(AppError::BadRequest(format!(
                "metadata.name '{meta_name}' does not match URL path '{name}'"
            )));
        }
    } else {
        dep.metadata.name = Some(name.clone());
    }
    if let Some(meta_ns) = dep.metadata.namespace.as_deref() {
        if meta_ns != ns {
            return Err(AppError::BadRequest(format!(
                "metadata.namespace '{meta_ns}' does not match URL path '{ns}'"
            )));
        }
    } else {
        dep.metadata.namespace = Some(ns.clone());
    }

    let api = state.deployments_api(&ns)?;

    // Preserve the current resourceVersion so replace() succeeds even if the client
    // stripped it from the YAML they were editing.
    if dep.metadata.resource_version.is_none() {
        let t = K8sTimer::new("deployments", "get");
        let existing = api.get(&name).await;
        t.finish(existing.is_ok());
        let existing = existing?;
        dep.metadata.resource_version = existing.metadata.resource_version;
    }

    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;

    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentDetailResponse>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.image.is_empty() {
        return Err(AppError::BadRequest("image is required".to_string()));
    }

    let api = state.deployments_api(&ns)?;

    let mut labels = req.labels.unwrap_or_default();
    labels.insert("app".to_string(), req.name.clone());
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let env_vars: Option<Vec<EnvVar>> = req.env.map(|vars| {
        vars.into_iter()
            .map(|v| EnvVar {
                name: v.name,
                value: Some(v.value),
                ..Default::default()
            })
            .collect()
    });

    let ports = resolve_ports(req.port, req.ports);

    let resources = build_resources(req.resource_requests, req.resource_limits);

    let container = Container {
        name: req.name.clone(),
        image: Some(req.image),
        ports,
        env: env_vars,
        command: req.command,
        args: req.args,
        resources,
        liveness_probe: req.liveness_probe.map(build_probe),
        readiness_probe: req.readiness_probe.map(build_probe),
        startup_probe: req.startup_probe.map(build_probe),
        ..Default::default()
    };

    let deployment = Deployment {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.clone()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(req.replicas.unwrap_or(1)),
            selector: LabelSelector {
                match_labels: Some(BTreeMap::from([("app".to_string(), req.name.clone())])),
                ..Default::default()
            },
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let t = K8sTimer::new("deployments", "create");
    let created = api.create(&PostParams::default(), &deployment).await;
    t.finish(created.is_ok());
    let created = created?;
    let detail = deployment_detail(&created);

    if let Err(e) = audit::log_action(
        &state.db,
        "create",
        "deployment",
        &req.name,
        &ns,
        &format!("created deployment with image {}", detail.image),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok((
        StatusCode::CREATED,
        Json(DeploymentDetailResponse {
            detail,
            pods: vec![],
            ingresses: vec![],
        }),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<UpdateDeploymentRequest>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;

    let mut dep = existing.clone();

    // Only overwrite container.ports when the caller explicitly said something
    // about ports — otherwise we'd wipe out ports set through the YAML editor.
    let ports_specified = req.port.is_some() || req.ports.is_some();
    let resolved_ports = if ports_specified {
        resolve_ports(req.port, req.ports)
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
                container.resources = build_resources(req.resource_requests, req.resource_limits);
                if let Some(probe) = req.liveness_probe {
                    container.liveness_probe = Some(build_probe(probe));
                }
                if let Some(probe) = req.readiness_probe {
                    container.readiness_probe = Some(build_probe(probe));
                }
                if let Some(probe) = req.startup_probe {
                    container.startup_probe = Some(build_probe(probe));
                }
            }
        }
    }

    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;

    if let Err(e) = audit::log_action(
        &state.db,
        "update",
        "deployment",
        &name,
        &ns,
        &format!("updated deployment (image: {})", detail.image),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;

    if let Err(e) = audit::log_action(
        &state.db,
        "delete",
        "deployment",
        &name,
        &ns,
        "deleted deployment",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn restart(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let api = state.deployments_api(&ns)?;
    let now = jiff::Timestamp::now().to_string();
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "metadata": {
                    "annotations": {
                        "kubectl.kubernetes.io/restartedAt": now
                    }
                }
            }
        }
    });
    let t = K8sTimer::new("deployments", "patch");
    let res = api
        .patch(&name, &PatchParams::default(), &Patch::Strategic(patch))
        .await;
    t.finish(res.is_ok());
    res?;

    if let Err(e) = audit::log_action(
        &state.db,
        "restart",
        "deployment",
        &name,
        &ns,
        "initiated rolling restart",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(
        serde_json::json!({"message": "rolling restart initiated"}),
    ))
}

pub async fn scale(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<ScaleRequest>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let patch = serde_json::json!({
        "spec": {
            "replicas": req.replicas
        }
    });
    let t = K8sTimer::new("deployments", "patch");
    let dep = api
        .patch(&name, &PatchParams::default(), &Patch::Merge(patch))
        .await;
    t.finish(dep.is_ok());
    let dep = dep?;
    let detail = deployment_detail(&dep);
    let pods = list_pods_for_deployment(&state, &ns, &dep).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;

    if let Err(e) = audit::log_action(
        &state.db,
        "scale",
        "deployment",
        &name,
        &ns,
        &format!("scaled to {} replicas", req.replicas),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

async fn list_pods_for_deployment(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
) -> Result<Vec<PodSummary>, AppError> {
    let pods_api = state.pods_api(ns)?;
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
    let lp = ListParams::default().labels(&selector);
    let t = K8sTimer::new("pods", "list");
    let pods = pods_api.list(&lp).await;
    t.finish(pods.is_ok());
    let pods = pods?;
    Ok(pods.iter().map(pod_summary).collect())
}

// Merge the legacy single-`port` field with the newer `ports` array into a
// single Vec<ContainerPort> for the k8s API. `ports` wins if both are given.
// Protocol values are uppercased because the k8s API rejects lowercase.
pub(crate) fn resolve_ports(
    legacy: Option<i32>,
    ports: Option<Vec<PortInput>>,
) -> Option<Vec<ContainerPort>> {
    let inputs = match (ports, legacy) {
        (Some(list), _) => list,
        (None, Some(p)) => vec![PortInput {
            port: p,
            name: None,
            protocol: None,
        }],
        (None, None) => return None,
    };

    if inputs.is_empty() {
        return None;
    }

    Some(
        inputs
            .into_iter()
            .map(|p| ContainerPort {
                container_port: p.port,
                name: p.name.filter(|s| !s.is_empty()),
                protocol: p
                    .protocol
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_uppercase()),
                ..Default::default()
            })
            .collect(),
    )
}

pub(crate) fn build_resources(
    requests: Option<ResourceSpec>,
    limits: Option<ResourceSpec>,
) -> Option<ResourceRequirements> {
    let req_map = requests.map(|r| {
        let mut m = BTreeMap::new();
        if let Some(cpu) = r.cpu {
            m.insert("cpu".to_string(), Quantity(cpu));
        }
        if let Some(memory) = r.memory {
            m.insert("memory".to_string(), Quantity(memory));
        }
        m
    });

    let lim_map = limits.map(|l| {
        let mut m = BTreeMap::new();
        if let Some(cpu) = l.cpu {
            m.insert("cpu".to_string(), Quantity(cpu));
        }
        if let Some(memory) = l.memory {
            m.insert("memory".to_string(), Quantity(memory));
        }
        m
    });

    if req_map.is_some() || lim_map.is_some() {
        Some(ResourceRequirements {
            requests: req_map,
            limits: lim_map,
            ..Default::default()
        })
    } else {
        None
    }
}

async fn list_ingresses_for_service(
    state: &AppState,
    ns: &str,
    service_name: &str,
) -> Result<Vec<IngressSummary>, AppError> {
    let ing_api = state.ingresses_api(ns)?;
    let t = K8sTimer::new("ingresses", "list");
    let all = ing_api.list(&ListParams::default()).await;
    t.finish(all.is_ok());
    let all = all?;
    let matching: Vec<IngressSummary> = all
        .iter()
        .filter(|ing| {
            ing.spec
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
                                        .map(|s| s.name == service_name)
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .map(ingress_summary)
        .collect();
    Ok(matching)
}

pub(crate) fn build_probe(input: ProbeInput) -> Probe {
    let mut probe = Probe {
        initial_delay_seconds: input.initial_delay_seconds,
        period_seconds: input.period_seconds,
        timeout_seconds: input.timeout_seconds,
        failure_threshold: input.failure_threshold,
        success_threshold: input.success_threshold,
        ..Default::default()
    };

    let port = input.port.unwrap_or(80);

    match input.probe_type.as_str() {
        "httpGet" => {
            probe.http_get = Some(HTTPGetAction {
                path: input.path,
                port: IntOrString::Int(port),
                ..Default::default()
            });
        }
        "tcpSocket" => {
            probe.tcp_socket = Some(TCPSocketAction {
                port: IntOrString::Int(port),
                ..Default::default()
            });
        }
        "exec" => {
            probe.exec = Some(ExecAction {
                command: input.command,
            });
        }
        _ => {}
    }

    probe
}

// ============================================================
// Probe update endpoint
// ============================================================

#[derive(Deserialize)]
pub struct UpdateProbesRequest {
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub liveness_probe: Option<Option<ProbeInput>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub readiness_probe: Option<Option<ProbeInput>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub startup_probe: Option<Option<ProbeInput>>,
}

fn deserialize_optional_field<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

pub async fn update_probes(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<UpdateProbesRequest>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let container_name = existing
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first())
        .map(|c| c.name.clone())
        .ok_or_else(|| AppError::BadRequest("deployment has no primary container".to_string()))?;

    let mut container_patch = serde_json::json!({ "name": container_name });
    let obj = container_patch.as_object_mut().unwrap();
    if let Some(v) = req.liveness_probe {
        obj.insert(
            "livenessProbe".to_string(),
            match v {
                Some(p) => serde_json::to_value(build_probe(p))
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
                None => serde_json::Value::Null,
            },
        );
    }
    if let Some(v) = req.readiness_probe {
        obj.insert(
            "readinessProbe".to_string(),
            match v {
                Some(p) => serde_json::to_value(build_probe(p))
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
                None => serde_json::Value::Null,
            },
        );
    }
    if let Some(v) = req.startup_probe {
        obj.insert(
            "startupProbe".to_string(),
            match v {
                Some(p) => serde_json::to_value(build_probe(p))
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
                None => serde_json::Value::Null,
            },
        );
    }

    let patch = serde_json::json!({
        "spec": { "template": { "spec": { "containers": [container_patch] } } }
    });

    let t = K8sTimer::new("deployments", "patch");
    let updated = api
        .patch(&name, &PatchParams::default(), &Patch::Strategic(patch))
        .await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

// ============================================================
// Custom sidecar container endpoints
// ============================================================

#[derive(Deserialize)]
pub struct AddContainerRequest {
    pub name: String,
    pub image: String,
    pub port: Option<i32>,
    pub env: Option<Vec<EnvVarInput>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub resource_limits: Option<ResourceSpec>,
    pub resource_requests: Option<ResourceSpec>,
}

pub async fn add_container(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<AddContainerRequest>,
) -> Result<(StatusCode, Json<DeploymentDetailResponse>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest(
            "container name is required".to_string(),
        ));
    }
    if req.image.is_empty() {
        return Err(AppError::BadRequest(
            "container image is required".to_string(),
        ));
    }
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut dep = existing.clone();
    let pod_spec = dep
        .spec
        .as_mut()
        .and_then(|s| s.template.spec.as_mut())
        .ok_or_else(|| AppError::BadRequest("deployment has no pod spec".to_string()))?;
    if pod_spec.containers.iter().any(|c| c.name == req.name) {
        return Err(AppError::BadRequest(format!(
            "container '{}' already exists",
            req.name
        )));
    }
    let env_vars: Option<Vec<EnvVar>> = req.env.map(|vars| {
        vars.into_iter()
            .map(|v| EnvVar {
                name: v.name,
                value: Some(v.value),
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
    pod_spec.containers.push(Container {
        name: req.name,
        image: Some(req.image),
        ports,
        env: env_vars,
        command: req.command,
        args: req.args,
        resources: build_resources(req.resource_requests, req.resource_limits),
        ..Default::default()
    });
    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok((
        StatusCode::CREATED,
        Json(DeploymentDetailResponse {
            detail,
            pods,
            ingresses,
        }),
    ))
}

pub async fn remove_container(
    State(state): State<AppState>,
    Path((ns, name, container_name)): Path<(String, String, String)>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut dep = existing.clone();
    let pod_spec = dep
        .spec
        .as_mut()
        .and_then(|s| s.template.spec.as_mut())
        .ok_or_else(|| AppError::BadRequest("deployment has no pod spec".to_string()))?;
    if pod_spec.containers.is_empty() {
        return Err(AppError::BadRequest("no containers to remove".to_string()));
    }
    if pod_spec.containers[0].name == container_name {
        return Err(AppError::BadRequest(
            "cannot remove primary container".to_string(),
        ));
    }
    let before = pod_spec.containers.len();
    pod_spec.containers.retain(|c| c.name != container_name);
    if pod_spec.containers.len() == before {
        return Err(AppError::NotFound(format!(
            "container '{container_name}' not found"
        )));
    }
    if let Some(annotations) = dep
        .spec
        .as_mut()
        .and_then(|s| s.template.metadata.as_mut())
        .and_then(|m| m.annotations.as_mut())
    {
        annotations.remove(&format!("deckwatch.addon/{container_name}"));
    }
    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

#[cfg(test)]
#[path = "../handlers_deployments_tests.rs"]
mod handlers_deployments_tests;
