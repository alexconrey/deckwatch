//! External WASM plugin host.
//!
//! Deckwatch plugins are compiled WASM binaries hosted in Git repositories.
//! This module handles fetching them at startup/settings-update, executing
//! them per deployment operation, and merging their output into the
//! Kubernetes Deployment object before it reaches the API server.
//!
//! See `docs/PLUGINS.md` for the operator and developer guide.
//! See `deckwatch-plugin-sdk` for the shared types crate that plugin authors depend on.

use std::collections::BTreeMap;

use extism::{Function, Manifest, Plugin, UserData, Val, ValType, Wasm};
use k8s_openapi::api::core::v1::{
    ConfigMapKeySelector, Container, ContainerPort, EnvVar, EnvVarSource, ResourceRequirements,
    SecretKeySelector,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{DynamicObject, Patch, PatchParams};
use kube::Api;
use serde::{Deserialize, Serialize};

use crate::handlers::settings::{DeckwatchSettings, PluginConfig, PluginSource};
use crate::state::AppState;

// ── Shared types (mirror of deckwatch-plugin-sdk v0.5.0) ─────────────────────
// These must stay in sync with `deckwatch-plugin-sdk/src/lib.rs`. The SDK is
// the canonical source; this is the host-side copy for deserializing plugin
// output without adding a workspace dependency on the SDK crate.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContext {
    pub namespace: String,
    pub deployment_name: String,
    pub annotations: std::collections::HashMap<String, String>,
    pub labels: std::collections::HashMap<String, String>,
    /// Outputs from plugins that have already run this invocation.
    /// Populated by `apply_plugins` before each plugin call.
    #[serde(default)]
    pub plugin_outputs:
        std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

/// Structured plugin output requesting SA creation/reconciliation.
///
/// When a plugin sets this field, deckwatch's own SA handler creates or patches
/// the ServiceAccount (with retry on 409) instead of going through the generic
/// `kubernetes_resources` path. This gives full visibility in the deckwatch UI
/// and audit log, and avoids silent failures when the ClusterRole is missing.
///
/// Mirrors `deckwatch_plugin_sdk::WantsServiceAccount` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WantsServiceAccount {
    pub name: String,
    /// IRSA role ARN for `eks.amazonaws.com/role-arn` annotation.
    /// Empty string means no IRSA — a plain SA is created.
    #[serde(default)]
    pub irsa_role_arn: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginResult {
    #[serde(default)]
    pub env_vars: Vec<EnvVarSpec>,
    #[serde(default)]
    pub sidecars: Vec<SidecarSpec>,
    #[serde(default)]
    pub kubernetes_resources: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,
    /// Structured SA request — preferred over emitting a raw SA in
    /// `kubernetes_resources`. Deckwatch handles create/patch with retry and
    /// surfaces the result in the UI and audit log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wants_service_account: Option<WantsServiceAccount>,
    /// Key-value data shared with downstream plugins via `ctx.plugin_outputs`.
    #[serde(default)]
    pub outputs: std::collections::HashMap<String, String>,

    /// Errors the plugin wants surfaced in deckwatch's structured logs.
    ///
    /// Plugins populate this instead of calling `extism_pdk::log!()` — the
    /// extism log pipe cannot be bridged into deckwatch's tracing subscriber
    /// without a global callback conflict. Each entry is logged at ERROR level
    /// after `apply()` returns; the deployment is not blocked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvVarSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_from: Option<EnvVarSourceSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarSourceSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key_ref: Option<SecretKeyRefSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_map_key_ref: Option<ConfigMapKeyRefSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretKeyRefSpec {
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMapKeyRefSpec {
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSpec {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub env: Vec<EnvVarSpec>,
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
}

// ── Annotation key prefixes ───────────────────────────────────────────────────

const PLUGIN_ENV_ANNOTATION_PREFIX: &str = "deckwatch.plugin-env/";
const PLUGIN_SIDECAR_ANNOTATION_PREFIX: &str = "deckwatch.plugin-sidecar/";

// ── LoadedPlugin ─────────────────────────────────────────────────────────────

/// The data type of a plugin configuration field.
///
/// Mirrors `deckwatch_plugin_sdk::ConfigFieldType` — kept in sync manually.
/// See `deckwatch-plugin-sdk/src/lib.rs` for the canonical definition.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    /// Plain text input. Stored in plugin config as-is.
    #[default]
    String,
    /// Masked text input. Stored encrypted in `PluginConfig.config` using the
    /// same AES-256-GCM envelope as `DeckwatchSettings.credentials`.
    Secret,
    /// Checkbox. Stored as `"true"` or `"false"`.
    Bool,
    /// Dropdown. The field must have a non-empty `options` list.
    Select,
}

/// A single field in a plugin's self-declared configuration schema.
///
/// Mirrors `deckwatch_plugin_sdk::ConfigField` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigField {
    /// Machine-readable key. Matched against keys in `PluginConfig.config` and `inherit_env_keys`.
    pub key: String,
    /// Human-readable label for the settings form.
    pub label: String,
    /// Help text rendered below the field.
    #[serde(default)]
    pub description: String,
    /// The type of form control to render.
    pub field_type: ConfigFieldType,
    /// Default value rendered in the form when no saved value exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Whether the field must be non-empty before deckwatch accepts settings.
    #[serde(default)]
    pub required: bool,
    /// Allowed values for `select` fields. Ignored for other field types.
    #[serde(default)]
    pub options: Vec<String>,
    /// When `Some`, this field is sourced from an environment variable rather
    /// than direct user input. The UI renders the field as read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_source: Option<String>,
}

/// Metadata returned by the plugin's `metadata()` WASM export.
/// Mirrors `deckwatch_plugin_sdk::PluginMetadata` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub optional_depends_on: Vec<String>,
    /// Schema for this plugin's operator-supplied configuration.
    /// Old plugins that do not export this field deserialize to an empty Vec.
    #[serde(default)]
    pub config_schema: Vec<ConfigField>,
    /// Resource types this plugin can provision (e.g. RDS, S3).
    /// Deckwatch reads this at load time to render Infrastructure buttons on Application pages.
    /// Old plugins that do not export this field deserialize to an empty Vec.
    #[serde(default)]
    pub resources: Vec<PluginResource>,
}

