use axum::extract::State;
use axum::Json;
use kube::api::ListParams;

use crate::error::AppError;
use crate::kube_ext::{node_summary, NodeSummary};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct NodeListResponse {
    pub nodes: Vec<NodeSummary>,
}

pub async fn list_nodes(State(state): State<AppState>) -> Result<Json<NodeListResponse>, AppError> {
    let api = state.nodes_api();
    let t = K8sTimer::new("nodes", "list");
    let list = api.list(&ListParams::default()).await;
    t.finish(list.is_ok());
    let list = list?;
    let nodes = list.iter().map(node_summary).collect();
    Ok(Json(NodeListResponse { nodes }))
}
