use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Container, ContainerPort, EnvVar, ResourceRequirements};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{ListParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::deployments::{DeploymentDetailResponse, EnvVarInput, ResourceSpec};
use crate::kube_ext::{
    deployment_detail, ingress_summary, pod_summary, IngressSummary, PodSummary,
};
use crate::metrics::K8sTimer;
use crate::state::AppState;

const ADDON_ANNOTATION_PREFIX: &str = "deckwatch.addon/";
// Annotation used to record which env-var names on the primary container were
// injected by a given addon, so detach() can precisely remove them without
// clobbering user-supplied env vars that happen to share a name.
const ADDON_INJECTED_ENV_ANNOTATION_PREFIX: &str = "deckwatch.addon-env/";

#[derive(Clone, Serialize)]
pub struct AddonDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub image: String,
    pub default_port: Option<i32>,
    pub default_env: Vec<EnvVarOutput>,
    pub default_resources: Option<ResourceSpecOutput>,
}

#[derive(Clone, Serialize)]
pub struct EnvVarOutput {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Serialize)]
pub struct ResourceSpecOutput {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Serialize)]
pub struct AddonListResponse {
    pub addons: Vec<AddonDefinition>,
}

#[derive(Deserialize, Default)]
pub struct AttachAddonRequest {
    #[serde(default)]
    pub container_name: Option<String>,
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub env: Option<Vec<EnvVarInput>>,
    #[serde(default)]
    pub resource_limits: Option<ResourceSpec>,
    #[serde(default)]
    pub resource_requests: Option<ResourceSpec>,
}

// PATCH body for editing an already-attached addon. All fields are optional;
// only supplied fields are applied. `env` replaces the sidecar's env list
// wholesale when present (matches attach() semantics) and re-runs primary
// container env injection so downstream env vars stay in sync.
#[derive(Deserialize, Default)]
pub struct UpdateAddonRequest {
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub env: Option<Vec<EnvVarInput>>,
    #[serde(default)]
    pub resource_limits: Option<ResourceSpec>,
    #[serde(default)]
    pub resource_requests: Option<ResourceSpec>,
}

fn catalog() -> Vec<AddonDefinition> {
    vec![
        AddonDefinition {
            id: "redis".to_string(),
            name: "Redis".to_string(),
            description: "In-memory key-value store, useful as a cache or ephemeral state sidecar."
                .to_string(),
            image: "redis:7-alpine".to_string(),
            default_port: Some(6379),
            default_env: vec![EnvVarOutput {
                name: "REDIS_URL".to_string(),
                value: "redis://localhost:6379".to_string(),
            }],
            default_resources: Some(ResourceSpecOutput {
                cpu: Some("100m".to_string()),
                memory: Some("128Mi".to_string()),
            }),
        },
        AddonDefinition {
            id: "memcached".to_string(),
            name: "Memcached".to_string(),
            description: "Distributed memory caching sidecar.".to_string(),
            image: "memcached:1.6-alpine".to_string(),
            default_port: Some(11211),
            default_env: vec![EnvVarOutput {
                name: "MEMCACHED_URL".to_string(),
                value: "memcached://localhost:11211".to_string(),
            }],
            default_resources: Some(ResourceSpecOutput {
                cpu: Some("100m".to_string()),
                memory: Some("128Mi".to_string()),
            }),
        },
        AddonDefinition {
            id: "nginx-proxy".to_string(),
            name: "Nginx Proxy".to_string(),
            description:
                "Nginx sidecar for TLS termination or caching in front of the primary container."
                    .to_string(),
            image: "nginx:1.27-alpine".to_string(),
            default_port: Some(8080),
            default_env: vec![EnvVarOutput {
                name: "PROXY_URL".to_string(),
                value: "http://localhost:8080".to_string(),
            }],
            default_resources: Some(ResourceSpecOutput {
                cpu: Some("50m".to_string()),
                memory: Some("64Mi".to_string()),
            }),
        },
        AddonDefinition {
            id: "fluent-bit".to_string(),
            name: "Fluent Bit".to_string(),
            description: "Lightweight logging sidecar that forwards container logs downstream."
                .to_string(),
            image: "fluent/fluent-bit:3.1".to_string(),
            default_port: None,
            default_env: vec![],
            default_resources: Some(ResourceSpecOutput {
                cpu: Some("50m".to_string()),
                memory: Some("64Mi".to_string()),
            }),
        },
        AddonDefinition {
            id: "otel-collector".to_string(),
            name: "OpenTelemetry Collector".to_string(),
            description:
                "Sidecar that collects and forwards traces/metrics to the cluster tracing backend."
                    .to_string(),
            image: "otel/opentelemetry-collector:latest".to_string(),
            default_port: Some(4317),
            // {deployment_name} is substituted per-attach against the target
            // Deployment's metadata.name; see interpolate() below. Keeps the
            // service name in the tracing backend aligned with the workload
            // name without requiring the user to type it in.
            default_env: vec![
                EnvVarOutput {
                    name: "OTEL_EXPORTER_OTLP_ENDPOINT".to_string(),
                    value: "http://localhost:4317".to_string(),
                },
                EnvVarOutput {
                    name: "OTEL_SERVICE_NAME".to_string(),
                    value: "{deployment_name}".to_string(),
                },
                EnvVarOutput {
                    name: "OTEL_TRACES_EXPORTER".to_string(),
                    value: "otlp".to_string(),
                },
            ],
            default_resources: Some(ResourceSpecOutput {
                cpu: Some("100m".to_string()),
                memory: Some("128Mi".to_string()),
            }),
        },
    ]
}