/// A provisionable infrastructure resource declared by a plugin.
/// Mirrors `deckwatch_plugin_sdk::PluginResource` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginResource {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub singleton: bool,
    #[serde(default)]
    pub fields: Vec<ConfigField>,
    #[serde(default)]
    pub output_keys: Vec<String>,
}

/// Request sent to the plugin's `provision()` WASM export.
/// Mirrors `deckwatch_plugin_sdk::ResourceProvisionRequest` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceProvisionRequest {
    pub application_name: String,
    pub namespace: String,
    pub resource_id: String,
    pub fields: std::collections::HashMap<String, String>,
}

/// Result returned from the plugin's `provision()` WASM export.
/// Mirrors `deckwatch_plugin_sdk::ResourceProvisionResult` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceProvisionResult {
    #[serde(default)]
    pub state: std::collections::HashMap<String, String>,
    /// Annotations to stamp on all application deployments (e.g. `"deckwatch.io/aws-s3-bucket"`).
    /// Old plugins without this field deserialize to an empty map.
    #[serde(default)]
    pub deployment_annotations: std::collections::HashMap<String, String>,
    /// Sidecars to inject into all application deployments and cronjobs.
    /// Old plugins that do not return this field deserialize to an empty vec.
    #[serde(default)]
    pub sidecars: Vec<SidecarSpec>,
    #[serde(default)]
    pub kubernetes_resources: Vec<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Input to a plugin's `deprovision()` WASM export.
/// Mirrors `deckwatch_plugin_sdk::ResourceDeprovisionRequest` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDeprovisionRequest {
    pub application_name: String,
    pub namespace: String,
    pub resource_id: String,
    #[serde(default)]
    pub state: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub fields: std::collections::HashMap<String, String>,
}

/// What the plugin returns from `deprovision()`.
/// Mirrors `deckwatch_plugin_sdk::ResourceDeprovisionResult` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceDeprovisionResult {
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub message: String,
}

