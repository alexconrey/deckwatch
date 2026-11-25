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
use std::sync::Arc;
use std::time::{Duration, Instant};

use extism::{Function, Manifest, Plugin, UserData, Val, ValType, Wasm};
use k8s_openapi::api::core::v1::{
    ConfigMapKeySelector, ConfigMapVolumeSource, Container, ContainerPort, EmptyDirVolumeSource,
    EnvVar, EnvVarSource, ResourceRequirements, SecretKeySelector, SecretVolumeSource, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{DynamicObject, Patch, PatchParams};
use kube::Api;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{Instrument, Span};

use crate::handlers::settings::{DeckwatchSettings, PluginConfig, PluginSource};
use crate::metrics;
use crate::plugin_events::{
    self, codes, host_event, Health, LegacyNotices, PluginEventReport, PluginTelemetry,
    SecretScrubber,
};
use crate::state::AppState;

// ── Shared types (mirror of deckwatch-plugin-sdk v0.9.0, plus the v0.10.0
// `PluginEvent`s and `trace_context`) ─────────────────────────────────────────
// These must stay in sync with `deckwatch-plugin-sdk/src/lib.rs`. The SDK is
// the canonical source; this is the host-side copy for deserializing plugin
// output without adding a workspace dependency on the SDK crate.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Provisioned infrastructure resources for this application, keyed by
    /// `"{plugin_name}/{resource_id}"`. Each value is the state map persisted
    /// when the resource was provisioned (e.g. `{"DB_HOST": "...", "DB_PORT": "5432"}`).
    ///
    /// Populated by `reconcile_application_plugins` before each `run_plugin` call.
    /// Plugins use this to avoid redundant cloud API calls on every reconcile cycle.
    /// Old hosts that do not populate this field deserialize to an empty map.
    #[serde(default)]
    pub provisioned_resources:
        std::collections::HashMap<String, std::collections::HashMap<String, String>>,

    /// W3C `traceparent` of the plugin call, set only while deckwatch exports
    /// traces (an OpenTelemetry layer is active). A plugin can put it on its
    /// outbound HTTP calls so they join the trace. Filled by the call funnel;
    /// callers leave it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<String>,
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

    /// Annotations to stamp on the deployment's `metadata.annotations`.
    ///
    /// Keys should be namespaced (e.g. `"myorg.io/vault-injected": "true"`).
    /// Deckwatch merges these into the SSA patch `metadata.annotations`.
    /// Old plugins deserialize to an empty map.
    #[serde(default)]
    pub deployment_annotations: std::collections::HashMap<String, String>,

    /// Labels to stamp on the deployment's `metadata.labels`.
    ///
    /// Keys should be namespaced (e.g. `"myorg.io/managed-by-vault": "true"`).
    /// Deckwatch includes these in the SSA patch `metadata.labels`.
    /// Old plugins deserialize to an empty map.
    #[serde(default)]
    pub deployment_labels: std::collections::HashMap<String, String>,

    /// Init containers to prepend to `spec.template.spec.initContainers`.
    /// Old plugins that do not emit this field deserialize to an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_containers: Vec<SidecarSpec>,

    /// Errors the plugin wants surfaced. Each becomes an `error` event with
    /// code `plugin.unspecified` (unless it's the SDK's copy of a structured
    /// event); the deployment is not blocked. Prefer `events`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,

    /// Non-fatal warnings. Each becomes a `warning` event, like `errors`; the
    /// deployment is not blocked. Old plugins deserialize to an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    /// Structured notices (SDK `PluginEvent`). Merged with `errors` and
    /// `warnings` by the call funnel (`plugin_events::normalize`) and returned
    /// to whoever triggered the call. The funnel reads them from the raw
    /// output, so after a call this is always empty; use `CallOutcome::events`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PluginEvent>,
}

/// Severity of a [`PluginEvent`].
///
/// Mirrors `deckwatch_plugin_sdk::EventSeverity` — kept in sync manually.
/// `Unknown` (a severity from a newer SDK) is treated as `Warning`.
#[non_exhaustive]
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    #[default]
    Info,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

/// A notice, warning or error a plugin reports from a call.
///
/// Mirrors `deckwatch_plugin_sdk::PluginEvent` (SDK 0.10.0) — kept in sync
/// manually. Every key may be missing (`#[serde(default)]` on the struct), so
/// one partial or hand-written event can't make deckwatch reject the whole
/// result: a missing severity reads as `info`, and an empty code is treated as
/// `plugin.unspecified`. The host also uses this type for failures it detects
/// itself, with the reserved `plugin.*` codes (see `plugin_events::codes`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct PluginEvent {
    pub severity: EventSeverity,
    /// `<plugin-name>.<code>`. `plugin.*` is reserved for the host.
    pub code: String,
    pub message: String,
    /// The `PluginResource` id the event is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// The Kubernetes object the event is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<KubernetesObjectRef>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl PluginEvent {
    /// The copy the SDK's result helpers put in the legacy `errors`,
    /// `warnings` and deprovision `message` fields for older hosts:
    /// `"<code>: <message>"`, or just the message when `code` is empty.
    /// Mirrors `deckwatch_plugin_sdk::PluginEvent::legacy_text`; the funnel
    /// uses it to report each problem once.
    pub fn legacy_text(&self) -> String {
        if self.code.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.code, self.message)
        }
    }
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

/// A volume to attach to the pod, declared by a plugin sidecar.
/// Mirrors `deckwatch_plugin_sdk::VolumeSpec` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub name: String,
    pub volume_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_limit: Option<String>,
}

/// A volume mount descriptor.
/// Mirrors `deckwatch_plugin_sdk::VolumeMountSpec` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMountSpec {
    pub name: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarSpec {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub env: Vec<EnvVarSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    /// CPU request. Takes precedence over deprecated `cpu`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_request: Option<String>,
    /// CPU limit. Takes precedence over deprecated `cpu`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<String>,
    /// Memory request. Takes precedence over deprecated `memory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_request: Option<String>,
    /// Memory limit. Takes precedence over deprecated `memory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
    /// Deprecated: use `cpu_request`/`cpu_limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Deprecated: use `memory_request`/`memory_limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    /// Volumes required by this sidecar (merged into pod `spec.volumes`).
    #[serde(default)]
    pub volumes: Vec<VolumeSpec>,
    /// Volume mounts for this sidecar container.
    #[serde(default)]
    pub volume_mounts: Vec<VolumeMountSpec>,
    /// Volume mounts to add to the primary container on behalf of this sidecar.
    #[serde(default)]
    pub primary_volume_mounts: Vec<VolumeMountSpec>,
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
    /// Key/value pairs, edited as one row per pair (e.g. resource tags). Stored
    /// as a JSON object of string keys to string values, e.g.
    /// `{"Team":"platform"}`; empty means no pairs. Validated on save by
    /// [`validate_key_value_field`].
    KeyValue,
    /// Dropdown. The field must have a non-empty `options` list.
    Select,
    /// A field type this deckwatch doesn't know (from a newer plugin SDK).
    ///
    /// Without this fallback, one unrecognised `field_type` fails the whole
    /// `metadata()` parse, and the plugin silently loses every config field and
    /// resource it declares. Rendered as a plain text input. serde requires
    /// `#[serde(other)]` on the last variant.
    #[serde(other)]
    Unknown,
}

/// Validate every `key_value` field among `updates` against `schema`. Returns
/// the first error, prefixed with the field key. Keys not in the schema, or of
/// other types, are not checked here.
pub fn validate_key_value_updates<'a>(
    schema: &[ConfigField],
    updates: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> Result<(), String> {
    for (key, value) in updates {
        let is_key_value = schema
            .iter()
            .any(|f| f.key == *key && f.field_type == ConfigFieldType::KeyValue);
        if is_key_value {
            validate_key_value_field(value).map_err(|e| format!("{key}: {e}"))?;
        }
    }
    Ok(())
}

/// Check the stored value of a [`ConfigFieldType::KeyValue`] field: a JSON
/// object of string keys to string values, or empty for no pairs. Mirrors
/// `deckwatch_plugin_sdk::parse_key_value_field`, which plugins use to read it.
pub fn validate_key_value_field(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("not a JSON object: {e}"))?;
    let object = value
        .as_object()
        .ok_or("expected a JSON object of string keys to string values")?;
    match object.iter().find(|(_, v)| !v.is_string()) {
        Some((k, v)) => Err(format!("value for {k:?} must be a string, got {v}")),
        None => Ok(()),
    }
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

/// A field that an operator fills in to tune MCP guidance for a specific plugin.
/// Mirrors `deckwatch_plugin_sdk::McpTuningField` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpTuningField {
    /// Machine-readable key. Matched against keys in `PluginConfig.mcp_tuning`.
    pub key: String,
    /// Human-readable label for the MCP Tuning settings form.
    pub label: String,
    /// Help text rendered below the field.
    #[serde(default)]
    pub description: String,
    /// Example value shown as the input placeholder.
    #[serde(default)]
    pub placeholder: String,
    /// Default value pre-filled in the form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
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
    /// Minimum deckwatch version required to run this plugin.
    /// Old plugins that do not export this field default to None (no constraint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_deckwatch_version: Option<String>,
    /// Operator-facing fields for MCP-level guidance tuning.
    /// Saved values are injected into every MCP `initialize` response as a
    /// `[Plugin: {name}]` block so the AI agent sees current conventions.
    /// Old plugins that do not export this field deserialize to an empty Vec.
    #[serde(default)]
    pub mcp_tuning_fields: Vec<McpTuningField>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceProvisionRequest {
    pub application_name: String,
    pub namespace: String,
    pub resource_id: String,
    pub fields: std::collections::HashMap<String, String>,
    /// W3C `traceparent`, as in [`PluginContext::trace_context`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<String>,
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
    /// Init containers to prepend to `initContainers` on all application deployments.
    /// Old plugins that do not emit this field deserialize to an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_containers: Vec<SidecarSpec>,
    #[serde(default)]
    pub kubernetes_resources: Vec<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<String>,
    /// Non-fatal warnings to surface in deckwatch logs at WARN level.
    /// Old plugins deserialize to an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Provisioning lifecycle status. Defaults to `Pending` for old plugins.
    #[serde(default)]
    pub status: ProvisioningStatus,
    /// Minimum seconds before deckwatch should re-poll this resource.
    /// `None` means poll every cycle (the existing behaviour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    /// Structured notices, as in [`PluginResult::events`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PluginEvent>,
}

/// Input to a plugin's `deprovision()` WASM export.
/// Mirrors `deckwatch_plugin_sdk::ResourceDeprovisionRequest` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceDeprovisionRequest {
    pub application_name: String,
    pub namespace: String,
    pub resource_id: String,
    #[serde(default)]
    pub state: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub fields: std::collections::HashMap<String, String>,
    /// W3C `traceparent`, as in [`PluginContext::trace_context`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<String>,
}

/// Provisioning lifecycle status of a plugin-managed infrastructure resource.
///
/// Returned by the plugin's `provision()` export. Deckwatch uses this to drive
/// the polling loop in `reconcile_pending_resources`.
///
/// Mirrors `deckwatch_plugin_sdk::ProvisioningStatus` — kept in sync manually.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum ProvisioningStatus {
    /// Resource creation is still in progress — re-poll next cycle.
    #[default]
    Pending,
    /// Resource is fully provisioned and available — stop polling.
    Available,
    /// Resource provisioning failed — surface in UI and stop polling.
    Failed,
}

