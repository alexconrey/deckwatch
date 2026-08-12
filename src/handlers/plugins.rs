//! Plugin API handlers.
//!
//! Exposes:
//! - `GET /api/plugins` — list all loaded plugins with metadata including `config_schema`
//! - `GET /api/plugins/{name}/schema` — return `config_schema` for a named plugin
//! - `POST /api/plugins/{name}/config` — save plugin config, encrypting `Secret`-typed fields

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::error::AppError;
use crate::plugins::ConfigField;
use crate::state::AppState;

/// Summary of a loaded plugin returned by `GET /api/plugins`.
#[derive(Serialize)]
pub struct PluginSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub provides: Vec<String>,
    pub depends_on: Vec<String>,
    pub config_schema: Vec<ConfigField>,
    pub resources: Vec<crate::plugins::PluginResource>,
    pub wasm_size_bytes: usize,
}

/// GET /api/plugins
///
/// Returns the full list of loaded plugins with metadata including `config_schema`.
pub async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginSummary>>, AppError> {
    let plugins = state.plugins.read().await;
    let summaries: Vec<PluginSummary> = plugins
        .iter()
        .map(|p| PluginSummary {
            name: p.name.clone(),
            version: p.metadata.version.clone(),
            description: p.metadata.description.clone(),
            provides: p.metadata.provides.clone(),
            depends_on: p.metadata.depends_on.clone(),
            config_schema: p.metadata.config_schema.clone(),
            resources: p.metadata.resources.clone(),
            wasm_size_bytes: p.wasm_bytes.len(),
        })
        .collect();
    Ok(Json(summaries))
}

/// GET /api/plugins/{name}/schema
///
/// Returns the `config_schema` for the named loaded plugin.
pub async fn get_plugin_schema(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<ConfigField>>, AppError> {
    let plugins = state.plugins.read().await;
    let plugin = plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| AppError::NotFound(format!("plugin '{name}' not found or not loaded")))?;
    Ok(Json(plugin.metadata.config_schema.clone()))
}

/// POST /api/plugins/{name}/config
///
/// Accepts `{ "key": "value", ... }`, encrypts `Secret`-typed fields using
/// AES-256-GCM (same as git token encryption in `src/crypto.rs`), and stores
/// the values in `DeckwatchSettings.plugins[].config`.
pub async fn save_plugin_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, AppError> {
    // Load current settings.
    let mut settings = crate::handlers::settings::load_settings_from_db(&state).await;

    // Find the plugin config entry.
    let plugin_cfg = settings
        .plugins
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "plugin '{name}' not found in settings — add it under Settings > Plugins first"
            ))
        })?;

    // Look up the schema so we know which fields are Secret-typed.
    let schema: Vec<ConfigField> = {
        let loaded = state.plugins.read().await;
        loaded
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.metadata.config_schema.clone())
            .unwrap_or_default()
    };

    // Insert each submitted key-value pair, encrypting Secret fields.
    for (key, value) in &body {
        let is_secret = schema
            .iter()
            .any(|f| f.key == *key && f.field_type == crate::plugins::ConfigFieldType::Secret);

        let stored_value = if is_secret && !state.encryption_key.is_empty() {
            crate::crypto::encrypt(&state.encryption_key, value)
                .map_err(|e| AppError::BadRequest(format!("failed to encrypt secret field: {e}")))?
        } else {
            value.clone()
        };

        plugin_cfg.config.insert(key.clone(), stored_value);
    }

    // Persist updated settings.
    crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save plugin config: {e}")))?;

    // Log the action.
    if let Err(e) = crate::audit::log_action(
        &state.db,
        "update",
        "plugin-config",
        &name,
        "",
        &format!("updated config for plugin '{name}'"),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log for plugin config update");
    }

    Ok(StatusCode::NO_CONTENT)
}