/// A fetched plugin ready to execute. Stores raw WASM bytes and instantiates
/// a fresh `extism::Plugin` per call to avoid thread-safety concerns.
#[derive(Clone)]
pub struct LoadedPlugin {
    pub name: String,
    pub wasm_bytes: Vec<u8>,
    /// Hosts the plugin is allowed to reach via extism's HTTP host function.
    pub allowed_hosts: Vec<String>,
    /// Operator-supplied key-value config injected into the extism manifest.
    /// Cloud credentials, endpoints, and any plugin-specific settings go here.
    pub config: std::collections::BTreeMap<String, String>,
    /// Environment variable names to read from the deckwatch process environment
    /// and inject into the extism manifest config at invocation time.
    /// These override any same-named key in `config` so that live/rotated
    /// credentials always take precedence over static config entries.
    pub inherit_env_keys: Vec<String>,
    /// Reads file contents from paths stored in env vars, injecting into extism
    /// config. Map of config_key → env_var_holding_file_path. Applied after
    /// `inherit_env_keys` so file contents take final precedence.
    pub inherit_env_file_keys: std::collections::BTreeMap<String, String>,
    /// Metadata from the plugin's `metadata()` export — populated at load time.
    /// Plugins without a `metadata()` export get a default with no dependencies.
    pub metadata: PluginMetadata,
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Result of a plugin dry-run validation. Returned by [`fetch_and_validate`].
pub struct ValidationResult {
    /// Size of the fetched WASM binary in bytes.
    pub wasm_size_bytes: usize,
    /// Whether the `apply` function was found and callable.
    pub apply_export_found: bool,
    /// The test context that was passed to `apply`.
    pub test_context: PluginContext,
    /// The result returned by `apply`, if it succeeded.
    pub result: Option<PluginResult>,
    /// Error message if fetch or execution failed.
    pub error: Option<String>,
}

/// Fetch a plugin from its configured source, validate that it loads as a
/// WASM module with an `apply` export, and dry-run it against `test_ctx`.
///
/// Used by the `validate_plugin` MCP tool. Does not modify any state.
pub async fn fetch_and_validate(
    cfg: &PluginConfig,
    test_ctx: PluginContext,
    state: &AppState,
) -> ValidationResult {
    let settings = crate::handlers::settings::load_settings_from_db(state).await;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let bytes = match fetch_bytes(cfg, &http, &settings, state).await {
        Ok(b) => b,
        Err(e) => {
            return ValidationResult {
                wasm_size_bytes: 0,
                apply_export_found: false,
                test_context: test_ctx,
                result: None,
                error: Some(format!("fetch failed: {e}")),
            };
        }
    };

    let size = bytes.len();
    let plugin = LoadedPlugin {
        name: "__validation__".to_string(),
        wasm_bytes: bytes,
        allowed_hosts: cfg.allowed_hosts.clone(),
        config: cfg.config.clone(),
        inherit_env_keys: cfg.inherit_env_keys.clone(),
        inherit_env_file_keys: cfg.inherit_env_file_keys.clone(),
        metadata: PluginMetadata::default(),
    };

    match run_plugin(&plugin, &test_ctx) {
        Ok(result) => ValidationResult {
            wasm_size_bytes: size,
            apply_export_found: true,
            test_context: test_ctx,
            result: Some(result),
            error: None,
        },
        Err(e) => ValidationResult {
            wasm_size_bytes: size,
            // If we got past the fetch but extism failed, distinguish between
            // "apply not found" and "apply threw an error".
            apply_export_found: !e.to_string().contains("not found"),
            test_context: test_ctx,
            result: None,
            error: Some(format!("apply() failed: {e}")),
        },
    }
}

// ── Fetching ─────────────────────────────────────────────────────────────────

/// Build the `now` host function exposed to every plugin.
///
/// Plugins call `now()` (imported from `extism:host/user`) to get the current
/// Unix timestamp in seconds. This lets the AWS plugin sign requests without
/// making a network round-trip and without touching `SystemTime` (unavailable
/// in `wasm32-unknown-unknown`).
fn host_now_fn() -> Function {
    Function::new(
        "now",
        [],
        [ValType::I64],
        UserData::<()>::default(),
        |_ctx, _inputs, outputs, _ud| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            outputs[0] = Val::I64(secs as i64);
            Ok(())
        },
    )
    .with_namespace("extism:host/user")
}

/// Call the plugin's `metadata()` export and deserialize the result.
/// Returns an error if the export is missing or the output can't be parsed —
/// callers should fall back to `PluginMetadata::default()`.
fn load_metadata_from_bytes(
    wasm_bytes: &[u8],
    allowed_hosts: &[String],
    config: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<PluginMetadata> {
    let wasm = Wasm::data(wasm_bytes.to_vec());
    let mut manifest = Manifest::new([wasm]);
    if !allowed_hosts.is_empty() {
        manifest.allowed_hosts = Some(allowed_hosts.to_vec());
    }
    manifest.config.extend(config.clone());
    let mut p = Plugin::new(&manifest, [host_now_fn()], false)?;
    let output = p.call::<&str, &str>("metadata", "")?;
    Ok(serde_json::from_str(output)?)
}

/// Fetch and load all enabled plugins from their configured sources.
/// Per-plugin errors are logged and skipped; a broken plugin never blocks startup.
pub async fn fetch_plugins(settings: &DeckwatchSettings, state: &AppState) -> Vec<LoadedPlugin> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let mut loaded = Vec::new();
    for cfg in &settings.plugins {
        if !cfg.enabled {
            tracing::debug!(plugin = %cfg.name, "plugin disabled, skipping");
            continue;
        }
        match fetch_bytes(cfg, &http, settings, state).await {
            Ok(bytes) => {
                tracing::info!(plugin = %cfg.name, bytes = bytes.len(), "loaded plugin");
                let metadata = load_metadata_from_bytes(&bytes, &cfg.allowed_hosts, &cfg.config)
                    .unwrap_or_else(|e| {
                        tracing::debug!(plugin = %cfg.name, error = %e, "metadata() not exported or failed; using defaults");
                        PluginMetadata { name: cfg.name.clone(), ..Default::default() }
                    });
                loaded.push(LoadedPlugin {
                    name: cfg.name.clone(),
                    wasm_bytes: bytes,
                    allowed_hosts: cfg.allowed_hosts.clone(),
                    config: cfg.config.clone(),
                    inherit_env_keys: cfg.inherit_env_keys.clone(),
                    inherit_env_file_keys: cfg.inherit_env_file_keys.clone(),
                    metadata,
                });
            }
            Err(e) => {
                tracing::error!(plugin = %cfg.name, error = %e, "failed to fetch plugin WASM");
            }
        }
    }
    loaded
}

/// Directory where uploaded WASM binaries are stored.
pub const UPLOADS_DIR: &str = "/data/uploads";