/// What the plugin returns from `deprovision()`.
/// Mirrors `deckwatch_plugin_sdk::ResourceDeprovisionResult` — kept in sync manually.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceDeprovisionResult {
    #[serde(default)]
    pub errors: Vec<String>,
    /// What was and wasn't cleaned up. Returned to the operator (HTTP
    /// `DELETE` body and the MCP `deprovision_resource` result) as an `info`
    /// event.
    #[serde(default)]
    pub message: String,
    /// Structured notices, as in [`PluginResult::events`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PluginEvent>,
    /// Kubernetes objects the plugin wants deleted from the application's
    /// namespace once `deprovision()` returns (SDK v0.10.0). Deckwatch only
    /// deletes allowed kinds; see
    /// `handlers::application_resources::delete_requested_objects`.
    /// Old plugins deserialize to an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete_kubernetes_resources: Vec<KubernetesObjectRef>,
}

/// A Kubernetes object named by a plugin. There is no namespace: deckwatch
/// only ever acts in the application's namespace, and ignores a `namespace`
/// key if a plugin sends one.
///
/// Mirrors `deckwatch_plugin_sdk::KubernetesObjectRef` (SDK v0.10.0) — kept in
/// sync manually. Missing fields default to empty here, so one malformed entry
/// is refused on its own instead of discarding the plugin's whole result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KubernetesObjectRef {
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// A fetched plugin ready to execute. Stores raw WASM bytes and instantiates
/// a fresh `extism::Plugin` per call to avoid thread-safety concerns.
///
/// `wasm_bytes` is wrapped in `Arc` so that cloning a `LoadedPlugin` (which
/// happens whenever the watcher snapshots the plugin list) is O(1) instead of
/// copying the full WASM binary. The bytes are still passed by value to
/// `Wasm::data` at invocation time, but that is unavoidable given the extism
/// API; only the snapshot-clone cost is eliminated here.
#[derive(Clone)]
pub struct LoadedPlugin {
    pub name: String,
    pub wasm_bytes: Arc<Vec<u8>>,
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
    /// Lowercase hex SHA-256 of `wasm_bytes`, computed once at load time.
    /// Identifies the exact binary in memory even when two builds report the
    /// same `metadata.version` (e.g. an uploaded fix build without a version bump).
    pub sha256: String,
    /// RFC 3339 UTC timestamp of when this binary was loaded into memory.
    pub loaded_at: String,
}

/// Lowercase hex SHA-256 digest of `bytes` — same format as `shasum -a 256`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::crypto::hex_lower(&Sha256::digest(bytes))
}

/// Swap in a freshly reloaded copy of one plugin, leaving every other loaded
/// plugin untouched.
///
/// `reloaded` is the output of [`fetch_plugins`] run for just `name`: one
/// entry if the new binary loaded, or none if it failed — in which case the
/// plugin is unloaded, matching what a full reload does with a broken plugin.
/// The replacement keeps the plugin's original position in the list; a plugin
/// that wasn't loaded before is appended.
pub fn merge_reloaded_plugin(
    current: &mut Vec<LoadedPlugin>,
    name: &str,
    reloaded: Vec<LoadedPlugin>,
) {
    let position = current.iter().position(|p| p.name == name);
    current.retain(|p| p.name != name);
    let at = position.unwrap_or(current.len());
    for (offset, plugin) in reloaded.into_iter().filter(|p| p.name == name).enumerate() {
        current.insert(at + offset, plugin);
    }
}

/// Current UTC time as an RFC 3339 string, without a direct `chrono` dependency.
fn now_rfc3339() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    sea_orm::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
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
    /// Everything the fetch, `metadata()` and `apply()` calls reported,
    /// normalized and sanitized.
    pub events: Vec<PluginEventReport>,
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
    let http = crate::outbound::global().client(crate::outbound::Purpose::PluginDownload);

    let start = std::time::Instant::now();

    // The whole dry run is one `plugin validate` span; the fetch, metadata()
    // and apply() calls are its children.
    let telemetry = &state.plugin_telemetry;
    let namespace = test_ctx.namespace.clone();
    let deployment = test_ctx.deployment_name.clone();
    let call = PluginCall::new(telemetry, Trigger::Validate)
        .namespace(&namespace)
        .deployment(&deployment);
    let span = plugin_span(Export::Validate, &cfg.name, "", &call);

    let load = load_plugin(cfg, &http, &settings, state, Trigger::Validate)
        .instrument(span.clone())
        .await;
    let mut events = load.reports();
    let load_health = load.health();
    let Some(plugin) = load.plugin else {
        let summary = plugin_events::failure_summary(&events)
            .unwrap_or_else(|| "the plugin failed to load".to_string());
        span.in_scope(|| finish_span(&span, Health::Failing, &load.events));
        metrics::record_plugin_execution(
            &cfg.name,
            "validate",
            "error",
            start.elapsed().as_secs_f64(),
        );
        return ValidationResult {
            wasm_size_bytes: load.wasm_size_bytes,
            apply_export_found: false,
            test_context: test_ctx,
            result: None,
            error: Some(summary),
            events,
        };
    };

    let outcome = span.in_scope(|| run_plugin(&plugin, &test_ctx, &call));
    events.extend(outcome.reports());
    let apply_export_found = outcome.result.is_some()
        || !outcome.events.iter().any(|e| {
            e.code == codes::LOAD_FAILED && e.message == missing_export_message(Export::Apply)
        });
    let health = outcome.health().max(load_health);
    let all_events: Vec<PluginEvent> = events.iter().map(|r| r.event.clone()).collect();
    span.in_scope(|| finish_span(&span, health, &all_events));
    metrics::record_plugin_execution(
        &cfg.name,
        "validate",
        if outcome.result.is_some() {
            "ok"
        } else {
            "error"
        },
        start.elapsed().as_secs_f64(),
    );
    let error = outcome.result.is_none().then(|| outcome.failure_summary());
    ValidationResult {
        wasm_size_bytes: plugin.wasm_bytes.len(),
        apply_export_found,
        test_context: test_ctx,
        result: outcome.result,
        error,
        events,
    }
}

// ── Version helpers ───────────────────────────────────────────────────────────

/// Compare two semver strings of the form `MAJOR.MINOR.PATCH`.
///
/// Returns `true` when `a >= b`. Extra components beyond the third and
/// non-numeric parts are silently ignored — missing components default to 0.
/// No external crate dependency is needed for the simple comparison deckwatch
/// requires at plugin load time.
fn version_gte(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.').filter_map(|p| p.parse::<u32>().ok());
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    };
    parse(a) >= parse(b)
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

/// One plugin's load: the plugin, if it loaded, and what happened.
pub struct PluginLoad {
    pub name: String,
    pub plugin: Option<LoadedPlugin>,
    /// Size of the fetched binary (0 if the fetch failed).
    pub wasm_size_bytes: usize,
    /// Fetch, `metadata()` and version-check events.
    pub events: Vec<PluginEvent>,
    pub trace_id: Option<String>,
}

impl PluginLoad {
    pub fn health(&self) -> Health {
        if self.plugin.is_none() {
            Health::Failing
        } else {
            Health::from_events(&self.events)
        }
    }

    /// The load as a status update for the plugin-wide row.
    pub fn status_update(&self) -> crate::plugin_status::StatusUpdate {
        let reports = self.reports();
        let export = if reports.iter().any(|r| r.export == Export::Fetch.as_str()) {
            Export::Fetch
        } else {
            Export::Metadata
        };
        crate::plugin_status::StatusUpdate {
            key: crate::plugin_status::StatusKey::plugin_wide(&self.name),
            export: export.as_str().to_string(),
            trigger: Trigger::Load.as_str().to_string(),
            health: self.health(),
            events: reports,
            trace_id: self.trace_id.clone(),
            ..Default::default()
        }
    }

    pub fn reports(&self) -> Vec<PluginEventReport> {
        self.events
            .iter()
            .map(|event| PluginEventReport {
                plugin: self.name.clone(),
                export: if event.code == codes::FETCH_FAILED
                    || event.code == codes::CHECKSUM_MISMATCH
                {
                    Export::Fetch.as_str().to_string()
                } else {
                    Export::Metadata.as_str().to_string()
                },
                event: event.clone(),
                trace_id: self.trace_id.clone(),
            })
            .collect()
    }
}

/// The downloaded binary doesn't match the configured checksum.
#[derive(Debug)]
struct ChecksumMismatch {
    plugin: String,
    expected: String,
    actual: String,
}

impl std::fmt::Display for ChecksumMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SHA-256 mismatch for plugin '{}': expected {}, got {}",
            self.plugin, self.expected, self.actual
        )
    }
}

impl std::error::Error for ChecksumMismatch {}

fn missing_export_message(export: Export) -> String {
    format!("the plugin has no `{}` export", export.as_str())
}

/// Fetch, verify and load one plugin through the funnel: a `plugin fetch`
/// span for the download, then `metadata()`, then the
/// `min_deckwatch_version` check.
async fn load_plugin(
    cfg: &PluginConfig,
    http: &reqwest::Client,
    settings: &DeckwatchSettings,
    state: &AppState,
    trigger: Trigger,
) -> PluginLoad {
    let telemetry = &state.plugin_telemetry;
    let call = PluginCall::new(telemetry, trigger);
    let span = plugin_span(Export::Fetch, &cfg.name, "", &call);
    let trace_id = plugin_events::trace_context(&span)
        .as_deref()
        .and_then(plugin_events::trace_id_of)
        .map(str::to_string);
    let start = Instant::now();
    let fetched = fetch_bytes(cfg, http, settings, state)
        .instrument(span.clone())
        .await;
    let (bytes, fetch_events, failed_status) = match fetched {
        Ok(bytes) => (Some(bytes), Vec::new(), ""),
        Err(e) => {
            let (code, status) = if e.downcast_ref::<ChecksumMismatch>().is_some() {
                (codes::CHECKSUM_MISMATCH, "checksum_mismatch")
            } else {
                (codes::FETCH_FAILED, "fetch_failed")
            };
            let event = host_event(
                EventSeverity::Error,
                code,
                format!("{e:#}"),
                &SecretScrubber::default(),
            );
            (None, vec![event], status)
        }
    };
    span.in_scope(|| {
        finish_call(
            &cfg.name,
            Export::Fetch,
            bytes.is_some(),
            start.elapsed(),
            &fetch_events,
            0,
            telemetry,
        )
    });
    let Some(bytes) = bytes else {
        metrics::record_plugin_load(&cfg.name, failed_status);
        return PluginLoad {
            name: cfg.name.clone(),
            plugin: None,
            wasm_size_bytes: 0,
            events: fetch_events,
            trace_id,
        };
    };

    let sha256 = sha256_hex(&bytes);
    let wasm_size_bytes = bytes.len();
    tracing::info!(plugin = %cfg.name, bytes = wasm_size_bytes, sha256 = %sha256, "loaded plugin");
    let mut plugin = LoadedPlugin {
        name: cfg.name.clone(),
        wasm_bytes: Arc::new(bytes),
        allowed_hosts: cfg.allowed_hosts.clone(),
        config: cfg.config.clone(),
        inherit_env_keys: cfg.inherit_env_keys.clone(),
        inherit_env_file_keys: cfg.inherit_env_file_keys.clone(),
        metadata: PluginMetadata {
            name: cfg.name.clone(),
            ..Default::default()
        },
        sha256,
        loaded_at: now_rfc3339(),
    };

    // A plugin without a metadata() export gets defaults; one whose
    // metadata() fails still loads with defaults, and the failure is
    // reported.
    let meta = run_metadata(&plugin, &call);
    let mut events = meta.events;
    if let Some(mut metadata) = meta.result {
        if metadata.name.is_empty() {
            metadata.name = cfg.name.clone();
        }
        plugin.metadata = metadata;
    }

    // Enforce min_deckwatch_version before accepting the plugin.
    if let Some(ref min_ver) = plugin.metadata.min_deckwatch_version {
        let host_ver = env!("CARGO_PKG_VERSION");
        if !version_gte(host_ver, min_ver) {
            let event = host_event(
                EventSeverity::Error,
                codes::LOAD_FAILED,
                format!("the plugin requires deckwatch {min_ver} or newer; this is {host_ver}"),
                &meta.scrubber,
            );
            metrics::record_plugin_load(&cfg.name, "version_incompatible");
            metrics::record_plugin_error(&cfg.name, "version_incompatible");
            telemetry.log_events(
                &cfg.name,
                Export::Metadata.as_str(),
                std::slice::from_ref(&event),
            );
            events.push(event);
            return PluginLoad {
                name: cfg.name.clone(),
                plugin: None,
                wasm_size_bytes,
                events,
                trace_id,
            };
        }
    }

    metrics::record_plugin_load(&cfg.name, "loaded");
    PluginLoad {
        name: cfg.name.clone(),
        plugin: Some(plugin),
        wasm_size_bytes,
        events,
        trace_id,
    }
}

