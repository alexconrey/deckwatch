use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, ContainerPort, EnvVar, HTTPGetAction, PodSpec, PodTemplateSpec, Probe,
    ResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::kube_ext::{
    compute_application_health, cronjob_summary, deployment_summary, ApplicationDetail,
    ApplicationGitConfig, ApplicationSummary, CronJobSummary, DeploymentSummary,
};
use crate::metrics::K8sTimer;
use crate::state::AppState;
use crate::watcher::ann;

const APP_DATA_KEY: &str = "application";
const APP_LABEL: &str = "deckwatch.io/application";
const COMPONENT_LABEL: &str = "app.kubernetes.io/component";
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const APP_COMPONENT: &str = "application";
const MANAGED_BY: &str = "deckwatch";
const APP_SELECTOR: &str =
    "app.kubernetes.io/component=application,app.kubernetes.io/managed-by=deckwatch";

/// Sentinel image used when we seed a Deployment for a GitOps-backed app
/// before the first kaniko build has completed. It intentionally does not
/// exist in any registry: the resulting ImagePullBackOff is the signal to
/// the operator that a build is still pending. Once `watcher::monitor_builds`
/// promotes a successful build, the image is patched to the real
/// `<oci-repo>:<sha>` tag.
pub const GITOPS_PLACEHOLDER_IMAGE: &str = "deckwatch-placeholder:awaiting-build";

/// JSON persisted inside the ConfigMap under `data.application`.
#[derive(Serialize, Deserialize, Clone)]
struct ApplicationData {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git: Option<ApplicationGitConfig>,
}

fn cm_name(app: &str) -> String {
    format!("deckwatch-app-{app}")
}

fn app_labels(app_name: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert(MANAGED_BY_LABEL.to_string(), MANAGED_BY.to_string());
    labels.insert(COMPONENT_LABEL.to_string(), APP_COMPONENT.to_string());
    labels.insert(APP_LABEL.to_string(), app_name.to_string());
    labels
}

fn parse_app_data(cm: &ConfigMap) -> Option<ApplicationData> {
    cm.data
        .as_ref()
        .and_then(|d| d.get(APP_DATA_KEY))
        .and_then(|s| serde_json::from_str::<ApplicationData>(s).ok())
}

fn member_selector(app_name: &str) -> String {
    format!("{APP_LABEL}={app_name}")
}

// ============================================================
// Requests / responses
// ============================================================

#[derive(Serialize)]
pub struct ApplicationListResponse {
    pub applications: Vec<ApplicationSummary>,
}

#[derive(Deserialize)]
pub struct ApplicationRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub git: Option<ApplicationGitConfig>,
    #[serde(default)]
    pub create_deployment: Option<bool>,
    #[serde(default)]
    pub template_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ApplicationUpdateRequest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub git: Option<Option<ApplicationGitConfig>>,
}

fn deserialize_optional_field<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub kind: String,
    pub resource_name: String,
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    #[serde(default)]
    pub cascade: Option<bool>,
}