async fn fetch_bytes(
    cfg: &PluginConfig,
    http: &reqwest::Client,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> anyhow::Result<Vec<u8>> {
    // Uploaded WASMs are read from disk — no HTTP needed.
    if let PluginSource::Upload { filename } = &cfg.source {
        let path = std::path::Path::new(UPLOADS_DIR).join(filename);
        return tokio::fs::read(&path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read uploaded plugin {filename}: {e}"));
    }

    let url = resolve_url(&cfg.source);
    let mut builder = http.get(&url);

    if let Some(token_secret_name) = &cfg.token_secret {
        if let Some(token) = resolve_token(token_secret_name, settings, state).await {
            builder = builder.bearer_auth(token);
        }
    }

    let resp = builder.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status} fetching plugin from {url}"));
    }
    Ok(resp.bytes().await?.to_vec())
}

fn resolve_url(source: &PluginSource) -> String {
    match source {
        PluginSource::Github {
            repo,
            git_ref,
            path,
            use_release,
        } => {
            if *use_release {
                format!("https://github.com/{repo}/releases/download/{git_ref}/{path}")
            } else {
                format!("https://raw.githubusercontent.com/{repo}/{git_ref}/{path}")
            }
        }
        PluginSource::Url { url } => url.clone(),
        // Upload source has no URL — handled in fetch_bytes before this is called.
        PluginSource::Upload { filename } => {
            format!("file://{UPLOADS_DIR}/{filename}")
        }
    }
}

async fn resolve_token(
    secret_ref_name: &str,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> Option<String> {
    let entry = settings
        .git_token_secrets
        .iter()
        .find(|s| s.name == secret_ref_name)?;

    if let Some(enc) = &entry.encrypted_token {
        if !state.encryption_key.is_empty() {
            return crate::crypto::decrypt(&state.encryption_key, enc).ok();
        }
    }

    let ns = if entry.namespace.is_empty() {
        &state.settings_namespace
    } else {
        &entry.namespace
    };
    let secrets_api = state.secrets_api(ns).ok()?;
    let secret = secrets_api.get(&entry.secret_name).await.ok()?;
    let raw = secret.data?.get("token")?.0.clone();
    String::from_utf8(raw).ok()
}

// ── Dependency resolution ─────────────────────────────────────────────────────

/// Topologically sort plugins so those that `provides` a capability run before
/// those that `depends_on` or `optional_depends_on` that capability.
///
/// Uses Kahn's algorithm. Plugins with no dependency edges keep their original
/// relative order. Cycles are detected and broken with a warning (the full set
/// is still run).
pub fn sort_by_dependencies(plugins: &[LoadedPlugin]) -> Vec<&LoadedPlugin> {
    if plugins.len() <= 1 {
        return plugins.iter().collect();
    }

    // Map capability → indices of plugins that provide it.
    let mut providers: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, p) in plugins.iter().enumerate() {
        for cap in &p.metadata.provides {
            providers.insert(cap.as_str(), i);
        }
    }

    // Build adjacency: edges[i] = set of plugins that must run AFTER i.
    let n = plugins.len();
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

    for (i, p) in plugins.iter().enumerate() {
        let all_deps = p
            .metadata
            .depends_on
            .iter()
            .chain(p.metadata.optional_depends_on.iter());
        for cap in all_deps {
            if let Some(&provider_idx) = providers.get(cap.as_str()) {
                if provider_idx != i {
                    adj[provider_idx].push(i);
                    in_degree[i] += 1;
                }
            }
        }
    }

    // Kahn's BFS.
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut sorted: Vec<usize> = Vec::with_capacity(n);

    while let Some(node) = queue.pop_front() {
        sorted.push(node);
        for &next in &adj[node] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push_back(next);
            }
        }
    }

    // Append any remaining (cycle participants) in original order.
    if sorted.len() < n {
        let in_sorted: std::collections::HashSet<usize> = sorted.iter().copied().collect();
        let remaining: Vec<usize> = (0..n).filter(|i| !in_sorted.contains(i)).collect();
        tracing::warn!(
            plugins = ?remaining.iter().map(|&i| &plugins[i].name).collect::<Vec<_>>(),
            "plugin dependency cycle detected; running in original order"
        );
        sorted.extend(remaining);
    }

    sorted.iter().map(|&i| &plugins[i]).collect()
}

// ── Applying ─────────────────────────────────────────────────────────────────

