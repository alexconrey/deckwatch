//! The application's Observability section (#398):
//!
//! * **Prometheus monitoring:** the PodMonitor of each of the application's
//!   Deployments, the same per-Deployment toggle as the deployment page
//!   (`crate::handlers::monitoring`). This section reports them; they're
//!   turned on, changed and off with `PUT`/`DELETE
//!   /api/namespaces/{ns}/deployments/{name}/monitor`.
//! * **OpenTelemetry injection:** on or off (off by default), an optional
//!   service name and extra resource attributes, stored with the
//!   application. The reconciler applies them (`crate::otel_inject`), so
//!   saving only stores them: the variables reach the workloads within a
//!   poll interval, and turning injection on restarts their pods once.
//!
//! `GET /api/namespaces/{ns}/applications/{name}/observability` returns
//! both, with each workload's injection state; `PUT` changes the
//! OpenTelemetry settings and returns the same.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use kube::api::ListParams;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, EntityTrait};
use serde::{Deserialize, Serialize};

use crate::entities::applications;
use crate::error::AppError;
use crate::handlers::{monitoring, settings};
use crate::otel_inject::{
    workload_status, AppEndpoints, ApplicationObservability, ApplicationWorkloads, OtelInjection,
    ResourceAttribute, WorkloadInjection,
};
use crate::state::AppState;
use crate::telemetry::endpoints::{join, Signal};

/// `GET` and `PUT` response.
#[derive(Debug, Serialize)]
pub struct ObservabilityResponse {
    pub prometheus: PrometheusMonitoring,
    pub opentelemetry: OpenTelemetry,
}

/// The PodMonitors of the application's Deployments.
#[derive(Debug, Serialize)]
pub struct PrometheusMonitoring {
    /// Prometheus monitoring is on in Settings and the PodMonitor CRD is
    /// installed.
    pub available: bool,
    /// Why not, when it isn't.
    pub unavailable_reason: Option<String>,
    pub deployments: Vec<DeploymentMonitor>,
}

/// One Deployment's PodMonitor, or the defaults it would get.
#[derive(Debug, Serialize)]
pub struct DeploymentMonitor {
    pub deployment: String,
    pub enabled: bool,
    /// The PodMonitor's name.
    pub name: String,
    pub port: String,
    pub path: String,
    pub interval: String,
}

/// OpenTelemetry injection: the settings, where workloads send each signal,
/// and each workload's state.
#[derive(Debug, Serialize)]
pub struct OpenTelemetry {
    pub enabled: bool,
    pub service_name: Option<String>,
    pub resource_attributes: Vec<ResourceAttribute>,
    /// The endpoints workloads get, from Settings → Observability (not
    /// deckwatch's own `OTEL_*` variables). `None`: not set.
    pub otlp_endpoint: Option<String>,
    pub otlp_metrics_endpoint: Option<String>,
    /// Where each signal goes; `endpoint: None` means turned off
    /// (`OTEL_<SIGNAL>_EXPORTER=none`).
    pub signals: Vec<SignalDestination>,
    /// The application's Deployments, StatefulSets and CronJobs.
    pub workloads: Vec<WorkloadInjection>,
    /// Why `workloads` is empty or incomplete, if it is.
    pub workloads_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignalDestination {
    pub signal: Signal,
    pub endpoint: Option<String>,
}

/// `PUT` body. A missing `opentelemetry` keeps the stored settings.
#[derive(Debug, Deserialize)]
pub struct ObservabilityUpdate {
    #[serde(default)]
    pub opentelemetry: Option<OtelInjection>,
}

async fn find_application(
    state: &AppState,
    ns: &str,
    name: &str,
) -> Result<applications::Model, AppError> {
    if !state.is_namespace_allowed(ns) {
        return Err(AppError::NamespaceNotAllowed(ns.to_string()));
    }
    applications::Entity::find_by_id(format!("{ns}/{name}"))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("application '{name}' not found")))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<ObservabilityResponse>, AppError> {
    let row = find_application(&state, &ns, &name).await?;
    let stored = ApplicationObservability::from_column(row.observability.as_deref())
        .map_err(AppError::BadRequest)?;
    Ok(Json(response(&state, &ns, &name, &stored).await))
}

pub async fn put(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(update): Json<ObservabilityUpdate>,
) -> Result<Json<ObservabilityResponse>, AppError> {
    let row = find_application(&state, &ns, &name).await?;
    // Settings that no longer read are replaced, not merged into.
    let stored =
        ApplicationObservability::from_column(row.observability.as_deref()).unwrap_or_default();
    let mut updated = stored.clone();
    if let Some(otel) = update.opentelemetry {
        updated.opentelemetry = otel.validated().map_err(AppError::BadRequest)?;
    }
    if updated != stored {
        let mut active: applications::ActiveModel = row.into();
        active.observability = Set(updated.to_column());
        active.updated_at = Set(now_utc());
        active
            .update(&state.db)
            .await
            .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?;
        let detail = describe_change(&name, &stored.opentelemetry, &updated.opentelemetry);
        if let Err(e) = crate::audit::log_action(
            &state.db,
            "update",
            "application-observability",
            &name,
            &ns,
            &detail,
            "",
        )
        .await
        {
            tracing::warn!(error = %e, "failed to write audit log for application observability");
        }
    }
    Ok(Json(response(&state, &ns, &name, &updated).await))
}

fn now_utc() -> sea_orm::prelude::DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    sea_orm::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

