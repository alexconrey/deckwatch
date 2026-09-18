use axum::extract::{Path, Query, State};
use axum::Json;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, LogParams, PostParams};
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

pub async fn trigger(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let cj_api = state.cronjobs_api(&ns)?;
    let t = K8sTimer::new("cronjobs", "get");
    let cj = cj_api.get(&name).await;
    t.finish(cj.is_ok());
    let cj = cj?;

    let job_template = cj
        .spec
        .ok_or_else(|| AppError::BadRequest("CronJob has no spec".to_string()))?
        .job_template;

    let suffix = uuid::Uuid::new_v4().to_string();
    let job_name = format!("{name}-manual-{}", &suffix[..8]);

    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.clone()),
            namespace: Some(ns.clone()),
            annotations: Some(
                [(
                    "cronjob.kubernetes.io/instantiate".to_string(),
                    "manual".to_string(),
                )]
                .into(),
            ),
            ..Default::default()
        },
        spec: job_template.spec,
        ..Default::default()
    };

    let jobs_api = state.jobs_api(&ns)?;
    let t = K8sTimer::new("jobs", "create");
    let result = jobs_api.create(&PostParams::default(), &job).await;
    t.finish(result.is_ok());
    result?;

    Ok(Json(serde_json::json!({ "job_name": job_name })))
}

pub async fn recent_logs(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let jobs_api = state.jobs_api(&ns)?;
    let t = K8sTimer::new("jobs", "list");
    let all_jobs = jobs_api.list(&ListParams::default()).await;
    t.finish(all_jobs.is_ok());
    let all_jobs = all_jobs?;

    let mut owned_jobs: Vec<_> = all_jobs
        .items
        .into_iter()
        .filter(|j| {
            j.metadata
                .owner_references
                .as_ref()
                .map(|refs| refs.iter().any(|r| r.name == name && r.kind == "CronJob"))
                .unwrap_or(false)
        })
        .collect();

    if owned_jobs.is_empty() {
        return Err(AppError::NotFound(format!(
            "No jobs found for cronjob '{name}'"
        )));
    }

    owned_jobs.sort_by_key(|j| {
        j.metadata
            .creation_timestamp
            .clone()
            .map(|t| t.0)
            .unwrap_or_default()
    });

    let recent_job = owned_jobs.last().unwrap();
    let job_name = recent_job
        .metadata
        .name
        .as_deref()
        .unwrap_or("")
        .to_string();

    let pods_api = state.pods_api(&ns)?;
    let lp = ListParams::default().labels(&format!("job-name={job_name}"));
    let t = K8sTimer::new("pods", "list");
    let pods = pods_api.list(&lp).await;
    t.finish(pods.is_ok());
    let pods = pods?;

    let pod = pods
        .items
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound(format!("No pods found for job '{job_name}'")))?;
    let pod_name = pod.metadata.name.as_deref().unwrap_or("").to_string();

    let t = K8sTimer::new("pods", "logs");
    let logs = pods_api.logs(&pod_name, &LogParams::default()).await;
    t.finish(logs.is_ok());
    let logs = logs?;

    Ok(Json(serde_json::json!({
        "job_name": job_name,
        "pod_name": pod_name,
        "logs": logs,
    })))
}