/// Run all loaded plugins against `ctx`, merging env vars and sidecars into
/// `pod_spec`/`dep_annotations`. Returns a tuple of:
/// - `Vec<serde_json::Value>` — raw Kubernetes resources emitted by plugins
///   (applied via `apply_kubernetes_resources` after the deployment is committed)
/// - `Vec<WantsServiceAccount>` — structured SA requests that should be
///   handled via `apply_wanted_service_accounts` with create-or-patch retry
///
/// Plugins are sorted by declared dependency before execution. Each plugin
/// receives the accumulated `outputs` from all prior plugins in its context.
pub fn apply_plugins(
    plugins: &[LoadedPlugin],
    ctx: &PluginContext,
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
) -> (Vec<serde_json::Value>, Vec<WantsServiceAccount>) {
    let mut all_k8s_resources: Vec<serde_json::Value> = Vec::new();
    let mut all_wanted_sas: Vec<WantsServiceAccount> = Vec::new();
    let mut accumulated_outputs: std::collections::HashMap<
        String,
        std::collections::HashMap<String, String>,
    > = std::collections::HashMap::new();

    for plugin in sort_by_dependencies(plugins) {
        // Build enriched context with outputs from prior plugins.
        let mut enriched_ctx = ctx.clone();
        enriched_ctx.plugin_outputs = accumulated_outputs.clone();

        match run_plugin(plugin, &enriched_ctx) {
            Ok(result) => {
                apply_env_vars(plugin, &result.env_vars, pod_spec, dep_annotations);
                apply_sidecars(plugin, &result.sidecars, pod_spec, dep_annotations);
                all_k8s_resources.extend(result.kubernetes_resources.clone());
                if let Some(sa) = result.service_account_name.clone() {
                    apply_service_account(plugin, sa, pod_spec, dep_annotations);
                }
                if let Some(wanted_sa) = result.wants_service_account.clone() {
                    // Set the SA name on the pod spec so pods bind to it immediately.
                    apply_service_account(
                        plugin,
                        wanted_sa.name.clone(),
                        pod_spec,
                        dep_annotations,
                    );
                    all_wanted_sas.push(wanted_sa);
                }
                // Record this plugin's outputs for subsequent plugins.
                if !result.outputs.is_empty() {
                    accumulated_outputs.insert(plugin.name.clone(), result.outputs);
                }
            }
            Err(e) => {
                tracing::error!(plugin = %plugin.name, error = %e, "plugin apply() failed; skipping");
            }
        }
    }

    (all_k8s_resources, all_wanted_sas)
}

/// Create or patch ServiceAccounts declared via `WantsServiceAccount` plugin
/// output. Called after the deployment is committed; errors are logged but do
/// not fail the request. Uses deckwatch's SA handler which provides retry
/// semantics and full UI/audit-log visibility.
pub async fn apply_wanted_service_accounts(
    wanted: &[WantsServiceAccount],
    namespace: &str,
    kube_client: &kube::Client,
) {
    for sa in wanted {
        crate::handlers::serviceaccounts::ensure_service_account(
            kube_client,
            namespace,
            &sa.name,
            &sa.irsa_role_arn,
        )
        .await;
    }
}

/// Apply a list of Kubernetes resources collected from plugins via server-side
/// apply. Called after the deployment is committed; errors are logged but do
/// not fail the request. Resources that reference CRDs not installed in the
/// cluster will fail here with a clear log message.
pub async fn apply_kubernetes_resources(
    resources: &[serde_json::Value],
    kube_client: &kube::Client,
) {
    for resource in resources {
        if let Err(e) = apply_one_resource(resource, kube_client).await {
            let kind = resource
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let name = resource
                .pointer("/metadata/name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::error!(kind, name, error = %e, "plugin kubernetes_resource apply failed");
        }
    }
}

async fn apply_one_resource(
    resource: &serde_json::Value,
    client: &kube::Client,
) -> anyhow::Result<()> {
    let api_version = resource["apiVersion"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing apiVersion"))?;
    let kind = resource["kind"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing kind"))?;
    let name = resource
        .pointer("/metadata/name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing metadata.name"))?;
    let namespace = resource
        .pointer("/metadata/namespace")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let (group, version) = if let Some((g, v)) = api_version.split_once('/') {
        (g.to_string(), v.to_string())
    } else {
        (String::new(), api_version.to_string())
    };

    let gvk = kube::api::GroupVersionKind::gvk(&group, &version, kind);
    let (ar, _caps) = kube::discovery::pinned_kind(client, &gvk)
        .await
        .map_err(|e| anyhow::anyhow!("CRD {api_version}/{kind} not found in cluster: {e}"))?;

    let obj: DynamicObject = serde_json::from_value(resource.clone())?;
    let pp = PatchParams::apply("deckwatch-plugin").force();

    if namespace.is_empty() {
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
        api.patch(name, &pp, &Patch::Apply(&obj)).await?;
    } else {
        let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
        api.patch(name, &pp, &Patch::Apply(&obj)).await?;
    }

    tracing::info!(kind, name, namespace, "plugin resource applied");
    Ok(())
}

pub fn run_provision(
    plugin: &LoadedPlugin,
    req: &ResourceProvisionRequest,
) -> anyhow::Result<ResourceProvisionResult> {
    tracing::info!(
        plugin = %plugin.name,
        namespace = %req.namespace,
        application = %req.application_name,
        resource_id = %req.resource_id,
        "calling plugin provision()"
    );

    let wasm = Wasm::data(plugin.wasm_bytes.clone());
    let mut manifest = Manifest::new([wasm]);

    if !plugin.allowed_hosts.is_empty() {
        manifest.allowed_hosts = Some(plugin.allowed_hosts.clone());
    }
    manifest.config.extend(plugin.config.clone());

    // Inject the current timestamp so plugins can sign AWS requests without
    // needing a system clock (unavailable in wasm32-unknown-unknown).
    // Injected at call time so it is always fresh — plugins read it via
    // extism_pdk::config::get("CURRENT_TIMESTAMP").
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    manifest
        .config
        .insert("CURRENT_TIMESTAMP".to_string(), now_ts.to_string());

    for key in &plugin.inherit_env_keys {
        if let Ok(val) = std::env::var(key) {
            manifest.config.insert(key.clone(), val);
        }
    }

    for (config_key, env_var) in &plugin.inherit_env_file_keys {
        if let Ok(path) = std::env::var(env_var) {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    manifest
                        .config
                        .insert(config_key.clone(), content.trim().to_string());
                }
                Err(e) => {
                    tracing::warn!(
                        plugin = %plugin.name,
                        config_key = %config_key,
                        path = %path,
                        error = %e,
                        "inherit_env_file_keys: failed to read file"
                    );
                }
            }
        }
    }

    let mut p = Plugin::new(&manifest, [host_now_fn()], false)?;
    let input = serde_json::to_string(req)?;
    let output = p.call::<&str, &str>("provision", &input)?;
    let result: ResourceProvisionResult = serde_json::from_str(output)?;

    for err in &result.errors {
        tracing::error!(
            plugin = %plugin.name,
            namespace = %req.namespace,
            application = %req.application_name,
            resource_id = %req.resource_id,
            "plugin provision() reported error: {err}"
        );
    }

    tracing::info!(
        plugin = %plugin.name,
        namespace = %req.namespace,
        application = %req.application_name,
        resource_id = %req.resource_id,
        state_keys = result.state.len(),
        k8s_resources = result.kubernetes_resources.len(),
        errors = result.errors.len(),
        "plugin provision() completed"
    );

    Ok(result)
}

