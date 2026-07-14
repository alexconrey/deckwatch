use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{ConfigMap, Event, Namespace, Node, Pod, Secret, Service};
use k8s_openapi::api::networking::v1::{Ingress, IngressClass};
use kube::api::{ApiResource, DynamicObject, GroupVersionKind};
use kube::discovery;
use kube::Api;

use crate::error::AppError;
use crate::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub kube_client: kube::Client,
    pub allowed_namespaces: Vec<String>,
    pub settings_namespace: String,
    pub settings_configmap_name: String,
    pub entitlements: std::sync::Arc<crate::license::Entitlements>,
    /// In-cluster URL of the embedded OCI registry, if enabled. Consumed
    /// by both the settings handler (auto-populates the built-in
    /// registries dropdown) and by watcher.rs (decides whether kaniko
    /// needs `--insecure` for pushes to the local registry).
    pub registry_public_url: Option<String>,
    /// Per-namespace rate limiter for AI-agent jobs (diagnostics + ai-fix).
    /// Shared across all handlers via `AppState` clones — the limiter
    /// itself is an `Arc<Mutex<..>>` internally so counters are consistent
    /// no matter which handler task runs the check. See `rate_limit.rs`
    /// for the semantics and docs/AI_SAFETY.md for the rationale.
    pub ai_rate_limiter: RateLimiter,
    /// Database connection (SQLite, PostgreSQL, or MySQL). Tables are
    /// created automatically via SeaORM migrations at startup.
    pub db: sea_orm::DatabaseConnection,
}

impl AppState {
    pub fn is_namespace_allowed(&self, ns: &str) -> bool {
        self.allowed_namespaces.is_empty() || self.allowed_namespaces.iter().any(|n| n == ns)
    }

    fn check_namespace(&self, ns: &str) -> Result<(), AppError> {
        if !self.is_namespace_allowed(ns) {
            return Err(AppError::NamespaceNotAllowed(ns.to_string()));
        }
        Ok(())
    }

    pub fn deployments_api(&self, ns: &str) -> Result<Api<Deployment>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn pods_api(&self, ns: &str) -> Result<Api<Pod>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn ingresses_api(&self, ns: &str) -> Result<Api<Ingress>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn services_api(&self, ns: &str) -> Result<Api<Service>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn jobs_api(&self, ns: &str) -> Result<Api<Job>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn cronjobs_api(&self, ns: &str) -> Result<Api<CronJob>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn secrets_api(&self, ns: &str) -> Result<Api<Secret>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn configmaps_api(&self, ns: &str) -> Result<Api<ConfigMap>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn events_api(&self, ns: &str) -> Result<Api<Event>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub fn events_api_all(&self) -> Api<Event> {
        Api::all(self.kube_client.clone())
    }

    pub fn hpa_api(&self, ns: &str) -> Result<Api<HorizontalPodAutoscaler>, AppError> {
        self.check_namespace(ns)?;
        Ok(Api::namespaced(self.kube_client.clone(), ns))
    }

    pub async fn podmonitors_api(&self, ns: &str) -> Result<Api<DynamicObject>, String> {
        self.check_namespace(ns).map_err(|e| e.to_string())?;
        let gvk = GroupVersionKind::gvk("monitoring.coreos.com", "v1", "PodMonitor");
        let (ar, _caps) = discovery::pinned_kind(&self.kube_client, &gvk).await.map_err(|e| {
            format!(
                "prometheus-operator CRD monitoring.coreos.com/v1 PodMonitor                  not found in the cluster ({e}). Install the prometheus-operator                  to enable per-deployment scrape configuration."
            )
        })?;
        Ok(Self::dynamic_namespaced(&self.kube_client, ns, &ar))
    }

    fn dynamic_namespaced(client: &kube::Client, ns: &str, ar: &ApiResource) -> Api<DynamicObject> {
        Api::namespaced_with(client.clone(), ns, ar)
    }

    pub fn namespaces_api(&self) -> Api<Namespace> {
        Api::all(self.kube_client.clone())
    }

    pub fn nodes_api(&self) -> Api<Node> {
        Api::all(self.kube_client.clone())
    }

    pub fn ingressclasses_api(&self) -> Api<IngressClass> {
        Api::all(self.kube_client.clone())
    }
}