/// Fetch and load all enabled plugins from their configured sources.
/// Per-plugin errors are logged and skipped; a broken plugin never blocks startup.
pub async fn fetch_plugins(settings: &DeckwatchSettings, state: &AppState) -> Vec<LoadedPlugin> {
    let http = crate::outbound::global().client(crate::outbound::Purpose::PluginDownload);

    let mut loaded = Vec::new();
    for cfg in &settings.plugins {
        if !cfg.enabled {
            tracing::debug!(plugin = %cfg.name, "plugin disabled, skipping");
            metrics::record_plugin_load(&cfg.name, "disabled");
            continue;
        }
        let load = load_plugin(cfg, &http, settings, state, Trigger::Load).await;
        state
            .plugin_telemetry
            .record_load(&load.name, load.reports());
        state.plugin_telemetry.report(load.status_update());
        if let Some(plugin) = load.plugin {
            loaded.push(plugin);
        }
    }
    metrics::set_plugins_loaded(loaded.len() as f64);
    loaded
}

/// The configured names of the enabled plugins, for pruning the status
/// rows of plugins that were disabled or removed.
pub fn enabled_plugin_names(settings: &DeckwatchSettings) -> Vec<String> {
    settings
        .plugins
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.name.clone())
        .collect()
}

/// Directory where uploaded WASM binaries are stored.
pub const UPLOADS_DIR: &str = "/data/uploads";

/// The path of the uploaded plugin file `filename` in `dir`, or `None` if
/// `filename` isn't a plain file name: ASCII letters, digits, `.`, `_` and
/// `-`, not starting with `.`. Whoever can edit settings or call the upload
/// route chooses the name, so it must never reach outside `dir` (`/`, `..`,
/// an absolute path).
pub fn upload_path(dir: &std::path::Path, filename: &str) -> Option<std::path::PathBuf> {
    let plain = !filename.is_empty()
        && filename.len() <= 255
        && !filename.starts_with('.')
        && filename
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    plain.then(|| dir.join(filename))
}

async fn fetch_bytes(
    cfg: &PluginConfig,
    http: &reqwest::Client,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> anyhow::Result<Vec<u8>> {
    // Uploaded WASMs are read from disk — no HTTP needed.
    if let PluginSource::Upload { filename } = &cfg.source {
        return read_upload(std::path::Path::new(UPLOADS_DIR), cfg, filename).await;
    }

    let endpoints = settings.endpoints.clone().unwrap_or_default();
    let policy = crate::outbound::global().policy();
    let (url, github_endpoint) = resolve_url(&cfg.source, &endpoints, &policy)?;
    let mut builder = http.get(&url);

    if let Some(token_secret_name) = &cfg.token_secret {
        if let Some((token, source)) = resolve_git_token(token_secret_name, settings, state).await {
            // GitHub downloads only carry the token to the GitHub host it
            // was saved for. `url` sources are operator-chosen hosts and
            // keep their existing behaviour.
            if let Some(endpoint) = &github_endpoint {
                crate::outbound::authorize(
                    &format!("git token '{token_secret_name}'"),
                    &source,
                    endpoint,
                )?;
            }
            builder = builder.bearer_auth(token);
        }
    }

    let resp = builder
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", crate::outbound::error_chain(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status} fetching plugin from {url}"));
    }
    let bytes = resp.bytes().await?.to_vec();

    // Log a warning (but allow load) when no checksum is configured, so
    // development workflows aren't blocked while still surfacing the gap.
    if !verify_checksum(cfg, &bytes)? {
        tracing::warn!(
            plugin = %cfg.name,
            url = %url,
            "no sha256 checksum configured for plugin — \
             consider pinning a checksum in settings to prevent supply-chain tampering"
        );
    }

    Ok(bytes)
}

/// Read the uploaded binary `filename` of plugin `cfg` from `dir`. A pin
/// applies to uploads as to downloads.
async fn read_upload(
    dir: &std::path::Path,
    cfg: &PluginConfig,
    filename: &str,
) -> anyhow::Result<Vec<u8>> {
    let path = upload_path(dir, filename).ok_or_else(|| {
        anyhow::anyhow!(
            "uploaded plugin file {filename:?} isn't a plain file name in the uploads directory"
        )
    })?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read uploaded plugin {filename}: {e}"))?;
    verify_checksum(cfg, &bytes)?;
    Ok(bytes)
}

/// Check `bytes` against the plugin's `sha256` pin, rejecting a mismatch so
/// tampered or corrupted WASM never loads. `Ok(false)` when there's no pin.
fn verify_checksum(cfg: &PluginConfig, bytes: &[u8]) -> anyhow::Result<bool> {
    let Some(expected_hex) = &cfg.sha256 else {
        return Ok(false);
    };
    let actual_hex = sha256_hex(bytes);
    if actual_hex != *expected_hex {
        return Err(ChecksumMismatch {
            plugin: cfg.name.clone(),
            expected: expected_hex.clone(),
            actual: actual_hex,
        }
        .into());
    }
    tracing::debug!(plugin = %cfg.name, "SHA-256 checksum verified");
    Ok(true)
}

/// Download URL for a plugin source, plus the GitHub endpoint it points at
/// (for token binding) when the source is a GitHub repository. GitHub
/// sources use the configured GitHub web base (releases) or raw base.
fn resolve_url(
    source: &PluginSource,
    endpoints: &crate::outbound::EndpointSettings,
    policy: &crate::outbound::EndpointPolicy,
) -> anyhow::Result<(String, Option<crate::outbound::Endpoint>)> {
    use crate::outbound::EndpointId;
    match source {
        PluginSource::Github {
            repo,
            git_ref,
            path,
            use_release,
        } => {
            let (id, url_path) = if *use_release {
                (
                    EndpointId::GithubWeb,
                    format!("/{repo}/releases/download/{git_ref}/{path}"),
                )
            } else {
                (EndpointId::GithubRaw, format!("/{repo}/{git_ref}/{path}"))
            };
            let endpoint = endpoints.resolve(id, policy)?;
            Ok((endpoint.url(&url_path), Some(endpoint)))
        }
        PluginSource::Url { url } => Ok((url.clone(), None)),
        // Upload source has no URL — handled in fetch_bytes before this is called.
        PluginSource::Upload { filename } => Ok((format!("file://{UPLOADS_DIR}/{filename}"), None)),
    }
}

