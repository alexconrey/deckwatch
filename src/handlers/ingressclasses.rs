use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use k8s_openapi::api::networking::v1::{
    IngressClass, IngressClassParametersReference, IngressClassSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, PostParams};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::error::AppError;
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Serialize)]
pub struct IngressClassParametersRef {
    pub api_group: Option<String>,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct IngressClassSummary {
    pub name: String,
    pub controller: String,
    pub is_default: bool,
    pub parameters: Option<IngressClassParametersRef>,
}

#[derive(Serialize)]
pub struct IngressClassListResponse {
    pub ingress_classes: Vec<IngressClassSummary>,
}

#[derive(Deserialize)]
pub struct CreateIngressClassParametersRef {
    pub api_group: Option<String>,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub scope: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateIngressClassRequest {
    pub name: String,
    pub controller: String,
    pub is_default: Option<bool>,
    pub parameters: Option<CreateIngressClassParametersRef>,
}

fn summarize(ic: &IngressClass) -> IngressClassSummary {
    let name = ic.metadata.name.clone().unwrap_or_default();
    let is_default = ic
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("ingressclass.kubernetes.io/is-default-class"))
        .map(|v| v == "true")
        .unwrap_or(false);

    let controller = ic
        .spec
        .as_ref()
        .and_then(|s| s.controller.clone())
        .unwrap_or_default();

    let parameters = ic
        .spec
        .as_ref()
        .and_then(|s| s.parameters.as_ref())
        .map(|p| IngressClassParametersRef {
            api_group: p.api_group.clone(),
            kind: p.kind.clone(),
            name: p.name.clone(),
            namespace: p.namespace.clone(),
            scope: p.scope.clone(),
        });

    IngressClassSummary {
        name,
        controller,
        is_default,
        parameters,
    }
}

pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<IngressClassListResponse>, AppError> {
    let api = state.ingressclasses_api();
    let t = K8sTimer::new("ingressclasses", "list");
    let result = api.list(&ListParams::default()).await;
    t.finish(result.is_ok());
    let list = result?;

    let ingress_classes = list.iter().map(summarize).collect();

    Ok(Json(IngressClassListResponse { ingress_classes }))
}

pub async fn get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<IngressClassSummary>, AppError> {
    let api = state.ingressclasses_api();
    let t = K8sTimer::new("ingressclasses", "get");
    let result = api.get(&name).await;
    t.finish(result.is_ok());
    let ic = result?;
    Ok(Json(summarize(&ic)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateIngressClassRequest>,
) -> Result<(StatusCode, Json<IngressClassSummary>), AppError> {
    if req.name.is_empty() {
        return Err(AppError::BadRequest("name is required".to_string()));
    }
    if req.controller.is_empty() {
        return Err(AppError::BadRequest("controller is required".to_string()));
    }

    let mut annotations = BTreeMap::new();
    if req.is_default.unwrap_or(false) {
        annotations.insert(
            "ingressclass.kubernetes.io/is-default-class".to_string(),
            "true".to_string(),
        );
    }

    let mut labels = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "deckwatch".to_string(),
    );

    let parameters = req.parameters.map(|p| IngressClassParametersReference {
        api_group: p.api_group,
        kind: p.kind,
        name: p.name,
        namespace: p.namespace,
        scope: p.scope,
    });

    let ic = IngressClass {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            labels: Some(labels),
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations)
            },
            ..Default::default()
        },
        spec: Some(IngressClassSpec {
            controller: Some(req.controller),
            parameters,
        }),
    };

    let api = state.ingressclasses_api();
    let t = K8sTimer::new("ingressclasses", "create");
    let created = api.create(&PostParams::default(), &ic).await;
    t.finish(created.is_ok());
    let created = created?;

    if let Err(e) = audit::log_action(
        &state.db,
        "create",
        "ingressclass",
        &req.name,
        "",
        &format!("created ingress class {}", req.name),
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok((StatusCode::CREATED, Json(summarize(&created))))
}

