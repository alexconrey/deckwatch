use axum::extract::{Path, Query, State};
use axum::Json;
use kube::api::ListParams;
use serde::Deserialize;

use crate::error::AppError;
use crate::kube_ext::{cronjob_summary, CronJobSummary};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub label_selector: Option<String>,
}

#[derive(serde::Serialize)]
pub struct CronJobListResponse {
    pub cronjobs: Vec<CronJobSummary>,
}

#[derive(serde::Serialize)]
pub struct CronJobDetailResponse {
    #[serde(flatten)]
    pub summary: CronJobSummary,
}

pub async fn list(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<CronJobListResponse>, AppError> {
    let api = state.cronjobs_api(&ns)?;
    let mut lp = ListParams::default();
    if let Some(selector) = query.label_selector {
        lp = lp.labels(&selector);
    }
    let t = K8sTimer::new("cronjobs", "list");
    let list = api.list(&lp).await;
    t.finish(list.is_ok());
    let list = list?;
    let cronjobs = list.iter().map(cronjob_summary).collect();
    Ok(Json(CronJobListResponse { cronjobs }))
}

pub async fn get(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<CronJobDetailResponse>, AppError> {
    let api = state.cronjobs_api(&ns)?;
    let t = K8sTimer::new("cronjobs", "get");
    let cj = api.get(&name).await;
    t.finish(cj.is_ok());
    let cj = cj?;
    let summary = cronjob_summary(&cj);
    Ok(Json(CronJobDetailResponse { summary }))
}