/// Call the plugin's `deprovision()` WASM export with the given request.
///
/// Returns `Ok(ResourceDeprovisionResult)` even if the plugin reports errors —
/// errors are surfaced to the caller for logging, but do not block DB record removal.
/// Returns `Err` only if the WASM call itself fails (e.g. export not found).
pub fn run_deprovision(
    plugin: &LoadedPlugin,
    req: &ResourceDeprovisionRequest,
) -> anyhow::Result<ResourceDeprovisionResult> {
    tracing::info!(
        plugin = %plugin.name,
        namespace = %req.namespace,
        application = %req.application_name,
        resource_id = %req.resource_id,
        "calling plugin deprovision()"
    );

    let wasm = Wasm::data(plugin.wasm_bytes.clone());
    let mut manifest = Manifest::new([wasm]);
    if !plugin.allowed_hosts.is_empty() {
        manifest.allowed_hosts = Some(plugin.allowed_hosts.clone());
    }
    manifest.config.extend(plugin.config.clone());

    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    manifest
        .config
        .insert("CURRENT_TIMESTAMP".to_string(), now_ts.to_string());

    for key in &plugin.inherit_env_keys {
        if let Ok(val) = std::env::var(key) {
            manifest.config.insert(key.clone(), val);
        }
    }
    for (config_key, env_var) in &plugin.inherit_env_file_keys {
        if let Ok(path) = std::env::var(env_var) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                manifest
                    .config
                    .insert(config_key.clone(), content.trim().to_string());
            }
        }
    }

    let mut p = Plugin::new(&manifest, [host_now_fn()], false)?;
    let input = serde_json::to_string(req)?;

    let output = p.call::<&str, &str>("deprovision", &input).map_err(|e| {
        // deprovision() is optional — if the export doesn't exist, return a default result
        if e.to_string().contains("not found") || e.to_string().contains("export") {
            return anyhow::anyhow!("__no_deprovision_export__");
        }
        e
    });

    match output {
        Ok(out) => {
            let result: ResourceDeprovisionResult = serde_json::from_str(out)?;
            for err in &result.errors {
                tracing::error!(
                    plugin = %plugin.name,
                    resource_id = %req.resource_id,
                    "plugin deprovision() reported error: {err}"
                );
            }
            tracing::info!(
                plugin = %plugin.name,
                resource_id = %req.resource_id,
                errors = result.errors.len(),
                message = %result.message,
                "plugin deprovision() completed"
            );
            Ok(result)
        }
        Err(e) if e.to_string().contains("__no_deprovision_export__") => {
            tracing::debug!(plugin = %plugin.name, "plugin has no deprovision() export — skipping");
            Ok(ResourceDeprovisionResult::default())
        }
        Err(e) => Err(e),
    }
}