pub async fn list() -> Json<AddonListResponse> {
    Json(AddonListResponse { addons: catalog() })
}

// Substitution context resolved from the target Deployment at attach() time.
// New tokens can be added here without touching call sites — see interpolate().
struct InterpolationCtx<'a> {
    deployment_name: &'a str,
    namespace: &'a str,
}

// Replace `{deployment_name}` / `{namespace}` placeholders in a default env
// value. Used for the otel-collector addon so `OTEL_SERVICE_NAME` picks up
// the target Deployment's name automatically. Non-placeholder values pass
// through unchanged.
fn interpolate(v: &str, ctx: &InterpolationCtx) -> String {
    v.replace("{deployment_name}", ctx.deployment_name)
        .replace("{namespace}", ctx.namespace)
}

fn resolve_default_env(def_env: &[EnvVarOutput], ctx: &InterpolationCtx) -> Vec<EnvVarOutput> {
    def_env
        .iter()
        .map(|v| EnvVarOutput {
            name: v.name.clone(),
            value: interpolate(&v.value, ctx),
        })
        .collect()
}

pub async fn attach(
    State(state): State<AppState>,
    Path((ns, name, addon_id)): Path<(String, String, String)>,
    req: Option<Json<AttachAddonRequest>>,
) -> Result<(StatusCode, Json<DeploymentDetailResponse>), AppError> {
    let overrides = req.map(|r| r.0).unwrap_or_default();
    let def = catalog()
        .into_iter()
        .find(|a| a.id == addon_id)
        .ok_or_else(|| AppError::NotFound(format!("addon '{addon_id}' not found")))?;

    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut dep = existing.clone();
    let container_name = overrides
        .container_name
        .clone()
        .unwrap_or_else(|| format!("addon-{}", def.id));

    let annotation_key = format!("{ADDON_ANNOTATION_PREFIX}{container_name}");
    let injected_env_key = format!("{ADDON_INJECTED_ENV_ANNOTATION_PREFIX}{container_name}");

    // Duplicate-attach check reads from deployment-level annotations, which
    // is the source of truth surfaced by DeploymentDetail. The pod template
    // mirror is written for anyone inspecting the Pod directly.
    if dep
        .metadata
        .annotations
        .as_ref()
        .map(|a| a.contains_key(&annotation_key))
        .unwrap_or(false)
    {
        return Err(AppError::BadRequest(format!(
            "addon '{addon_id}' already attached as '{container_name}'"
        )));
    }

    let template = dep
        .spec
        .as_mut()
        .map(|s| &mut s.template)
        .ok_or_else(|| AppError::BadRequest("deployment has no pod template".to_string()))?;

    let template_annotations = template
        .metadata
        .get_or_insert_with(Default::default)
        .annotations
        .get_or_insert_with(BTreeMap::new);
    template_annotations.insert(annotation_key.clone(), def.id.clone());

    let pod_spec = template
        .spec
        .as_mut()
        .ok_or_else(|| AppError::BadRequest("deployment has no pod spec".to_string()))?;
    if pod_spec.containers.iter().any(|c| c.name == container_name) {
        return Err(AppError::BadRequest(format!(
            "container '{container_name}' already exists"
        )));
    }

    let ctx = InterpolationCtx {
        deployment_name: &name,
        namespace: &ns,
    };
    let resolved_default_env = resolve_default_env(&def.default_env, &ctx);

    let port = overrides.port.or(def.default_port);
    let ports = port.map(|p| {
        vec![ContainerPort {
            container_port: p,
            ..Default::default()
        }]
    });
    let env: Vec<EnvVar> = overrides
        .env
        .map(|vars| {
            vars.into_iter()
                .map(|v| EnvVar {
                    name: v.name,
                    value: Some(v.value),
                    ..Default::default()
                })
                .collect()
        })
        .unwrap_or_else(|| {
            resolved_default_env
                .iter()
                .map(|v| EnvVar {
                    name: v.name.clone(),
                    value: Some(v.value.clone()),
                    ..Default::default()
                })
                .collect()
        });

    let resources = build_resources_from_overrides(
        overrides.resource_requests,
        overrides.resource_limits,
        def.default_resources.as_ref(),
    );

    pod_spec.containers.push(Container {
        name: container_name.clone(),
        image: Some(def.image.clone()),
        ports,
        env: if env.is_empty() { None } else { Some(env) },
        resources,
        ..Default::default()
    });

    // Inject the addon's default env vars into the primary (first) container
    // so the app can discover the sidecar (e.g. REDIS_URL=redis://localhost:6379).
    // Record injected names so detach() can remove exactly those env vars
    // later without clobbering user-supplied ones.
    let injected_names = inject_addon_env_into_primary(pod_spec, &resolved_default_env);
    if !injected_names.is_empty() {
        let template_annotations = template
            .metadata
            .as_mut()
            .and_then(|m| m.annotations.as_mut())
            .expect("annotations map inserted above");
        template_annotations.insert(injected_env_key.clone(), injected_names.join(","));
    }

    // Mirror the addon annotations at the deployment metadata level so
    // DeploymentDetail (which surfaces deployment-level meta.annotations)
    // reflects attached addons for the frontend's AddonsCard.
    {
        let dep_annotations = dep.metadata.annotations.get_or_insert_with(BTreeMap::new);
        dep_annotations.insert(annotation_key, def.id.clone());
        if !injected_names.is_empty() {
            dep_annotations.insert(injected_env_key, injected_names.join(","));
        }
    }

    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok((
        StatusCode::CREATED,
        Json(DeploymentDetailResponse {
            detail,
            pods,
            ingresses,
        }),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name, addon_id)): Path<(String, String, String)>,
    req: Option<Json<UpdateAddonRequest>>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let overrides = req.map(|r| r.0).unwrap_or_default();
    let def = catalog()
        .into_iter()
        .find(|a| a.id == addon_id)
        .ok_or_else(|| AppError::NotFound(format!("addon '{addon_id}' not found")))?;

    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut dep = existing.clone();
    let template = dep
        .spec
        .as_mut()
        .map(|s| &mut s.template)
        .ok_or_else(|| AppError::BadRequest("deployment has no pod template".to_string()))?;

    // Find the addon's container by its annotation. addon_id -> container_name
    // mapping lives in `deckwatch.addon/<container_name>: <addon_id>`.
    let annotations = template
        .metadata
        .as_mut()
        .and_then(|m| m.annotations.as_mut())
        .ok_or_else(|| AppError::NotFound(format!("addon '{addon_id}' is not attached")))?;
    let container_name = annotations
        .iter()
        .find(|(k, v)| k.starts_with(ADDON_ANNOTATION_PREFIX) && v.as_str() == addon_id.as_str())
        .map(|(k, _)| k.trim_start_matches(ADDON_ANNOTATION_PREFIX).to_string())
        .ok_or_else(|| AppError::NotFound(format!("addon '{addon_id}' is not attached")))?;

    // Track previously-injected env-var names before we mutate the annotation
    // so we can un-inject the old set from the primary before applying the new.
    let injected_key = format!("{ADDON_INJECTED_ENV_ANNOTATION_PREFIX}{container_name}");
    let previously_injected: Vec<String> = annotations
        .get(&injected_key)
        .map(|v| {
            v.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    let pod_spec = template
        .spec
        .as_mut()
        .ok_or_else(|| AppError::BadRequest("deployment has no pod spec".to_string()))?;
    let sidecar = pod_spec
        .containers
        .iter_mut()
        .find(|c| c.name == container_name)
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "addon container '{container_name}' not found in pod spec"
            ))
        })?;

    if let Some(port) = overrides.port {
        sidecar.ports = Some(vec![ContainerPort {
            container_port: port,
            ..Default::default()
        }]);
    }

    let env_changed = overrides.env.is_some();
    let new_env_for_injection: Vec<EnvVarOutput> = if let Some(vars) = overrides.env {
        let out: Vec<EnvVarOutput> = vars
            .iter()
            .map(|v| EnvVarOutput {
                name: v.name.clone(),
                value: v.value.clone(),
            })
            .collect();
        let env: Vec<EnvVar> = vars
            .into_iter()
            .map(|v| EnvVar {
                name: v.name,
                value: Some(v.value),
                ..Default::default()
            })
            .collect();
        sidecar.env = if env.is_empty() { None } else { Some(env) };
        out
    } else {
        Vec::new()
    };

    if overrides.resource_requests.is_some() || overrides.resource_limits.is_some() {
        sidecar.resources = build_resources_from_overrides(
            overrides.resource_requests,
            overrides.resource_limits,
            def.default_resources.as_ref(),
        );
    }

    // If env changed, re-run the injection cycle: remove the old injected
    // names from the primary and inject the new set, updating the annotation
    // so a future detach removes exactly the right names.
    let injected_names_after: Option<Vec<String>> = if env_changed {
        remove_injected_env_from_primary(pod_spec, &previously_injected);
        let injected_names = inject_addon_env_into_primary(pod_spec, &new_env_for_injection);
        let template_annotations = template
            .metadata
            .as_mut()
            .and_then(|m| m.annotations.as_mut())
            .expect("annotations already located above");
        if injected_names.is_empty() {
            template_annotations.remove(&injected_key);
        } else {
            template_annotations.insert(injected_key.clone(), injected_names.join(","));
        }
        Some(injected_names)
    } else {
        None
    };

    // Mirror the updated injected-env annotation at the deployment level so
    // the DeploymentDetail response reflects the current addon env exposure.
    if let Some(injected_names) = injected_names_after {
        let dep_annotations = dep.metadata.annotations.get_or_insert_with(BTreeMap::new);
        if injected_names.is_empty() {
            dep_annotations.remove(&injected_key);
        } else {
            dep_annotations.insert(injected_key, injected_names.join(","));
        }
    }

    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

