//! Plugin status endpoints: the current state of every plugin call target
//! (see `crate::plugin_status`).
//!
//! - `GET /api/plugin-status` — every row, optionally filtered by
//!   `health` (comma-separated, e.g. `failing,degraded`), `plugin` and
//!   `namespace`. The UI's Problems view uses `health=failing,degraded`.
//! - `GET /api/namespaces/{ns}/applications/{name}/plugin-status` — one
//!   application's rows.
//! - `GET /api/plugins/{name}/status` — one plugin's rows, plugin-wide
//!   (fetch/load) included.
//!
//! Rows are listed most severe first, then most recently updated. Rows for
//! namespaces outside the allowlist are left out; plugin-wide rows are
//! always visible.

use axum::extract::{Path, Query, State};
use axum::Json;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::entities::plugin_status;
use crate::error::AppError;
use crate::plugin_events::{Health, PluginEventReport};
use crate::state::AppState;

/// One row, as returned to clients.
#[derive(Debug, Serialize)]
pub struct PluginStatusView {
    /// Empty for plugin-wide rows.
    pub namespace: String,
    /// Empty for plugin-wide rows.
    pub application: String,
    pub plugin: String,
    /// The plugin resource id, or empty for `apply()` and plugin-wide rows.
    pub resource_id: String,
    pub health: Health,
    /// The export of the last recorded call.
    pub last_export: String,
    /// The latest call's events (plugin text, sanitized).
    pub last_events: Vec<PluginEventReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// When the current run of failures started.
    pub first_failure_at: Option<String>,
    pub consecutive_failures: i32,
    pub last_success_at: Option<String>,
    /// When `health` last changed.
    pub last_changed_at: String,
    pub updated_at: String,
}

impl From<plugin_status::Model> for PluginStatusView {
    fn from(row: plugin_status::Model) -> Self {
        let (namespace, application) = row
            .application_id
            .split_once('/')
            .map(|(ns, app)| (ns.to_string(), app.to_string()))
            .unwrap_or_default();
        let health = match row.health.as_str() {
            "failing" => Health::Failing,
            "degraded" => Health::Degraded,
            _ => Health::Healthy,
        };
        PluginStatusView {
            namespace,
            application,
            plugin: row.plugin_name,
            resource_id: row.resource_id,
            health,
            last_export: row.last_export,
            last_events: serde_json::from_str(&row.last_events).unwrap_or_default(),
            trace_id: row.trace_id,
            first_failure_at: row.first_failure_at.map(|t| t.to_rfc3339()),
            consecutive_failures: row.consecutive_failures,
            last_success_at: row.last_success_at.map(|t| t.to_rfc3339()),
            last_changed_at: row.last_changed_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct StatusQuery {
    /// Comma-separated healths to include, e.g. `failing,degraded`.
    pub health: Option<String>,
    pub plugin: Option<String>,
    pub namespace: Option<String>,
}

/// Sorted, allowlist-filtered views of `rows`.
fn views(
    state: &AppState,
    rows: Vec<plugin_status::Model>,
    wanted: Option<&[Health]>,
) -> Vec<PluginStatusView> {
    let mut out: Vec<PluginStatusView> = rows
        .into_iter()
        .map(PluginStatusView::from)
        .filter(|v| v.namespace.is_empty() || state.is_namespace_allowed(&v.namespace))
        .filter(|v| wanted.is_none_or(|w| w.contains(&v.health)))
        .collect();
    out.sort_by(|a, b| {
        b.health
            .cmp(&a.health)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    out
}

fn parse_healths(raw: &str) -> Result<Vec<Health>, AppError> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s {
            "healthy" => Ok(Health::Healthy),
            "degraded" => Ok(Health::Degraded),
            "failing" => Ok(Health::Failing),
            other => Err(AppError::BadRequest(format!(
                "unknown health '{other}' (expected healthy, degraded or failing)"
            ))),
        })
        .collect()
}

fn db_error(e: sea_orm::DbErr) -> AppError {
    AppError::BadRequest(format!("database error: {e}"))
}

/// GET /api/plugin-status
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<Vec<PluginStatusView>>, AppError> {
    let wanted = query.health.as_deref().map(parse_healths).transpose()?;
    let mut select = plugin_status::Entity::find();
    if let Some(plugin) = &query.plugin {
        select = select.filter(plugin_status::Column::PluginName.eq(plugin));
    }
    if let Some(ns) = &query.namespace {
        state.deployments_api(ns)?; // namespace allowlist check
        select = select.filter(plugin_status::Column::ApplicationId.starts_with(format!("{ns}/")));
    }
    let rows = select.all(&state.db).await.map_err(db_error)?;
    Ok(Json(views(&state, rows, wanted.as_deref())))
}

/// GET /api/namespaces/{ns}/applications/{name}/plugin-status
pub async fn for_application(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<Vec<PluginStatusView>>, AppError> {
    state.deployments_api(&ns)?; // namespace allowlist check
    let rows = plugin_status::Entity::find()
        .filter(plugin_status::Column::ApplicationId.eq(format!("{ns}/{name}")))
        .all(&state.db)
        .await
        .map_err(db_error)?;
    Ok(Json(views(&state, rows, None)))
}

/// GET /api/plugins/{name}/status
pub async fn for_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<PluginStatusView>>, AppError> {
    let rows = plugin_status::Entity::find()
        .filter(plugin_status::Column::PluginName.eq(&name))
        .all(&state.db)
        .await
        .map_err(db_error)?;
    Ok(Json(views(&state, rows, None)))
}

#[cfg(test)]
#[path = "plugin_status_tests.rs"]
mod tests;