fn run_plugin(plugin: &LoadedPlugin, ctx: &PluginContext) -> anyhow::Result<PluginResult> {
    tracing::info!(
        plugin = %plugin.name,
        namespace = %ctx.namespace,
        deployment = %ctx.deployment_name,
        "calling plugin apply()"
    );

    let wasm = Wasm::data(plugin.wasm_bytes.clone());
    let mut manifest = Manifest::new([wasm]);

    // Allow the plugin to reach configured hosts via extism's HTTP host function.
    if !plugin.allowed_hosts.is_empty() {
        manifest.allowed_hosts = Some(plugin.allowed_hosts.clone());
    }

    // Inject operator-supplied config (credentials, endpoints, etc.).
    // Cloud-specific values live here, not in deckwatch core.
    // Plugins read these via `extism_pdk::config::get("KEY")`.
    manifest.config.extend(plugin.config.clone());

    // Fresh timestamp for AWS signing (same as run_plugin).
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    manifest
        .config
        .insert("CURRENT_TIMESTAMP".to_string(), now_ts.to_string());

    // Inject inherited env vars from the deckwatch process environment.
    // Applied after static config so live/rotated values override static entries.
    for key in &plugin.inherit_env_keys {
        if let Ok(val) = std::env::var(key) {
            manifest.config.insert(key.clone(), val);
        }
    }

    // Inject file contents from paths stored in env vars.
    // Cloud-agnostic: deckwatch reads the file; the plugin decides what to do
    // with the content (e.g. exchange a workload identity token for credentials).
    // Applied last so file contents take final precedence.
    for (config_key, env_var) in &plugin.inherit_env_file_keys {
        if let Ok(path) = std::env::var(env_var) {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    manifest
                        .config
                        .insert(config_key.clone(), content.trim().to_string());
                }
                Err(e) => {
                    tracing::warn!(
                        plugin = %plugin.name,
                        config_key = %config_key,
                        path = %path,
                        error = %e,
                        "inherit_env_file_keys: failed to read file"
                    );
                }
            }
        }
    }

    // WASI is disabled; plugins use extism's HTTP host function for outbound
    // network calls, scoped to allowed_hosts above.
    let mut p = Plugin::new(&manifest, [host_now_fn()], false)?;
    let input = serde_json::to_string(ctx)?;
    let output = p.call::<&str, &str>("apply", &input).map_err(|e| {
        tracing::error!(
            plugin = %plugin.name,
            namespace = %ctx.namespace,
            deployment = %ctx.deployment_name,
            error = %e,
            "plugin apply() failed"
        );
        e
    })?;
    let result: PluginResult = serde_json::from_str(output)?;

    // Drain any errors the plugin reported via result.errors. These replace
    // extism_pdk::log!() calls, which cannot reach deckwatch's tracing subscriber.
    for err in &result.errors {
        tracing::error!(
            plugin = %plugin.name,
            namespace = %ctx.namespace,
            deployment = %ctx.deployment_name,
            "plugin reported error: {err}"
        );
    }

    tracing::info!(
        plugin = %plugin.name,
        namespace = %ctx.namespace,
        deployment = %ctx.deployment_name,
        env_vars = result.env_vars.len(),
        sidecars = result.sidecars.len(),
        k8s_resources = result.kubernetes_resources.len(),
        service_account = ?result.service_account_name,
        errors = result.errors.len(),
        "plugin apply() completed"
    );
    Ok(result)
}

fn apply_env_vars(
    plugin: &LoadedPlugin,
    env_vars: &[EnvVarSpec],
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
) {
    if env_vars.is_empty() {
        return;
    }
    let Some(primary) = pod_spec.containers.first_mut() else {
        return;
    };
    let env_vec = primary.env.get_or_insert_with(Vec::new);
    let mut injected: Vec<String> = Vec::new();

    for spec in env_vars {
        if env_vec.iter().any(|e| e.name == spec.name) {
            continue;
        }
        let kube_var = build_kube_env_var(spec);
        env_vec.push(kube_var);
        injected.push(spec.name.clone());
    }

    if !injected.is_empty() {
        dep_annotations.insert(
            format!("{PLUGIN_ENV_ANNOTATION_PREFIX}{}", plugin.name),
            injected.join(","),
        );
    }
}