/// Resolve a git token by display name: tries `encrypted_token` first, then
/// falls back to reading the `token` key from the referenced k8s secret.
/// The source says which GitHub hosts the token may be sent to
/// (`crate::outbound::binding`).
pub async fn resolve_git_token(
    secret_ref_name: &str,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> Option<(String, crate::outbound::CredentialSource)> {
    let entry = settings
        .git_token_secrets
        .iter()
        .find(|s| s.name == secret_ref_name)?;

    if let Some(enc) = &entry.encrypted_token {
        if !state.encryption_key.is_empty() {
            let token = crate::crypto::decrypt(&state.encryption_key, enc).ok()?;
            return Some((
                token,
                crate::outbound::CredentialSource::Stored(entry.binding.clone()),
            ));
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
    let token = String::from_utf8(raw).ok()?;
    Some((
        token,
        crate::outbound::CredentialSource::KubernetesSecret(entry.secret_name.clone()),
    ))
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
        let cycle_plugins: Vec<_> = remaining.iter().map(|&i| &plugins[i].name).collect();
        tracing::warn!(
            plugins = ?cycle_plugins,
            "plugin dependency cycle detected; running in original order"
        );
        for name in &cycle_plugins {
            metrics::record_plugin_error(name, "dependency_cycle");
        }
        sorted.extend(remaining);
    }

    // Build the full capability set from every plugin that survived loading.
    // A plugin that was already filtered (e.g. version-incompatible) is not in
    // `plugins` at all, so its capabilities are correctly absent here.
    let all_provided: std::collections::HashSet<&str> = plugins
        .iter()
        .flat_map(|p| p.metadata.provides.iter().map(String::as_str))
        .collect();

    // Validate hard dependencies and remove plugins with unmet capabilities.
    sorted
        .iter()
        .map(|&i| &plugins[i])
        .filter(|p| {
            let unmet: Vec<&str> = p
                .metadata
                .depends_on
                .iter()
                .map(String::as_str)
                .filter(|cap| !all_provided.contains(cap))
                .collect();

            if unmet.is_empty() {
                return true;
            }
            for cap in &unmet {
                tracing::error!(
                    plugin = %p.name,
                    capability = %cap,
                    "plugin requires capability but no loaded plugin provides it — disabling"
                );
            }
            metrics::record_plugin_error(&p.name, "unmet_dependency");
            false
        })
        .collect()
}

// ── Applying ─────────────────────────────────────────────────────────────────

/// A Kubernetes object a plugin returned for deckwatch to apply.
#[derive(Debug, Clone)]
pub struct PluginK8sResource {
    pub plugin: String,
    /// The export whose output contained the object.
    pub export: Export,
    pub manifest: serde_json::Value,
    pub trace_id: Option<String>,
    scrubber: Arc<SecretScrubber>,
}

impl<T> CallOutcome<T> {
    /// Tag `manifests` from this call's output with the plugin, so a failure
    /// to apply one is reported against it.
    pub fn k8s_resources(&self, manifests: &[serde_json::Value]) -> Vec<PluginK8sResource> {
        manifests
            .iter()
            .map(|manifest| PluginK8sResource {
                plugin: self.plugin.clone(),
                export: self.export,
                manifest: manifest.clone(),
                trace_id: self.trace_id.clone(),
                scrubber: self.scrubber.clone(),
            })
            .collect()
    }
}

/// What running every plugin's `apply()` produced.
#[derive(Default)]
pub struct PluginApplyReport {
    /// Raw Kubernetes objects to apply with [`apply_kubernetes_resources`]
    /// once the Deployment is committed.
    pub kubernetes_resources: Vec<PluginK8sResource>,
    /// Structured ServiceAccount requests for
    /// [`apply_wanted_service_accounts`].
    pub wanted_service_accounts: Vec<WantsServiceAccount>,
    /// One outcome per plugin, in the order they ran.
    pub outcomes: Vec<CallOutcome<()>>,
}

impl PluginApplyReport {
    /// Every plugin's events, for the API response.
    pub fn reports(&self) -> Vec<PluginEventReport> {
        self.outcomes
            .iter()
            .flat_map(CallOutcome::reports)
            .collect()
    }
}

/// Run all loaded plugins against `ctx`, merging env vars and sidecars into
/// `pod_spec`/`dep_annotations`.
///
/// Plugins are sorted by declared dependency before execution. Each plugin
/// receives the accumulated `outputs` from all prior plugins in its context.
/// A failing plugin is skipped; its outcome says why.
pub fn apply_plugins(
    plugins: &[LoadedPlugin],
    ctx: &PluginContext,
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
    call: &PluginCall<'_>,
) -> PluginApplyReport {
    let mut report = PluginApplyReport::default();
    let mut accumulated_outputs: std::collections::HashMap<
        String,
        std::collections::HashMap<String, String>,
    > = std::collections::HashMap::new();

    for plugin in sort_by_dependencies(plugins) {
        // Build enriched context with outputs from prior plugins.
        let mut enriched_ctx = ctx.clone();
        enriched_ctx.plugin_outputs = accumulated_outputs.clone();

        let (result, mut outcome) = run_plugin(plugin, &enriched_ctx, call).take_result();
        if let Some(result) = result {
            apply_env_vars(plugin, &result.env_vars, pod_spec, dep_annotations);
            apply_sidecars(plugin, &result.sidecars, pod_spec, dep_annotations);
            apply_init_containers(&result.init_containers, pod_spec);
            report
                .kubernetes_resources
                .extend(outcome.k8s_resources(&result.kubernetes_resources));
            let mut refusals = Vec::new();
            if let Some(sa) = result.service_account_name.clone() {
                refusals.extend(apply_service_account(
                    plugin,
                    sa,
                    pod_spec,
                    dep_annotations,
                    &outcome.scrubber,
                ));
            }
            if let Some(wanted_sa) = result.wants_service_account.clone() {
                // Set the SA name on the pod spec so pods bind to it immediately.
                refusals.extend(apply_service_account(
                    plugin,
                    wanted_sa.name.clone(),
                    pod_spec,
                    dep_annotations,
                    &outcome.scrubber,
                ));
                report.wanted_service_accounts.push(wanted_sa);
            }
            outcome.add_host_events(call.telemetry, refusals);
            // Record this plugin's outputs for subsequent plugins.
            if !result.outputs.is_empty() {
                accumulated_outputs.insert(plugin.name.clone(), result.outputs);
            }
        }
        report.outcomes.push(outcome);
    }

    report
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
/// apply. Called after the deployment is committed; failures never fail the
/// request. Each failure (a CRD that isn't installed, a rejected object) is
/// returned as a `plugin.k8s_apply_failed` event against the plugin that
/// asked for the object, and logged.
pub async fn apply_kubernetes_resources(
    resources: &[PluginK8sResource],
    kube_client: &kube::Client,
    telemetry: &PluginTelemetry,
) -> Vec<PluginEventReport> {
    let mut failures = Vec::new();
    for resource in resources {
        let manifest = &resource.manifest;
        let field = |pointer: &str| {
            manifest
                .pointer(pointer)
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string()
        };
        let (api_version, kind, name) = (
            field("/apiVersion"),
            field("/kind"),
            field("/metadata/name"),
        );
        match apply_one_resource(manifest, kube_client).await {
            Ok(()) => metrics::record_plugin_k8s_resource(&kind, "ok"),
            Err(e) => {
                metrics::record_plugin_k8s_resource(&kind, "error");
                metrics::record_plugin_error(&resource.plugin, "k8s_apply_failed");
                let mut event = host_event(
                    EventSeverity::Error,
                    codes::K8S_APPLY_FAILED,
                    format!("failed to apply {kind} {name}: {e:#}"),
                    &resource.scrubber,
                );
                event.object = Some(plugin_events::sanitize_object_ref(
                    &KubernetesObjectRef {
                        api_version,
                        kind,
                        name,
                    },
                    &resource.scrubber,
                ));
                telemetry.log_events(
                    &resource.plugin,
                    resource.export.as_str(),
                    std::slice::from_ref(&event),
                );
                failures.push(PluginEventReport {
                    plugin: resource.plugin.clone(),
                    export: resource.export.as_str().to_string(),
                    event,
                    trace_id: resource.trace_id.clone(),
                });
            }
        }
    }
    failures
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

/// Deckwatch's own secrets. Plugin config, including `inherit_env_keys`, is
/// editable through settings and the MCP `update_plugin_config` tool, so a
/// plugin must never be able to pull these out of the process environment:
/// the encryption key decrypts every stored credential, and the database URL
/// can carry a password.
const NON_INHERITABLE_ENV: &[&str] = &["DECKWATCH_ENCRYPTION_KEY", "DECKWATCH_DATABASE_URL"];

/// `std::env::var` for `inherit_env_keys` / `inherit_env_file_keys`,
/// refusing [`NON_INHERITABLE_ENV`].
fn inherited_env_var(plugin: &str, key: &str) -> Option<String> {
    inherited_env_var_from(plugin, key, |k| std::env::var(k).ok())
}

fn inherited_env_var_from(
    plugin: &str,
    key: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if NON_INHERITABLE_ENV.contains(&key) {
        tracing::warn!(
            plugin,
            key,
            "refusing to pass deckwatch secret env var to plugin"
        );
        return None;
    }
    lookup(key)
}

// ── Plugin call funnel ───────────────────────────────────────────────────────
//
// Every plugin interaction goes through here: fetch and load (`load_plugin`),
// `metadata`, validation, `apply` (deployment create/update and the
// reconcile loop), `provision` (submit, refresh and status polling) and
// `deprovision`. For each call the funnel:
//
// 1. opens a span `plugin {export}` carrying the plugin, its version, the
//    export, what triggered the call, and the application, namespace,
//    deployment and resource id it's for. extism turns the plugin's own
//    `log_*` calls into `tracing` events (target `extism::pdk`) on the calling
//    thread, so they land inside this span;
// 2. records `deckwatch_plugin_executions_total`,
//    `deckwatch_plugin_execution_duration_seconds` and
//    `deckwatch_plugin_errors_total`, with labels from fixed host-side sets;
// 3. normalizes what happened into `PluginEvent`s (`plugin_events`): the
//    plugin's structured events, its legacy `errors`/`warnings`/`message`
//    strings, and host-detected failures with the reserved `plugin.*` codes,
//    all sanitized and with secret config values masked;
// 4. logs those events inside the span, rate limited per plugin.
//
// Callers get a `CallOutcome` and route its events onward: to the API or MCP
// response of the action that triggered the call, and to the background
// sinks.

/// A plugin export, or a host step with its own span (`fetch`, `validate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Export {
    Fetch,
    Metadata,
    Apply,
    Provision,
    Deprovision,
    Validate,
}

impl Export {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Metadata => "metadata",
            Self::Apply => "apply",
            Self::Provision => "provision",
            Self::Deprovision => "deprovision",
            Self::Validate => "validate",
        }
    }
}

/// What made deckwatch call a plugin. A fixed set, so it can go on spans,
/// logs and (later) metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// Startup, a settings save, an upload or `enable_plugin`.
    Load,
    DeploymentCreate,
    DeploymentUpdate,
    /// The 10-second application reconcile loop.
    Reconcile,
    /// An operator or agent provisioning a resource.
    Provision,
    /// An operator or agent refreshing a resource.
    Refresh,
    /// The background re-poll of pending resources.
    StatusPoll,
    Deprovision,
    /// MCP `validate_plugin`.
    Validate,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::DeploymentCreate => "deployment_create",
            Self::DeploymentUpdate => "deployment_update",
            Self::Reconcile => "reconcile",
            Self::Provision => "provision",
            Self::Refresh => "refresh",
            Self::StatusPoll => "status_poll",
            Self::Deprovision => "deprovision",
            Self::Validate => "validate",
        }
    }
}

/// Who a plugin call is for. Empty strings mean "not applicable".
#[derive(Clone, Copy)]
pub struct PluginCall<'a> {
    pub telemetry: &'a PluginTelemetry,
    pub trigger: Trigger,
    pub namespace: &'a str,
    pub application: &'a str,
    pub deployment: &'a str,
    pub resource_id: &'a str,
}

impl<'a> PluginCall<'a> {
    pub fn new(telemetry: &'a PluginTelemetry, trigger: Trigger) -> Self {
        Self {
            telemetry,
            trigger,
            namespace: "",
            application: "",
            deployment: "",
            resource_id: "",
        }
    }

    pub fn namespace(mut self, namespace: &'a str) -> Self {
        self.namespace = namespace;
        self
    }

    pub fn application(mut self, application: &'a str) -> Self {
        self.application = application;
        self
    }

    pub fn deployment(mut self, deployment: &'a str) -> Self {
        self.deployment = deployment;
        self
    }

    pub fn resource(mut self, resource_id: &'a str) -> Self {
        self.resource_id = resource_id;
        self
    }
}

/// What one plugin call produced. `result` is `None` when the call failed
/// (the events then say why); `events` are already normalized and safe to
/// return, store or log.
#[must_use = "route the events to the response and the sinks"]
#[derive(Debug, Clone)]
pub struct CallOutcome<T> {
    /// The configured plugin name (never the plugin's self-reported one).
    pub plugin: String,
    pub export: Export,
    pub result: Option<T>,
    pub events: Vec<PluginEvent>,
    /// Trace id of the call's span, when traces are exported.
    pub trace_id: Option<String>,
    /// Masks this call's secret config values in events added later, such as
    /// a failure to apply a Kubernetes object the plugin returned.
    pub scrubber: Arc<SecretScrubber>,
    /// What triggered the call.
    pub trigger: Trigger,
}

impl<T> CallOutcome<T> {
    pub fn health(&self) -> Health {
        if self.result.is_none() {
            Health::Failing
        } else {
            Health::from_events(&self.events)
        }
    }

    /// The events as returned to API and MCP clients.
    pub fn reports(&self) -> Vec<PluginEventReport> {
        self.events
            .iter()
            .map(|event| PluginEventReport {
                plugin: self.plugin.clone(),
                export: self.export.as_str().to_string(),
                event: event.clone(),
                trace_id: self.trace_id.clone(),
            })
            .collect()
    }

    /// This outcome as a status update for `key`.
    pub fn status_update(
        &self,
        key: crate::plugin_status::StatusKey,
    ) -> crate::plugin_status::StatusUpdate {
        crate::plugin_status::StatusUpdate {
            key,
            export: self.export.as_str().to_string(),
            trigger: self.trigger.as_str().to_string(),
            health: self.health(),
            events: self.reports(),
            trace_id: self.trace_id.clone(),
            ..Default::default()
        }
    }

    /// `code: message` of the first error, for error replies.
    pub fn failure_summary(&self) -> String {
        plugin_events::failure_summary(&self.reports())
            .unwrap_or_else(|| format!("{}() failed", self.export.as_str()))
    }

