//! Handlers for toggling Prometheus PodMonitor scraping per Deployment.
//!
//! This wires the design in `docs/PROMETHEUS_INTEGRATION.md` slice A:
//! deckwatch materializes a PodMonitor CRD in the same namespace as the
//! target Deployment, matching the Deployment's pod selector, so the
//! prometheus-operator picks it up automatically. On delete or when the
//! parent Deployment is garbage collected, the ownerReference cleans up.
//!
//! Graceful degrade: if the `monitoring.coreos.com/v1` CRD is not
//! installed, every endpoint short-circuits to 503 with a machine-readable
//! `unavailable_reason` field so the UI can render a helpful callout
//! instead of a red banner. Same pattern as `resource_metrics.rs`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{DeleteParams, DynamicObject, GroupVersionKind, Patch, PatchParams};
use kube::core::TypeMeta;
use kube::discovery;
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AppError;
use crate::handlers::settings;
use crate::state::AppState;

async fn check_prometheus_enabled(state: &AppState) -> bool {
    settings::load_settings_from_db(state)
        .await
        .prometheus_enabled
}

const PODMONITOR_GROUP: &str = "monitoring.coreos.com";
const PODMONITOR_VERSION: &str = "v1";
const PODMONITOR_KIND: &str = "PodMonitor";
const FIELD_MANAGER: &str = "deckwatch-monitor";
const PODMONITOR_SUFFIX: &str = "-deckwatch";

#[derive(Debug, Deserialize)]
pub struct MonitorConfigRequest {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub interval: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct MonitorResponse {
    pub enabled: bool,
    pub name: String,
    pub namespace: String,
    pub port: String,
    pub path: String,
    pub interval: String,
    pub matching_pods: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<(StatusCode, Json<MonitorResponse>), AppError> {
    if !check_prometheus_enabled(&state).await {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(MonitorResponse {
                enabled: false,
                name: podmonitor_name(&name),
                namespace: ns,
                port: "metrics".to_string(),
                path: "/metrics".to_string(),
                interval: "30s".to_string(),
                matching_pods: 0,
                unavailable_reason: Some(
                    "Prometheus monitoring is disabled. Enable it in Settings.".to_string(),
                ),
            }),
        ));
    }
    let dep_api = state.deployments_api(&ns)?;
    let deployment = dep_api.get(&name).await?;
    let default_port = pick_default_port(&deployment);

    let api = match state.podmonitors_api(&ns).await {
        Ok(api) => api,
        Err(reason) => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(MonitorResponse {
                    enabled: false,
                    name: podmonitor_name(&name),
                    namespace: ns,
                    port: default_port,
                    path: "/metrics".to_string(),
                    interval: "30s".to_string(),
                    matching_pods: 0,
                    unavailable_reason: Some(reason),
                }),
            ));
        }
    };

    let pm_name = podmonitor_name(&name);
    match api.get(&pm_name).await {
        Ok(pm) => {
            let (port, path, interval) = extract_endpoint(&pm).unwrap_or((
                default_port.clone(),
                "/metrics".to_string(),
                "30s".to_string(),
            ));
            let matching_pods = count_matching_pods(&state, &ns, &deployment).await;
            Ok((
                StatusCode::OK,
                Json(MonitorResponse {
                    enabled: true,
                    name: pm_name,
                    namespace: ns,
                    port,
                    path,
                    interval,
                    matching_pods,
                    unavailable_reason: None,
                }),
            ))
        }
        Err(kube::Error::Api(e)) if e.code == 404 => Ok((
            StatusCode::OK,
            Json(MonitorResponse {
                enabled: false,
                name: pm_name,
                namespace: ns,
                port: default_port,
                path: "/metrics".to_string(),
                interval: "30s".to_string(),
                matching_pods: 0,
                unavailable_reason: None,
            }),
        )),
        Err(e) => Err(AppError::Kube(e)),
    }
}

