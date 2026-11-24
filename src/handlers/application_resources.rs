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
//! DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
//! ```

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::PodSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams};
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::entities::application_plugin_resources;
use crate::error::AppError;
use crate::metrics;
use crate::plugins::{
    KubernetesObjectRef, ResourceProvisionRequest, ResourceProvisionResult, SidecarSpec,
};
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
    };

    let result: ResourceProvisionResult = crate::plugins::run_provision(plugin, &req)
        .map_err(|e| AppError::BadRequest(format!("provision() failed: {e}")))?;

    // Drop the read lock before the async apply call.
    let kubernetes_resources = result.kubernetes_resources.clone();
    let state_map = result.state.clone();
    let deployment_annotations = result.deployment_annotations.clone();
    let sidecars: Vec<SidecarSpec> = result.sidecars.clone();
    drop(plugins);

    // Apply any Kubernetes resources emitted by the plugin.
    if !kubernetes_resources.is_empty() {
        crate::plugins::apply_kubernetes_resources(&kubernetes_resources, &state.kube_client).await;
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

    Ok((StatusCode::CREATED, Json(row_to_response(inserted))))
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
    };

    let result: ResourceProvisionResult = crate::plugins::run_provision(plugin, &req)
        .map_err(|e| AppError::BadRequest(format!("provision() failed: {e}")))?;

    let kubernetes_resources = result.kubernetes_resources.clone();
    let state_map = result.state.clone();
    let deployment_annotations = result.deployment_annotations.clone();
    let sidecars: Vec<SidecarSpec> = result.sidecars.clone();
    drop(plugins);

    if !kubernetes_resources.is_empty() {
        crate::plugins::apply_kubernetes_resources(&kubernetes_resources, &state.kube_client).await;
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

    Ok(Json(row_to_response(updated)))
}

// ── DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}

pub async fn deprovision(
    State(state): State<AppState>,
    Path((ns, app_name, plugin_name, resource_id)): Path<(String, String, String, String)>,
) -> Result<StatusCode, AppError> {
    let resource = DeprovisionTarget {
        namespace: &ns,
        application: &app_name,
        plugin: &plugin_name,
        resource_id: &resource_id,
    };
    deprovision_resource(&state, &resource).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The resource being deprovisioned, for lookups, logs and audit entries.
pub struct DeprovisionTarget<'a> {
    pub namespace: &'a str,
    pub application: &'a str,
    pub plugin: &'a str,
    pub resource_id: &'a str,
}

/// Call the plugin's `deprovision()`, delete the Kubernetes objects it asks
/// for, then remove the resource record. Shared by the HTTP handler and the
/// MCP `deprovision_resource` tool. Returns what happened to each object the
/// plugin asked deckwatch to delete.
pub async fn deprovision_resource(
    state: &AppState,
    resource: &DeprovisionTarget<'_>,
) -> Result<Vec<KubernetesObjectDeletion>, AppError> {
    let DeprovisionTarget {
        namespace: ns,
        application: app_name,
        plugin: plugin_name,
        resource_id,
    } = *resource;
    let _ = state.deployments_api(ns)?; // namespace allowlist check

    let app_id = format!("{ns}/{app_name}");

    let row = application_plugin_resources::Entity::find()
        .filter(application_plugin_resources::Column::ApplicationId.eq(&app_id))
        .filter(application_plugin_resources::Column::PluginName.eq(plugin_name))
        .filter(application_plugin_resources::Column::ResourceId.eq(resource_id))
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
        application_name: app_name.to_string(),
        namespace: ns.to_string(),
        resource_id: resource_id.to_string(),
        state: state_map,
        fields: fields_map,
    };

    let requested: Vec<KubernetesObjectRef> = {
        let plugins = state.plugins.read().await;
        match plugins.iter().find(|p| p.name == plugin_name) {
            Some(plugin) => match crate::plugins::run_deprovision(plugin, &dep_req) {
                Ok(result) => {
                    if !result.message.is_empty() {
                        tracing::info!(
                            plugin = %plugin_name,
                            resource_id = %resource_id,
                            "deprovision: {}", result.message
                        );
                    }
                    result.delete_kubernetes_resources
                }
                Err(e) => {
                    tracing::warn!(
                        plugin = %plugin_name,
                        resource_id = %resource_id,
                        error = %e,
                        "deprovision() call failed — removing DB record anyway"
                    );
                    Vec::new()
                }
            },
            None => Vec::new(),
        }
    };

    finish_deprovision(state, resource, row, &requested).await
}

