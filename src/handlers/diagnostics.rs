use std::collections::BTreeMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use futures::{AsyncBufReadExt, StreamExt};
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, EnvVar, EnvVarSource, Pod, PodSpec,
    PodTemplateSpec, SecretKeySelector, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::{ListParams, LogParams, Patch, PatchParams, PostParams};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::AppError;
use crate::log_sanitize::{sanitize_logs, wrap_prompt};
use crate::metrics;
use crate::state::AppState;

/// Maximum length for Kubernetes label values (and the names we use as
/// label values). RFC 1123 / K8s validation caps label values at 63 chars.
const K8S_LABEL_VALUE_MAX: usize = 63;

const DIAG_LABEL_KEY: &str = "deckwatch.io/diagnostic";
const DIAG_POD_LABEL_KEY: &str = "deckwatch.io/diagnostic-source-pod";
const DIAG_AGENT_LABEL_KEY: &str = "deckwatch.io/diagnostic-agent";

const DEFAULT_CLAUDE_IMAGE: &str = "node:24-slim";
const DEFAULT_CODEX_IMAGE: &str = "ghcr.io/openai/codex:latest";
const DEFAULT_CLAUDE_SECRET: &str = "deckwatch-anthropic-api-key";
const DEFAULT_CODEX_SECRET: &str = "deckwatch-openai-api-key";

const DIAG_PROMPT: &str =
    "Analyze these Kubernetes pod logs and diagnose the issue. Suggest fixes.";
const LOGS_MOUNT_DIR: &str = "/diag";
const LOGS_FILE_NAME: &str = "prompt.txt";
const LOGS_TRUNCATE_BYTES: usize = 256 * 1024;

// SSE stream tuning. The pod-wait loop polls the API server for the Job's pod;
// once the pod exists we watch its phase until logs are readable. Both bounds
// are generous — a slow image pull can easily blow past 30 s, and giving up
// early would force the frontend into its polling fallback for no good reason.
const STREAM_POD_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const STREAM_POD_WAIT_INTERVAL: Duration = Duration::from_millis(750);
const STREAM_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Deserialize)]
pub struct DiagnoseRequest {
    pub pod_name: String,
    pub container: Option<String>,
    pub logs: String,
    pub agent: DiagAgent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagAgent {
    Claude,
    Codex,
}

impl DiagAgent {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagAgent::Claude => "claude",
            DiagAgent::Codex => "codex",
        }
    }

    fn image(&self) -> String {
        match self {
            DiagAgent::Claude => std::env::var("DECKWATCH_DIAG_CLAUDE_IMAGE")
                .unwrap_or_else(|_| DEFAULT_CLAUDE_IMAGE.to_string()),
            DiagAgent::Codex => std::env::var("DECKWATCH_DIAG_CODEX_IMAGE")
                .unwrap_or_else(|_| DEFAULT_CODEX_IMAGE.to_string()),
        }
    }

    fn api_key_env(&self) -> &'static str {
        match self {
            DiagAgent::Claude => "ANTHROPIC_API_KEY",
            DiagAgent::Codex => "OPENAI_API_KEY",
        }
    }

    fn api_key_secret(&self) -> String {
        match self {
            DiagAgent::Claude => std::env::var("DECKWATCH_DIAG_CLAUDE_SECRET")
                .unwrap_or_else(|_| DEFAULT_CLAUDE_SECRET.to_string()),
            DiagAgent::Codex => std::env::var("DECKWATCH_DIAG_CODEX_SECRET")
                .unwrap_or_else(|_| DEFAULT_CODEX_SECRET.to_string()),
        }
    }
}

