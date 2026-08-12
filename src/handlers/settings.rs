use std::collections::BTreeMap;

use axum::extract::{Path, State};
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
    pub default_storage_class: Option<String>,
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
    /// Selects which AI backend provider to use for Claude API calls.
    /// Supports native Anthropic API, Google Vertex AI, and AWS Bedrock.
    /// Defaults to `native` with the standard API key secret.
    #[serde(default)]
    pub ai_provider: AiProviderConfig,
    /// Encrypted API credentials stored in the database. Values are
    /// AES-256-GCM encrypted with the `DECKWATCH_ENCRYPTION_KEY`. On GET,
    /// non-null entries are masked as `"configured"` -- the actual key is
    /// never returned to the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<EncryptedCredentials>,
    /// Named annotation presets for ingress creation. Admins define templates
    /// once (e.g. ALB annotations for EKS) and users pick one in the dialog.
    #[serde(default)]
    pub ingress_templates: Vec<IngressTemplate>,
    #[serde(default = "default_build_architectures")]
    pub build_architectures: Vec<BuildArchitecture>,
    #[serde(default = "default_build_settings")]
    pub build_settings: BuildSettings,
    /// External plugins to load at startup and on settings update. Each
    /// plugin is a compiled WASM binary fetched from a Git repository.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    /// Resource-scoped hints injected into MCP tool descriptions at `tools/list`
    /// time. Each hint is appended to the description of every tool in that
    /// resource group so the AI only sees it when the relevant tools are in scope.
    #[serde(default)]
    pub mcp_tuning: McpTuning,
}

/// Per-resource-group instruction hints for MCP tools.
///
/// Each field is injected into the `description` of tools in that group at
/// `tools/list` time. `global` is additionally included in the `initialize`
/// response `instructions` field (applies to all tool interactions).
///
/// Groups map to tools by name pattern:
/// - `namespaces`  → tools containing "namespace"
/// - `deployments` → tools containing "deployment" + scale/restart/rollback
/// - `applications`→ create_application, list_addons, attach_addon, detach_addon, list_templates
/// - `gitops`      → tools containing "gitops" or "build"
/// - `ingresses`   → tools containing "ingress"
/// - `pods`        → tools containing "pod"
/// - `secrets`     → tools containing "secret"
/// - `nodes`       → tools containing "node"
/// - `storage`     → tools containing "pvc", "pv", or "storageclass"
/// - `plugins`     → tools containing "plugin"
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpTuning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespaces: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applications: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gitops: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingresses: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pods: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<String>,
}

/// Encrypted credential storage. Each field holds an AES-256-GCM ciphertext
/// (base64 encoded) produced by [`crate::crypto::encrypt`]. Stored as-is in
/// the settings JSON blob in the database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EncryptedCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_sa_key: Option<String>,
}

/// Configuration for the AI provider backend. Tagged enum so the JSON
/// representation includes a `"type"` discriminator and only the fields
/// relevant to the chosen provider are present.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiProviderConfig {
    Native {
        #[serde(default = "default_native_secret")]
        api_key_secret: String,
    },
    VertexAi {
        project_id: String,
        region: String,
        #[serde(default = "default_vertex_secret")]
        sa_key_secret: String,
    },
    Bedrock {
        region: String,
        #[serde(default)]
        model_id: String,
    },
}

impl Default for AiProviderConfig {
    fn default() -> Self {
        Self::Native {
            api_key_secret: default_native_secret(),
        }
    }
}

fn default_native_secret() -> String {
    "deckwatch-anthropic-api-key".to_string()
}

