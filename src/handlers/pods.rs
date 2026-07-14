use axum::extract::{Path, State};
use axum::Json;
use kube::api::ListParams;

use crate::error::AppError;
use crate::kube_ext::{pod_summary, PodSummary};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct PodListResponse {
    pub pods: Vec<PodSummary>,
}

pub async fn list_for_deployment(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<PodListResponse>, AppError> {
    let pods_api = state.pods_api(&ns)?;
    let lp = ListParams::default().labels(&format!("app={name}"));
    let t = K8sTimer::new("pods", "list");
    let pods = pods_api.list(&lp).await;
    t.finish(pods.is_ok());
    let pods = pods?;
    Ok(Json(PodListResponse {
        pods: pods.iter().map(pod_summary).collect(),
    }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, pod_name)): Path<(String, String)>,
) -> Result<Json<PodSummary>, AppError> {
    let pods_api = state.pods_api(&ns)?;
    let t = K8sTimer::new("pods", "get");
    let pod = pods_api.get(&pod_name).await;
    t.finish(pod.is_ok());
    let pod = pod?;
    Ok(Json(pod_summary(&pod)))
}