fn parse_agent(v: &str) -> Option<DiagAgent> {
    match v {
        "claude" => Some(DiagAgent::Claude),
        "codex" => Some(DiagAgent::Codex),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
pub struct DiagnoseResponse {
    pub job_name: String,
    pub status: DiagStatus,
    pub agent: DiagAgent,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticStatusResponse {
    pub job_name: String,
    pub status: DiagStatus,
    pub agent: Option<DiagAgent>,
    pub source_pod: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticResultResponse {
    pub job_name: String,
    pub status: DiagStatus,
    pub output: String,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticHistoryItem {
    pub job_name: String,
    pub status: DiagStatus,
    pub agent: Option<DiagAgent>,
    pub source_pod: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticHistoryResponse {
    pub items: Vec<DiagnosticHistoryItem>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

pub async fn create_diagnostic(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<DiagnoseRequest>,
) -> Result<Json<DiagnoseResponse>, AppError> {
    if req.pod_name.is_empty() {
        return Err(AppError::BadRequest("pod_name is required".to_string()));
    }
    if req.logs.trim().is_empty() {
        return Err(AppError::BadRequest("logs must not be empty".to_string()));
    }

    // Sanitize + hard-truncate before we ever hand the text to an LLM.
    // Order matters: strip control bytes first so the byte-based truncation
    // isn't fooled by embedded terminal escapes inflating the length.
    let sanitized = sanitize_logs(&req.logs);
    let truncated = truncate_logs(&sanitized, LOGS_TRUNCATE_BYTES);
    let context_header = match req.container.as_deref() {
        Some(c) => format!("Pod: {}\nContainer: {}", req.pod_name, c),
        None => format!("Pod: {}", req.pod_name),
    };
    let wrapped_prompt = wrap_prompt(DIAG_PROMPT, &context_header, &truncated);

    let agent = req.agent;
    let ts = jiff::Timestamp::now().as_second();
    let job_name = make_short_name("dw-diag", agent.as_str(), &req.pod_name, ts);
    let cm_name = format!("{job_name}-logs");

    create_logs_configmap(
        &state,
        &ns,
        &cm_name,
        &job_name,
        &req.pod_name,
        agent,
        &wrapped_prompt,
    )
    .await?;

    let job_uid = match create_diag_job(
        &state,
        &ns,
        &job_name,
        &cm_name,
        &req.pod_name,
        req.container.as_deref(),
        agent,
    )
    .await
    {
        Ok(uid) => uid,
        Err(e) => {
            if let Ok(cm_api) = state.configmaps_api(&ns) {
                let _ = cm_api.delete(&cm_name, &Default::default()).await;
            }
            return Err(e);
        }
    };

    // Chain the ConfigMap lifetime to the Job so K8s garbage-collects it
    // when the Job TTL expires. Non-fatal: a warning here is better than
    // failing the diagnostic just because we couldn't stamp an owner ref.
    if let Some(uid) = job_uid {
        if let Err(e) = patch_configmap_owner(&state, &ns, &cm_name, &job_name, &uid).await {
            tracing::warn!(
                cm = cm_name,
                job = job_name,
                error = %e,
                "failed to stamp ownerReference on diagnostic ConfigMap; \
                 it will linger until manually cleaned",
            );
        }
    }

    Ok(Json(DiagnoseResponse {
        job_name,
        status: DiagStatus::Pending,
        agent,
    }))
}

pub async fn get_diagnostic_status(
    State(state): State<AppState>,
    Path((ns, job_name)): Path<(String, String)>,
) -> Result<Json<DiagnosticStatusResponse>, AppError> {
    let jobs_api = state.jobs_api(&ns)?;
    let job = jobs_api.get(&job_name).await?;

    let status = job_status(&job);
    let labels = job.metadata.labels.as_ref();
    // Prefer the annotation (full pod name) over the label (may be truncated).
    let source_pod = job
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("deckwatch.io/source-pod"))
        .cloned()
        .or_else(|| labels.and_then(|l| l.get(DIAG_POD_LABEL_KEY)).cloned());
    let agent = labels
        .and_then(|l| l.get(DIAG_AGENT_LABEL_KEY))
        .and_then(|v| parse_agent(v.as_str()));

    let js = job.status.as_ref();
    let started_at = js
        .and_then(|s| s.start_time.as_ref())
        .map(|t| t.0.to_string());
    let completed_at = js
        .and_then(|s| s.completion_time.as_ref())
        .map(|t| t.0.to_string());
    let message = js
        .and_then(|s| s.conditions.as_ref())
        .and_then(|c| c.iter().find_map(|c| c.message.clone()));

    Ok(Json(DiagnosticStatusResponse {
        job_name,
        status,
        agent,
        source_pod,
        started_at,
        completed_at,
        message,
    }))
}

pub async fn get_diagnostic_result(
    State(state): State<AppState>,
    Path((ns, job_name)): Path<(String, String)>,
) -> Result<Json<DiagnosticResultResponse>, AppError> {
    let jobs_api = state.jobs_api(&ns)?;
    let job = jobs_api.get(&job_name).await?;
    let status = job_status(&job);

    let pod_name = find_job_pod_name(&state, &ns, &job_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("no pod found for diagnostic job {job_name}")))?;

    let pods_api = state.pods_api(&ns)?;
    let log_params = LogParams {
        follow: false,
        timestamps: false,
        ..Default::default()
    };

    let output = pods_api
        .logs(&pod_name, &log_params)
        .await
        .unwrap_or_default();

    Ok(Json(DiagnosticResultResponse {
        job_name,
        status,
        output,
    }))
}

/// Stream the diagnostic Job's pod stdout as it arrives, over Server-Sent
/// Events. Distinct from `logs::stream_logs` because:
///
///   * The client subscribes immediately after `create_diagnostic` returns —
///     the pod may not yet exist and will spend time in Pending while the
///     kubelet pulls the agent image. `stream_logs` refuses Pending outright;
///     here we poll for the pod, then poll its phase, and emit `status`
///     events so the UI can render a "waiting for agent..." affordance.
///   * We emit a `done` event with the final Job status so the client knows
///     to stop expecting more lines (an EventSource can't observe a clean
///     upstream EOF; it just reconnects). The client should call `.close()`
///     when it sees `done`.
pub async fn stream_diagnostic_output(
    State(state): State<AppState>,
    Path((ns, job_name)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    // Confirm the job exists up front so an obviously-bad request 404s cleanly
    // instead of the client staring at a hanging SSE stream.
    let jobs_api = state.jobs_api(&ns)?;
    let _job = jobs_api.get(&job_name).await?;

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(STREAM_CHANNEL_CAPACITY);

    // Everything the driver task needs is cloned in; the AppState clone is
    // cheap (Arc-wrapped clients) and lets the task outlive this handler
    // future once axum has taken the response body.
    let driver_state = state.clone();
    let driver_ns = ns.clone();
    let driver_job = job_name.clone();

    tokio::spawn(async move {
        drive_diag_stream(driver_state, driver_ns, driver_job, tx).await;
    });

    let stream = TrackedSseStream::new(ReceiverStream::new(rx));

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Background driver for the diagnostic SSE stream. Owns the state machine
/// (wait for pod, wait for phase, stream logs, emit terminal event) and
/// forwards each transition as an SSE event. All send errors are swallowed
/// because a full/closed channel just means the client disconnected — nothing
/// left to do.
async fn drive_diag_stream(
    state: AppState,
    ns: String,
    job_name: String,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) {
    let _ = send_status(&tx, "waiting_for_pod", None).await;

    let pod_name = match wait_for_pod(&state, &ns, &job_name).await {
        Ok(Some(name)) => name,
        Ok(None) => {
            let _ = send_error(
                &tx,
                &format!(
                    "timed out after {}s waiting for diagnostic pod to appear",
                    STREAM_POD_WAIT_TIMEOUT.as_secs()
                ),
            )
            .await;
            let _ = send_done(&tx, &state, &ns, &job_name).await;
            return;
        }
        Err(e) => {
            let _ = send_error(&tx, &format!("failed to locate diagnostic pod: {e}")).await;
            let _ = send_done(&tx, &state, &ns, &job_name).await;
            return;
        }
    };

    let _ = send_status(&tx, "pod_found", Some(&pod_name)).await;

    // Wait until the pod is out of Pending. Once it moves to Running,
    // Succeeded, or Failed the kubelet has attached stdout and `logs`
    // will return something (even for a completed pod).
    if let Err(e) = wait_for_pod_started(&state, &ns, &pod_name).await {
        let _ = send_error(&tx, &format!("pod {pod_name} never left Pending: {e}")).await;
        let _ = send_done(&tx, &state, &ns, &job_name).await;
        return;
    }

    let _ = send_status(&tx, "streaming", Some(&pod_name)).await;

    // `follow=true` is safe even if the pod has already terminated — the
    // apiserver just returns the buffered log and closes the stream.
    let pods_api = match state.pods_api(&ns) {
        Ok(api) => api,
        Err(e) => {
            let _ = send_error(&tx, &format!("failed to bind pods API: {e}")).await;
            let _ = send_done(&tx, &state, &ns, &job_name).await;
            return;
        }
    };

    let log_params = LogParams {
        follow: true,
        timestamps: false,
        ..Default::default()
    };

    let log_reader = match pods_api.log_stream(&pod_name, &log_params).await {
        Ok(reader) => reader,
        Err(e) => {
            let _ = send_error(&tx, &format!("failed to open log stream: {e}")).await;
            let _ = send_done(&tx, &state, &ns, &job_name).await;
            return;
        }
    };

    let mut lines = log_reader.lines();
    while let Some(next) = lines.next().await {
        match next {
            Ok(line) => {
                let payload = serde_json::json!({ "line": line });
                let event = Event::default().event("log").data(payload.to_string());
                if tx.send(Ok(event)).await.is_err() {
                    // Client disconnected — stop reading logs.
                    return;
                }
            }
            Err(e) => {
                let _ = send_error(&tx, &format!("log stream error: {e}")).await;
                break;
            }
        }
    }

    let _ = send_done(&tx, &state, &ns, &job_name).await;
}

async fn send_status(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    phase: &str,
    pod: Option<&str>,
) -> Result<(), mpsc::error::SendError<Result<Event, Infallible>>> {
    let payload = match pod {
        Some(p) => serde_json::json!({ "phase": phase, "pod": p }),
        None => serde_json::json!({ "phase": phase }),
    };
    let event = Event::default().event("status").data(payload.to_string());
    tx.send(Ok(event)).await
}

async fn send_error(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    message: &str,
) -> Result<(), mpsc::error::SendError<Result<Event, Infallible>>> {
    let payload = serde_json::json!({ "message": message });
    let event = Event::default().event("error").data(payload.to_string());
    tx.send(Ok(event)).await
}

/// Emit a terminal `done` event with the Job's final status so the client can
/// close its EventSource without waiting on a browser reconnect. Best-effort
/// status lookup — a stale/deleted job just reports "pending".
async fn send_done(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    state: &AppState,
    ns: &str,
    job_name: &str,
) -> Result<(), mpsc::error::SendError<Result<Event, Infallible>>> {
    let status = match state.jobs_api(ns) {
        Ok(api) => match api.get(job_name).await {
            Ok(job) => job_status(&job),
            Err(_) => DiagStatus::Pending,
        },
        Err(_) => DiagStatus::Pending,
    };
    let status_str = match status {
        DiagStatus::Pending => "pending",
        DiagStatus::Running => "running",
        DiagStatus::Succeeded => "succeeded",
        DiagStatus::Failed => "failed",
    };
    let payload = serde_json::json!({ "status": status_str });
    let event = Event::default().event("done").data(payload.to_string());
    tx.send(Ok(event)).await
}

/// Locate the (most recent) pod for a Job by its `job-name` label. Returns
/// `None` when no pods exist yet — the caller decides whether to retry.
async fn find_job_pod_name(
    state: &AppState,
    ns: &str,
    job_name: &str,
) -> Result<Option<String>, AppError> {
    let pods_api = state.pods_api(ns)?;
    let lp = ListParams::default().labels(&format!("job-name={job_name}"));
    let pods = pods_api.list(&lp).await?;
    let name = pods
        .iter()
        .max_by_key(|p| {
            p.status
                .as_ref()
                .and_then(|s| s.start_time.as_ref())
                .map(|t| t.0)
        })
        .map(|p| p.name_any());
    Ok(name)
}

async fn wait_for_pod(
    state: &AppState,
    ns: &str,
    job_name: &str,
) -> Result<Option<String>, AppError> {
    let deadline = tokio::time::Instant::now() + STREAM_POD_WAIT_TIMEOUT;
    loop {
        if let Some(name) = find_job_pod_name(state, ns, job_name).await? {
            return Ok(Some(name));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(STREAM_POD_WAIT_INTERVAL).await;
    }
}

/// Wait for the pod to leave Pending. Runs on the same deadline as
/// `wait_for_pod` because slow image pulls happen here, not before the pod
/// object exists.
async fn wait_for_pod_started(state: &AppState, ns: &str, pod_name: &str) -> Result<(), AppError> {
    let pods_api = state.pods_api(ns)?;
    let deadline = tokio::time::Instant::now() + STREAM_POD_WAIT_TIMEOUT;
    loop {
        let pod: Pod = pods_api.get(pod_name).await?;
        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_deref())
            .unwrap_or("Unknown");
        if phase != "Pending" {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::BadRequest(format!(
                "pod {pod_name} still Pending after {}s",
                STREAM_POD_WAIT_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(STREAM_POD_WAIT_INTERVAL).await;
    }
}

/// Stream wrapper that tracks active SSE connections. Mirror of the
/// `logs::stream_logs` guard — increments the gauge on construction,
/// decrements on drop (regardless of how the stream ends).
struct TrackedSseStream<S> {
    inner: S,
}

impl<S> TrackedSseStream<S> {
    fn new(inner: S) -> Self {
        metrics::sse_opened();
        Self { inner }
    }
}

impl<S> Drop for TrackedSseStream<S> {
    fn drop(&mut self) {
        metrics::sse_closed();
    }
}

impl<S, T> Stream for TrackedSseStream<S>
where
    S: Stream<Item = T> + Unpin,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

pub async fn list_diagnostics(
    State(state): State<AppState>,
    Path(ns): Path<String>,
) -> Result<Json<DiagnosticHistoryResponse>, AppError> {
    let jobs_api = state.jobs_api(&ns)?;
    let lp = ListParams::default().labels(&format!("{DIAG_LABEL_KEY}=true"));
    let jobs = jobs_api.list(&lp).await?;

    let mut items: Vec<DiagnosticHistoryItem> = jobs
        .iter()
        .map(|job| {
            let labels = job.metadata.labels.as_ref();
            let agent = labels
                .and_then(|l| l.get(DIAG_AGENT_LABEL_KEY))
                .and_then(|v| parse_agent(v.as_str()));
            // Prefer annotation (full pod name) over label (may be truncated).
            let source_pod = job
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("deckwatch.io/source-pod"))
                .cloned()
                .or_else(|| labels.and_then(|l| l.get(DIAG_POD_LABEL_KEY)).cloned());
            let js = job.status.as_ref();
            let started_at = js
                .and_then(|s| s.start_time.as_ref())
                .map(|t| t.0.to_string());
            let completed_at = js
                .and_then(|s| s.completion_time.as_ref())
                .map(|t| t.0.to_string());
            let created_at = job
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0.to_string());

            DiagnosticHistoryItem {
                job_name: job.name_any(),
                status: job_status(job),
                agent,
                source_pod,
                started_at,
                completed_at,
                created_at,
            }
        })
        .collect();

    // Newest first — the UI's "history" view is much more useful when
    // recent diagnostics are on top; fall back to job_name for stability
    // when timestamps are equal or missing.
    items.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.job_name.cmp(&a.job_name))
    });

    Ok(Json(DiagnosticHistoryResponse { items }))
}

fn job_status(job: &Job) -> DiagStatus {
    let s = match job.status.as_ref() {
        Some(s) => s,
        None => return DiagStatus::Pending,
    };
    let succeeded = s.succeeded.unwrap_or(0);
    let failed = s.failed.unwrap_or(0);
    let active = s.active.unwrap_or(0);
    if succeeded > 0 {
        DiagStatus::Succeeded
    } else if failed > 0 {
        DiagStatus::Failed
    } else if active > 0 {
        DiagStatus::Running
    } else {
        DiagStatus::Pending
    }
}

async fn create_logs_configmap(
    state: &AppState,
    ns: &str,
    cm_name: &str,
    job_name: &str,
    pod_name: &str,
    agent: DiagAgent,
    wrapped_prompt: &str,
) -> Result<(), AppError> {
    let mut labels = BTreeMap::new();
    labels.insert(DIAG_LABEL_KEY.to_string(), "true".to_string());
    labels.insert(DIAG_AGENT_LABEL_KEY.to_string(), agent.as_str().to_string());
    labels.insert(
        DIAG_POD_LABEL_KEY.to_string(),
        truncate_label_value(&sanitize_name_segment(pod_name)),
    );
    labels.insert(
        "deckwatch.io/diagnostic-job".to_string(),
        job_name.to_string(),
    );

    // Store the full (un-truncated) pod name in an annotation so it's
    // always recoverable even when the label was shortened to fit the
    // 63-char K8s limit.
    let mut annotations = BTreeMap::new();
    annotations.insert("deckwatch.io/source-pod".to_string(), pod_name.to_string());

    // The whole prompt (task + fenced sanitized logs) lives as a single
    // ConfigMap key so the in-cluster shell script doesn't need to
    // reconstruct the fence. That eliminates a shell-injection surface —
    // we no longer interpolate the prompt into the command line.
    let mut data = BTreeMap::new();
    data.insert(LOGS_FILE_NAME.to_string(), wrapped_prompt.to_string());
    data.insert("source_pod".to_string(), pod_name.to_string());

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(cm_name.to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let cm_api = state.configmaps_api(ns)?;
    cm_api.create(&PostParams::default(), &cm).await?;
    Ok(())
}

async fn create_diag_job(
    state: &AppState,
    ns: &str,
    job_name: &str,
    cm_name: &str,
    pod_name: &str,
    container: Option<&str>,
    agent: DiagAgent,
) -> Result<Option<String>, AppError> {
    let mut labels = BTreeMap::new();
    labels.insert(DIAG_LABEL_KEY.to_string(), "true".to_string());
    labels.insert(DIAG_AGENT_LABEL_KEY.to_string(), agent.as_str().to_string());
    labels.insert(
        DIAG_POD_LABEL_KEY.to_string(),
        truncate_label_value(&sanitize_name_segment(pod_name)),
    );

    let mut annotations = BTreeMap::new();
    annotations.insert("deckwatch.io/source-pod".to_string(), pod_name.to_string());

    let mut env = vec![
        EnvVar {
            name: agent.api_key_env().to_string(),
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: agent.api_key_secret(),
                    key: "api-key".to_string(),
                    optional: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_DIAG_POD".to_string(),
            value: Some(pod_name.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_DIAG_PROMPT_PATH".to_string(),
            value: Some(format!("{LOGS_MOUNT_DIR}/{LOGS_FILE_NAME}")),
            ..Default::default()
        },
    ];

    if let Some(c) = container {
        env.push(EnvVar {
            name: "DECKWATCH_DIAG_CONTAINER".to_string(),
            value: Some(c.to_string()),
            ..Default::default()
        });
    }

    // Agent CLI selection. Note: `claude` runs WITHOUT
    // `--dangerously-skip-permissions` — that flag lets the agent modify
    // files, execute shell commands, and follow instructions in the
    // prompt without prompting. For a diagnose-only workflow that just
    // reads logs and prints back a diagnosis, the default (no tool use)
    // is strictly safer against prompt-injection payloads. See
    // docs/AI_SAFETY.md for the discussion.
    // The prompt is piped in from a file, never interpolated into the
    // shell command. That's why the script here just cats and pipes —
    // no `printf "$VAR"` with prompt content.
    //
    // Claude: installed on-the-fly via `npx` from a Node.js base image
    // (the ghcr.io/anthropics/claude-code image is not publicly pullable).
    // Codex: expected to be pre-installed in the image.
    let shell_script = match agent {
        DiagAgent::Claude => format!(
            r#"set -eu
if ! command -v npx >/dev/null 2>&1; then
  echo "error: npx not found in image PATH — is this a Node.js base image?" >&2
  exit 127
fi
cat "$DECKWATCH_DIAG_PROMPT_PATH" | npx -y @anthropic-ai/claude-code@latest --print
"#
        ),
        DiagAgent::Codex => {
            let cli = "codex";
            let cli_flags = "exec --sandbox read-only --";
            format!(
                r#"set -eu
if ! command -v {cli} >/dev/null 2>&1; then
  echo "error: {cli} CLI not found in image PATH" >&2
  exit 127
fi
cat "$DECKWATCH_DIAG_PROMPT_PATH" | {cli} {cli_flags}
"#
            )
        }
    };

    let container_spec = Container {
        name: "agent".to_string(),
        image: Some(agent.image()),
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string()]),
        args: Some(vec![shell_script]),
        env: Some(env),
        volume_mounts: Some(vec![VolumeMount {
            name: "diag-logs".to_string(),
            mount_path: LOGS_MOUNT_DIR.to_string(),
            read_only: Some(true),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(JobSpec {
            ttl_seconds_after_finished: Some(3600),
            backoff_limit: Some(0),
            active_deadline_seconds: Some(600),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some({
                        let mut l = BTreeMap::new();
                        l.insert(DIAG_LABEL_KEY.to_string(), "true".to_string());
                        l.insert(DIAG_AGENT_LABEL_KEY.to_string(), agent.as_str().to_string());
                        l
                    }),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    containers: vec![container_spec],
                    volumes: Some(vec![Volume {
                        name: "diag-logs".to_string(),
                        config_map: Some(ConfigMapVolumeSource {
                            name: cm_name.to_string(),
                            optional: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let jobs_api = state.jobs_api(ns)?;
    let created = jobs_api.create(&PostParams::default(), &job).await?;
    Ok(created.metadata.uid)
}

async fn patch_configmap_owner(
    state: &AppState,
    ns: &str,
    cm_name: &str,
    job_name: &str,
    job_uid: &str,
) -> Result<(), AppError> {
    // block_owner_deletion=false so a foreground-deletion on the Job
    // doesn't stall on the CM; controller=false because the Job doesn't
    // "reconcile" the CM in the controller sense — GC is all we want.
    let owner = OwnerReference {
        api_version: "batch/v1".to_string(),
        kind: "Job".to_string(),
        name: job_name.to_string(),
        uid: job_uid.to_string(),
        controller: Some(false),
        block_owner_deletion: Some(false),
    };

    let patch = serde_json::json!({
        "metadata": {
            "ownerReferences": [owner],
        }
    });

    let cm_api = state.configmaps_api(ns)?;
    cm_api
        .patch(cm_name, &PatchParams::default(), &Patch::Merge(patch))
        .await?;
    Ok(())
}

fn truncate_logs(logs: &str, max_bytes: usize) -> String {
    if logs.len() <= max_bytes {
        return logs.to_string();
    }
    // Prefer keeping the tail of the log (usually the failure).
    let start = logs.len() - max_bytes;
    let mut boundary = start;
    while boundary < logs.len() && !logs.is_char_boundary(boundary) {
        boundary += 1;
    }
    let mut out = String::with_capacity(max_bytes + 128);
    out.push_str("[...truncated to last ");
    out.push_str(&max_bytes.to_string());
    out.push_str(" bytes...]\n");
    out.push_str(&logs[boundary..]);
    out
}

fn sanitize_name_segment(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    let s = if trimmed.is_empty() { "pod" } else { trimmed };
    if s.len() > 40 {
        s[..40].to_string()
    } else {
        s.to_string()
    }
}

/// Truncate a string to fit within the K8s label value limit (63 chars).
/// Trims trailing dashes so the result is a valid label value.
fn truncate_label_value(s: &str) -> String {
    if s.len() <= K8S_LABEL_VALUE_MAX {
        return s.to_string();
    }
    s[..K8S_LABEL_VALUE_MAX].trim_end_matches('-').to_string()
}

/// Build a short, K8s-safe resource name that fits within the 63-char label
/// value limit. The full source identifier (pod name, app name) is stored in
/// annotations by the caller; the resource name only needs to be unique and
/// recognizable in `kubectl get jobs` output.
///
/// Format: `{prefix}-{agent}-{hash}` where hash is an 8-char hex digest
/// derived from the source identifier + timestamp. The longest possible
/// result is `dw-diag-claude-xxxxxxxx` (24 chars), well under 63.
fn make_short_name(prefix: &str, agent: &str, source: &str, ts: i64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    ts.hash(&mut hasher);
    let hash = hasher.finish();
    let short_hash = format!("{:016x}", hash);

    // Use only the first 8 hex chars for brevity; collision risk is
    // negligible in practice (different ts values for the same pod).
    format!("{}-{}-{}", prefix, agent, &short_hash[..8])
}
