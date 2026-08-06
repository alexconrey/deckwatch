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

use extism::{Manifest, Plugin, Wasm};
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

// ── Shared types (mirror of deckwatch-plugin-sdk v0.2.0) ─────────────────────
// These must stay in sync with `deckwatch-plugin-sdk/src/lib.rs`. The SDK is
// the canonical source; this is the host-side copy for deserializing plugin
// output without adding a workspace dependency on the SDK crate.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContext {
    pub namespace: String,
    pub deployment_name: String,
    pub annotations: std::collections::HashMap<String, String>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginResult {
    #[serde(default)]
    pub env_vars: Vec<EnvVarSpec>,
    #[serde(default)]
    pub sidecars: Vec<SidecarSpec>,
    #[serde(default)]
    pub kubernetes_resources: Vec<serde_json::Value>,
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

/// A fetched plugin ready to execute. Stores raw WASM bytes and instantiates
/// a fresh `extism::Plugin` per call to avoid thread-safety concerns.
#[derive(Clone)]
pub struct LoadedPlugin {
    pub name: String,
    pub wasm_bytes: Vec<u8>,
}

// ── Fetching ─────────────────────────────────────────────────────────────────

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
                loaded.push(LoadedPlugin { name: cfg.name.clone(), wasm_bytes: bytes });
            }
            Err(e) => {
                tracing::error!(plugin = %cfg.name, error = %e, "failed to fetch plugin WASM");
            }
        }
    }
    loaded
}

async fn fetch_bytes(
    cfg: &PluginConfig,
    http: &reqwest::Client,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> anyhow::Result<Vec<u8>> {
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
        PluginSource::Github { repo, git_ref, path, use_release } => {
            if *use_release {
                format!("https://github.com/{repo}/releases/download/{git_ref}/{path}")
            } else {
                format!("https://raw.githubusercontent.com/{repo}/{git_ref}/{path}")
            }
        }
        PluginSource::Url { url } => url.clone(),
    }
}

async fn resolve_token(
    secret_ref_name: &str,
    settings: &DeckwatchSettings,
    state: &AppState,
) -> Option<String> {
    let entry = settings.git_token_secrets.iter().find(|s| s.name == secret_ref_name)?;

    if let Some(enc) = &entry.encrypted_token {
        if !state.encryption_key.is_empty() {
            return crate::crypto::decrypt(&state.encryption_key, enc).ok();
        }
    }

    let ns = if entry.namespace.is_empty() { &state.settings_namespace } else { &entry.namespace };
    let secrets_api = state.secrets_api(ns).ok()?;
    let secret = secrets_api.get(&entry.secret_name).await.ok()?;
    let raw = secret.data?.get("token")?.0.clone();
    String::from_utf8(raw).ok()
}

// ── Applying ─────────────────────────────────────────────────────────────────

/// Run all loaded plugins against `ctx`, merging env vars and sidecars into
/// `pod_spec`/`dep_annotations`. Returns the collected `kubernetes_resources`
/// from all plugins so the caller can apply them asynchronously after the
/// deployment is committed.
pub fn apply_plugins(
    plugins: &[LoadedPlugin],
    ctx: &PluginContext,
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    dep_annotations: &mut BTreeMap<String, String>,
) -> Vec<serde_json::Value> {
    let mut all_k8s_resources: Vec<serde_json::Value> = Vec::new();

    for plugin in plugins {
        match run_plugin(plugin, ctx) {
            Ok(result) => {
                apply_env_vars(plugin, &result.env_vars, pod_spec, dep_annotations);
                apply_sidecars(plugin, &result.sidecars, pod_spec, dep_annotations);
                all_k8s_resources.extend(result.kubernetes_resources);
            }
            Err(e) => {
                tracing::error!(plugin = %plugin.name, error = %e, "plugin apply() failed; skipping");
            }
        }
    }

    all_k8s_resources
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
            let kind = resource.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
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

fn run_plugin(plugin: &LoadedPlugin, ctx: &PluginContext) -> anyhow::Result<PluginResult> {
    let wasm = Wasm::data(plugin.wasm_bytes.clone());
    let manifest = Manifest::new([wasm]);
    let mut p = Plugin::new(&manifest, [], false)?;
    let input = serde_json::to_string(ctx)?;
    let output = p.call::<&str, &str>("apply", &input)?;
    Ok(serde_json::from_str(output)?)
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
        value: if spec.value.is_empty() { None } else { Some(spec.value.clone()) },
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
            vec![ContainerPort { container_port: p, ..Default::default() }]
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