pub async fn upsert(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<MonitorConfigRequest>,
) -> Result<(StatusCode, Json<MonitorResponse>), AppError> {
    if !check_prometheus_enabled(&state).await {
        return Err(AppError::BadRequest(
            "Prometheus monitoring is disabled. Enable it in Settings.".to_string(),
        ));
    }
    if !req.enabled {
        let status = delete(State(state.clone()), Path((ns.clone(), name.clone()))).await?;
        return Ok((
            status,
            Json(MonitorResponse {
                enabled: false,
                name: podmonitor_name(&name),
                namespace: ns,
                port: String::new(),
                path: String::new(),
                interval: String::new(),
                matching_pods: 0,
                unavailable_reason: None,
            }),
        ));
    }

    let dep_api = state.deployments_api(&ns)?;
    let deployment = dep_api.get(&name).await?;

    let port = req.port.unwrap_or_else(|| pick_default_port(&deployment));
    let path = req.path.unwrap_or_else(|| "/metrics".to_string());
    let interval = req.interval.unwrap_or_else(|| "30s".to_string());

    if !is_prom_duration(&interval) {
        return Err(AppError::BadRequest(format!(
            "interval '{interval}' is not a valid Prometheus duration (e.g. 30s, 1m, 5m)",
        )));
    }

    let api = match state.podmonitors_api(&ns).await {
        Ok(api) => api,
        Err(reason) => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(MonitorResponse {
                    enabled: false,
                    name: podmonitor_name(&name),
                    namespace: ns,
                    port,
                    path,
                    interval,
                    matching_pods: 0,
                    unavailable_reason: Some(reason),
                }),
            ));
        }
    };

    let match_labels = deployment
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.clone())
        .unwrap_or_default();

    let owner_uid = deployment
        .metadata
        .uid
        .clone()
        .ok_or_else(|| AppError::BadRequest("deployment has no UID".to_string()))?;

    let pm_name = podmonitor_name(&name);
    let pm = build_podmonitor(
        &pm_name,
        &ns,
        &name,
        &owner_uid,
        &match_labels,
        &port,
        &path,
        &interval,
    );

    let params = PatchParams::apply(FIELD_MANAGER).force();
    let applied = api.patch(&pm_name, &params, &Patch::Apply(&pm)).await?;

    let (out_port, out_path, out_interval) =
        extract_endpoint(&applied).unwrap_or((port.clone(), path.clone(), interval.clone()));
    let matching_pods = count_matching_pods(&state, &ns, &deployment).await;

    Ok((
        StatusCode::OK,
        Json(MonitorResponse {
            enabled: true,
            name: pm_name,
            namespace: ns,
            port: out_port,
            path: out_path,
            interval: out_interval,
            matching_pods,
            unavailable_reason: None,
        }),
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = match state.podmonitors_api(&ns).await {
        Ok(api) => api,
        Err(_) => return Ok(StatusCode::NO_CONTENT),
    };

    let pm_name = podmonitor_name(&name);
    match api.delete(&pm_name, &DeleteParams::default()).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(AppError::Kube(e)),
    }
}

fn podmonitor_name(deployment: &str) -> String {
    format!("{deployment}{PODMONITOR_SUFFIX}")
}

fn pick_default_port(deployment: &k8s_openapi::api::apps::v1::Deployment) -> String {
    let containers = deployment
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .map(|ps| ps.containers.as_slice())
        .unwrap_or(&[]);

    let ports: Vec<&k8s_openapi::api::core::v1::ContainerPort> = containers
        .iter()
        .flat_map(|c| c.ports.as_deref().unwrap_or(&[]).iter())
        .collect();

    for p in &ports {
        if let Some(n) = &p.name {
            if n.contains("metrics") {
                return n.clone();
            }
        }
    }
    if let Some(first_named) = ports.iter().find_map(|p| p.name.clone()) {
        return first_named;
    }
    if let Some(first_num) = ports.first() {
        return first_num.container_port.to_string();
    }
    "metrics".to_string()
}