    /// Split off the result, keeping the events.
    pub fn take_result(self) -> (Option<T>, CallOutcome<()>) {
        let CallOutcome {
            plugin,
            export,
            result,
            events,
            trace_id,
            scrubber,
            trigger,
        } = self;
        let failed = result.is_none();
        let rest = CallOutcome {
            plugin,
            export,
            result: if failed { None } else { Some(()) },
            events,
            trace_id,
            scrubber,
            trigger,
        };
        (result, rest)
    }

    /// Add events the host raised after the call (policy refusals, failed
    /// Kubernetes applies). They're logged and counted like the call's own.
    pub fn add_host_events(&mut self, telemetry: &PluginTelemetry, events: Vec<PluginEvent>) {
        if events.is_empty() {
            return;
        }
        for event in &events {
            if plugin_events::effective_severity(event.severity) == EventSeverity::Error {
                metrics::record_plugin_error(&self.plugin, codes::metric_label(&event.code));
            }
        }
        telemetry.log_events(&self.plugin, self.export.as_str(), &events);
        for event in events {
            if self.events.len() >= plugin_events::MAX_EVENTS_PER_CALL {
                metrics::record_plugin_events_dropped(&self.plugin, "over_cap", 1);
                continue;
            }
            if !self.events.contains(&event) {
                self.events.push(event);
            }
        }
    }
}

/// A plugin result type the funnel can parse, with its legacy plain-string
/// notices.
pub trait PluginOutput: serde::de::DeserializeOwned + Default {
    fn legacy(&self) -> LegacyNotices<'_> {
        LegacyNotices::default()
    }
}

impl PluginOutput for PluginResult {
    fn legacy(&self) -> LegacyNotices<'_> {
        LegacyNotices {
            errors: &self.errors,
            warnings: &self.warnings,
            info: None,
        }
    }
}

impl PluginOutput for ResourceProvisionResult {
    fn legacy(&self) -> LegacyNotices<'_> {
        LegacyNotices {
            errors: &self.errors,
            warnings: &self.warnings,
            info: None,
        }
    }
}

impl PluginOutput for ResourceDeprovisionResult {
    fn legacy(&self) -> LegacyNotices<'_> {
        LegacyNotices {
            errors: &self.errors,
            warnings: &[],
            info: Some(self.message.as_str()).filter(|m| !m.trim().is_empty()),
        }
    }
}

impl PluginOutput for PluginMetadata {}

/// Open the span for one plugin call. Span names must be literals, hence
/// one arm per export.
fn plugin_span(export: Export, plugin: &str, version: &str, call: &PluginCall<'_>) -> Span {
    macro_rules! span {
        ($name:literal) => {
            tracing::info_span!(
                $name,
                "deckwatch.plugin.name" = plugin,
                "deckwatch.plugin.version" = version,
                "deckwatch.plugin.export" = export.as_str(),
                "deckwatch.plugin.trigger" = call.trigger.as_str(),
                "deckwatch.application" = call.application,
                "k8s.namespace.name" = call.namespace,
                "k8s.deployment.name" = call.deployment,
                "deckwatch.plugin.resource_id" = call.resource_id,
                "deckwatch.plugin.outcome" = tracing::field::Empty,
                "deckwatch.plugin.events" = tracing::field::Empty,
                "otel.status_code" = tracing::field::Empty,
                "error.type" = tracing::field::Empty,
                "exception.type" = tracing::field::Empty,
                "exception.message" = tracing::field::Empty,
            )
        };
    }
    match export {
        Export::Fetch => span!("plugin fetch"),
        Export::Metadata => span!("plugin metadata"),
        Export::Apply => span!("plugin apply"),
        Export::Provision => span!("plugin provision"),
        Export::Deprovision => span!("plugin deprovision"),
        Export::Validate => span!("plugin validate"),
    }
}

/// Record a call's outcome on its span (OpenTelemetry reads `otel.*`,
/// `error.type` and `exception.*`).
fn finish_span(span: &Span, health: Health, events: &[PluginEvent]) {
    span.record("deckwatch.plugin.outcome", health.as_str());
    span.record("deckwatch.plugin.events", events.len());
    if health == Health::Failing {
        span.record("otel.status_code", "error");
        if let Some(e) = events
            .iter()
            .find(|e| plugin_events::effective_severity(e.severity) == EventSeverity::Error)
        {
            span.record("error.type", e.code.as_str());
            span.record("exception.type", e.code.as_str());
            span.record("exception.message", e.message.as_str());
        }
    }
}

/// Count, log and trace a finished call. Runs inside the call's span, which
/// it records the outcome on.
fn finish_call(
    plugin: &str,
    export: Export,
    succeeded: bool,
    elapsed: Duration,
    events: &[PluginEvent],
    over_cap: usize,
    telemetry: &PluginTelemetry,
) -> Health {
    let health = if succeeded {
        Health::from_events(events)
    } else {
        Health::Failing
    };
    metrics::record_plugin_execution(
        plugin,
        export.as_str(),
        if succeeded { "ok" } else { "error" },
        elapsed.as_secs_f64(),
    );
    for event in events {
        if plugin_events::effective_severity(event.severity) != EventSeverity::Error {
            continue;
        }
        // Host failures count under their code, everything the plugin
        // reported as `reported_error`: labels stay a fixed set.
        metrics::record_plugin_error(plugin, codes::metric_label(&event.code));
    }
    if over_cap > 0 {
        metrics::record_plugin_events_dropped(plugin, "over_cap", over_cap as u64);
    }
    finish_span(&Span::current(), health, events);
    telemetry.log_events(plugin, export.as_str(), events);
    health
}

/// The manifest for one call, and what the plugin was given.
struct BuiltManifest {
    manifest: Manifest,
    /// The config the plugin receives, after inherited env and files.
    config: BTreeMap<String, String>,
    /// Config keys filled from files (`inherit_env_file_keys`).
    file_keys: Vec<String>,
    /// `inherit_env*` keys refused because they name deckwatch's own secrets.
    refused: Vec<String>,
}

/// Build the extism manifest for a call. `runtime_config` adds
/// `CURRENT_TIMESTAMP` and the inherited env vars and files; `metadata()`
/// runs with the static config only, as it always has.
fn build_manifest(
    plugin: &LoadedPlugin,
    runtime_config: bool,
    timeout: Option<Duration>,
) -> BuiltManifest {
    // as_slice() borrows the Arc<Vec<u8>>; Wasm::data copies the bytes into
    // its own Vec. The Arc keeps cloning a LoadedPlugin cheap.
    let mut manifest = Manifest::new([Wasm::data(plugin.wasm_bytes.as_slice())]);
    // Allow the plugin to reach configured hosts via extism's HTTP host function.
    if !plugin.allowed_hosts.is_empty() {
        manifest.allowed_hosts = Some(plugin.allowed_hosts.clone());
    }
    if let Some(timeout) = timeout {
        manifest = manifest.with_timeout(timeout);
    }
    // Operator-supplied config (credentials, endpoints, etc.). Cloud-specific
    // values live here, not in deckwatch core. Plugins read these via
    // `extism_pdk::config::get("KEY")`.
    manifest.config.extend(plugin.config.clone());

    let mut file_keys = Vec::new();
    let mut refused = Vec::new();
    if runtime_config {
        // Fresh timestamp for AWS request signing.
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        manifest
            .config
            .insert("CURRENT_TIMESTAMP".to_string(), now_ts.to_string());

        // Inherited env vars override static config so live/rotated values win.
        for key in &plugin.inherit_env_keys {
            if NON_INHERITABLE_ENV.contains(&key.as_str()) {
                refused.push(key.clone());
            }
            if let Some(val) = inherited_env_var(&plugin.name, key) {
                manifest.config.insert(key.clone(), val);
            }
        }

        // File contents from paths stored in env vars take final precedence.
        // Cloud-agnostic: deckwatch reads the file; the plugin decides what
        // to do with it (e.g. exchange a workload identity token).
        for (config_key, env_var) in &plugin.inherit_env_file_keys {
            if NON_INHERITABLE_ENV.contains(&env_var.as_str()) {
                refused.push(env_var.clone());
            }
            if let Some(path) = inherited_env_var(&plugin.name, env_var) {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        manifest
                            .config
                            .insert(config_key.clone(), content.trim().to_string());
                        file_keys.push(config_key.clone());
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
    }

    let config = manifest.config.clone();
    BuiltManifest {
        manifest,
        config,
        file_keys,
        refused,
    }
}

/// The scrubber for a call: every value passed for a `secret` field of the
/// plugin's config schema, plus file-sourced values.
fn secret_scrubber(plugin: &LoadedPlugin, built: &BuiltManifest, key: &str) -> SecretScrubber {
    let secret_keys = plugin
        .metadata
        .config_schema
        .iter()
        .filter(|f| f.field_type == ConfigFieldType::Secret)
        .map(|f| f.key.as_str());
    let values = secret_keys
        .chain(built.file_keys.iter().map(String::as_str))
        .filter_map(|k| built.config.get(k));
    SecretScrubber::new(values, key)
}

/// What extism did with a call.
enum Invocation {
    Output(String),
    MissingExport,
    Failed { code: &'static str, message: String },
}

fn invoke(manifest: &Manifest, export: &str, input: &str) -> Invocation {
    // WASI is disabled; plugins reach the network only through extism's
    // HTTP host function, scoped to allowed_hosts.
    let mut plugin = match Plugin::new(manifest, [host_now_fn()], false) {
        Ok(p) => p,
        Err(e) => {
            return Invocation::Failed {
                code: codes::LOAD_FAILED,
                message: format!("the WASM module failed to load: {e:#}"),
            }
        }
    };
    match plugin.call::<&str, &str>(export, input) {
        Ok(output) => Invocation::Output(output.to_string()),
        Err(e) => classify_call_error(&e, export),
    }
}

/// Map an extism call error to a host code. extism reports a timeout as
/// the bare message `timeout`, a missing export as `Function not found: X`,
/// and a trap with a `wasm trap: ...` cause; anything else is text the
/// plugin set itself through `error_set` (a `#[plugin_fn]` returning `Err`).
fn classify_call_error(e: &anyhow::Error, export: &str) -> Invocation {
    let root = e.root_cause().to_string();
    let full = format!("{e:#}");
    if root == "timeout" {
        return Invocation::Failed {
            code: codes::TIMEOUT,
            message: format!("{export}() was stopped after running past its time limit"),
        };
    }
    if full.starts_with("Function not found:") {
        return Invocation::MissingExport;
    }
    if root.contains("ran out of fuel") {
        return Invocation::Failed {
            code: codes::TIMEOUT,
            message: format!("{export}() ran out of fuel"),
        };
    }
    if root == "oom" {
        return Invocation::Failed {
            code: codes::TRAP,
            message: format!("{export}() ran out of memory"),
        };
    }
    if full.contains("wasm trap") || full.contains("wasm backtrace") {
        let reason = e
            .chain()
            .map(|c| c.to_string())
            .find(|m| m.starts_with("wasm trap"))
            .unwrap_or(root);
        return Invocation::Failed {
            code: codes::TRAP,
            message: format!("{export}() trapped: {reason}"),
        };
    }
    Invocation::Failed {
        code: codes::UNSPECIFIED,
        message: format!("{export}() returned an error: {full}"),
    }
}

/// Most structured events read from one output; the rest are ignored.
const MAX_RAW_EVENTS: usize = 100;

struct ParsedOutput<T> {
    result: T,
    events: Vec<PluginEvent>,
    /// One message per event that didn't parse, with its path.
    bad_events: Vec<String>,
}

/// Parse a plugin's JSON output. `events` are read one by one, so a single
/// malformed event costs only itself; any other problem fails the whole
/// output with the serde path of the offending field.
fn parse_output<T: serde::de::DeserializeOwned>(raw: &str) -> Result<ParsedOutput<T>, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("not valid JSON ({e})"))?;
    let mut events = Vec::new();
    let mut bad_events = Vec::new();
    if let Some(obj) = value.as_object_mut() {
        match obj.remove("events") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(items)) => {
                let total = items.len();
                for (i, item) in items.into_iter().enumerate().take(MAX_RAW_EVENTS) {
                    match serde_path_to_error::deserialize::<_, PluginEvent>(item) {
                        Ok(e) => events.push(e),
                        Err(e) => {
                            let path = e.path().to_string();
                            let at = if path == "." {
                                format!("events[{i}]")
                            } else {
                                format!("events[{i}].{path}")
                            };
                            bad_events.push(format!("{at}: {}", e.inner()));
                        }
                    }
                }
                if total > MAX_RAW_EVENTS {
                    bad_events.push(format!(
                        "events: {total} events; only the first {MAX_RAW_EVENTS} are read"
                    ));
                }
            }
            Some(_) => bad_events.push("events: expected an array".to_string()),
        }
    }
    let result = serde_path_to_error::deserialize::<_, T>(value).map_err(|e| {
        let path = e.path().to_string();
        if path == "." {
            e.inner().to_string()
        } else {
            format!("{path}: {}", e.inner())
        }
    })?;
    Ok(ParsedOutput {
        result,
        events,
        bad_events,
    })
}

