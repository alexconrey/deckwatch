use axum::extract::State;
use axum::Json;
use k8s_openapi::api::core::v1::ConfigMap;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::Set;
use serde::{Deserialize, Serialize};

use crate::entities::settings as settings_entity;
use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::notifications::{NotificationClient, NotificationEvent};
use crate::state::AppState;

const SETTINGS_KEY: &str = "settings";
const DB_SETTINGS_KEY: &str = "main";

/// Display name used for the auto-populated deckwatch registry entry. Kept
/// as a const so the frontend can special-case it (badge as "local", hide
/// the edit button in the settings screen).
pub const DECKWATCH_REGISTRY_NAME: &str = "Deckwatch Registry (local)";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeckwatchSettings {
    #[serde(default)]
    pub allowed_namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_resource_limits: Option<ResourceDefaults>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<NotificationSettings>,
    /// Managed list of Git repositories that operators can pick from in the
    /// GitOps dialog. A "Custom" option on the frontend still allows free-form
    /// URLs for one-off use.
    #[serde(default)]
    pub git_repositories: Vec<GitRepository>,
    /// Managed list of OCI registries. Replaces the ECR-only assumption in
    /// the legacy GitOps annotations -- anything OCI-compliant works.
    ///
    /// When the embedded registry is enabled, a "Deckwatch Registry (local)"
    /// entry is injected into the returned list on every GET; it is filtered
    /// out on PUT so it doesn't get persisted (the deployment env var is the
    /// source of truth).
    #[serde(default)]
    pub oci_registries: Vec<OciRegistry>,
    /// Shared Kubernetes Secret references holding a `token` key. Multiple
    /// deployments can reference the same entry so operators do not re-type
    /// the secret name per deployment.
    #[serde(default)]
    pub git_token_secrets: Vec<GitTokenSecret>,
    /// Distributed-tracing wiring for the OpenTelemetry Collector addon and
    /// the trace-viewer UI. Optional so a settings ConfigMap that predates
    /// this field still deserializes; a missing block means "tracing not
    /// configured" and the tracing handler returns `unavailable_reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracing: Option<TracingSettings>,
    /// Runtime toggle for Prometheus monitoring features (PodMonitor CRD
    /// management, per-deployment scrape configuration). When false, the
    /// monitoring endpoints return 404 and the frontend hides the section.
    /// Defaults to true so clusters with prometheus-operator get it
    /// automatically; operators on clusters without the CRDs toggle it off
    /// in the settings pane.
    #[serde(default = "default_true")]
    pub prometheus_enabled: bool,
    /// Runtime toggle for the embedded container registry UI. When false,
    /// the Registry nav link is hidden and the registry page is
    /// inaccessible. Defaults to false so the feature is opt-in.
    #[serde(default)]
    pub registry_enabled: bool,
    /// Runtime toggle for the Claude AI diagnostic provider. When false,
    /// the "Diagnose with AI" / "Fix with AI" buttons hide Claude as an
    /// option across all users. Defaults to true (the shipping provider).
    #[serde(default = "default_true")]
    pub ai_claude_enabled: bool,
    /// Runtime toggle for the Codex AI diagnostic provider. When false,
    /// Codex is hidden as an option. Defaults to true so it's available
    /// once the backend wiring ships.
    #[serde(default = "default_true")]
    pub ai_codex_enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default)]
    pub namespaces: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitRepository {
    /// Display name shown in the dropdown. Must be unique within the list;
    /// the frontend uses it as a v-select item key.
    pub name: String,
    /// Clone URL (HTTPS). The GitOps poller talks to this via
    /// `/info/refs?service=git-upload-pack` using the associated token.
    pub url: String,
    /// Branch pre-selected when this repo is picked. The branch dropdown
    /// still populates from the live `/api/git/branches` query.
    #[serde(default)]
    pub default_branch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OciRegistry {
    /// Display name shown in the dropdown.
    pub name: String,
    /// Registry hostname or full repository prefix. Kaniko's `--destination`
    /// uses `{url}:{tag}` verbatim, so include the repo path
    /// (e.g. `docker.io/myorg/api`) when the registry demands it.
    pub url: String,
    /// One of: `ecr`, `dockerhub`, `ghcr`, `gar`, `harbor`, `deckwatch`, `generic`.
    /// Descriptive today (used for the UI icon and future auth-mode hints) --
    /// the build path itself is OCI-generic.
    #[serde(default = "default_registry_type")]
    pub registry_type: String,
    /// True when this entry was injected by the server (the embedded
    /// deckwatch registry). The frontend uses this flag to hide edit +
    /// delete controls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
}

fn default_registry_type() -> String {
    "generic".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitTokenSecret {
    /// Display name shown in the dropdown.
    pub name: String,
    /// Kubernetes Secret holding a `token` data key. The GitOps poller and
    /// Kaniko job both read this at build time.
    pub secret_name: String,
    /// Namespace the Secret lives in. Usually the same as the deployment,
    /// but split out so a single "shared" token in one namespace can be
    /// referenced from many.
    pub namespace: String,
}

/// Distributed-tracing consumer settings. Written by the operator, read by
/// the tracing handler and the OpenTelemetry Collector addon. See
/// `docs/TRACING.md` sec 6.3 for the mapping to values.yaml.
///
/// Split into `otlp_endpoint` (write path: where the sidecar collector
/// exports to) and `query_url` (read path: where deckwatch pulls trace
/// summaries from) because Tempo/Jaeger typically expose different ports
/// for each -- 4317/gRPC vs 3200/HTTP for Tempo, 4317/gRPC vs 16686/HTTP
/// for Jaeger. Collapsing them into one field would force operators to
/// pick which one to break.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TracingSettings {
    /// `tempo` | `jaeger`. Controls the trace-URL template used by the
    /// frontend "Open in UI" deep-link. Blank/unknown defaults to `tempo`.
    #[serde(default)]
    pub backend_kind: String,
    /// OTLP gRPC endpoint the OpenTelemetry Collector sidecar exports to.
    /// Empty means the sidecar addon will point at localhost by default --
    /// operators must set this for spans to actually leave the pod.
    #[serde(default)]
    pub otlp_endpoint: String,
    /// True when the OTLP endpoint is plaintext gRPC. In-cluster deployments
    /// typically want `true`; managed backends (Grafana Cloud, Honeycomb)
    /// need `false`.
    #[serde(default)]
    pub otlp_insecure: bool,
    /// HTTP URL the tracing handler proxies through to fetch trace summaries.
    /// Tempo: `http://<release>-tempo:3200`. Jaeger: `http://<release>-jaeger:16686`.
    /// Empty disables the tracing handler (returns `unavailable_reason`).
    #[serde(default)]
    pub query_url: String,
    /// Public deep-link base URL for the backend UI. Frontend opens
    /// `{ui_url}/trace/{trace_id}` (Jaeger) or a Grafana Explore URL with the
    /// datasource query pre-filled (Tempo). Empty hides the "Open in UI" link.
    #[serde(default)]
    pub ui_url: String,
}

pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<DeckwatchSettings>, AppError> {
    let mut settings = load_settings_from_db(&state).await;
    inject_builtin_registry(&state, &mut settings);
    Ok(Json(settings))
}

/// Load settings from the database. If the DB row doesn't exist yet, attempt
/// a one-time migration from the legacy ConfigMap. If neither source has data,
/// return compiled-in defaults.
pub async fn load_settings_from_db(state: &AppState) -> DeckwatchSettings {
    // Try database first.
    match settings_entity::Entity::find_by_id(DB_SETTINGS_KEY)
        .one(&state.db)
        .await
    {
        Ok(Some(row)) => {
            if let Ok(s) = serde_json::from_str::<DeckwatchSettings>(&row.value) {
                return s;
            }
            tracing::warn!("settings row in DB has invalid JSON; falling back to defaults");
        }
        Ok(None) => {
            // DB is empty -- try to seed from the legacy ConfigMap so existing
            // deployments don't lose their settings on upgrade.
            if let Some(s) = migrate_settings_from_configmap(state).await {
                return s;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to query settings from DB");
        }
    }
    default_settings(state)
}

/// One-time migration: read the settings ConfigMap and persist it into the
/// database so subsequent reads go straight to the DB. Returns the migrated
/// settings on success, or `None` if no ConfigMap exists.
async fn migrate_settings_from_configmap(state: &AppState) -> Option<DeckwatchSettings> {
    let api = match state.configmaps_api(&state.settings_namespace) {
        Ok(a) => a,
        Err(_) => return None,
    };
    let t = K8sTimer::new("configmaps", "get");
    let cm = match api.get(&state.settings_configmap_name).await {
        Ok(cm) => {
            t.finish(true);
            cm
        }
        Err(_) => {
            t.finish(false);
            return None;
        }
    };
    let settings = parse_settings(&cm);
    // Persist to DB so we never read the ConfigMap again.
    if let Err(e) = upsert_settings_to_db(&state.db, &settings).await {
        tracing::warn!(error = %e, "failed to seed DB from ConfigMap; will retry next read");
    } else {
        tracing::info!("migrated settings from ConfigMap to database");
    }
    Some(settings)
}

pub async fn put_settings(
    State(state): State<AppState>,
    Json(mut settings): Json<DeckwatchSettings>,
) -> Result<Json<DeckwatchSettings>, AppError> {
    // Strip the injected builtin entry before persisting -- it's derived
    // from the deployment env var, not user data.
    settings.oci_registries.retain(|r| !r.builtin);

    upsert_settings_to_db(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save settings: {e}")))?;

    if let Err(e) = crate::audit::log_action(
        &state.db,
        "update",
        "settings",
        "main",
        "",
        "updated application settings",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    let mut result = settings;
    inject_builtin_registry(&state, &mut result);

    Ok(Json(result))
}

/// Return the current UTC time as a `DateTimeUtc`.
fn now_utc() -> sea_orm::entity::prelude::DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    sea_orm::entity::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

/// Upsert the entire settings blob into the `settings` table as a single
/// JSON value with key `"main"`.
async fn upsert_settings_to_db(
    db: &sea_orm::DatabaseConnection,
    settings: &DeckwatchSettings,
) -> Result<(), sea_orm::DbErr> {
    let json_str = serde_json::to_string_pretty(settings)
        .map_err(|e| sea_orm::DbErr::Custom(format!("JSON serialization failed: {e}")))?;
    let now = now_utc();

    let model = settings_entity::ActiveModel {
        key: Set(DB_SETTINGS_KEY.to_string()),
        value: Set(json_str),
        updated_at: Set(now),
    };

    // Try to find existing row first; insert or update accordingly.
    let existing = settings_entity::Entity::find_by_id(DB_SETTINGS_KEY)
        .one(db)
        .await?;

    if existing.is_some() {
        settings_entity::Entity::update(model).exec(db).await?;
    } else {
        settings_entity::Entity::insert(model).exec(db).await?;
    }

    Ok(())
}

/// If this deckwatch instance runs the embedded registry, prepend it to
/// the OCI registries list so it shows up first in the GitOps dialog.
/// De-duped by name so it isn't added twice if someone persisted it by
/// mistake (older frontend).
fn inject_builtin_registry(state: &AppState, settings: &mut DeckwatchSettings) {
    let Some(url) = state.registry_public_url.as_deref() else {
        return;
    };
    settings
        .oci_registries
        .retain(|r| r.name != DECKWATCH_REGISTRY_NAME);
    let entry = OciRegistry {
        name: DECKWATCH_REGISTRY_NAME.to_string(),
        url: url.to_string(),
        registry_type: "deckwatch".to_string(),
        builtin: true,
    };
    let mut merged = Vec::with_capacity(settings.oci_registries.len() + 1);
    merged.push(entry);
    merged.append(&mut settings.oci_registries);
    settings.oci_registries = merged;
}

fn parse_settings(cm: &ConfigMap) -> DeckwatchSettings {
    cm.data
        .as_ref()
        .and_then(|d| d.get(SETTINGS_KEY))
        .and_then(|s| serde_json::from_str::<DeckwatchSettings>(s).ok())
        .unwrap_or_default()
}

fn default_settings(state: &AppState) -> DeckwatchSettings {
    DeckwatchSettings {
        allowed_namespaces: state.allowed_namespaces.clone(),
        default_resource_limits: None,
        auth: Some(AuthSettings::default()),
        notifications: Some(NotificationSettings::default()),
        git_repositories: Vec::new(),
        oci_registries: Vec::new(),
        git_token_secrets: Vec::new(),
        tracing: Some(TracingSettings::default()),
        prometheus_enabled: true,
        registry_enabled: false,
        ai_claude_enabled: true,
        ai_codex_enabled: true,
    }
}

pub async fn test_notification(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let client = NotificationClient::new(state);
    client
        .send_now(NotificationEvent::Test {
            source: "deckwatch settings page".to_string(),
        })
        .await
        .map_err(|e| AppError::BadRequest(format!("test notification failed: {e}")))?;
    Ok(Json(serde_json::json!({"status": "sent"})))
}

#[cfg(test)]
#[path = "../settings_tests.rs"]
mod settings_tests;