/// Delete the objects the plugin asked for, then the resource record.
/// Problems deleting objects are logged and audited, and never stop the
/// record from being removed.
async fn finish_deprovision(
    state: &AppState,
    resource: &DeprovisionTarget<'_>,
    row: application_plugin_resources::Model,
    requested: &[KubernetesObjectRef],
) -> Result<Vec<KubernetesObjectDeletion>, AppError> {
    let deletions = delete_requested_objects(state, resource, requested).await;

    tracing::info!(
        namespace = %resource.namespace,
        application = %resource.application,
        plugin = %resource.plugin,
        resource_id = %resource.resource_id,
        "plugin resource deprovisioned — DB record removed"
    );

    application_plugin_resources::Entity::delete_by_id(row.id)
        .exec(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to delete resource: {e}")))?;

    Ok(deletions)
}

// ── Deleting Kubernetes objects on a plugin's behalf ─────────────────────────

/// `(apiVersion, kind)` pairs a plugin's `deprovision()` may ask deckwatch to
/// delete, in the order they're deleted: Jobs first, because their pods may
/// use a ServiceAccount that's also being deleted.
const DELETABLE_KINDS: &[(&str, &str)] = &[("batch/v1", "Job"), ("v1", "ServiceAccount")];

/// What happened to one object a plugin asked deckwatch to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionOutcome {
    Deleted,
    /// It was already gone, which counts as deleted.
    NotFound,
    /// Not attempted: a kind deckwatch doesn't delete, an invalid name, or a
    /// ServiceAccount that workloads still use.
    Refused,
    /// The Kubernetes API call failed.
    Failed,
}

/// One object a plugin asked deckwatch to delete, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KubernetesObjectDeletion {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: String,
    pub outcome: DeletionOutcome,
    /// Why it was refused or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Position in [`DELETABLE_KINDS`], or `None` for a kind deckwatch never
/// deletes for a plugin.
fn deletable_kind(api_version: &str, kind: &str) -> Option<usize> {
    DELETABLE_KINDS
        .iter()
        .position(|&(v, k)| v == api_version && k == kind)
}

/// RFC 1123 label, the format of a namespace name.
fn is_dns1123_label(s: &str) -> bool {
    s.len() <= 63 && is_dns1123_part(s)
}

/// RFC 1123 subdomain, the format of ServiceAccount and Job names. Rejecting
/// anything else also keeps a name from adding segments to the API path.
fn is_dns1123_subdomain(s: &str) -> bool {
    s.len() <= 253 && s.split('.').all(is_dns1123_part)
}

fn is_dns1123_part(s: &str) -> bool {
    let bytes = s.as_bytes();
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    match (bytes.first(), bytes.last()) {
        (Some(first), Some(last)) => {
            alnum(first) && alnum(last) && bytes.iter().all(|b| alnum(b) || *b == b'-')
        }
        _ => false,
    }
}

