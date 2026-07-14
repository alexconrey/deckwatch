use std::collections::BTreeMap;

use axum::extract::State;
use axum::Json;
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Patch, PatchParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

/// Name of the ConfigMap holding per-cluster template overrides. Kept fixed
/// (rather than plumbed through config) so the operator only has to look in
/// one place — same namespace as the settings ConfigMap.
pub const TEMPLATES_CONFIGMAP_NAME: &str = "deckwatch-templates";
const TEMPLATES_KEY: &str = "templates";
const FIELD_MANAGER: &str = "deckwatch";

/// A pre-filled deployment payload that can be POSTed to
/// `/api/namespaces/{ns}/deployments` after the operator fills in the name.
///
/// This is kept as a plain JSON value (rather than a typed
/// [`crate::handlers::deployments::CreateDeploymentRequest`]) so templates
/// can carry hints — e.g. a suggested container port for a probe — even when
/// the fields don't line up 1:1 with the create schema. The frontend copies
/// the fields into the same form that a hand-authored deployment uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: TemplateCategory,
    pub payload: serde_json::Value,
    /// True when the entry is one of the compiled-in defaults. Set by the
    /// server on every list response so the frontend can badge the row and
    /// enable "Reset to Default" only where it makes sense. Never persisted —
    /// the server recomputes it from the default catalog on load.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateCategory {
    WebApp,
    Worker,
    CronJob,
    StaticSite,
}

#[derive(Serialize)]
pub struct TemplateListResponse {
    pub templates: Vec<DeploymentTemplate>,
}

#[derive(Deserialize)]
pub struct TemplatesUpdateRequest {
    /// Full set of custom + overridden entries. The server diffs against the
    /// default catalog and only persists entries that differ (either an
    /// override of a builtin, or a wholly-new custom entry). Sending the
    /// unchanged defaults back is safe — they roundtrip cleanly.
    pub templates: Vec<DeploymentTemplate>,
}

// Templates intentionally do NOT pre-fill resource_requests / resource_limits.
// The deployment form pulls defaults from the cluster Settings page
// (`default_resource_limits`) when configured, and otherwise leaves the fields
// blank so operators explicitly opt in to a limit rather than inheriting a
// silent one from a template.
fn default_catalog() -> Vec<DeploymentTemplate> {
    vec![
        DeploymentTemplate {
            id: "web-app".to_string(),
            name: "Web App".to_string(),
            description: "HTTP service listening on port 80 with a readiness probe on '/'. \
                 One replica. Good starting point for a stateless web service."
                .to_string(),
            icon: "mdi-web".to_string(),
            category: TemplateCategory::WebApp,
            payload: serde_json::json!({
                "name": "",
                "image": "nginx:1.27-alpine",
                "replicas": 1,
                "port": 80,
                "readiness_probe": {
                    "probe_type": "httpGet",
                    "path": "/",
                    "port": 80,
                    "initial_delay_seconds": 5,
                    "period_seconds": 10,
                },
            }),
            builtin: true,
        },
        DeploymentTemplate {
            id: "worker".to_string(),
            name: "Worker".to_string(),
            description: "Background worker with no exposed port and no ingress. \
                 One replica."
                .to_string(),
            icon: "mdi-cog-transfer".to_string(),
            category: TemplateCategory::Worker,
            payload: serde_json::json!({
                "name": "",
                "image": "",
                "replicas": 1,
            }),
            builtin: true,
        },
        DeploymentTemplate {
            id: "cron-job".to_string(),
            name: "Cron Job".to_string(),
            description: "One-shot container intended to be wrapped by a CronJob resource. \
                 Deckwatch currently deploys this as a scale-to-zero Deployment so \
                 you can iterate on the container; convert to a CronJob when the \
                 command line is stable."
                .to_string(),
            icon: "mdi-clock-outline".to_string(),
            category: TemplateCategory::CronJob,
            payload: serde_json::json!({
                "name": "",
                "image": "alpine:3",
                "replicas": 0,
                "command": ["/bin/sh", "-c"],
                "args": ["echo 'replace me with your cron command' && sleep 5"],
            }),
            builtin: true,
        },
        DeploymentTemplate {
            id: "static-site".to_string(),
            name: "Static Site".to_string(),
            description: "Nginx serving static assets on port 80 with a readiness probe. \
                 Pair with an Ingress from the deployment detail page for public \
                 exposure."
                .to_string(),
            icon: "mdi-file-document-outline".to_string(),
            category: TemplateCategory::StaticSite,
            payload: serde_json::json!({
                "name": "",
                "image": "nginx:1.27-alpine",
                "replicas": 1,
                "port": 80,
                "readiness_probe": {
                    "probe_type": "httpGet",
                    "path": "/",
                    "port": 80,
                    "initial_delay_seconds": 2,
                    "period_seconds": 10,
                },
            }),
            builtin: true,
        },
    ]
}

