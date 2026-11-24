//! Plugin-declared provisioned resource endpoints.
//!
//! Implements the three REST endpoints for managing infrastructure resources
//! that plugins provision on behalf of applications (e.g. RDS databases,
//! S3 buckets). The provisioned state is persisted in
//! `application_plugin_resources` and injected as env vars into all
//! deployments and cronjobs in the application by the reconciler.
//!
//! ```
//! GET    /api/namespaces/{ns}/applications/{name}/resources
//! POST   /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
//! PATCH  /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
//! DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
//! ```
//!
//! POST, PATCH and DELETE return what the plugin reported (`plugin_events`,
//! sanitized). A `provision()` call that fails outright returns 502 with the
//! events in the error body.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::entities::application_plugin_resources;
use crate::error::AppError;
use crate::plugin_events::PluginEventReport;
use crate::plugins::{PluginCall, ResourceProvisionRequest, SidecarSpec, Trigger};
use crate::state::AppState;

fn now_utc() -> sea_orm::prelude::DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    sea_orm::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

// ── Request / response types ──────────────────────────────────────────────────

/// Operator-submitted form values when provisioning a resource.
#[derive(Debug, Deserialize, Default)]
pub struct ProvisionRequest {
    /// Field values keyed by `PluginResource.fields[*].key`.
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

/// A persisted provisioned resource row returned to the client.
#[derive(Debug, Serialize)]
pub struct ProvisionedResource {
    pub id: String,
    pub application_id: String,
    pub plugin_name: String,
    pub resource_id: String,
    /// Operator-submitted form values (for display/audit).
    pub fields: serde_json::Value,
    /// Plugin-returned state (env var key-value pairs).
    pub state: serde_json::Value,
    /// Deployment annotations stamped by the plugin.
    pub annotations: serde_json::Value,
    /// Sidecars injected into all application deployments by the plugin.
    pub sidecars: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    /// What the plugin reported on this provision or refresh, including
    /// warnings on success. Omitted from list responses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_events: Vec<PluginEventReport>,
}

/// Body of `DELETE …/resources/{plugin}/{resource_id}` (deckwatch#250).
#[derive(Debug, Serialize)]
pub struct DeprovisionResponse {
    /// What the plugin's `deprovision()` says it did and didn't clean up
    /// (`ResourceDeprovisionResult.message`, sanitized), e.g. "bucket kept
    /// because it isn't empty". Empty when the plugin said nothing.
    pub message: String,
    /// Everything the call reported: its errors and warnings, the message as
    /// an `info` event, or why the call failed. The record is removed either
    /// way.
    pub plugin_events: Vec<PluginEventReport>,
}

fn row_to_response(row: application_plugin_resources::Model) -> ProvisionedResource {
    let fields: serde_json::Value =
        serde_json::from_str(&row.fields).unwrap_or(serde_json::Value::Object(Default::default()));
    let state: serde_json::Value =
        serde_json::from_str(&row.state).unwrap_or(serde_json::Value::Object(Default::default()));
    let annotations: serde_json::Value = serde_json::from_str(&row.annotations)
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let sidecars: serde_json::Value =
        serde_json::from_str(&row.sidecars).unwrap_or(serde_json::Value::Array(Default::default()));
    ProvisionedResource {
        id: row.id,
        application_id: row.application_id,
        plugin_name: row.plugin_name,
        resource_id: row.resource_id,
        fields,
        state,
        annotations,
        sidecars,
        created_at: row.created_at.to_string(),
        updated_at: row.updated_at.to_string(),
        plugin_events: Vec::new(),
    }
}

// ── GET /api/namespaces/{ns}/applications/{name}/resources ────────────────────

pub async fn list(
    State(state): State<AppState>,
    Path((ns, app_name)): Path<(String, String)>,
) -> Result<Json<Vec<ProvisionedResource>>, AppError> {
    let _ = state.deployments_api(&ns)?; // namespace allowlist check

    let app_id = format!("{ns}/{app_name}");
    let rows = application_plugin_resources::Entity::find()
        .filter(application_plugin_resources::Column::ApplicationId.eq(&app_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?;

    Ok(Json(rows.into_iter().map(row_to_response).collect()))
}

// ── POST /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}

pub async fn provision(
    State(state): State<AppState>,
    Path((ns, app_name, plugin_name, resource_id)): Path<(String, String, String, String)>,
    body: Option<Json<ProvisionRequest>>,
) -> Result<(StatusCode, Json<ProvisionedResource>), AppError> {
    let _ = state.deployments_api(&ns)?; // namespace allowlist check

    let app_id = format!("{ns}/{app_name}");
    let fields = body.map(|b| b.0.fields).unwrap_or_default();

    // Find the plugin by name.
    let plugins = state.plugins.read().await;
    let plugin = plugins
        .iter()
        .find(|p| p.name == plugin_name)
        .ok_or_else(|| AppError::NotFound(format!("plugin '{plugin_name}' is not loaded")))?;

    // Validate the resource id is declared by the plugin.
    let resource_decl = plugin
        .metadata
        .resources
        .iter()
        .find(|r| r.id == resource_id)
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "plugin '{plugin_name}' does not declare resource '{resource_id}'"
            ))
        })?;

    // For singleton resources, reject if one already exists.
    if resource_decl.singleton {
        let existing = application_plugin_resources::Entity::find()
            .filter(application_plugin_resources::Column::ApplicationId.eq(&app_id))
            .filter(application_plugin_resources::Column::PluginName.eq(&plugin_name))
            .filter(application_plugin_resources::Column::ResourceId.eq(&resource_id))
            .one(&state.db)
            .await
            .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?;
        if existing.is_some() {
            return Err(AppError::BadRequest(format!(
                "resource '{resource_id}' is a singleton and already exists for application '{app_name}'"
            )));
        }
    }

    // Build the provision request and call the plugin.
    let req = ResourceProvisionRequest {
        application_name: app_name.clone(),
        namespace: ns.clone(),
        resource_id: resource_id.clone(),
        fields: fields.clone(),
        trace_context: None,
    };

    let call = PluginCall::new(&state.plugin_telemetry, Trigger::Provision)
        .namespace(&ns)
        .application(&app_name)
        .resource(&resource_id);
    let (result, outcome) = crate::plugins::run_provision(plugin, &req, &call).take_result();
    let Some(result) = result else {
        return Err(AppError::PluginFailed {
            summary: outcome.failure_summary(),
            events: outcome.reports(),
        });
    };

    // Drop the read lock before the async apply call.
    let kubernetes_resources = outcome.k8s_resources(&result.kubernetes_resources);
    let state_map = result.state.clone();
    let deployment_annotations = result.deployment_annotations.clone();
    let sidecars: Vec<SidecarSpec> = result.sidecars.clone();
    drop(plugins);

    // Apply any Kubernetes resources emitted by the plugin.
    let mut plugin_events = outcome.reports();
    if !kubernetes_resources.is_empty() {
        plugin_events.extend(
            crate::plugins::apply_kubernetes_resources(
                &kubernetes_resources,
                &state.kube_client,
                &state.plugin_telemetry,
            )
            .await,
        );
    }

    // Persist to DB.
    let now = now_utc();
    let id = uuid::Uuid::new_v4().to_string();
    let fields_json = serde_json::to_string(&fields).unwrap_or_else(|_| "{}".to_string());
    let state_json = serde_json::to_string(&state_map).unwrap_or_else(|_| "{}".to_string());
    let annotations_json =
        serde_json::to_string(&deployment_annotations).unwrap_or_else(|_| "{}".to_string());
    let sidecars_json = serde_json::to_string(&sidecars).unwrap_or_else(|_| "[]".to_string());

    let row = application_plugin_resources::ActiveModel {
        id: Set(id),
        application_id: Set(app_id),
        plugin_name: Set(plugin_name),
        resource_id: Set(resource_id),
        fields: Set(fields_json),
        state: Set(state_json),
        annotations: Set(annotations_json),
        sidecars: Set(sidecars_json),
        init_containers: Set("[]".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let inserted = row
        .insert(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to persist resource: {e}")))?;

    let mut response = row_to_response(inserted);
    response.plugin_events = plugin_events;
    Ok((StatusCode::CREATED, Json(response)))
}

// ── PATCH /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
//
// Re-runs provision() with the stored fields and updates the record in-place.
// Used to refresh state after async provisioning completes (e.g. RDS becomes
// available after the initial "provisioning" response).

pub async fn refresh(
    State(state): State<AppState>,
    Path((ns, app_name, plugin_name, resource_id)): Path<(String, String, String, String)>,
) -> Result<Json<ProvisionedResource>, AppError> {
    let _ = state.deployments_api(&ns)?;

    let app_id = format!("{ns}/{app_name}");

    let row = application_plugin_resources::Entity::find()
        .filter(application_plugin_resources::Column::ApplicationId.eq(&app_id))
        .filter(application_plugin_resources::Column::PluginName.eq(&plugin_name))
        .filter(application_plugin_resources::Column::ResourceId.eq(&resource_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "no provisioned resource '{resource_id}' from plugin '{plugin_name}' found for application '{app_name}'"
            ))
        })?;

    let fields: HashMap<String, String> = serde_json::from_str(&row.fields).unwrap_or_default();

    let plugins = state.plugins.read().await;
    let plugin = plugins
        .iter()
        .find(|p| p.name == plugin_name)
        .ok_or_else(|| AppError::NotFound(format!("plugin '{plugin_name}' is not loaded")))?;

    let req = ResourceProvisionRequest {
        application_name: app_name.clone(),
        namespace: ns.clone(),
        resource_id: resource_id.clone(),
        fields: fields.clone(),
        trace_context: None,
    };

    let call = PluginCall::new(&state.plugin_telemetry, Trigger::Refresh)
        .namespace(&ns)
        .application(&app_name)
        .resource(&resource_id);
    let (result, outcome) = crate::plugins::run_provision(plugin, &req, &call).take_result();
    let Some(result) = result else {
        return Err(AppError::PluginFailed {
            summary: outcome.failure_summary(),
            events: outcome.reports(),
        });
    };

    let kubernetes_resources = outcome.k8s_resources(&result.kubernetes_resources);
    let state_map = result.state.clone();
    let deployment_annotations = result.deployment_annotations.clone();
    let sidecars: Vec<SidecarSpec> = result.sidecars.clone();
    drop(plugins);

    let mut plugin_events = outcome.reports();
    if !kubernetes_resources.is_empty() {
        plugin_events.extend(
            crate::plugins::apply_kubernetes_resources(
                &kubernetes_resources,
                &state.kube_client,
                &state.plugin_telemetry,
            )
            .await,
        );
    }

    let now = now_utc();
    let state_json = serde_json::to_string(&state_map).unwrap_or_else(|_| "{}".to_string());
    let annotations_json =
        serde_json::to_string(&deployment_annotations).unwrap_or_else(|_| "{}".to_string());
    let sidecars_json = serde_json::to_string(&sidecars).unwrap_or_else(|_| "[]".to_string());

    let mut active: application_plugin_resources::ActiveModel = row.into();
    active.state = Set(state_json);
    active.annotations = Set(annotations_json);
    active.sidecars = Set(sidecars_json);
    active.updated_at = Set(now);

    let updated = active
        .update(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to update resource: {e}")))?;

    let mut response = row_to_response(updated);
    response.plugin_events = plugin_events;
    Ok(Json(response))
}

// ── DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}

pub async fn deprovision(
    State(state): State<AppState>,
    Path((ns, app_name, plugin_name, resource_id)): Path<(String, String, String, String)>,
) -> Result<Json<DeprovisionResponse>, AppError> {
    let _ = state.deployments_api(&ns)?; // namespace allowlist check

    let app_id = format!("{ns}/{app_name}");

    let row = application_plugin_resources::Entity::find()
        .filter(application_plugin_resources::Column::ApplicationId.eq(&app_id))
        .filter(application_plugin_resources::Column::PluginName.eq(&plugin_name))
        .filter(application_plugin_resources::Column::ResourceId.eq(&resource_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("database error: {e}")))?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "no provisioned resource '{resource_id}' from plugin '{plugin_name}' found for application '{app_name}'"
            ))
        })?;

    // Call plugin's deprovision() if it exports one.
    // We call it before removing the DB record so the plugin still has
    // access to state/fields. Errors are logged but never block record removal.
    let state_map: std::collections::HashMap<String, String> =
        serde_json::from_str(&row.state).unwrap_or_default();
    let fields_map: std::collections::HashMap<String, String> =
        serde_json::from_str(&row.fields).unwrap_or_default();

    let dep_req = crate::plugins::ResourceDeprovisionRequest {
        application_name: app_name.clone(),
        namespace: ns.clone(),
        resource_id: resource_id.clone(),
        state: state_map,
        fields: fields_map,
        trace_context: None,
    };

    let response = {
        let plugins = state.plugins.read().await;
        match plugins.iter().find(|p| p.name == plugin_name) {
            Some(plugin) => {
                let call = PluginCall::new(&state.plugin_telemetry, Trigger::Deprovision)
                    .namespace(&ns)
                    .application(&app_name)
                    .resource(&resource_id);
                let (result, outcome) =
                    crate::plugins::run_deprovision(plugin, &dep_req, &call).take_result();
                if result.is_none() {
                    tracing::warn!(
                        plugin = %plugin_name,
                        resource_id = %resource_id,
                        "deprovision() call failed — removing DB record anyway"
                    );
                }
                let message = result
                    .map(|r| {
                        crate::plugin_events::sanitize_text(
                            &r.message,
                            crate::plugin_events::MAX_MESSAGE_BYTES,
                            &outcome.scrubber,
                        )
                    })
                    .unwrap_or_default();
                DeprovisionResponse {
                    message,
                    plugin_events: outcome.reports(),
                }
            }
            None => DeprovisionResponse {
                message: String::new(),
                plugin_events: Vec::new(),
            },
        }
    };

    tracing::info!(
        namespace = %ns,
        application = %app_name,
        plugin = %plugin_name,
        resource_id = %resource_id,
        "plugin resource deprovisioned — DB record removed"
    );

    application_plugin_resources::Entity::delete_by_id(row.id)
        .exec(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to delete resource: {e}")))?;

    Ok(Json(response))
}

#[cfg(test)]
#[path = "application_resources_events_tests.rs"]
mod events_tests;