/// Call one export of `plugin` through the funnel. `optional` exports
/// (`metadata`, `deprovision`) that the plugin lacks succeed with
/// `T::default()`. `input` builds the request JSON, given the call's
/// `traceparent`.
fn call_export<T: PluginOutput>(
    plugin: &LoadedPlugin,
    export: Export,
    call: &PluginCall<'_>,
    optional: bool,
    input: impl FnOnce(Option<String>) -> serde_json::Result<String>,
) -> CallOutcome<T> {
    let span = plugin_span(export, &plugin.name, &plugin.metadata.version, call);
    let start = Instant::now();
    span.in_scope(|| {
        tracing::debug!(plugin = %plugin.name, export = export.as_str(), "calling plugin");
        let trace_context = plugin_events::trace_context(&span);
        let trace_id = trace_context
            .as_deref()
            .and_then(plugin_events::trace_id_of)
            .map(str::to_string);
        let built = build_manifest(
            plugin,
            export != Export::Metadata,
            call.telemetry.call_timeout(),
        );
        let scrubber = secret_scrubber(plugin, &built, call.telemetry.encryption_key());

        let mut host: Vec<PluginEvent> = built
            .refused
            .iter()
            .map(|key| {
                host_event(
                    EventSeverity::Warning,
                    codes::POLICY_REJECTED,
                    format!("deckwatch did not pass {key} to the plugin: it is one of deckwatch's own secrets"),
                    &scrubber,
                )
            })
            .collect();

        let fail = |code: &str, message: String, host: &mut Vec<PluginEvent>| {
            host.push(host_event(EventSeverity::Error, code, message, &scrubber));
        };

        let (result, structured): (Option<T>, Vec<PluginEvent>) = match input(trace_context) {
            Err(e) => {
                fail(
                    codes::LOAD_FAILED,
                    format!("deckwatch could not encode the {}() input: {e}", export.as_str()),
                    &mut host,
                );
                (None, Vec::new())
            }
            Ok(input) => match invoke(&built.manifest, export.as_str(), &input) {
                Invocation::Output(raw) => match parse_output::<T>(&raw) {
                    Ok(parsed) => {
                        for bad in parsed.bad_events {
                            host.push(host_event(
                                EventSeverity::Warning,
                                codes::OUTPUT_INVALID,
                                format!("ignored a malformed event: {bad}"),
                                &scrubber,
                            ));
                        }
                        (Some(parsed.result), parsed.events)
                    }
                    Err(err) => {
                        fail(
                            codes::OUTPUT_INVALID,
                            format!(
                                "{}() returned output deckwatch can't read: {err}",
                                export.as_str()
                            ),
                            &mut host,
                        );
                        (None, Vec::new())
                    }
                },
                Invocation::MissingExport if optional => (Some(T::default()), Vec::new()),
                Invocation::MissingExport => {
                    fail(
                        codes::LOAD_FAILED,
                        format!("the plugin has no `{}` export", export.as_str()),
                        &mut host,
                    );
                    (None, Vec::new())
                }
                Invocation::Failed { code, message } => {
                    fail(code, message, &mut host);
                    (None, Vec::new())
                }
            },
        };

        let legacy = result.as_ref().map(T::legacy).unwrap_or_default();
        let (events, over_cap) =
            plugin_events::normalize(&plugin.name, host, &structured, legacy, &scrubber);
        finish_call(
            &plugin.name,
            export,
            result.is_some(),
            start.elapsed(),
            &events,
            over_cap,
            call.telemetry,
        );
        CallOutcome {
            plugin: plugin.name.clone(),
            export,
            result,
            events,
            trace_id,
            scrubber: Arc::new(scrubber),
            trigger: call.trigger,
        }
    })
}

/// Call the plugin's `provision()` export.
pub fn run_provision(
    plugin: &LoadedPlugin,
    req: &ResourceProvisionRequest,
    call: &PluginCall<'_>,
) -> CallOutcome<ResourceProvisionResult> {
    let outcome = call_export::<ResourceProvisionResult>(
        plugin,
        Export::Provision,
        call,
        false,
        |trace_context| {
            let mut req = req.clone();
            req.trace_context = trace_context;
            serde_json::to_string(&req)
        },
    );
    if let Some(result) = &outcome.result {
        tracing::info!(
            plugin = %plugin.name,
            namespace = %req.namespace,
            application = %req.application_name,
            resource_id = %req.resource_id,
            state_keys = result.state.len(),
            k8s_resources = result.kubernetes_resources.len(),
            events = outcome.events.len(),
            "plugin provision() completed"
        );
    }
    outcome
}

/// Call the plugin's `deprovision()` export. A plugin without one succeeds
/// with an empty result. Errors never block removal of the resource record;
/// callers return the events to the operator.
pub fn run_deprovision(
    plugin: &LoadedPlugin,
    req: &ResourceDeprovisionRequest,
    call: &PluginCall<'_>,
) -> CallOutcome<ResourceDeprovisionResult> {
    let outcome = call_export::<ResourceDeprovisionResult>(
        plugin,
        Export::Deprovision,
        call,
        true,
        |trace_context| {
            let mut req = req.clone();
            req.trace_context = trace_context;
            serde_json::to_string(&req)
        },
    );
    if outcome.result.is_some() {
        tracing::info!(
            plugin = %plugin.name,
            resource_id = %req.resource_id,
            events = outcome.events.len(),
            "plugin deprovision() completed"
        );
    }
    outcome
}

/// Call the plugin's `apply()` export.
pub fn run_plugin(
    plugin: &LoadedPlugin,
    ctx: &PluginContext,
    call: &PluginCall<'_>,
) -> CallOutcome<PluginResult> {
    let outcome =
        call_export::<PluginResult>(plugin, Export::Apply, call, false, |trace_context| {
            let mut ctx = ctx.clone();
            ctx.trace_context = trace_context;
            serde_json::to_string(&ctx)
        });
    if let Some(result) = &outcome.result {
        tracing::debug!(
            plugin = %plugin.name,
            namespace = %ctx.namespace,
            deployment = %ctx.deployment_name,
            env_vars = result.env_vars.len(),
            sidecars = result.sidecars.len(),
            k8s_resources = result.kubernetes_resources.len(),
            service_account = ?result.service_account_name,
            events = outcome.events.len(),
            "plugin apply() completed"
        );
    }
    outcome
}