/// Fetch the templates override ConfigMap. NotFound is treated as "no
/// overrides configured" — a fresh install has no CM until the operator
/// saves one, and that's the common case, so it doesn't inflate the k8s
/// error counter.
async fn load_overrides(state: &AppState) -> Vec<DeploymentTemplate> {
    let Ok(api) = state.configmaps_api(&state.settings_namespace) else {
        return Vec::new();
    };
    let t = K8sTimer::new("configmaps", "get");
    let res = api.get(TEMPLATES_CONFIGMAP_NAME).await;
    let is_notfound = matches!(&res, Err(kube::Error::Api(e)) if e.code == 404);
    t.finish(res.is_ok() || is_notfound);
    let Ok(cm) = res else {
        return Vec::new();
    };
    cm.data
        .as_ref()
        .and_then(|d| d.get(TEMPLATES_KEY))
        .and_then(|s| serde_json::from_str::<Vec<DeploymentTemplate>>(s).ok())
        .unwrap_or_default()
}

/// Merge overrides on top of the default catalog. Entries with an `id` that
/// matches a default replace it in place (preserving list order for the
/// stable defaults). New entries are appended at the end.
fn merge_catalog(
    defaults: Vec<DeploymentTemplate>,
    overrides: Vec<DeploymentTemplate>,
) -> Vec<DeploymentTemplate> {
    let mut by_id: std::collections::HashMap<String, DeploymentTemplate> = overrides
        .into_iter()
        .map(|mut t| {
            // Overrides never get to claim `builtin: true`; the server
            // recomputes that flag below based on default-catalog membership.
            t.builtin = false;
            (t.id.clone(), t)
        })
        .collect();

    let mut merged: Vec<DeploymentTemplate> = Vec::with_capacity(defaults.len() + by_id.len());
    for def in defaults {
        if let Some(over) = by_id.remove(&def.id) {
            // Preserve builtin=true so the frontend can offer "Reset to
            // Default" for overridden defaults.
            let mut t = over;
            t.builtin = true;
            merged.push(t);
        } else {
            merged.push(def);
        }
    }
    // Whatever's left in the map is a wholly-custom entry — append after
    // the defaults. Sort by id so the UI order is stable across reloads
    // (HashMap iteration is non-deterministic).
    let mut custom: Vec<DeploymentTemplate> = by_id.into_values().collect();
    custom.sort_by(|a, b| a.id.cmp(&b.id));
    for mut t in custom {
        t.builtin = false;
        merged.push(t);
    }
    merged
}

pub async fn list(State(state): State<AppState>) -> Result<Json<TemplateListResponse>, AppError> {
    let overrides = load_overrides(&state).await;
    let merged = merge_catalog(default_catalog(), overrides);
    Ok(Json(TemplateListResponse { templates: merged }))
}