fn default_vertex_secret() -> String {
    "deckwatch-gcp-sa-key".to_string()
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
    pub name: String,
    #[serde(default)]
    pub secret_name: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_token: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngressTemplate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_class: Option<String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArchitecture {
    pub platform: String,
    pub arch: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildSettings {
    #[serde(default = "default_kaniko_image")]
    pub kaniko_image: String,
    #[serde(default = "default_crane_image")]
    pub crane_image: String,
    #[serde(default = "default_platform_flag")]
    pub platform_flag: String,
    #[serde(default)]
    pub extra_kaniko_args: Vec<String>,
    #[serde(default = "default_true")]
    pub cache_enabled: bool,
    #[serde(default = "default_snapshot_mode")]
    pub snapshot_mode: String,
    #[serde(default)]
    pub docker_media_types: bool,
    #[serde(default = "default_job_ttl")]
    pub job_ttl_seconds: i32,
    #[serde(default)]
    pub kaniko_backoff_limit: i32,
    #[serde(default = "default_crane_backoff")]
    pub crane_backoff_limit: i32,
}

fn default_kaniko_image() -> String {
    "gcr.io/kaniko-project/executor:v1.24.0".to_string()
}

fn default_crane_image() -> String {
    "gcr.io/go-containerregistry/crane:latest".to_string()
}

fn default_platform_flag() -> String {
    "--custom-platform".to_string()
}

fn default_snapshot_mode() -> String {
    "redo".to_string()
}

fn default_job_ttl() -> i32 {
    3600
}

fn default_crane_backoff() -> i32 {
    1
}

pub fn default_build_settings() -> BuildSettings {
    BuildSettings {
        kaniko_image: default_kaniko_image(),
        crane_image: default_crane_image(),
        platform_flag: default_platform_flag(),
        extra_kaniko_args: Vec::new(),
        cache_enabled: true,
        snapshot_mode: default_snapshot_mode(),
        docker_media_types: false,
        job_ttl_seconds: default_job_ttl(),
        kaniko_backoff_limit: 0,
        crane_backoff_limit: default_crane_backoff(),
    }
}

pub fn default_build_architectures() -> Vec<BuildArchitecture> {
    vec![
        BuildArchitecture {
            platform: "linux/amd64".into(),
            arch: "amd64".into(),
            enabled: true,
        },
        BuildArchitecture {
            platform: "linux/arm64".into(),
            arch: "arm64".into(),
            enabled: true,
        },
    ]
}

/// Where deckwatch fetches the compiled WASM binary for a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginSource {
    /// Fetch a file from a GitHub repository via raw.githubusercontent.com
    /// (when `use_release` is false) or from a GitHub Release asset
    /// (when `use_release` is true).
    Github {
        /// `"owner/repo"` — e.g. `"alexconrey/deckwatch-plugin-example"`.
        repo: String,
        /// Git ref: tag, branch, or full SHA.
        #[serde(rename = "ref")]
        git_ref: String,
        /// Path to the `.wasm` file within the repo or release assets.
        path: String,
        /// When true, download from GitHub Releases instead of raw file.
        #[serde(default)]
        use_release: bool,
    },
    /// Arbitrary HTTPS URL — for self-hosted Gitea, Forgejo, S3, etc.
    Url { url: String },
    /// Locally uploaded WASM binary stored on the deckwatch server.
    /// Set automatically by `POST /api/plugins/{name}/upload`. Intended for
    /// local development — upload a locally-built WASM without pushing to GitHub.
    Upload {
        /// Filename under the uploads directory (e.g. `"aws.wasm"`).
        filename: String,
    },
}

/// Configuration for a single external plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Unique name used in annotation keys (e.g. `deckwatch.plugin-env/<name>`).
    /// Must be a valid Kubernetes annotation suffix: lowercase alphanumeric + `-`.
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: PluginSource,
    /// Name of a `git_token_secrets` entry to use for authenticated fetches
    /// (private repos). Leave unset for public repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_secret: Option<String>,
    /// Hosts the plugin is permitted to reach via extism's HTTP host function.
    /// Supports glob patterns, e.g. `"*.amazonaws.com"`, `"vault.corp.internal"`.
    /// An empty list denies all outbound HTTP from the plugin.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Arbitrary key-value pairs injected into the plugin's extism config
    /// namespace. The plugin reads these via `extism_pdk::config::get("KEY")`.
    ///
    /// Use this to pass credentials, endpoints, or any cloud-specific config
    /// without deckwatch needing to know about the underlying cloud provider.
    /// For example an AWS plugin operator would set `AWS_ACCESS_KEY_ID`,
    /// `AWS_SECRET_ACCESS_KEY`, and `AWS_REGION` here; a GCP plugin operator
    /// would set `GCP_SERVICE_ACCOUNT_JSON`; and so on.
    ///
    /// Values are stored in the settings blob — use `git_token_secrets` for
    /// values that need rotation, or store long-lived credentials in a
    /// Kubernetes Secret and reference it from your plugin's logic.
    #[serde(default)]
    pub config: std::collections::BTreeMap<String, String>,
    /// Environment variable names to read from the deckwatch process environment
    /// and inject into the plugin's extism config at invocation time. Injected
    /// values overwrite any same-named key in `config`, so live credentials
    /// (e.g. rotated by a Kubernetes secret or IRSA) always win over static
    /// config entries.
    ///
    /// Use this when credentials are already present as pod environment variables
    /// rather than stored in settings. For example, to pass through AWS credentials
    /// that are mounted as env vars via a Kubernetes Secret:
    /// `inherit_env_keys: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION"]`
    #[serde(default)]
    pub inherit_env_keys: Vec<String>,
    /// Reads the contents of files whose paths are stored in environment variables,
    /// injecting the file content into the plugin's extism config.
    ///
    /// Keys are the config key to inject; values are the env var whose value is
    /// the file path. For example, to pass a workload identity token to a plugin:
    /// `{ "AWS_IDENTITY_TOKEN": "AWS_WEB_IDENTITY_TOKEN_FILE" }`
    ///
    /// Deckwatch reads `$AWS_WEB_IDENTITY_TOKEN_FILE` from its filesystem and
    /// injects the file content as `AWS_IDENTITY_TOKEN` into the plugin config.
    /// This is cloud-agnostic — deckwatch has no knowledge of what the file
    /// contains. The plugin is responsible for using it (e.g. STS exchange).
    ///
    /// Injected after `inherit_env_keys` so file contents override env vars
    /// of the same name.
    #[serde(default)]
    pub inherit_env_file_keys: std::collections::BTreeMap<String, String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<DeckwatchSettings>, AppError> {
    let mut settings = load_settings_from_db(&state).await;
    inject_builtin_registry(&state, &mut settings);
    // Never return actual encrypted keys to the frontend. Replace non-null
    // values with the sentinel "configured" so the UI can show a badge.
    mask_credentials(&mut settings);
    Ok(Json(settings))
}

/// Replace actual encrypted credential values with `"configured"` so the
/// API never leaks ciphertext (or plaintext) to the frontend.
fn mask_credentials(settings: &mut DeckwatchSettings) {
    if let Some(creds) = &mut settings.credentials {
        if creds.anthropic_api_key.is_some() {
            creds.anthropic_api_key = Some("configured".to_string());
        }
        if creds.gcp_sa_key.is_some() {
            creds.gcp_sa_key = Some("configured".to_string());
        }
    }
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

    for t in &mut settings.git_token_secrets {
        if t.namespace.is_empty() {
            t.namespace.clone_from(&state.settings_namespace);
        }
    }

    upsert_settings_to_db(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save settings: {e}")))?;

    // Re-fetch plugins in the background so the response isn't delayed by
    // network I/O. The new plugin set is live for subsequent deploys.
    if !settings.plugins.is_empty() {
        let state_clone = state.clone();
        let plugins_snapshot = settings.plugins.clone();
        tokio::spawn(async move {
            let git_token_secrets = load_settings_from_db(&state_clone).await.git_token_secrets;
            let s = DeckwatchSettings {
                git_token_secrets,
                plugins: plugins_snapshot,
                ..Default::default()
            };
            let loaded = crate::plugins::fetch_plugins(&s, &state_clone).await;
            tracing::info!(count = loaded.len(), "plugin refresh complete");
            *state_clone.plugins.write().await = loaded;
        });
    }

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
pub async fn upsert_settings_to_db_pub(
    db: &sea_orm::DatabaseConnection,
    settings: &DeckwatchSettings,
) -> Result<(), sea_orm::DbErr> {
    upsert_settings_to_db(db, settings).await
}

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
        default_storage_class: None,
        auth: Some(AuthSettings::default()),
        notifications: Some(NotificationSettings::default()),
        git_repositories: Vec::new(),
        oci_registries: Vec::new(),
        git_token_secrets: Vec::new(),
        tracing: Some(TracingSettings::default()),
        prometheus_enabled: true,
        ai_claude_enabled: true,
        ai_codex_enabled: true,
        ai_provider: AiProviderConfig::default(),
        credentials: None,
        ingress_templates: Vec::new(),
        build_architectures: default_build_architectures(),
        build_settings: default_build_settings(),
        plugins: Vec::new(),
        mcp_tuning: McpTuning::default(),
    }
}

// ---- Credential management ----

/// Request body for `POST /api/settings/credentials`. The frontend sends
/// plaintext values; we encrypt them before persisting.
#[derive(Debug, Deserialize)]
pub struct SetCredentialsRequest {
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub gcp_sa_key: Option<String>,
}

/// Response returned after setting credentials -- mirrors what GET returns
/// (masked values, never the ciphertext).
#[derive(Debug, Serialize)]
pub struct SetCredentialsResponse {
    pub anthropic_api_key: Option<String>,
    pub gcp_sa_key: Option<String>,
}

/// `POST /api/settings/credentials` -- encrypt and store API keys.
///
/// Only the keys present in the request body are updated; omitted keys are
/// left unchanged so the frontend can update one provider without clearing
/// the other. Sending an explicit empty string (`""`) clears that key.
pub async fn set_credentials(
    State(state): State<AppState>,
    Json(req): Json<SetCredentialsRequest>,
) -> Result<Json<SetCredentialsResponse>, AppError> {
    if state.encryption_key.is_empty() {
        return Err(AppError::BadRequest(
            "DECKWATCH_ENCRYPTION_KEY is not set — cannot store encrypted credentials".to_string(),
        ));
    }

    let mut settings = load_settings_from_db(&state).await;
    let mut creds = settings.credentials.take().unwrap_or_default();

    // Anthropic API key.
    if let Some(raw) = &req.anthropic_api_key {
        if raw.is_empty() {
            creds.anthropic_api_key = None;
        } else {
            let encrypted = crate::crypto::encrypt(&state.encryption_key, raw)
                .map_err(|e| AppError::BadRequest(format!("encryption failed: {e}")))?;
            creds.anthropic_api_key = Some(encrypted);
        }
    }

    // GCP service account key.
    if let Some(raw) = &req.gcp_sa_key {
        if raw.is_empty() {
            creds.gcp_sa_key = None;
        } else {
            let encrypted = crate::crypto::encrypt(&state.encryption_key, raw)
                .map_err(|e| AppError::BadRequest(format!("encryption failed: {e}")))?;
            creds.gcp_sa_key = Some(encrypted);
        }
    }

    let has_any = creds.anthropic_api_key.is_some() || creds.gcp_sa_key.is_some();
    settings.credentials = if has_any { Some(creds) } else { None };

    upsert_settings_to_db(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save credentials: {e}")))?;

    if let Err(e) = crate::audit::log_action(
        &state.db,
        "update",
        "settings",
        "credentials",
        "",
        "updated encrypted credentials",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log for credential update");
    }

    // Return masked values.
    let resp = SetCredentialsResponse {
        anthropic_api_key: if settings
            .credentials
            .as_ref()
            .and_then(|c| c.anthropic_api_key.as_ref())
            .is_some()
        {
            Some("configured".to_string())
        } else {
            None
        },
        gcp_sa_key: if settings
            .credentials
            .as_ref()
            .and_then(|c| c.gcp_sa_key.as_ref())
            .is_some()
        {
            Some("configured".to_string())
        } else {
            None
        },
    };

    Ok(Json(resp))
}

pub async fn list_ingress_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<IngressTemplate>>, AppError> {
    let settings = load_settings_from_db(&state).await;
    Ok(Json(settings.ingress_templates))
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

// ---- Git token secret management ----

#[derive(Debug, Deserialize)]
pub struct GitTokenSecretRequest {
    pub name: String,
    #[serde(default)]
    pub secret_name: Option<String>,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct GitTokenSecretResponse {
    pub name: String,
    pub secret_name: String,
    pub namespace: String,
}

pub async fn put_git_token_secret(
    State(state): State<AppState>,
    Json(req): Json<GitTokenSecretRequest>,
) -> Result<Json<GitTokenSecretResponse>, AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("display name is required".to_string()));
    }
    if req.token.is_empty() {
        return Err(AppError::BadRequest("token is required".to_string()));
    }

    let encrypted = crate::crypto::encrypt(&state.encryption_key, &req.token)
        .map_err(|e| AppError::BadRequest(format!("encryption failed: {e}")))?;

    let mut settings = load_settings_from_db(&state).await;
    let secret_name;
    if let Some(existing) = settings
        .git_token_secrets
        .iter_mut()
        .find(|e| e.name == req.name)
    {
        existing.encrypted_token = Some(encrypted);
        secret_name = existing.secret_name.clone();
    } else {
        secret_name = req.secret_name.unwrap_or_default();
        settings.git_token_secrets.push(GitTokenSecret {
            name: req.name.clone(),
            secret_name: secret_name.clone(),
            namespace: String::new(),
            encrypted_token: Some(encrypted),
        });
    }
    upsert_settings_to_db(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save settings: {e}")))?;

    if let Err(e) = crate::audit::log_action(
        &state.db,
        "create",
        "git-token",
        &req.name,
        "",
        &format!("created/updated git token '{}'", req.name),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log for git token");
    }

    Ok(Json(GitTokenSecretResponse {
        name: req.name,
        secret_name,
        namespace: String::new(),
    }))
}

pub async fn delete_git_token_secret(
    State(state): State<AppState>,
    Path(token_name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut settings = load_settings_from_db(&state).await;
    let before = settings.git_token_secrets.len();
    settings.git_token_secrets.retain(|e| e.name != token_name);
    if settings.git_token_secrets.len() == before {
        return Err(AppError::NotFound(format!(
            "git token '{token_name}' not found"
        )));
    }
    upsert_settings_to_db(&state.db, &settings)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to save settings: {e}")))?;

    if let Err(e) = crate::audit::log_action(
        &state.db,
        "delete",
        "git-token",
        &token_name,
        "",
        &format!("deleted git token '{token_name}'"),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log for git token secret deletion");
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod settings_tests;
