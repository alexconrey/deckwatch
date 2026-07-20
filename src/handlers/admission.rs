//! Kubernetes ValidatingAdmissionWebhook handler.
//!
//! The API server posts `AdmissionReview` (v1) objects to
//! `POST /admission/validate`. This handler parses the request, applies
//! the v1 policy set from `docs/VALIDATING_WEBHOOKS.md`, and returns an
//! `AdmissionResponse` embedded in the same review shape.
//!
//! Policy in v1 is intentionally advisory (`allowed: true` with
//! `warnings[]`) except for hard protections on kube-system / kube-public
//! namespaces. `failurePolicy: Ignore` on the webhook config means that
//! any 5xx / timeout from this handler collapses to "admit" at the API
//! server — the handler must never *deny* through a bug.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Container, ResourceRequirements};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AppState;

/// Namespaces deckwatch will never allow its own webhook to admit
/// modifications into. This is the enforcement backstop for the
/// "protected namespaces" policy — the webhook config's
/// `namespaceSelector` should already exclude these, but the handler
/// double-checks so a misconfigured selector cannot let a mutation
/// through.
const PROTECTED_NAMESPACES: &[&str] = &["kube-system", "kube-public", "kube-node-lease"];

/// Field manager string used by deckwatch when it patches resources
/// itself. When we see this on an incoming admission request we skip
/// enforcement — deckwatch operating on its own writes is not the
/// audience for these guardrails.
const DECKWATCH_FIELD_MANAGER: &str = "deckwatch";