/// Workloads whose pods run as the ServiceAccount `name`, as `kind/name`.
/// Deleting a ServiceAccount that a pod template still names stops the
/// controller from creating pods, so such a ServiceAccount is kept. Objects
/// that are already being deleted don't count.
fn service_account_users(
    name: &str,
    deployments: &[Deployment],
    cronjobs: &[CronJob],
    jobs: &[Job],
) -> Vec<String> {
    fn uses(meta: &ObjectMeta, pod: Option<&PodSpec>, name: &str) -> bool {
        meta.deletion_timestamp.is_none()
            && pod.is_some_and(|p| {
                p.service_account_name
                    .as_deref()
                    .or(p.service_account.as_deref())
                    .unwrap_or("default")
                    == name
            })
    }
    let mut users = Vec::new();
    for d in deployments {
        let pod = d.spec.as_ref().and_then(|s| s.template.spec.as_ref());
        if uses(&d.metadata, pod, name) {
            users.push(format!(
                "deployment/{}",
                d.metadata.name.as_deref().unwrap_or("")
            ));
        }
    }
    for c in cronjobs {
        let pod = c
            .spec
            .as_ref()
            .and_then(|s| s.job_template.spec.as_ref())
            .and_then(|s| s.template.spec.as_ref());
        if uses(&c.metadata, pod, name) {
            users.push(format!(
                "cronjob/{}",
                c.metadata.name.as_deref().unwrap_or("")
            ));
        }
    }
    for j in jobs {
        let pod = j.spec.as_ref().and_then(|s| s.template.spec.as_ref());
        if uses(&j.metadata, pod, name) {
            users.push(format!("job/{}", j.metadata.name.as_deref().unwrap_or("")));
        }
    }
    users
}

/// Delete the Kubernetes objects a plugin's `deprovision()` asked for.
///
/// Objects are only ever deleted in the application's namespace: a
/// [`KubernetesObjectRef`] has no namespace field. Only the kinds in
/// [`DELETABLE_KINDS`] are deleted, with background propagation so a Job's
/// pods go too, Jobs before ServiceAccounts. A ServiceAccount that a
/// Deployment, CronJob or Job in the namespace still uses is kept, and so is
/// the namespace's `default` ServiceAccount. An object that's already gone
/// counts as deleted.
///
/// Every request is audited with its outcome; refusals and failures are
/// also logged as warnings and counted in `deckwatch_plugin_errors_total`.
/// Nothing here returns an error, so the caller always goes on to remove the
/// resource record.
async fn delete_requested_objects(
    state: &AppState,
    resource: &DeprovisionTarget<'_>,
    requested: &[KubernetesObjectRef],
) -> Vec<KubernetesObjectDeletion> {
    let mut ordered: Vec<&KubernetesObjectRef> = requested.iter().collect();
    ordered.sort_by_key(|obj| deletable_kind(&obj.api_version, &obj.kind).unwrap_or(usize::MAX));

    let mut deletions = Vec::with_capacity(ordered.len());
    for obj in ordered {
        let (outcome, reason) = delete_one(state, resource.namespace, obj).await;
        let deletion = KubernetesObjectDeletion {
            api_version: obj.api_version.clone(),
            kind: obj.kind.clone(),
            name: obj.name.clone(),
            namespace: resource.namespace.to_string(),
            outcome,
            reason,
        };
        record_deletion(state, resource, &deletion).await;
        deletions.push(deletion);
    }
    deletions
}