fn build_kube_env_var(spec: &EnvVarSpec) -> EnvVar {
    if let Some(vf) = &spec.value_from {
        if let Some(skr) = &vf.secret_key_ref {
            return EnvVar {
                name: spec.name.clone(),
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(SecretKeySelector {
                        name: skr.name.clone(),
                        key: skr.key.clone(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
        }
        if let Some(cmkr) = &vf.config_map_key_ref {
            return EnvVar {
                name: spec.name.clone(),
                value_from: Some(EnvVarSource {
                    config_map_key_ref: Some(ConfigMapKeySelector {
                        name: cmkr.name.clone(),
                        key: cmkr.key.clone(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
        }
    }
    EnvVar {
        name: spec.name.clone(),
        value: if spec.value.is_empty() {
            None
        } else {
            Some(spec.value.clone())
        },
        ..Default::default()
    }
}

fn apply_sidecars(
    plugin: &LoadedPlugin,
    sidecars: &[SidecarSpec],
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
) {
    let mut injected: Vec<String> = Vec::new();

    for spec in sidecars {
        if pod_spec.containers.iter().any(|c| c.name == spec.name) {
            continue;
        }
        let env: Vec<EnvVar> = spec.env.iter().map(build_kube_env_var).collect();
        let ports = spec.port.map(|p| {
            vec![ContainerPort {
                container_port: p,
                ..Default::default()
            }]
        });
        let resources = build_resources(spec.cpu.as_deref(), spec.memory.as_deref());

        pod_spec.containers.push(Container {
            name: spec.name.clone(),
            image: Some(spec.image.clone()),
            env: if env.is_empty() { None } else { Some(env) },
            ports,
            resources,
            ..Default::default()
        });
        injected.push(spec.name.clone());
    }

    if !injected.is_empty() {
        dep_annotations.insert(
            format!("{PLUGIN_SIDECAR_ANNOTATION_PREFIX}{}", plugin.name),
            injected.join(","),
        );
    }
}

fn apply_service_account(
    plugin: &LoadedPlugin,
    sa_name: String,
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
) {
    let current = pod_spec
        .service_account_name
        .as_deref()
        .unwrap_or("default");
    if current != "default" {
        tracing::debug!(
            plugin = %plugin.name,
            current_sa = current,
            requested_sa = %sa_name,
            "skipping service_account_name — pod already has a non-default service account"
        );
        return;
    }
    pod_spec.service_account_name = Some(sa_name.clone());
    dep_annotations.insert(format!("deckwatch.plugin-sa/{}", plugin.name), sa_name);
}

fn build_resources(cpu: Option<&str>, memory: Option<&str>) -> Option<ResourceRequirements> {
    if cpu.is_none() && memory.is_none() {
        return None;
    }
    let mut map = BTreeMap::new();
    if let Some(c) = cpu {
        map.insert("cpu".to_string(), Quantity(c.to_string()));
    }
    if let Some(m) = memory {
        map.insert("memory".to_string(), Quantity(m.to_string()));
    }
    Some(ResourceRequirements {
        requests: Some(map.clone()),
        limits: Some(map),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PluginMetadata` deserializes with `resources: []` when the field is absent.
    /// This ensures old WASM plugins (compiled before resources support) still load cleanly.
    #[test]
    fn plugin_metadata_missing_resources_defaults_to_empty() {
        let json = r#"{
            "name": "old-plugin",
            "version": "0.1.0",
            "description": "a legacy plugin",
            "provides": [],
            "depends_on": [],
            "optional_depends_on": [],
            "config_schema": []
        }"#;
        let meta: PluginMetadata = serde_json::from_str(json).expect("should deserialize");
        assert!(
            meta.resources.is_empty(),
            "resources should default to [] when absent"
        );
        assert_eq!(meta.name, "old-plugin");
    }

    /// `PluginMetadata` deserializes with `config_schema: []` when the field is absent.
    /// This ensures old WASM plugins (compiled before v0.5.0 of the SDK) still load cleanly.
    #[test]
    fn plugin_metadata_missing_config_schema_defaults_to_empty() {
        let json = r#"{
            "name": "old-plugin",
            "version": "0.1.0",
            "description": "a legacy plugin",
            "provides": [],
            "depends_on": [],
            "optional_depends_on": []
        }"#;
        let meta: PluginMetadata = serde_json::from_str(json).expect("should deserialize");
        assert!(
            meta.config_schema.is_empty(),
            "config_schema should default to [] when absent"
        );
        assert_eq!(meta.name, "old-plugin");
    }

    /// Round-trip a `PluginMetadata` with a populated `config_schema`.
    #[test]
    fn plugin_metadata_with_config_schema_round_trips() {
        let meta = PluginMetadata {
            name: "aws".to_string(),
            version: "0.5.0".to_string(),
            description: "AWS plugin".to_string(),
            provides: vec!["aws:iam-role".to_string()],
            depends_on: vec![],
            optional_depends_on: vec![],
            resources: vec![],
            config_schema: vec![
                ConfigField {
                    key: "AWS_REGION".to_string(),
                    label: "AWS Region".to_string(),
                    description: "The AWS region".to_string(),
                    field_type: ConfigFieldType::String,
                    default: Some("us-east-1".to_string()),
                    required: true,
                    options: vec![],
                    env_source: Some("AWS_REGION".to_string()),
                },
                ConfigField {
                    key: "AWS_SECRET_ACCESS_KEY".to_string(),
                    label: "Secret Access Key".to_string(),
                    description: String::new(),
                    field_type: ConfigFieldType::Secret,
                    default: None,
                    required: false,
                    options: vec![],
                    env_source: None,
                },
            ],
        };

        let json = serde_json::to_string(&meta).expect("should serialize");
        let back: PluginMetadata = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(back.config_schema.len(), 2);
        assert_eq!(back.config_schema[0].key, "AWS_REGION");
        assert_eq!(back.config_schema[0].field_type, ConfigFieldType::String);
        assert!(back.config_schema[0].required);
        assert_eq!(back.config_schema[1].key, "AWS_SECRET_ACCESS_KEY");
        assert_eq!(back.config_schema[1].field_type, ConfigFieldType::Secret);
        assert!(!back.config_schema[1].required);
        assert!(back.config_schema[1].default.is_none());
    }

    /// `ConfigFieldType` serializes with snake_case names.
    #[test]
    fn config_field_type_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&ConfigFieldType::Secret).unwrap(),
            "\"secret\""
        );
        assert_eq!(
            serde_json::to_string(&ConfigFieldType::Select).unwrap(),
            "\"select\""
        );
        assert_eq!(
            serde_json::to_string(&ConfigFieldType::Bool).unwrap(),
            "\"bool\""
        );
    }
}