pub async fn detach(
    State(state): State<AppState>,
    Path((ns, name, addon_id)): Path<(String, String, String)>,
) -> Result<Json<DeploymentDetailResponse>, AppError> {
    let api = state.deployments_api(&ns)?;
    let t = K8sTimer::new("deployments", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut dep = existing.clone();
    let template = dep
        .spec
        .as_mut()
        .map(|s| &mut s.template)
        .ok_or_else(|| AppError::BadRequest("deployment has no pod template".to_string()))?;
    let annotations = template
        .metadata
        .as_mut()
        .and_then(|m| m.annotations.as_mut());
    let (container_name, injected_env_names) = match annotations {
        Some(a) => {
            let key = a
                .iter()
                .find(|(k, v)| {
                    k.starts_with(ADDON_ANNOTATION_PREFIX) && v.as_str() == addon_id.as_str()
                })
                .map(|(k, _)| k.clone());
            match key {
                Some(k) => {
                    a.remove(&k);
                    let container_name = k.trim_start_matches(ADDON_ANNOTATION_PREFIX).to_string();
                    let injected_key =
                        format!("{ADDON_INJECTED_ENV_ANNOTATION_PREFIX}{container_name}");
                    let injected: Vec<String> = a
                        .remove(&injected_key)
                        .map(|v| {
                            v.split(',')
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    (container_name, injected)
                }
                None => {
                    return Err(AppError::NotFound(format!(
                        "addon '{addon_id}' is not attached"
                    )))
                }
            }
        }
        None => {
            return Err(AppError::NotFound(format!(
                "addon '{addon_id}' is not attached"
            )))
        }
    };
    let pod_spec = template
        .spec
        .as_mut()
        .ok_or_else(|| AppError::BadRequest("deployment has no pod spec".to_string()))?;
    if pod_spec.containers.is_empty() {
        return Err(AppError::BadRequest("no containers to remove".to_string()));
    }
    if pod_spec.containers[0].name == container_name {
        return Err(AppError::BadRequest(
            "addon container is the primary container; refusing to remove".to_string(),
        ));
    }
    pod_spec.containers.retain(|c| c.name != container_name);
    remove_injected_env_from_primary(pod_spec, &injected_env_names);

    // Mirror the removal at the deployment metadata level so DeploymentDetail
    // no longer reports the addon as attached.
    if let Some(dep_annotations) = dep.metadata.annotations.as_mut() {
        dep_annotations.remove(&format!("{ADDON_ANNOTATION_PREFIX}{container_name}"));
        dep_annotations.remove(&format!(
            "{ADDON_INJECTED_ENV_ANNOTATION_PREFIX}{container_name}"
        ));
    }

    let t = K8sTimer::new("deployments", "replace");
    let updated = api.replace(&name, &PostParams::default(), &dep).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    let detail = deployment_detail(&updated);
    let pods = list_pods_for_deployment(&state, &ns, &updated).await?;
    let ingresses = list_ingresses_for_service(&state, &ns, &name).await?;
    Ok(Json(DeploymentDetailResponse {
        detail,
        pods,
        ingresses,
    }))
}

// Inject each addon default_env var into the primary (first) container. Only
// injects names that aren't already present, so user-supplied env vars win.
// Returns the list of names that were actually inserted, so detach() can
// reverse exactly those.
fn inject_addon_env_into_primary(
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    addon_env: &[EnvVarOutput],
) -> Vec<String> {
    if addon_env.is_empty() {
        return Vec::new();
    }
    let primary = match pod_spec.containers.first_mut() {
        Some(c) => c,
        None => return Vec::new(),
    };
    let env_vec = primary.env.get_or_insert_with(Vec::new);
    let mut injected = Vec::new();
    for v in addon_env {
        if env_vec.iter().any(|e| e.name == v.name) {
            continue;
        }
        env_vec.push(EnvVar {
            name: v.name.clone(),
            value: Some(v.value.clone()),
            ..Default::default()
        });
        injected.push(v.name.clone());
    }
    injected
}

fn remove_injected_env_from_primary(
    pod_spec: &mut k8s_openapi::api::core::v1::PodSpec,
    injected_names: &[String],
) {
    if injected_names.is_empty() {
        return;
    }
    let primary = match pod_spec.containers.first_mut() {
        Some(c) => c,
        None => return,
    };
    if let Some(env_vec) = primary.env.as_mut() {
        env_vec.retain(|e| !injected_names.iter().any(|n| n == &e.name));
        if env_vec.is_empty() {
            primary.env = None;
        }
    }
}

fn build_resources_from_overrides(
    requests: Option<ResourceSpec>,
    limits: Option<ResourceSpec>,
    defaults: Option<&ResourceSpecOutput>,
) -> Option<ResourceRequirements> {
    let to_map = |spec: ResourceSpec| {
        let mut m = BTreeMap::new();
        if let Some(cpu) = spec.cpu {
            m.insert("cpu".to_string(), Quantity(cpu));
        }
        if let Some(memory) = spec.memory {
            m.insert("memory".to_string(), Quantity(memory));
        }
        if m.is_empty() {
            None
        } else {
            Some(m)
        }
    };
    let req_spec = requests.or_else(|| {
        defaults.map(|d| ResourceSpec {
            cpu: d.cpu.clone(),
            memory: d.memory.clone(),
        })
    });
    let lim_spec = limits.or_else(|| {
        defaults.map(|d| ResourceSpec {
            cpu: d.cpu.clone(),
            memory: d.memory.clone(),
        })
    });
    let requests_map = req_spec.and_then(to_map);
    let limits_map = lim_spec.and_then(to_map);
    if requests_map.is_some() || limits_map.is_some() {
        Some(ResourceRequirements {
            requests: requests_map,
            limits: limits_map,
            ..Default::default()
        })
    } else {
        None
    }
}

async fn list_pods_for_deployment(
    state: &AppState,
    ns: &str,
    dep: &Deployment,
) -> Result<Vec<PodSummary>, AppError> {
    let pods_api = state.pods_api(ns)?;
    let selector = dep
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let lp = ListParams::default().labels(&selector);
    let t = K8sTimer::new("pods", "list");
    let pods = pods_api.list(&lp).await;
    t.finish(pods.is_ok());
    let pods = pods?;
    Ok(pods.iter().map(pod_summary).collect())
}

async fn list_ingresses_for_service(
    state: &AppState,
    ns: &str,
    service_name: &str,
) -> Result<Vec<IngressSummary>, AppError> {
    let ing_api = state.ingresses_api(ns)?;
    let t = K8sTimer::new("ingresses", "list");
    let all = ing_api.list(&ListParams::default()).await;
    t.finish(all.is_ok());
    let all = all?;
    Ok(all
        .iter()
        .filter(|ing| {
            ing.spec
                .as_ref()
                .and_then(|s| s.rules.as_ref())
                .map(|rules| {
                    rules.iter().any(|r| {
                        r.http
                            .as_ref()
                            .map(|http| {
                                http.paths.iter().any(|p| {
                                    p.backend
                                        .service
                                        .as_ref()
                                        .map(|s| s.name == service_name)
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .map(ingress_summary)
        .collect())
}