async fn delete_one(
    state: &AppState,
    namespace: &str,
    obj: &KubernetesObjectRef,
) -> (DeletionOutcome, Option<String>) {
    let refuse = |reason: String| (DeletionOutcome::Refused, Some(reason));
    let fail = |reason: String| (DeletionOutcome::Failed, Some(reason));

    if !is_dns1123_label(namespace) {
        return refuse(format!("invalid namespace {namespace:?}"));
    }
    let Some(rank) = deletable_kind(&obj.api_version, &obj.kind) else {
        return refuse(format!(
            "deckwatch doesn't delete {}/{} for plugins (allowed: v1/ServiceAccount, batch/v1/Job)",
            obj.api_version, obj.kind
        ));
    };
    if !is_dns1123_subdomain(&obj.name) {
        return refuse(format!("invalid object name {:?}", obj.name));
    }

    let dp = DeleteParams::background();
    let result = match DELETABLE_KINDS[rank] {
        (_, "Job") => match state.jobs_api(namespace) {
            Ok(api) => api.delete(&obj.name, &dp).await.map(|_| ()),
            Err(e) => return refuse(e.to_string()),
        },
        (_, "ServiceAccount") => {
            if obj.name == "default" {
                return refuse("the namespace's default ServiceAccount is never deleted".into());
            }
            match service_account_users_in(state, namespace, &obj.name).await {
                Ok(users) if !users.is_empty() => {
                    return refuse(format!(
                        "still used by {}; deleting it would stop new pods from being created",
                        users.join(", ")
                    ));
                }
                Ok(_) => {}
                Err(e) => return fail(format!("could not check whether it is in use: {e}")),
            }
            match state.serviceaccounts_api(namespace) {
                Ok(api) => api.delete(&obj.name, &dp).await.map(|_| ()),
                Err(e) => return refuse(e.to_string()),
            }
        }
        (api_version, kind) => {
            return refuse(format!("no delete support for {api_version}/{kind}"));
        }
    };

    match result {
        Ok(()) => (DeletionOutcome::Deleted, None),
        Err(kube::Error::Api(e)) if e.code == 404 => (DeletionOutcome::NotFound, None),
        Err(e) => fail(e.to_string()),
    }
}

/// [`service_account_users`] for the live workloads in `namespace`.
async fn service_account_users_in(
    state: &AppState,
    namespace: &str,
    name: &str,
) -> Result<Vec<String>, AppError> {
    let lp = ListParams::default();
    let deployments = state.deployments_api(namespace)?.list(&lp).await?.items;
    let cronjobs = state.cronjobs_api(namespace)?.list(&lp).await?.items;
    let jobs = state.jobs_api(namespace)?.list(&lp).await?.items;
    Ok(service_account_users(name, &deployments, &cronjobs, &jobs))
}

/// Log, count and audit one deletion outcome.
async fn record_deletion(
    state: &AppState,
    resource: &DeprovisionTarget<'_>,
    deletion: &KubernetesObjectDeletion,
) {
    let summary = match (deletion.outcome, deletion.reason.as_deref()) {
        (DeletionOutcome::Deleted, _) => "deleted".to_string(),
        (DeletionOutcome::NotFound, _) => "already deleted".to_string(),
        (DeletionOutcome::Refused, reason) => format!("refused: {}", reason.unwrap_or("")),
        (DeletionOutcome::Failed, reason) => format!("failed: {}", reason.unwrap_or("")),
    };
    let detail = format!(
        "{summary} (requested by plugin '{}' deprovisioning resource '{}' of application '{}')",
        resource.plugin, resource.resource_id, resource.application
    );

    match deletion.outcome {
        DeletionOutcome::Deleted | DeletionOutcome::NotFound => tracing::info!(
            namespace = %deletion.namespace,
            kind = %deletion.kind,
            name = %deletion.name,
            plugin = %resource.plugin,
            "deprovision: {summary}"
        ),
        DeletionOutcome::Refused | DeletionOutcome::Failed => {
            tracing::warn!(
                namespace = %deletion.namespace,
                api_version = %deletion.api_version,
                kind = %deletion.kind,
                name = %deletion.name,
                plugin = %resource.plugin,
                resource_id = %resource.resource_id,
                "deprovision: object the plugin asked to delete was not deleted: {summary}"
            );
            let error_type = if deletion.outcome == DeletionOutcome::Refused {
                "k8s_delete_refused"
            } else {
                "k8s_delete_failed"
            };
            metrics::record_plugin_error(resource.plugin, error_type);
        }
    }

    let resource_type = match deletion.kind.as_str() {
        "" => "unknown".to_string(),
        kind => kind.to_ascii_lowercase(),
    };
    let name: String = deletion.name.chars().take(253).collect();
    if let Err(e) = audit::log_action(
        &state.db,
        "delete",
        &resource_type,
        &name,
        &deletion.namespace,
        &detail,
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit entry for plugin-requested delete");
    }
}

#[cfg(test)]
#[path = "application_resources_tests.rs"]
mod tests;