/// Audit-log text for a change of the OpenTelemetry settings. Attribute
/// keys only: the values belong in the application, not the log.
pub fn describe_change(app: &str, before: &OtelInjection, after: &OtelInjection) -> String {
    let what = match (before.enabled, after.enabled) {
        (false, true) => "turned OpenTelemetry injection on",
        (true, false) => "turned OpenTelemetry injection off",
        _ => "changed the OpenTelemetry injection settings",
    };
    let service_name = match &after.service_name {
        Some(name) => format!("service name {name:?}"),
        None => "each workload's name as the service name".to_string(),
    };
    let attributes = if after.resource_attributes.is_empty() {
        "no extra resource attributes".to_string()
    } else {
        let keys: Vec<&str> = after
            .resource_attributes
            .iter()
            .map(|a| a.key.as_str())
            .collect();
        format!("extra resource attributes {}", keys.join(", "))
    };
    format!("{what} for application '{app}' ({service_name}; {attributes})")
}

/// The section as it stands, with `stored` as the application's settings.
async fn response(
    state: &AppState,
    ns: &str,
    name: &str,
    stored: &ApplicationObservability,
) -> ObservabilityResponse {
    let settings = settings::load_settings_from_db(state).await;
    let (endpoints, _) = AppEndpoints::from_settings(&settings.observability.unwrap_or_default());
    let workloads = ApplicationWorkloads::list(state, ns, name).await;
    let owner = crate::watcher::plugin_fields_owner(&state.db).await;

    let config = &stored.opentelemetry;
    let (workload_states, workloads_error) = match (&workloads, &owner) {
        (Ok(workloads), Ok(owner)) => {
            let states = workloads
                .live()
                .iter()
                .map(|live| workload_status(live, &endpoints, config, name, owner))
                .collect();
            let error = workloads.statefulsets_forbidden.then(|| {
                "deckwatch may not list StatefulSets, so their injection isn't shown: grant its \
                 ClusterRole get, list and patch on apps/statefulsets"
                    .to_string()
            });
            (states, error)
        }
        (Err(e), _) => (
            Vec::new(),
            Some(format!("failed to list the workloads: {e}")),
        ),
        (_, Err(e)) => (Vec::new(), Some(format!("database error: {e}"))),
    };
    let signals = Signal::ALL
        .iter()
        .map(|&signal| SignalDestination {
            signal,
            endpoint: match signal {
                Signal::Metrics => endpoints.metrics.clone(),
                Signal::Traces | Signal::Logs => {
                    endpoints.otlp.as_deref().map(|b| join(b, signal.path()))
                }
            },
        })
        .collect();
    let deployments: &[Deployment] = workloads
        .as_ref()
        .map(|w| w.deployments.as_slice())
        .unwrap_or_default();
    ObservabilityResponse {
        prometheus: prometheus_monitoring(state, ns, deployments, settings.prometheus_enabled)
            .await,
        opentelemetry: OpenTelemetry {
            enabled: config.enabled,
            service_name: config.service_name.clone(),
            resource_attributes: config.resource_attributes.clone(),
            otlp_endpoint: endpoints.otlp.clone(),
            otlp_metrics_endpoint: endpoints.metrics.clone(),
            signals,
            workloads: workload_states,
            workloads_error,
        },
    }
}

/// The PodMonitor of each of `deployments`, as the deployment page shows it.
async fn prometheus_monitoring(
    state: &AppState,
    ns: &str,
    deployments: &[Deployment],
    prometheus_enabled: bool,
) -> PrometheusMonitoring {
    let defaults = |d: &Deployment| {
        let name = d.metadata.name.clone().unwrap_or_default();
        DeploymentMonitor {
            name: monitoring::podmonitor_name(&name),
            deployment: name,
            enabled: false,
            port: monitoring::pick_default_port(d),
            path: "/metrics".to_string(),
            interval: "30s".to_string(),
        }
    };
    let unavailable = |reason: String| PrometheusMonitoring {
        available: false,
        unavailable_reason: Some(reason),
        deployments: deployments.iter().map(defaults).collect(),
    };
    if !prometheus_enabled {
        return unavailable(
            "Prometheus monitoring is disabled. Enable it in Settings.".to_string(),
        );
    }
    let api = match state.podmonitors_api(ns).await {
        Ok(api) => api,
        Err(reason) => return unavailable(reason),
    };
    let lp = ListParams::default().labels("app.kubernetes.io/managed-by=deckwatch");
    let monitors: BTreeMap<String, kube::api::DynamicObject> = match api.list(&lp).await {
        Ok(list) => list
            .items
            .into_iter()
            .filter_map(|pm| Some((pm.metadata.name.clone()?, pm)))
            .collect(),
        Err(e) => return unavailable(format!("failed to list the PodMonitors: {e}")),
    };
    let deployments = deployments
        .iter()
        .map(|d| {
            let mut monitor = defaults(d);
            if let Some(pm) = monitors.get(&monitor.name) {
                monitor.enabled = true;
                if let Some((port, path, interval)) = monitoring::extract_endpoint(pm) {
                    monitor.port = port;
                    monitor.path = path;
                    monitor.interval = interval;
                }
            }
            monitor
        })
        .collect();
    PrometheusMonitoring {
        available: true,
        unavailable_reason: None,
        deployments,
    }
}

#[cfg(test)]
#[path = "app_observability_tests.rs"]
mod tests;