// ============================================================
// Handlers
// ============================================================

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<ApplicationListResponse>, AppError> {
    let cm_api = state.configmaps_api(&ns)?;
    let dep_api = state.deployments_api(&ns)?;
    let cj_api = state.cronjobs_api(&ns)?;

    let lp = ListParams::default().labels(APP_SELECTOR);
    let t = K8sTimer::new("configmaps", "list");
    let cms = cm_api.list(&lp).await;
    t.finish(cms.is_ok());
    let cms = cms?;

    let mut applications = Vec::with_capacity(cms.items.len());
    for cm in cms.iter() {
        let data = match parse_app_data(cm) {
            Some(d) => d,
            None => continue,
        };

        let member_lp = ListParams::default().labels(&member_selector(&data.name));
        let t = K8sTimer::new("deployments", "list");
        let deps = dep_api.list(&member_lp).await;
        t.finish(deps.is_ok());
        let deps = deps?;
        let t = K8sTimer::new("cronjobs", "list");
        let cjs = cj_api.list(&member_lp).await;
        t.finish(cjs.is_ok());
        let cjs = cjs?;

        let dep_summaries: Vec<DeploymentSummary> = deps.iter().map(deployment_summary).collect();
        let gitops_enabled = data.git.is_some()
            || deps.iter().any(|d| {
                d.metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get(&ann("git-enabled")))
                    .map(|v| v == "true")
                    .unwrap_or(false)
            });
        let health = compute_application_health(&dep_summaries);

        applications.push(ApplicationSummary {
            name: data.name,
            namespace: ns.clone(),
            description: data.description,
            created_at: data.created_at.or_else(|| {
                cm.metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.to_string())
            }),
            deployment_count: deps.items.len(),
            cronjob_count: cjs.items.len(),
            health,
            gitops_enabled,
        });
    }

    Ok(Json(ApplicationListResponse { applications }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<ApplicationDetail>, AppError> {
    let cm_api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let cm = cm_api.get(&cm_name(&name)).await;
    t.finish(cm.is_ok());
    let cm = cm?;
    let data = parse_app_data(&cm).ok_or_else(|| {
        AppError::BadRequest(format!("configmap for application '{name}' is malformed"))
    })?;

    let dep_api = state.deployments_api(&ns)?;
    let cj_api = state.cronjobs_api(&ns)?;
    let lp = ListParams::default().labels(&member_selector(&name));
    let t = K8sTimer::new("deployments", "list");
    let deps = dep_api.list(&lp).await;
    t.finish(deps.is_ok());
    let deps = deps?;
    let t = K8sTimer::new("cronjobs", "list");
    let cjs = cj_api.list(&lp).await;
    t.finish(cjs.is_ok());
    let cjs = cjs?;

    let deployments: Vec<DeploymentSummary> = deps.iter().map(deployment_summary).collect();
    let cronjobs: Vec<CronJobSummary> = cjs.iter().map(cronjob_summary).collect();
    let health = compute_application_health(&deployments);

    let created_at = data.created_at.clone().or_else(|| {
        cm.metadata
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string())
    });

    Ok(Json(ApplicationDetail {
        name: data.name,
        namespace: ns,
        description: data.description,
        created_at,
        updated_at: data.updated_at,
        git: data.git,
        deployments,
        cronjobs,
        health,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<ApplicationRequest>,
) -> Result<(StatusCode, Json<ApplicationDetail>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    // Names propagate into resource labels and a ConfigMap name; validate up front.
    if !req
        .name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || req.name.len() > 53
    {
        return Err(AppError::BadRequest(
            "name must be lowercase alphanumeric or '-', <= 53 chars".to_string(),
        ));
    }

    let cm_api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let existing = cm_api.get_opt(&cm_name(&req.name)).await;
    t.finish(existing.is_ok());
    if existing?.is_some() {
        return Err(AppError::BadRequest(format!(
            "application '{}' already exists",
            req.name
        )));
    }

    let now = jiff::Timestamp::now().to_string();
    let data = ApplicationData {
        name: req.name.clone(),
        description: req.description.unwrap_or_default(),
        created_at: Some(now.clone()),
        updated_at: Some(now.clone()),
        git: req.git.clone(),
    };
    let serialized = serde_json::to_string(&data)
        .map_err(|e| AppError::BadRequest(format!("failed to serialize application: {e}")))?;

    let mut cm_data = BTreeMap::new();
    cm_data.insert(APP_DATA_KEY.to_string(), serialized);

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(cm_name(&req.name)),
            namespace: Some(ns.clone()),
            labels: Some(app_labels(&req.name)),
            ..Default::default()
        },
        data: Some(cm_data),
        ..Default::default()
    };
    let t = K8sTimer::new("configmaps", "create");
    let create_res = cm_api.create(&PostParams::default(), &cm).await;
    t.finish(create_res.is_ok());
    create_res?;

    // Optionally create a starter Deployment. Default template is web-app.
    let should_seed = req.create_deployment.unwrap_or(false) || req.git.is_some();
    if should_seed {
        let template_id = req.template_id.as_deref().unwrap_or("web-app");
        if let Err(e) =
            create_seed_deployment(&state, &ns, &req.name, template_id, req.git.as_ref()).await
        {
            // Roll back the ConfigMap so we don't leave an orphaned Application record.
            let t = K8sTimer::new("configmaps", "delete");
            let del = cm_api.delete(&cm_name(&req.name), &Default::default()).await;
            t.finish(del.is_ok());
            return Err(e);
        }
    }

    let detail = get(State(state), Path((ns, req.name))).await?;
    Ok((StatusCode::CREATED, detail))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<ApplicationUpdateRequest>,
) -> Result<Json<ApplicationDetail>, AppError> {
    let cm_api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let cm = cm_api.get(&cm_name(&name)).await;
    t.finish(cm.is_ok());
    let cm = cm?;
    let mut data = parse_app_data(&cm).ok_or_else(|| {
        AppError::BadRequest(format!("configmap for application '{name}' is malformed"))
    })?;

    if let Some(desc) = req.description {
        data.description = desc;
    }
    if let Some(git) = req.git {
        data.git = git;
    }
    data.updated_at = Some(jiff::Timestamp::now().to_string());

    let serialized = serde_json::to_string(&data)
        .map_err(|e| AppError::BadRequest(format!("failed to serialize application: {e}")))?;

    let patch = serde_json::json!({
        "data": { "application": serialized }
    });
    let t = K8sTimer::new("configmaps", "patch");
    let res = cm_api
        .patch(&cm_name(&name), &PatchParams::default(), &Patch::Merge(patch))
        .await;
    t.finish(res.is_ok());
    res?;

    get(State(state), Path((ns, name))).await
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, AppError> {
    let cm_api = state.configmaps_api(&ns)?;
    let dep_api = state.deployments_api(&ns)?;
    let cj_api = state.cronjobs_api(&ns)?;
    let selector = member_selector(&name);
    let lp = ListParams::default().labels(&selector);

    let cascade = query.cascade.unwrap_or(false);
    if cascade {
        let t = K8sTimer::new("deployments", "list");
        let dep_list = dep_api.list(&lp).await;
        t.finish(dep_list.is_ok());
        for dep in dep_list?.iter() {
            if let Some(dn) = dep.metadata.name.as_deref() {
                // Best-effort delete; K8sTimer's Drop guard records the outcome
                // (ok/err) for any that panic or short-circuit.
                let t = K8sTimer::new("deployments", "delete");
                let res = dep_api.delete(dn, &Default::default()).await;
                t.finish(res.is_ok());
            }
        }
        let t = K8sTimer::new("cronjobs", "list");
        let cj_list = cj_api.list(&lp).await;
        t.finish(cj_list.is_ok());
        for cj in cj_list?.iter() {
            if let Some(cn) = cj.metadata.name.as_deref() {
                let t = K8sTimer::new("cronjobs", "delete");
                let res = cj_api.delete(cn, &Default::default()).await;
                t.finish(res.is_ok());
            }
        }
    } else {
        // Detach members by removing the application label.
        let null_label: BTreeMap<String, Option<String>> =
            [(APP_LABEL.to_string(), None)].into_iter().collect();
        let patch = serde_json::json!({
            "metadata": { "labels": null_label }
        });
        let t = K8sTimer::new("deployments", "list");
        let dep_list = dep_api.list(&lp).await;
        t.finish(dep_list.is_ok());
        for dep in dep_list?.iter() {
            if let Some(dn) = dep.metadata.name.as_deref() {
                let t = K8sTimer::new("deployments", "patch");
                let res = dep_api
                    .patch(dn, &PatchParams::default(), &Patch::Merge(&patch))
                    .await;
                t.finish(res.is_ok());
            }
        }
        let t = K8sTimer::new("cronjobs", "list");
        let cj_list = cj_api.list(&lp).await;
        t.finish(cj_list.is_ok());
        for cj in cj_list?.iter() {
            if let Some(cn) = cj.metadata.name.as_deref() {
                let t = K8sTimer::new("cronjobs", "patch");
                let res = cj_api
                    .patch(cn, &PatchParams::default(), &Patch::Merge(&patch))
                    .await;
                t.finish(res.is_ok());
            }
        }
    }

    let t = K8sTimer::new("configmaps", "delete");
    let res = cm_api.delete(&cm_name(&name), &Default::default()).await;
    t.finish(res.is_ok());
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_member(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<AddMemberRequest>,
) -> Result<StatusCode, AppError> {
    // Sanity check: the application must exist.
    let cm_api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let cm = cm_api.get(&cm_name(&name)).await;
    t.finish(cm.is_ok());
    cm?;

    let patch = serde_json::json!({
        "metadata": { "labels": { "deckwatch.io/application": name } }
    });
    let params = PatchParams::default();

    match req.kind.as_str() {
        "Deployment" => {
            let api = state.deployments_api(&ns)?;
            let t = K8sTimer::new("deployments", "patch");
            let res = api.patch(&req.resource_name, &params, &Patch::Merge(&patch)).await;
            t.finish(res.is_ok());
            res?;
        }
        "CronJob" => {
            let api = state.cronjobs_api(&ns)?;
            let t = K8sTimer::new("cronjobs", "patch");
            let res = api.patch(&req.resource_name, &params, &Patch::Merge(&patch)).await;
            t.finish(res.is_ok());
            res?;
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported member kind '{other}' (expected Deployment or CronJob)"
            )));
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(
    State(state): State<AppState>,
    Path((ns, name, kind, resource_name)): Path<(String, String, String, String)>,
) -> Result<StatusCode, AppError> {
    // Ensure the application record exists; also gives us a friendlier 404.
    let cm_api = state.configmaps_api(&ns)?;
    let t = K8sTimer::new("configmaps", "get");
    let cm = cm_api.get(&cm_name(&name)).await;
    t.finish(cm.is_ok());
    cm?;

    let null_label: BTreeMap<String, Option<String>> =
        [(APP_LABEL.to_string(), None)].into_iter().collect();
    let patch = serde_json::json!({
        "metadata": { "labels": null_label }
    });
    let params = PatchParams::default();

    match kind.as_str() {
        "Deployment" => {
            let api = state.deployments_api(&ns)?;
            let t = K8sTimer::new("deployments", "patch");
            let res = api.patch(&resource_name, &params, &Patch::Merge(&patch)).await;
            t.finish(res.is_ok());
            res?;
        }
        "CronJob" => {
            let api = state.cronjobs_api(&ns)?;
            let t = K8sTimer::new("cronjobs", "patch");
            let res = api.patch(&resource_name, &params, &Patch::Merge(&patch)).await;
            t.finish(res.is_ok());
            res?;
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported member kind '{other}' (expected Deployment or CronJob)"
            )));
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================
// Seed deployment (template default: web-app)
// ============================================================

struct SeedTemplate {
    image: String,
    replicas: i32,
    port: Option<i32>,
    cpu_request: Option<String>,
    memory_request: Option<String>,
    cpu_limit: Option<String>,
    memory_limit: Option<String>,
    readiness_path: Option<String>,
    command: Option<Vec<String>>,
    args: Option<Vec<String>>,
}

fn seed_template(id: &str) -> SeedTemplate {
    match id {
        "worker" => SeedTemplate {
            image: "".to_string(),
            replicas: 1,
            port: None,
            cpu_request: Some("200m".to_string()),
            memory_request: Some("256Mi".to_string()),
            cpu_limit: Some("1".to_string()),
            memory_limit: Some("512Mi".to_string()),
            readiness_path: None,
            command: None,
            args: None,
        },
        "cron-job" => SeedTemplate {
            image: "alpine:3".to_string(),
            replicas: 0,
            port: None,
            cpu_request: Some("100m".to_string()),
            memory_request: Some("128Mi".to_string()),
            cpu_limit: Some("500m".to_string()),
            memory_limit: Some("256Mi".to_string()),
            readiness_path: None,
            command: Some(vec!["/bin/sh".to_string(), "-c".to_string()]),
            args: Some(vec![
                "echo 'replace me with your cron command' && sleep 5".to_string()
            ]),
        },
        "static-site" => SeedTemplate {
            image: "nginx:1.27-alpine".to_string(),
            replicas: 1,
            port: Some(80),
            cpu_request: Some("50m".to_string()),
            memory_request: Some("64Mi".to_string()),
            cpu_limit: Some("200m".to_string()),
            memory_limit: Some("128Mi".to_string()),
            readiness_path: Some("/".to_string()),
            command: None,
            args: None,
        },
        // Default: "web-app"
        _ => SeedTemplate {
            image: "nginx:1.27-alpine".to_string(),
            replicas: 1,
            port: Some(80),
            cpu_request: Some("100m".to_string()),
            memory_request: Some("128Mi".to_string()),
            cpu_limit: Some("500m".to_string()),
            memory_limit: Some("256Mi".to_string()),
            readiness_path: Some("/".to_string()),
            command: None,
            args: None,
        },
    }
}

async fn create_seed_deployment(
    state: &AppState,
    ns: &str,
    app_name: &str,
    template_id: &str,
    git: Option<&ApplicationGitConfig>,
) -> Result<(), AppError> {
    let tmpl = seed_template(template_id);

    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), app_name.to_string());
    labels.insert(MANAGED_BY_LABEL.to_string(), MANAGED_BY.to_string());
    labels.insert(APP_LABEL.to_string(), app_name.to_string());

    let ports = tmpl.port.map(|p| {
        vec![ContainerPort {
            container_port: p,
            ..Default::default()
        }]
    });

    let readiness_probe = tmpl.readiness_path.as_ref().and_then(|path| {
        tmpl.port.map(|p| Probe {
            http_get: Some(HTTPGetAction {
                path: Some(path.clone()),
                port: IntOrString::Int(p),
                ..Default::default()
            }),
            initial_delay_seconds: Some(5),
            period_seconds: Some(10),
            ..Default::default()
        })
    });

    let resources = build_resources(
        tmpl.cpu_request.clone(),
        tmpl.memory_request.clone(),
        tmpl.cpu_limit.clone(),
        tmpl.memory_limit.clone(),
    );

    // With GitOps enabled, the template image (usually nginx) is misleading —
    // the deployment is meant to run the user's code once kaniko has built it.
    // Use an obviously-fake tag so operators immediately see "this is waiting
    // on a build" rather than a running nginx welcome page. The watcher patches
    // the real image in after the first successful build.
    let image = if git.is_some() {
        GITOPS_PLACEHOLDER_IMAGE.to_string()
    } else {
        tmpl.image
    };

    let container = Container {
        name: app_name.to_string(),
        image: Some(image),
        ports,
        command: tmpl.command,
        args: tmpl.args,
        resources,
        readiness_probe,
        env: None::<Vec<EnvVar>>,
        ..Default::default()
    };

    // GitOps annotations mirror the keys used by handlers/gitops.rs.
    let mut annotations: BTreeMap<String, String> = BTreeMap::new();
    if let Some(g) = git {
        annotations.insert(ann("git-enabled"), "true".to_string());
        annotations.insert(ann("git-repo"), g.repo_url.clone());
        annotations.insert(
            ann("git-branch"),
            g.branch.clone().unwrap_or_else(|| "main".to_string()),
        );
        if let Some(secret) = &g.token_secret {
            annotations.insert(ann("git-token-secret"), secret.clone());
        }
    }
    let annotations = if annotations.is_empty() {
        None
    } else {
        Some(annotations)
    };

    let deployment = Deployment {
        metadata: ObjectMeta {
            name: Some(app_name.to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels.clone()),
            annotations,
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(tmpl.replicas),
            selector: LabelSelector {
                match_labels: Some(BTreeMap::from([(
                    "app".to_string(),
                    app_name.to_string(),
                )])),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let dep_api = state.deployments_api(ns)?;
    let t = K8sTimer::new("deployments", "create");
    let res = dep_api.create(&PostParams::default(), &deployment).await;
    t.finish(res.is_ok());
    res?;
    Ok(())
}

fn build_resources(
    cpu_req: Option<String>,
    mem_req: Option<String>,
    cpu_lim: Option<String>,
    mem_lim: Option<String>,
) -> Option<ResourceRequirements> {
    let mut requests = BTreeMap::new();
    if let Some(v) = cpu_req {
        requests.insert("cpu".to_string(), Quantity(v));
    }
    if let Some(v) = mem_req {
        requests.insert("memory".to_string(), Quantity(v));
    }
    let mut limits = BTreeMap::new();
    if let Some(v) = cpu_lim {
        limits.insert("cpu".to_string(), Quantity(v));
    }
    if let Some(v) = mem_lim {
        limits.insert("memory".to_string(), Quantity(v));
    }
    if requests.is_empty() && limits.is_empty() {
        None
    } else {
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
}
