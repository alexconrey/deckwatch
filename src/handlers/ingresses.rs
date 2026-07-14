use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, IngressTLS, ServiceBackendPort,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{ListParams, PostParams};
use serde::Deserialize;

use crate::error::AppError;
use crate::kube_ext::{ingress_detail, ingress_summary, IngressDetail, IngressSummary};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct IngressListResponse {
    pub ingresses: Vec<IngressSummary>,
}

#[derive(Deserialize)]
pub struct CreateIngressRequest {
    pub name: String,
    pub host: Option<String>,
    pub paths: Vec<IngressPathInput>,
    pub ingress_class: Option<String>,
    pub annotations: Option<BTreeMap<String, String>>,
    pub tls: Option<Vec<TlsInput>>,
}

#[derive(Deserialize)]
pub struct IngressPathInput {
    pub path: String,
    pub path_type: Option<String>,
    pub service_name: String,
    pub service_port: i32,
}

#[derive(Deserialize)]
pub struct TlsInput {
    pub hosts: Vec<String>,
    pub secret_name: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<IngressListResponse>, AppError> {
    let api = state.ingresses_api(&ns)?;
    let t = K8sTimer::new("ingresses", "list");
    let ingresses = api.list(&ListParams::default()).await;
    t.finish(ingresses.is_ok());
    let ingresses = ingresses?;
    Ok(Json(IngressListResponse {
        ingresses: ingresses.iter().map(ingress_summary).collect(),
    }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<IngressDetail>, AppError> {
    let api = state.ingresses_api(&ns)?;
    let t = K8sTimer::new("ingresses", "get");
    let ing = api.get(&name).await;
    t.finish(ing.is_ok());
    let ing = ing?;
    Ok(Json(ingress_detail(&ing)))
}

pub async fn create(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateIngressRequest>,
) -> Result<(StatusCode, Json<IngressDetail>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.paths.is_empty() {
        return Err(AppError::BadRequest(
            "at least one path is required".to_string(),
        ));
    }

    let svc_api = state.services_api(&ns)?;
    for p in &req.paths {
        ensure_service(&svc_api, &p.service_name, p.service_port).await?;
    }

    let api = state.ingresses_api(&ns)?;

    let mut labels = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let http_paths: Vec<HTTPIngressPath> = req
        .paths
        .iter()
        .map(|p| HTTPIngressPath {
            path: Some(p.path.clone()),
            path_type: p.path_type.clone().unwrap_or_else(|| "Prefix".to_string()),
            backend: IngressBackend {
                service: Some(IngressServiceBackend {
                    name: p.service_name.clone(),
                    port: Some(ServiceBackendPort {
                        number: Some(p.service_port),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
        })
        .collect();

    let rules = vec![IngressRule {
        host: req.host.clone(),
        http: Some(HTTPIngressRuleValue { paths: http_paths }),
    }];

    let tls = req.tls.map(|tls_list| {
        tls_list
            .into_iter()
            .map(|t| IngressTLS {
                hosts: Some(t.hosts),
                secret_name: t.secret_name,
            })
            .collect()
    });

    let ingress = Ingress {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(ns.clone()),
            labels: Some(labels),
            annotations: req.annotations,
            ..Default::default()
        },
        spec: Some(IngressSpec {
            ingress_class_name: req.ingress_class,
            rules: Some(rules),
            tls,
            ..Default::default()
        }),
        ..Default::default()
    };

    let t = K8sTimer::new("ingresses", "create");
    let created = api.create(&PostParams::default(), &ingress).await;
    t.finish(created.is_ok());
    let created = created?;
    Ok((StatusCode::CREATED, Json(ingress_detail(&created))))
}

pub async fn update(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<CreateIngressRequest>,
) -> Result<Json<IngressDetail>, AppError> {
    let svc_api = state.services_api(&ns)?;
    for p in &req.paths {
        ensure_service(&svc_api, &p.service_name, p.service_port).await?;
    }

    let api = state.ingresses_api(&ns)?;
    let t = K8sTimer::new("ingresses", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let existing = existing?;
    let mut ing = existing;

    let http_paths: Vec<HTTPIngressPath> = req
        .paths
        .iter()
        .map(|p| HTTPIngressPath {
            path: Some(p.path.clone()),
            path_type: p.path_type.clone().unwrap_or_else(|| "Prefix".to_string()),
            backend: IngressBackend {
                service: Some(IngressServiceBackend {
                    name: p.service_name.clone(),
                    port: Some(ServiceBackendPort {
                        number: Some(p.service_port),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
        })
        .collect();

    let rules = vec![IngressRule {
        host: req.host.clone(),
        http: Some(HTTPIngressRuleValue { paths: http_paths }),
    }];

    let tls = req.tls.map(|tls_list| {
        tls_list
            .into_iter()
            .map(|t| IngressTLS {
                hosts: Some(t.hosts),
                secret_name: t.secret_name,
            })
            .collect()
    });

    if let Some(spec) = ing.spec.as_mut() {
        spec.ingress_class_name = req.ingress_class;
        spec.rules = Some(rules);
        spec.tls = tls;
    }

    if let Some(meta) = Some(&mut ing.metadata) {
        meta.annotations = req.annotations;
    }

    let t = K8sTimer::new("ingresses", "replace");
    let updated = api.replace(&name, &PostParams::default(), &ing).await;
    t.finish(updated.is_ok());
    let updated = updated?;
    Ok(Json(ingress_detail(&updated)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let api = state.ingresses_api(&ns)?;
    let t = K8sTimer::new("ingresses", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_service(
    svc_api: &kube::Api<Service>,
    name: &str,
    port: i32,
) -> Result<(), AppError> {
    let t = K8sTimer::new("services", "get");
    let get_res = svc_api.get(name).await;
    // A 404 here is expected — treat as ok=true so we don't inflate error counts
    // for the common "create if missing" path.
    let is_notfound = matches!(&get_res, Err(kube::Error::Api(e)) if e.code == 404);
    t.finish(get_res.is_ok() || is_notfound);
    match get_res {
        Ok(_) => return Ok(()),
        Err(kube::Error::Api(e)) if e.code == 404 => {}
        Err(e) => return Err(e.into()),
    }

    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), name.to_string());
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let mut selector = BTreeMap::new();
    selector.insert("app".to_string(), name.to_string());

    let svc = Service {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(selector),
            ports: Some(vec![ServicePort {
                port,
                target_port: Some(IntOrString::Int(port)),
                protocol: Some("TCP".to_string()),
                ..Default::default()
            }]),
            type_: Some("ClusterIP".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let t = K8sTimer::new("services", "create");
    let res = svc_api.create(&PostParams::default(), &svc).await;
    t.finish(res.is_ok());
    res?;
    Ok(())
}