fn build_podmonitor(
    name: &str,
    namespace: &str,
    deployment_name: &str,
    deployment_uid: &str,
    match_labels: &std::collections::BTreeMap<String, String>,
    port: &str,
    path: &str,
    interval: &str,
) -> DynamicObject {
    let selector = json!({ "matchLabels": match_labels });
    let endpoint = json!({
        "port": port,
        "path": path,
        "interval": interval,
        "honorLabels": true,
    });
    let spec = json!({
        "selector": selector,
        "podMetricsEndpoints": [endpoint],
    });

    let owner = OwnerReference {
        api_version: "apps/v1".to_string(),
        kind: "Deployment".to_string(),
        name: deployment_name.to_string(),
        uid: deployment_uid.to_string(),
        controller: Some(false),
        block_owner_deletion: Some(false),
    };

    let api_resource = kube::core::ApiResource {
        group: PODMONITOR_GROUP.to_string(),
        version: PODMONITOR_VERSION.to_string(),
        api_version: format!("{PODMONITOR_GROUP}/{PODMONITOR_VERSION}"),
        kind: PODMONITOR_KIND.to_string(),
        plural: "podmonitors".to_string(),
    };
    let mut obj = DynamicObject::new(name, &api_resource);
    obj.types = Some(TypeMeta {
        api_version: format!("{PODMONITOR_GROUP}/{PODMONITOR_VERSION}"),
        kind: PODMONITOR_KIND.to_string(),
    });
    obj.metadata.namespace = Some(namespace.to_string());
    obj.metadata.labels = Some(std::collections::BTreeMap::from([
        (
            "app.kubernetes.io/managed-by".to_string(),
            "deckwatch".to_string(),
        ),
        (
            "deckwatch.io/deployment".to_string(),
            deployment_name.to_string(),
        ),
    ]));
    obj.metadata.owner_references = Some(vec![owner]);
    obj.data = json!({ "spec": spec });
    obj
}

fn extract_endpoint(pm: &DynamicObject) -> Option<(String, String, String)> {
    let spec = pm.data.get("spec")?;
    let ep = spec.get("podMetricsEndpoints")?.as_array()?.first()?;
    let port = ep.get("port")?.as_str()?.to_string();
    let path = ep
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("/metrics")
        .to_string();
    let interval = ep
        .get("interval")
        .and_then(|v| v.as_str())
        .unwrap_or("30s")
        .to_string();
    Some((port, path, interval))
}

async fn count_matching_pods(
    state: &AppState,
    ns: &str,
    deployment: &k8s_openapi::api::apps::v1::Deployment,
) -> usize {
    let Some(match_labels) = deployment
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
    else {
        return 0;
    };
    if match_labels.is_empty() {
        return 0;
    }
    let selector = match_labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let Ok(pods_api) = state.pods_api(ns) else {
        return 0;
    };
    let lp = kube::api::ListParams::default().labels(&selector);
    pods_api.list(&lp).await.map(|l| l.items.len()).unwrap_or(0)
}

fn is_prom_duration(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let num = if let Some(n) = s.strip_suffix("ms") {
        n
    } else {
        let last = s.chars().last().unwrap();
        if !matches!(last, 's' | 'm' | 'h' | 'd' | 'w' | 'y') {
            return false;
        }
        &s[..s.len() - 1]
    };
    !num.is_empty() && num.chars().all(|c| c.is_ascii_digit())
}

pub async fn probe_podmonitor_crd(client: &kube::Client) -> Result<(), String> {
    let gvk = GroupVersionKind::gvk(PODMONITOR_GROUP, PODMONITOR_VERSION, PODMONITOR_KIND);
    match discovery::pinned_kind(client, &gvk).await {
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "prometheus-operator CRD {PODMONITOR_GROUP}/{PODMONITOR_VERSION} {PODMONITOR_KIND} \
             not found in the cluster ({e}). Install the prometheus-operator to enable per-\
             deployment scrape configuration."
        )),
    }
}

#[cfg(test)]
#[path = "../handlers_monitoring_tests.rs"]
mod handlers_monitoring_tests;