pub async fn update(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CreateIngressClassRequest>,
) -> Result<Json<IngressClassSummary>, AppError> {
    let api = state.ingressclasses_api();
    let t = K8sTimer::new("ingressclasses", "get");
    let existing = api.get(&name).await;
    t.finish(existing.is_ok());
    let mut ic = existing?;

    let parameters = req.parameters.map(|p| IngressClassParametersReference {
        api_group: p.api_group,
        kind: p.kind,
        name: p.name,
        namespace: p.namespace,
        scope: p.scope,
    });

    if let Some(spec) = ic.spec.as_mut() {
        spec.controller = Some(req.controller);
        spec.parameters = parameters;
    } else {
        ic.spec = Some(IngressClassSpec {
            controller: Some(req.controller),
            parameters,
        });
    }

    let annotations = ic.metadata.annotations.get_or_insert_with(BTreeMap::new);
    if req.is_default.unwrap_or(false) {
        annotations.insert(
            "ingressclass.kubernetes.io/is-default-class".to_string(),
            "true".to_string(),
        );
    } else {
        annotations.remove("ingressclass.kubernetes.io/is-default-class");
    }

    let t = K8sTimer::new("ingressclasses", "replace");
    let updated = api.replace(&name, &PostParams::default(), &ic).await;
    t.finish(updated.is_ok());
    let updated = updated?;

    if let Err(e) = audit::log_action(
        &state.db,
        "update",
        "ingressclass",
        &name,
        "",
        "updated ingress class",
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(Json(summarize(&updated)))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let api = state.ingressclasses_api();
    let t = K8sTimer::new("ingressclasses", "delete");
    let res = api.delete(&name, &Default::default()).await;
    t.finish(res.is_ok());
    res?;

    if let Err(e) = audit::log_action(
        &state.db,
        "delete",
        "ingressclass",
        &name,
        "",
        "deleted ingress class",
        "",
    )
    .await
    {
        tracing::warn!(error = %e, "failed to write audit log");
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_class_summary_serializes() {
        let ic = IngressClassSummary {
            name: "alb".to_string(),
            controller: "ingress.k8s.aws/alb".to_string(),
            is_default: true,
            parameters: None,
        };
        let json = serde_json::to_value(&ic).unwrap();
        assert_eq!(json["name"], "alb");
        assert_eq!(json["controller"], "ingress.k8s.aws/alb");
        assert_eq!(json["is_default"], true);
        assert!(json["parameters"].is_null());
    }

    #[test]
    fn ingress_class_summary_with_parameters() {
        let ic = IngressClassSummary {
            name: "nginx".to_string(),
            controller: "k8s.io/ingress-nginx".to_string(),
            is_default: false,
            parameters: Some(IngressClassParametersRef {
                api_group: Some("example.com".to_string()),
                kind: "IngressParameters".to_string(),
                name: "my-params".to_string(),
                namespace: Some("default".to_string()),
                scope: Some("Namespace".to_string()),
            }),
        };
        let json = serde_json::to_value(&ic).unwrap();
        assert_eq!(json["name"], "nginx");
        assert_eq!(json["parameters"]["api_group"], "example.com");
        assert_eq!(json["parameters"]["kind"], "IngressParameters");
        assert_eq!(json["parameters"]["name"], "my-params");
        assert_eq!(json["parameters"]["namespace"], "default");
        assert_eq!(json["parameters"]["scope"], "Namespace");
    }

    #[test]
    fn list_response_serializes() {
        let resp = IngressClassListResponse {
            ingress_classes: vec![
                IngressClassSummary {
                    name: "alb".to_string(),
                    controller: "ingress.k8s.aws/alb".to_string(),
                    is_default: true,
                    parameters: None,
                },
                IngressClassSummary {
                    name: "nginx".to_string(),
                    controller: "k8s.io/ingress-nginx".to_string(),
                    is_default: false,
                    parameters: None,
                },
            ],
        };
        let json = serde_json::to_value(&resp).unwrap();
        let classes = json["ingress_classes"].as_array().unwrap();
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0]["name"], "alb");
        assert_eq!(classes[1]["name"], "nginx");
    }

    #[test]
    fn create_request_deserializes() {
        let json = serde_json::json!({
            "name": "alb",
            "controller": "ingress.k8s.aws/alb",
            "is_default": true,
            "parameters": {
                "api_group": "elbv2.k8s.aws",
                "kind": "IngressClassParams",
                "name": "alb-params",
                "namespace": "kube-system",
                "scope": "Namespace"
            }
        });
        let req: CreateIngressClassRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "alb");
        assert_eq!(req.controller, "ingress.k8s.aws/alb");
        assert_eq!(req.is_default, Some(true));
        let params = req.parameters.unwrap();
        assert_eq!(params.api_group, Some("elbv2.k8s.aws".to_string()));
        assert_eq!(params.kind, "IngressClassParams");
        assert_eq!(params.name, "alb-params");
    }
}
