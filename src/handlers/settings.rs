use std::collections::BTreeMap;

use axum::extract::State;
use axum::Json;
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Patch, PatchParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::notifications::{NotificationClient, NotificationEvent};
use crate::metrics::K8sTimer;
use crate::rate_limit::DEFAULT_HOURLY_LIMIT;
use crate::state::AppState;

const SETTINGS_KEY: &str = "settings";
const FIELD_MANAGER: &str = "deckwatch";

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
    /// Safety knobs for AI-agent jobs (diagnostics + ai-fix). Optional so
    /// existing settings ConfigMaps deserialize cleanly; a missing block
    /// means "use the compiled-in defaults".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_safety: Option<AiSafetySettings>,
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

/// Operator-tunable safety controls for LLM-agent jobs. Persisted in the
/// deckwatch settings ConfigMap so a cluster admin can raise the cap
/// without redeploying. Applied at startup and on every PUT -- the running
/// rate limiter is hot-swapped from `put_settings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSafetySettings {
    /// Maximum AI-agent jobs (diagnostics + ai-fix combined) that any single
    /// namespace can start per rolling hour. `0` is coerced to `1` at the
    /// limiter level; use a big number if you want "unlimited" (there's no
    /// disabled state -- the guardrail is always on).
    #[serde(default = "default_hourly_limit")]
    pub jobs_per_namespace_per_hour: u32,
}

impl Default for AiSafetySettings {
    fn default() -> Self {
        Self {
            jobs_per_namespace_per_hour: DEFAULT_HOURLY_LIMIT,
        }
    }
}

fn default_hourly_limit() -> u32 {
    DEFAULT_HOURLY_LIMIT
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
    let api = state.configmaps_api(&state.settings_namespace)?;
    // The settings CM often doesn't exist yet on a fresh install -- a NotFound
    // is expected and falls back to defaults, so don't inflate the k8s error
    // counter for that case.
    let t = K8sTimer::new("configmaps", "get");
    let get_res = api.get(&state.settings_configmap_name).await;
    let is_notfound = matches!(&get_res, Err(kube::Error::Api(e)) if e.code == 404);
    t.finish(get_res.is_ok() || is_notfound);
    let mut settings = match get_res {
        Ok(cm) => parse_settings(&cm),
        Err(_) => default_settings(&state),
    };
    inject_builtin_registry(&state, &mut settings);
    Ok(Json(settings))
}

pub async fn put_settings(
    State(state): State<AppState>,
    Json(mut settings): Json<DeckwatchSettings>,
) -> Result<Json<DeckwatchSettings>, AppError> {
    // Strip the injected builtin entry before persisting -- it's derived
    // from the deployment env var, not user data.
    settings.oci_registries.retain(|r| !r.builtin);

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|e| AppError::BadRequest(format!("failed to serialize settings: {e}")))?;

    let mut data = BTreeMap::new();
    data.insert(SETTINGS_KEY.to_string(), serialized);

    let mut labels = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );
    labels.insert(
        "app.kubernetes.io/component".to_string(),
        "settings".to_string(),
    );

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(state.settings_configmap_name.clone()),
            namespace: Some(state.settings_namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let api = state.configmaps_api(&state.settings_namespace)?;
    let params = PatchParams::apply(FIELD_MANAGER).force();
    let t = K8sTimer::new("configmaps", "patch");
    let updated = api
        .patch(&state.settings_configmap_name, &params, &Patch::Apply(&cm))
        .await;
    t.finish(updated.is_ok());
    let updated = updated?;

    let mut result = parse_settings(&updated);
    inject_builtin_registry(&state, &mut result);

    // Hot-swap the running rate limiter so a Settings change takes effect
    // without a pod restart. The limiter is best-effort; a missing block
    // preserves the previous limit.
    if let Some(ai) = result.ai_safety.as_ref() {
        state.ai_rate_limiter.set_limit(ai.jobs_per_namespace_per_hour);
    }

    Ok(Json(result))
}

/// If this deckwatch instance runs the embedded registry, prepend it to
/// the OCI registries list so it shows up first in the GitOps dialog.
/// De-duped by name so it isn't added twice if someone persisted it by
/// mistake (older frontend).
fn inject_builtin_registry(state: &AppState, settings: &mut DeckwatchSettings) {
    let Some(url) = state.registry_public_url.as_deref() else {
        return;
    };
    settings.oci_registries.retain(|r| r.name != DECKWATCH_REGISTRY_NAME);
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
        ai_safety: Some(AiSafetySettings::default()),
        git_repositories: Vec::new(),
        oci_registries: Vec::new(),
        git_token_secrets: Vec::new(),
        tracing: Some(TracingSettings::default()),
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