/// Call the plugin's `metadata()` export. A plugin without one gets the
/// default metadata.
fn run_metadata(plugin: &LoadedPlugin, call: &PluginCall<'_>) -> CallOutcome<PluginMetadata> {
    call_export::<PluginMetadata>(plugin, Export::Metadata, call, true, |_| Ok(String::new()))
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

    // Add primary_volume_mounts from all sidecars to the primary container.
    let all_primary_mounts: Vec<VolumeMount> = sidecars
        .iter()
        .flat_map(|s| &s.primary_volume_mounts)
        .map(volume_mount_spec_to_k8s)
        .collect();
    if !all_primary_mounts.is_empty() {
        if let Some(primary) = pod_spec.containers.first_mut() {
            let mounts = primary.volume_mounts.get_or_insert_with(Vec::new);
            for vm in all_primary_mounts {
                if !mounts
                    .iter()
                    .any(|m| m.name == vm.name && m.mount_path == vm.mount_path)
                {
                    mounts.push(vm);
                }
            }
        }
    }

    // Merge volumes from all sidecars into pod_spec.volumes.
    let pod_volumes = pod_spec.volumes.get_or_insert_with(Vec::new);
    for spec in sidecars {
        for vol in &spec.volumes {
            if !pod_volumes.iter().any(|v| v.name == vol.name) {
                pod_volumes.push(volume_spec_to_k8s(vol));
            }
        }
    }

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
        let volume_mounts: Vec<VolumeMount> = spec
            .volume_mounts
            .iter()
            .map(volume_mount_spec_to_k8s)
            .collect();
        let resources = build_resources(
            spec.cpu_request.as_deref().or(spec.cpu.as_deref()),
            spec.cpu_limit.as_deref().or(spec.cpu.as_deref()),
            spec.memory_request.as_deref().or(spec.memory.as_deref()),
            spec.memory_limit.as_deref().or(spec.memory.as_deref()),
        );

        pod_spec.containers.push(Container {
            name: spec.name.clone(),
            image: Some(spec.image.clone()),
            env: if env.is_empty() { None } else { Some(env) },
            ports,
            resources,
            volume_mounts: if volume_mounts.is_empty() {
                None
            } else {
                Some(volume_mounts)
            },
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

fn apply_init_containers(
    sidecars: &[SidecarSpec],
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
) {
    let init_containers = pod_spec.init_containers.get_or_insert_with(Vec::new);
    for spec in sidecars {
        if init_containers.iter().any(|c| c.name == spec.name) {
            continue;
        }
        let env: Vec<EnvVar> = spec.env.iter().map(build_kube_env_var).collect();
        let ports = spec.port.map(|p| {
            vec![ContainerPort {
                container_port: p,
                ..Default::default()
            }]
        });
        let volume_mounts: Vec<VolumeMount> = spec
            .volume_mounts
            .iter()
            .map(volume_mount_spec_to_k8s)
            .collect();
        let resources = build_resources(
            spec.cpu_request.as_deref().or(spec.cpu.as_deref()),
            spec.cpu_limit.as_deref().or(spec.cpu.as_deref()),
            spec.memory_request.as_deref().or(spec.memory.as_deref()),
            spec.memory_limit.as_deref().or(spec.memory.as_deref()),
        );
        init_containers.push(Container {
            name: spec.name.clone(),
            image: Some(spec.image.clone()),
            env: if env.is_empty() { None } else { Some(env) },
            ports,
            resources,
            volume_mounts: if volume_mounts.is_empty() {
                None
            } else {
                Some(volume_mounts)
            },
            ..Default::default()
        });
    }
}

fn volume_mount_spec_to_k8s(vm: &VolumeMountSpec) -> VolumeMount {
    VolumeMount {
        name: vm.name.clone(),
        mount_path: vm.mount_path.clone(),
        read_only: if vm.read_only { Some(true) } else { None },
        sub_path: vm.sub_path.clone(),
        ..Default::default()
    }
}

fn volume_spec_to_k8s(v: &VolumeSpec) -> Volume {
    match v.volume_type.as_str() {
        "emptyDir" => Volume {
            name: v.name.clone(),
            empty_dir: Some(EmptyDirVolumeSource {
                size_limit: v.size_limit.as_deref().map(|s| Quantity(s.to_string())),
                ..Default::default()
            }),
            ..Default::default()
        },
        "secret" => Volume {
            name: v.name.clone(),
            secret: Some(SecretVolumeSource {
                secret_name: v.source_name.clone(),
                ..Default::default()
            }),
            ..Default::default()
        },
        "configMap" => Volume {
            name: v.name.clone(),
            config_map: Some(ConfigMapVolumeSource {
                name: v.source_name.clone().unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        },
        _ => Volume {
            name: v.name.clone(),
            ..Default::default()
        },
    }
}

/// Bind the pod to the ServiceAccount a plugin asked for, unless it already
/// uses another non-default one. A refusal is returned as a
/// `plugin.policy_rejected` warning.
fn apply_service_account(
    plugin: &LoadedPlugin,
    sa_name: String,
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
    scrubber: &SecretScrubber,
) -> Option<PluginEvent> {
    let current = pod_spec
        .service_account_name
        .as_deref()
        .unwrap_or("default");
    if current != "default" {
        // On update the pod usually already runs as the plugin's own SA.
        if current == sa_name {
            return None;
        }
        tracing::debug!(
            plugin = %plugin.name,
            current_sa = current,
            requested_sa = %sa_name,
            "skipping service_account_name — pod already has a non-default service account"
        );
        metrics::record_plugin_error(&plugin.name, "sa_conflict");
        return Some(host_event(
            EventSeverity::Warning,
            codes::POLICY_REJECTED,
            format!(
                "kept service account {current}: the plugin asked for {sa_name}, and deckwatch \
                 never replaces a non-default service account"
            ),
            scrubber,
        ));
    }
    pod_spec.service_account_name = Some(sa_name.clone());
    dep_annotations.insert(format!("deckwatch.plugin-sa/{}", plugin.name), sa_name);
    None
}

fn build_resources(
    cpu_request: Option<&str>,
    cpu_limit: Option<&str>,
    memory_request: Option<&str>,
    memory_limit: Option<&str>,
) -> Option<ResourceRequirements> {
    if cpu_request.is_none()
        && cpu_limit.is_none()
        && memory_request.is_none()
        && memory_limit.is_none()
    {
        return None;
    }
    let mut requests: BTreeMap<String, Quantity> = BTreeMap::new();
    let mut limits: BTreeMap<String, Quantity> = BTreeMap::new();
    if let Some(c) = cpu_request {
        requests.insert("cpu".to_string(), Quantity(c.to_string()));
    }
    if let Some(c) = cpu_limit {
        limits.insert("cpu".to_string(), Quantity(c.to_string()));
    }
    if let Some(m) = memory_request {
        requests.insert("memory".to_string(), Quantity(m.to_string()));
    }
    if let Some(m) = memory_limit {
        limits.insert("memory".to_string(), Quantity(m.to_string()));
    }
    Some(ResourceRequirements {
        requests: if requests.is_empty() {
            None
        } else {
            Some(requests)
        },
        limits: if limits.is_empty() {
            None
        } else {
            Some(limits)
        },
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
            mcp_tuning_fields: vec![],
            min_deckwatch_version: None,
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

    #[test]
    fn key_value_and_unknown_field_types_round_trip() {
        assert_eq!(
            serde_json::to_string(&ConfigFieldType::KeyValue).unwrap(),
            "\"key_value\""
        );
        let kv: ConfigFieldType = serde_json::from_str("\"key_value\"").unwrap();
        assert_eq!(kv, ConfigFieldType::KeyValue);
        let future: ConfigFieldType = serde_json::from_str("\"color_picker\"").unwrap();
        assert_eq!(future, ConfigFieldType::Unknown);
    }

    // Regression guard: an unrecognised field_type used to fail the whole
    // metadata() parse, so the plugin silently lost every config field and
    // resource it declares.
    #[test]
    fn metadata_with_a_future_field_type_still_parses() {
        let meta: PluginMetadata = serde_json::from_str(
            r#"{"name":"aws","config_schema":[
                {"key":"AWS_REGION","label":"Region","field_type":"string"},
                {"key":"RESOURCE_TAGS","label":"Tags","field_type":"key_value"},
                {"key":"FUTURE","label":"Future","field_type":"some_future_type"}
            ]}"#,
        )
        .expect("metadata must parse despite an unknown field type");
        let types: Vec<_> = meta
            .config_schema
            .iter()
            .map(|f| f.field_type.clone())
            .collect();
        assert_eq!(
            types,
            vec![
                ConfigFieldType::String,
                ConfigFieldType::KeyValue,
                ConfigFieldType::Unknown
            ]
        );
    }

    #[test]
    fn validate_key_value_updates_checks_only_key_value_fields() {
        let field = |key: &str, field_type| ConfigField {
            key: key.to_string(),
            label: key.to_string(),
            description: String::new(),
            field_type,
            default: None,
            required: false,
            options: vec![],
            env_source: None,
        };
        let schema = vec![
            field("RESOURCE_TAGS", ConfigFieldType::KeyValue),
            field("NOTE", ConfigFieldType::String),
        ];
        let good: std::collections::HashMap<String, String> = [
            (
                "RESOURCE_TAGS".to_string(),
                r#"{"Team":"platform"}"#.to_string(),
            ),
            (
                "NOTE".to_string(),
                "Team=platform is fine in a string field".to_string(),
            ),
            ("UNDECLARED".to_string(), "{not json".to_string()),
        ]
        .into();
        assert!(validate_key_value_updates(&schema, &good).is_ok());

        let bad: std::collections::HashMap<String, String> =
            [("RESOURCE_TAGS".to_string(), "Team=platform".to_string())].into();
        let err = validate_key_value_updates(&schema, &bad).unwrap_err();
        assert!(err.starts_with("RESOURCE_TAGS: "), "{err}");
    }

    #[test]
    fn validate_key_value_field_accepts_objects_of_strings_and_empty() {
        for ok in ["", "  ", "{}", r#"{"Team":"platform","CostCenter":"1234"}"#] {
            assert!(
                validate_key_value_field(ok).is_ok(),
                "{ok:?} should be accepted"
            );
        }
        for bad in [
            "Team=platform",
            r#"["Team"]"#,
            r#"{"Count":3}"#,
            r#"{"Nested":{"a":"b"}}"#,
            "{oops",
        ] {
            assert!(
                validate_key_value_field(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    fn make_plugin(name: &str, provides: Vec<&str>, depends_on: Vec<&str>) -> LoadedPlugin {
        LoadedPlugin {
            name: name.to_string(),
            wasm_bytes: Arc::new(vec![]),
            allowed_hosts: vec![],
            config: BTreeMap::new(),
            inherit_env_keys: vec![],
            inherit_env_file_keys: BTreeMap::new(),
            metadata: PluginMetadata {
                name: name.to_string(),
                provides: provides.into_iter().map(String::from).collect(),
                depends_on: depends_on.into_iter().map(String::from).collect(),
                ..Default::default()
            },
            sha256: sha256_hex(&[]),
            loaded_at: String::new(),
        }
    }

    fn with_sha(mut p: LoadedPlugin, sha: &str) -> LoadedPlugin {
        p.sha256 = sha.to_string();
        p
    }

    fn names_and_shas(plugins: &[LoadedPlugin]) -> Vec<(String, String)> {
        plugins
            .iter()
            .map(|p| (p.name.clone(), p.sha256.clone()))
            .collect()
    }

    // Regression: uploading one plugin used to replace the whole loaded set
    // with the single-plugin reload, unloading every other plugin.
    #[test]
    fn merge_reloaded_plugin_keeps_other_plugins_and_position() {
        let mut current = vec![
            with_sha(make_plugin("AWS", vec![], vec![]), "aws-old"),
            with_sha(make_plugin("entra", vec![], vec![]), "entra"),
            with_sha(make_plugin("example", vec![], vec![]), "example"),
        ];
        let reloaded = vec![with_sha(make_plugin("entra", vec![], vec![]), "entra-new")];

        merge_reloaded_plugin(&mut current, "entra", reloaded);

        assert_eq!(
            names_and_shas(&current),
            vec![
                ("AWS".into(), "aws-old".into()),
                ("entra".into(), "entra-new".into()),
                ("example".into(), "example".into()),
            ]
        );
    }

    // A broken upload unloads only that plugin, as a full reload would.
    #[test]
    fn merge_reloaded_plugin_failed_reload_unloads_only_that_plugin() {
        let mut current = vec![
            make_plugin("AWS", vec![], vec![]),
            make_plugin("entra", vec![], vec![]),
        ];

        merge_reloaded_plugin(&mut current, "AWS", vec![]);

        let names: Vec<_> = current.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["entra"]);
    }

    // First upload for a configured-but-not-yet-loaded plugin is appended.
    #[test]
    fn merge_reloaded_plugin_appends_newly_loaded_plugin() {
        let mut current = vec![make_plugin("AWS", vec![], vec![])];

        merge_reloaded_plugin(
            &mut current,
            "entra",
            vec![make_plugin("entra", vec![], vec![])],
        );

        let names: Vec<_> = current.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["AWS", "entra"]);
    }

    // Only entries for the named plugin are merged; anything else in the
    // reload result is ignored rather than duplicated.
    #[test]
    fn merge_reloaded_plugin_ignores_unrelated_entries() {
        let mut current = vec![with_sha(make_plugin("AWS", vec![], vec![]), "aws")];
        let reloaded = vec![
            with_sha(make_plugin("entra", vec![], vec![]), "entra-new"),
            with_sha(make_plugin("AWS", vec![], vec![]), "stray"),
        ];

        merge_reloaded_plugin(&mut current, "entra", reloaded);

        assert_eq!(
            names_and_shas(&current),
            vec![
                ("AWS".into(), "aws".into()),
                ("entra".into(), "entra-new".into()),
            ]
        );
    }

    #[test]
    fn sha256_hex_matches_shasum_format() {
        // `printf '' | shasum -a 256` and `printf 'abc' | shasum -a 256`
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn now_rfc3339_is_parseable_utc() {
        let ts = now_rfc3339();
        let parsed = sea_orm::prelude::DateTimeWithTimeZone::parse_from_rfc3339(&ts)
            .expect("loaded_at should be valid RFC 3339");
        assert_eq!(parsed.offset().local_minus_utc(), 0, "should be UTC");
        assert!(
            parsed.timestamp() > 1_700_000_000,
            "timestamp should be recent"
        );
    }

    #[test]
    fn sort_by_dependencies_respects_ordering() {
        let plugins = vec![
            make_plugin("consumer", vec![], vec!["capability-a"]),
            make_plugin("provider", vec!["capability-a"], vec![]),
        ];
        let sorted = sort_by_dependencies(&plugins);
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["provider", "consumer"],
            "provider must run before consumer"
        );
    }

    #[test]
    fn sort_by_dependencies_handles_cycle_without_panic() {
        let plugins = vec![
            make_plugin("a", vec!["cap-a"], vec!["cap-b"]),
            make_plugin("b", vec!["cap-b"], vec!["cap-a"]),
        ];
        let sorted = sort_by_dependencies(&plugins);
        assert_eq!(
            sorted.len(),
            2,
            "all plugins must be returned even with a cycle"
        );
        let names: std::collections::HashSet<&str> =
            sorted.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains("a"));
        assert!(names.contains("b"));
    }

    #[test]
    fn sort_by_dependencies_single_plugin() {
        let plugins = vec![make_plugin("solo", vec![], vec![])];
        let sorted = sort_by_dependencies(&plugins);
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].name, "solo");
    }

    #[test]
    fn sort_by_dependencies_preserves_order_without_edges() {
        let plugins = vec![
            make_plugin("alpha", vec![], vec![]),
            make_plugin("beta", vec![], vec![]),
            make_plugin("gamma", vec![], vec![]),
        ];
        let sorted = sort_by_dependencies(&plugins);
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "beta", "gamma"],
            "plugins with no deps keep original order"
        );
    }

    #[test]
    fn sort_by_dependencies_disables_plugin_with_unmet_dep() {
        // "consumer" depends on "capability-x" which no plugin provides.
        let plugins = vec![
            make_plugin("provider", vec!["capability-a"], vec![]),
            make_plugin("consumer", vec![], vec!["capability-x"]),
        ];
        let sorted = sort_by_dependencies(&plugins);
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"provider"),
            "provider should still be included"
        );
        assert!(
            !names.contains(&"consumer"),
            "consumer must be excluded when its dep is unmet"
        );
    }

    #[test]
    fn sort_by_dependencies_keeps_plugin_with_met_dep() {
        let plugins = vec![
            make_plugin("provider", vec!["capability-a"], vec![]),
            make_plugin("consumer", vec![], vec!["capability-a"]),
        ];
        let sorted = sort_by_dependencies(&plugins);
        assert_eq!(
            sorted.len(),
            2,
            "both plugins should survive when dep is met"
        );
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["provider", "consumer"]);
    }

    #[test]
    fn version_gte_basic_comparisons() {
        assert!(version_gte("0.7.0", "0.6.3"), "0.7.0 >= 0.6.3");
        assert!(version_gte("1.0.0", "0.9.9"), "1.0.0 >= 0.9.9");
        assert!(version_gte("0.6.3", "0.6.3"), "equal versions are >=");
        assert!(!version_gte("0.5.9", "0.6.0"), "0.5.9 < 0.6.0");
        assert!(!version_gte("0.6.2", "0.6.3"), "0.6.2 < 0.6.3");
        assert!(version_gte("0.6.4", "0.6.3"), "patch bump is sufficient");
    }

    #[test]
    fn version_gte_handles_missing_components() {
        assert!(
            version_gte("1", "0.9.9"),
            "single-component version treated as 1.0.0"
        );
        assert!(
            version_gte("1.2", "1.1.9"),
            "two-component version treated as 1.2.0"
        );
    }

    #[test]
    fn plugin_metadata_min_deckwatch_version_round_trips() {
        let meta = PluginMetadata {
            name: "test".to_string(),
            min_deckwatch_version: Some("0.7.0".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("min_deckwatch_version"));
        let back: PluginMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.min_deckwatch_version.as_deref(), Some("0.7.0"));
    }

    #[test]
    fn plugin_metadata_min_deckwatch_version_absent_defaults_to_none() {
        let json = r#"{"name":"old","version":"0.1.0","description":"","provides":[],"depends_on":[],"optional_depends_on":[]}"#;
        let m: PluginMetadata = serde_json::from_str(json).unwrap();
        assert!(
            m.min_deckwatch_version.is_none(),
            "absent field must default to None"
        );
    }

    #[test]
    fn apply_service_account_sets_sa_on_default() {
        let plugin = make_plugin("test-plugin", vec![], vec![]);
        let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
            containers: vec![],
            ..Default::default()
        };
        let mut annotations = BTreeMap::new();

        let refusal = apply_service_account(
            &plugin,
            "my-sa".to_string(),
            &mut pod_spec,
            &mut annotations,
            &SecretScrubber::default(),
        );
        assert!(refusal.is_none());

        assert_eq!(
            pod_spec.service_account_name.as_deref(),
            Some("my-sa"),
            "SA should be set when pod has default SA"
        );
        assert_eq!(
            annotations.get("deckwatch.plugin-sa/test-plugin"),
            Some(&"my-sa".to_string())
        );
    }

    #[test]
    fn apply_service_account_skips_non_default() {
        let plugin = make_plugin("test-plugin", vec![], vec![]);
        let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
            containers: vec![],
            service_account_name: Some("existing-sa".to_string()),
            ..Default::default()
        };
        let mut annotations = BTreeMap::new();

        let refusal = apply_service_account(
            &plugin,
            "new-sa".to_string(),
            &mut pod_spec,
            &mut annotations,
            &SecretScrubber::default(),
        );
        assert_eq!(refusal.unwrap().code, codes::POLICY_REJECTED);

        assert_eq!(
            pod_spec.service_account_name.as_deref(),
            Some("existing-sa"),
            "existing non-default SA must not be overwritten"
        );
        assert!(
            !annotations.contains_key("deckwatch.plugin-sa/test-plugin"),
            "no annotation should be set when SA is skipped"
        );
    }

    #[test]
    fn apply_env_vars_skips_duplicates() {
        let plugin = make_plugin("test-plugin", vec![], vec![]);
        let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
            containers: vec![k8s_openapi::api::core::v1::Container {
                name: "app".to_string(),
                env: Some(vec![k8s_openapi::api::core::v1::EnvVar {
                    name: "EXISTING".to_string(),
                    value: Some("old-value".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut annotations = BTreeMap::new();

        let env_vars = vec![
            EnvVarSpec {
                name: "EXISTING".to_string(),
                value: "new-value".to_string(),
                value_from: None,
            },
            EnvVarSpec {
                name: "NEW_VAR".to_string(),
                value: "added".to_string(),
                value_from: None,
            },
        ];

        apply_env_vars(&plugin, &env_vars, &mut pod_spec, &mut annotations);

        let env = pod_spec.containers[0].env.as_ref().unwrap();
        assert_eq!(env.len(), 2, "duplicate should not be added");
        assert_eq!(
            env[0].value.as_deref(),
            Some("old-value"),
            "existing var should keep original value"
        );
        assert_eq!(env[1].name, "NEW_VAR");
    }

    #[test]
    fn apply_sidecars_skips_duplicate_container_names() {
        let plugin = make_plugin("test-plugin", vec![], vec![]);
        let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
            containers: vec![k8s_openapi::api::core::v1::Container {
                name: "existing-sidecar".to_string(),
                image: Some("old:latest".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut annotations = BTreeMap::new();

        let sidecars = vec![
            SidecarSpec {
                name: "existing-sidecar".to_string(),
                image: "new:latest".to_string(),
                ..Default::default()
            },
            SidecarSpec {
                name: "new-sidecar".to_string(),
                image: "sidecar:v1".to_string(),
                port: Some(8080),
                ..Default::default()
            },
        ];

        apply_sidecars(&plugin, &sidecars, &mut pod_spec, &mut annotations);

        assert_eq!(
            pod_spec.containers.len(),
            2,
            "duplicate sidecar should not be added"
        );
        assert_eq!(
            pod_spec.containers[0].image.as_deref(),
            Some("old:latest"),
            "existing container image should not be overwritten"
        );
        assert_eq!(pod_spec.containers[1].name, "new-sidecar");
    }

    #[test]
    fn upload_path_allows_only_plain_file_names() {
        let dir = std::path::Path::new("/data/uploads");
        assert_eq!(upload_path(dir, "aws.wasm"), Some(dir.join("aws.wasm")));
        assert_eq!(
            upload_path(dir, "my_plugin-2.0.wasm"),
            Some(dir.join("my_plugin-2.0.wasm"))
        );
        for bad in [
            "",
            ".wasm",
            "..",
            "../x.wasm",
            "../../etc/passwd",
            "/etc/passwd",
            "a/b.wasm",
            "a\\b.wasm",
            "x\0.wasm",
            "a b.wasm",
            "\u{fc}.wasm",
        ] {
            assert_eq!(upload_path(dir, bad), None, "{bad:?}");
        }
        assert_eq!(upload_path(dir, &"a".repeat(256)), None);
    }

    /// Upload filenames come from settings, which anyone who can edit them
    /// chooses: one outside the uploads directory isn't read.
    #[tokio::test]
    async fn an_upload_source_outside_the_uploads_dir_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
        let http = reqwest::Client::new();
        let settings = DeckwatchSettings::default();
        for filename in ["../../etc/hosts", "/etc/hosts"] {
            let cfg = PluginConfig {
                name: "demo".into(),
                enabled: true,
                source: PluginSource::Upload {
                    filename: filename.into(),
                },
                token_secret: None,
                allowed_hosts: vec![],
                config: Default::default(),
                inherit_env_keys: vec![],
                inherit_env_file_keys: Default::default(),
                sha256: None,
                mcp_tuning: Default::default(),
            };
            let err = fetch_bytes(&cfg, &http, &settings, &state)
                .await
                .expect_err(filename);
            assert!(
                err.to_string().contains("isn't a plain file name"),
                "{filename}: {err}"
            );
        }
    }

    /// A pin used to be ignored for uploads: the bytes were returned before
    /// the checksum was compared.
    #[tokio::test]
    async fn a_pin_applies_to_uploaded_binaries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("demo.wasm"), b"\0asm uploaded").unwrap();
        let mut cfg = PluginConfig {
            name: "demo".into(),
            enabled: true,
            source: PluginSource::Upload {
                filename: "demo.wasm".into(),
            },
            token_secret: None,
            allowed_hosts: vec![],
            config: Default::default(),
            inherit_env_keys: vec![],
            inherit_env_file_keys: Default::default(),
            sha256: Some("0".repeat(64)),
            mcp_tuning: Default::default(),
        };
        let err = read_upload(dir.path(), &cfg, "demo.wasm")
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ChecksumMismatch>().is_some(), "{err:#}");

        cfg.sha256 = Some(sha256_hex(b"\0asm uploaded"));
        assert_eq!(
            read_upload(dir.path(), &cfg, "demo.wasm").await.unwrap(),
            b"\0asm uploaded"
        );
        cfg.sha256 = None;
        assert!(read_upload(dir.path(), &cfg, "demo.wasm").await.is_ok());
    }

    #[test]
    fn inherited_env_var_refuses_deckwatch_secrets() {
        let env = |k: &str| Some(format!("value-of-{k}"));
        for key in NON_INHERITABLE_ENV {
            assert_eq!(inherited_env_var_from("evil", key, env), None, "{key}");
        }
        assert_eq!(
            inherited_env_var_from("aws", "AWS_REGION", env).as_deref(),
            Some("value-of-AWS_REGION")
        );
        assert_eq!(inherited_env_var_from("aws", "UNSET", |_| None), None);
    }
}

#[cfg(test)]
#[path = "plugins_funnel_tests.rs"]
mod funnel_tests;