/// Persist template overrides. Only entries that actually differ from the
/// compiled-in default (or are wholly new) are written to the ConfigMap —
/// we don't want to freeze a copy of a default in etcd and have it drift
/// silently from a later deckwatch release.
pub async fn update(
    State(state): State<AppState>,
    Json(req): Json<TemplatesUpdateRequest>,
) -> Result<Json<TemplateListResponse>, AppError> {
    let defaults = default_catalog();
    let defaults_by_id: std::collections::HashMap<&str, &DeploymentTemplate> =
        defaults.iter().map(|t| (t.id.as_str(), t)).collect();

    let mut to_persist: Vec<DeploymentTemplate> = Vec::new();
    for mut entry in req.templates {
        // Strip the server-set `builtin` flag before comparing / persisting.
        entry.builtin = false;
        match defaults_by_id.get(entry.id.as_str()) {
            Some(default) => {
                let mut stripped_default = (*default).clone();
                stripped_default.builtin = false;
                if !templates_equal(&entry, &stripped_default) {
                    to_persist.push(entry);
                }
            }
            None => {
                if entry.id.trim().is_empty() {
                    return Err(AppError::BadRequest(
                        "custom template id must not be empty".to_string(),
                    ));
                }
                to_persist.push(entry);
            }
        }
    }

    let serialized = serde_json::to_string_pretty(&to_persist)
        .map_err(|e| AppError::BadRequest(format!("failed to serialize templates: {e}")))?;

    let mut data = BTreeMap::new();
    data.insert(TEMPLATES_KEY.to_string(), serialized);

    let mut labels = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );
    labels.insert(
        "app.kubernetes.io/component".to_string(),
        "templates".to_string(),
    );

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(TEMPLATES_CONFIGMAP_NAME.to_string()),
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
        .patch(TEMPLATES_CONFIGMAP_NAME, &params, &Patch::Apply(&cm))
        .await;
    t.finish(updated.is_ok());
    updated?;

    // Re-read via the same merge path so the client sees the exact list the
    // next `list()` call would return (including builtin flags).
    let overrides = load_overrides(&state).await;
    let merged = merge_catalog(default_catalog(), overrides);
    Ok(Json(TemplateListResponse { templates: merged }))
}

/// Structural equality on the persisted fields. `builtin` is intentionally
/// excluded because it's a derived, server-set marker.
fn templates_equal(a: &DeploymentTemplate, b: &DeploymentTemplate) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.description == b.description
        && a.icon == b.icon
        && serde_json::to_value(&a.category).ok() == serde_json::to_value(&b.category).ok()
        && a.payload == b.payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_custom_entries() {
        let defaults = default_catalog();
        let defaults_len = defaults.len();
        let custom = vec![DeploymentTemplate {
            id: "custom-thing".to_string(),
            name: "Custom".to_string(),
            description: "d".to_string(),
            icon: "mdi-star".to_string(),
            category: TemplateCategory::WebApp,
            payload: serde_json::json!({"name": ""}),
            builtin: false,
        }];
        let merged = merge_catalog(defaults, custom);
        assert_eq!(merged.len(), defaults_len + 1);
        assert_eq!(merged.last().unwrap().id, "custom-thing");
        assert!(!merged.last().unwrap().builtin);
    }

    #[test]
    fn merge_overrides_builtin_in_place() {
        let defaults = default_catalog();
        let overrides = vec![DeploymentTemplate {
            id: "web-app".to_string(),
            name: "Custom Web App".to_string(),
            description: "d".to_string(),
            icon: "mdi-web".to_string(),
            category: TemplateCategory::WebApp,
            payload: serde_json::json!({"image": "custom:latest"}),
            builtin: false,
        }];
        let merged = merge_catalog(defaults, overrides);
        let web = merged.iter().find(|t| t.id == "web-app").unwrap();
        assert_eq!(web.name, "Custom Web App");
        assert!(web.builtin, "overridden default should retain builtin=true");
    }
}