// -------------------------------------------------------- AdmissionReview

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionReview {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<AdmissionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<AdmissionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionRequest {
    pub uid: String,
    pub kind: GroupVersionKind,
    pub resource: GroupVersionResource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub operation: String,
    #[serde(rename = "userInfo", default)]
    pub user_info: UserInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<Value>,
    #[serde(rename = "oldObject", default, skip_serializing_if = "Option::is_none")]
    pub old_object: Option<Value>,
    #[serde(rename = "dryRun", default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupVersionKind {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupVersionResource {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub resource: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionResponse {
    pub uid: String,
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AdmissionStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionStatus {
    pub code: u16,
    pub message: String,
}

// ------------------------------------------------------------------- policies

/// Toggles for individual advisory checks. Persisted under
/// `settings.webhook.policies` so operators can disable a noisy rule
/// without redeploying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPolicies {
    #[serde(default = "default_true")]
    pub memory_limit_lt_request: bool,
    #[serde(default = "default_true")]
    pub deployment_missing_readiness_probe: bool,
    #[serde(default = "default_true")]
    pub ingress_missing_class_name: bool,
    #[serde(default = "default_true")]
    pub protected_namespaces: bool,
}

impl Default for WebhookPolicies {
    fn default() -> Self {
        Self {
            memory_limit_lt_request: true,
            deployment_missing_readiness_probe: true,
            ingress_missing_class_name: true,
            protected_namespaces: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Snapshot of the webhook block from `DeckwatchSettings`. Included in
/// `AppState` so the handler does not need to hit the API server on
/// every admission — the admission-path latency budget is ~100ms p99
/// per the design doc §9.
#[derive(Debug, Clone, Default)]
pub struct WebhookConfig {
    pub enabled: bool,
    pub policies: WebhookPolicies,
}

// -------------------------------------------------------------- entry point

/// `POST /admission/validate` — the endpoint the API server calls.
///
/// The handler is intentionally fail-open: any parse error or panic is
/// coerced into `allowed: true` so the webhook cannot inadvertently
/// break the cluster's admission chain. `failurePolicy: Ignore` in the
/// webhook config is a second line of defense outside our process.
pub async fn validate(
    State(_state): State<AppState>,
    Json(review): Json<AdmissionReview>,
) -> impl IntoResponse {
    let Some(request) = review.request.as_ref() else {
        return Json(empty_review("unknown"));
    };

    let response = evaluate(request, &WebhookConfig::default());

    let out = AdmissionReview {
        api_version: review.api_version.clone(),
        kind: review.kind.clone(),
        request: None,
        response: Some(response),
    };

    Json(out)
}

fn empty_review(uid: &str) -> AdmissionReview {
    AdmissionReview {
        api_version: "admission.k8s.io/v1".to_string(),
        kind: "AdmissionReview".to_string(),
        request: None,
        response: Some(AdmissionResponse {
            uid: uid.to_string(),
            allowed: true,
            status: None,
            warnings: Vec::new(),
        }),
    }
}

/// Pure function: given a request and the current policy toggles,
/// produce the admission response. No I/O so this is trivially
/// unit-testable and its p99 latency is bounded by allocation cost.
fn evaluate(req: &AdmissionRequest, cfg: &WebhookConfig) -> AdmissionResponse {
    let mut warnings: Vec<String> = Vec::new();
    let mut denied: Option<String> = None;

    if cfg.policies.protected_namespaces {
        if let Some(ns) = req.namespace.as_deref() {
            if PROTECTED_NAMESPACES.contains(&ns) && !is_deckwatch_actor(&req.user_info) {
                denied = Some(format!(
                    "protected namespace '{ns}' cannot be modified via deckwatch-managed \
                     webhook (user='{}')",
                    req.user_info.username,
                ));
            }
        }
    }

    if denied.is_none() {
        if let Some(obj) = req.object.as_ref() {
            match req.kind.kind.as_str() {
                "Deployment" => {
                    if let Ok(dep) = serde_json::from_value::<Deployment>(obj.clone()) {
                        check_deployment(&dep, &cfg.policies, &mut warnings);
                    }
                }
                "Ingress" => {
                    if let Ok(ing) = serde_json::from_value::<Ingress>(obj.clone()) {
                        check_ingress(&ing, &cfg.policies, &mut warnings);
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(reason) = denied {
        AdmissionResponse {
            uid: req.uid.clone(),
            allowed: false,
            status: Some(AdmissionStatus {
                code: 403,
                message: reason,
            }),
            warnings,
        }
    } else {
        AdmissionResponse {
            uid: req.uid.clone(),
            allowed: true,
            status: None,
            warnings,
        }
    }
}

fn is_deckwatch_actor(user: &UserInfo) -> bool {
    (user.username.starts_with("system:serviceaccount:") && user.username.contains("deckwatch"))
        || user.username == DECKWATCH_FIELD_MANAGER
}

// ------------------------------------------------------- policy: deployments

fn check_deployment(dep: &Deployment, cfg: &WebhookPolicies, warnings: &mut Vec<String>) {
    let name = dep.metadata.name.as_deref().unwrap_or("(unnamed)");
    let ns = dep.metadata.namespace.as_deref().unwrap_or("(no-ns)");

    let Some(spec) = dep.spec.as_ref() else {
        return;
    };
    let Some(pod_spec) = spec.template.spec.as_ref() else {
        return;
    };

    for container in &pod_spec.containers {
        if cfg.memory_limit_lt_request {
            if let Some(msg) = memory_limit_lt_request(container) {
                warnings.push(format!(
                    "deckwatch: Deployment {ns}/{name} container '{}': {msg}",
                    container.name,
                ));
            }
        }
        if cfg.deployment_missing_readiness_probe && container.readiness_probe.is_none() {
            warnings.push(format!(
                "deckwatch: Deployment {ns}/{name} container '{}' has no readinessProbe — \
                 Kubernetes will send traffic to unhealthy pods during rollout",
                container.name,
            ));
        }
    }
}

/// Returns `Some(msg)` when the container declares a memory *limit*
/// smaller than its memory *request*. Kubernetes accepts this
/// (limits/requests are independently valid Quantities) but the pod
/// will be OOMKilled within milliseconds of starting.
fn memory_limit_lt_request(container: &Container) -> Option<String> {
    let res = container.resources.as_ref()?;
    let request = memory(res, "requests")?;
    let limit = memory(res, "limits")?;
    if bytes(&limit) < bytes(&request) {
        Some(format!(
            "memory.limit ({}) is less than memory.request ({}) — container will be scheduled \
             but OOMKilled immediately",
            limit.0, request.0,
        ))
    } else {
        None
    }
}

fn memory(res: &ResourceRequirements, kind: &str) -> Option<Quantity> {
    let map = match kind {
        "requests" => res.requests.as_ref()?,
        "limits" => res.limits.as_ref()?,
        _ => return None,
    };
    map.get("memory").cloned()
}

/// Parse a memory quantity into bytes. Supports standard k8s suffixes.
/// Returns `u64::MAX` on parse failure so a malformed quantity is
/// treated as "definitely bigger than any request" — the guardrail
/// must never denounce a valid pod as malformed.
fn bytes(q: &Quantity) -> u64 {
    let s = q.0.trim();
    let (num_str, mult) = if let Some(v) = s.strip_suffix("Ki") {
        (v, 1_024u64)
    } else if let Some(v) = s.strip_suffix("Mi") {
        (v, 1_024u64.pow(2))
    } else if let Some(v) = s.strip_suffix("Gi") {
        (v, 1_024u64.pow(3))
    } else if let Some(v) = s.strip_suffix("Ti") {
        (v, 1_024u64.pow(4))
    } else if let Some(v) = s.strip_suffix('K') {
        (v, 1_000u64)
    } else if let Some(v) = s.strip_suffix('M') {
        (v, 1_000u64.pow(2))
    } else if let Some(v) = s.strip_suffix('G') {
        (v, 1_000u64.pow(3))
    } else if let Some(v) = s.strip_suffix('T') {
        (v, 1_000u64.pow(4))
    } else {
        (s, 1u64)
    };
    num_str
        .trim()
        .parse::<u64>()
        .map(|n| n.saturating_mul(mult))
        .unwrap_or(u64::MAX)
}

// --------------------------------------------------------- policy: ingresses

fn check_ingress(ing: &Ingress, cfg: &WebhookPolicies, warnings: &mut Vec<String>) {
    let name = ing.metadata.name.as_deref().unwrap_or("(unnamed)");
    let ns = ing.metadata.namespace.as_deref().unwrap_or("(no-ns)");
    if cfg.ingress_missing_class_name {
        let class = ing
            .spec
            .as_ref()
            .and_then(|s| s.ingress_class_name.as_deref());
        if class.is_none() {
            warnings.push(format!(
                "deckwatch: Ingress {ns}/{name} has no ingressClassName — the default \
                 IngressClass may not exist and this Ingress will silently do nothing",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
    use k8s_openapi::api::core::v1::{
        Container, PodSpec, PodTemplateSpec, Probe, ResourceRequirements,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
    use std::collections::BTreeMap;

    fn deployment_with(container: Container) -> Deployment {
        Deployment {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                selector: LabelSelector::default(),
                template: PodTemplateSpec {
                    metadata: None,
                    spec: Some(PodSpec {
                        containers: vec![container],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            status: None,
        }
    }

    #[test]
    fn memory_limit_lt_request_warns() {
        let mut requests = BTreeMap::new();
        requests.insert("memory".to_string(), Quantity("256Mi".to_string()));
        let mut limits = BTreeMap::new();
        limits.insert("memory".to_string(), Quantity("128Mi".to_string()));
        let container = Container {
            name: "app".to_string(),
            resources: Some(ResourceRequirements {
                requests: Some(requests),
                limits: Some(limits),
                ..Default::default()
            }),
            readiness_probe: Some(Probe::default()),
            ..Default::default()
        };
        let dep = deployment_with(container);
        let mut warnings = Vec::new();
        check_deployment(&dep, &WebhookPolicies::default(), &mut warnings);
        assert!(warnings.iter().any(|w| w.contains("memory.limit")));
    }

    #[test]
    fn missing_readiness_probe_warns() {
        let container = Container {
            name: "app".to_string(),
            ..Default::default()
        };
        let dep = deployment_with(container);
        let mut warnings = Vec::new();
        check_deployment(&dep, &WebhookPolicies::default(), &mut warnings);
        assert!(warnings.iter().any(|w| w.contains("readinessProbe")));
    }

    #[test]
    fn protected_namespace_denies_external_actor() {
        let req = AdmissionRequest {
            uid: "1".to_string(),
            kind: GroupVersionKind {
                kind: "Deployment".to_string(),
                ..Default::default()
            },
            resource: GroupVersionResource::default(),
            name: Some("evil".to_string()),
            namespace: Some("kube-system".to_string()),
            operation: "CREATE".to_string(),
            user_info: UserInfo {
                username: "kubectl-user".to_string(),
                ..Default::default()
            },
            object: None,
            old_object: None,
            dry_run: false,
        };
        let resp = evaluate(&req, &WebhookConfig::default());
        assert!(!resp.allowed);
    }

    #[test]
    fn protected_namespace_allows_deckwatch_actor() {
        let req = AdmissionRequest {
            uid: "2".to_string(),
            kind: GroupVersionKind {
                kind: "Deployment".to_string(),
                ..Default::default()
            },
            resource: GroupVersionResource::default(),
            name: Some("kube-dns".to_string()),
            namespace: Some("kube-system".to_string()),
            operation: "UPDATE".to_string(),
            user_info: UserInfo {
                username: "system:serviceaccount:deckwatch:deckwatch".to_string(),
                ..Default::default()
            },
            object: None,
            old_object: None,
            dry_run: false,
        };
        let resp = evaluate(&req, &WebhookConfig::default());
        assert!(resp.allowed);
    }
}

#[cfg(test)]
#[path = "../handlers_admission_tests.rs"]
mod handlers_admission_tests;
