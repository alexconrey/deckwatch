use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::anthropic::AnthropicClient;
use crate::error::AppError;
use crate::handlers::settings::{load_settings_from_db, AiProviderConfig};
use crate::log_sanitize::{sanitize_logs, wrap_prompt};
use crate::metrics;
use crate::state::AppState;

const DIAG_PROMPT: &str =
    "Analyze these Kubernetes pod logs and diagnose the issue. Suggest fixes.";
const LOGS_TRUNCATE_BYTES: usize = 256 * 1024;

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
}

#[cfg(test)]
fn parse_agent(v: &str) -> Option<DiagAgent> {
    match v {
        "claude" => Some(DiagAgent::Claude),
        "codex" => Some(DiagAgent::Codex),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub enum DiagStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

/// Read a single key from a Kubernetes Secret.
async fn read_secret_key(
    state: &AppState,
    ns: &str,
    secret_name: &str,
    data_key: &str,
) -> Result<String, AppError> {
    let secrets_api = state.secrets_api(ns)?;
    let secret = secrets_api.get(secret_name).await.map_err(|_| {
        AppError::BadRequest(format!(
            "Secret '{secret_name}' not found in namespace '{ns}'. \
             Create it with: kubectl -n {ns} create secret generic {secret_name} \
             --from-literal={data_key}=<value>"
        ))
    })?;
    let value = secret
        .data
        .as_ref()
        .and_then(|d| d.get(data_key))
        .map(|v| String::from_utf8_lossy(&v.0).to_string())
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Secret '{secret_name}' is missing the '{data_key}' key"
            ))
        })?;
    Ok(value)
}

/// Try to read a credential from the encrypted DB store first; fall back to
/// the Kubernetes Secret if no DB credential is configured (or the encryption
/// key is not set). This lets operators manage API keys via the Settings UI
/// while remaining backward-compatible with existing Secret-based deployments.
async fn read_credential(
    state: &AppState,
    ns: &str,
    credential_type: &str,
) -> Result<String, AppError> {
    let settings = load_settings_from_db(state).await;
    if let Some(creds) = &settings.credentials {
        let encrypted = match credential_type {
            "anthropic" => creds.anthropic_api_key.as_deref(),
            "gcp_sa" => creds.gcp_sa_key.as_deref(),
            _ => None,
        };
        if let Some(enc) = encrypted {
            if let Ok(decrypted) = crate::crypto::decrypt(&state.encryption_key, enc) {
                return Ok(decrypted);
            }
        }
    }
    // Fallback to K8s Secret.
    let (secret_name, data_key) = match credential_type {
        "anthropic" => ("deckwatch-anthropic-api-key", "api-key"),
        "gcp_sa" => ("deckwatch-gcp-sa-key", "gcp-sa-key"),
        _ => {
            return Err(AppError::BadRequest(format!(
                "Unknown credential type: {credential_type}"
            )))
        }
    };
    read_secret_key(state, ns, secret_name, data_key).await
}

/// Build an `AnthropicClient` configured for the provider selected in settings.
async fn build_ai_client(state: &AppState, ns: &str) -> Result<AnthropicClient, AppError> {
    let settings = load_settings_from_db(state).await;
    match &settings.ai_provider {
        AiProviderConfig::Native { api_key_secret } => {
            // Try DB-stored encrypted credential first, then env-var override,
            // then the configured K8s Secret name.
            let key = match read_credential(state, ns, "anthropic").await {
                Ok(k) => k,
                Err(_) => {
                    let secret_name = std::env::var("DECKWATCH_DIAG_CLAUDE_SECRET")
                        .unwrap_or_else(|_| api_key_secret.clone());
                    read_secret_key(state, ns, &secret_name, "api-key").await?
                }
            };
            Ok(AnthropicClient::native(key))
        }
        AiProviderConfig::VertexAi {
            project_id,
            region,
            sa_key_secret,
        } => {
            let sa_json = match read_credential(state, ns, "gcp_sa").await {
                Ok(k) => k,
                Err(_) => read_secret_key(state, ns, sa_key_secret, "gcp-sa-key").await?,
            };
            let token = crate::anthropic::exchange_sa_key_for_token(&sa_json)
                .await
                .map_err(|e| AppError::BadRequest(format!("GCP token exchange failed: {e}")))?;
            Ok(AnthropicClient::vertex(
                project_id.clone(),
                region.clone(),
                token,
            ))
        }
        AiProviderConfig::Bedrock { region, model_id } => {
            // Bedrock uses IRSA -- no secret needed, just region + model.
            Ok(AnthropicClient::bedrock(region.clone(), model_id.clone()))
        }
    }
}

/// Create a diagnostic and stream the AI response back via SSE.
///
/// Replaces the previous K8s Job-based approach with a direct Anthropic
/// Messages API call. The frontend SSE contract is preserved: events are
/// `status`, `log`, `error`, and `done`.
pub async fn create_diagnostic(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Json(req): Json<DiagnoseRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    if req.pod_name.is_empty() {
        return Err(AppError::BadRequest("pod_name is required".to_string()));
    }
    if req.logs.trim().is_empty() {
        return Err(AppError::BadRequest("logs must not be empty".to_string()));
    }

    if req.agent == DiagAgent::Codex {
        return Err(AppError::BadRequest(
            "Codex agent is not yet supported with direct API mode".to_string(),
        ));
    }

    // Build the AI client before we start streaming so a missing secret
    // returns a clean JSON error rather than an SSE error event.
    let ai_client = build_ai_client(&state, &ns).await?;

    // Sanitize + hard-truncate before we ever hand the text to an LLM.
    let sanitized = sanitize_logs(&req.logs);
    let truncated = truncate_logs(&sanitized, LOGS_TRUNCATE_BYTES);
    let context_header = match req.container.as_deref() {
        Some(c) => format!("Pod: {}\nContainer: {}", req.pod_name, c),
        None => format!("Pod: {}", req.pod_name),
    };
    let prompt = wrap_prompt(DIAG_PROMPT, &context_header, &truncated);

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(STREAM_CHANNEL_CAPACITY);

    let pod_name = req.pod_name.clone();
    let agent = req.agent;

    // Spawn the API call in a background task so the SSE response is
    // returned immediately.
    tokio::spawn(async move {
        let _ = send_status(&tx, "streaming", Some(&pod_name)).await;

        let client = ai_client;
        let (text_tx, mut text_rx) = mpsc::channel::<String>(STREAM_CHANNEL_CAPACITY);

        // Spawn the streaming API call.
        let stream_handle =
            tokio::spawn(async move { client.message_stream(&prompt, text_tx).await });

        // Forward text chunks as SSE `log` events.
        while let Some(chunk) = text_rx.recv().await {
            let payload = serde_json::json!({ "line": chunk });
            let event = Event::default().event("log").data(payload.to_string());
            if tx.send(Ok(event)).await.is_err() {
                // Client disconnected.
                return;
            }
        }

        // Check if the stream task finished with an error.
        match stream_handle.await {
            Ok(Ok(())) => {
                let _ = send_done_direct(&tx, "succeeded").await;
            }
            Ok(Err(e)) => {
                let _ = send_error(&tx, &format!("Anthropic API error: {e}")).await;
                let _ = send_done_direct(&tx, "failed").await;
            }
            Err(e) => {
                let _ = send_error(&tx, &format!("streaming task panicked: {e}")).await;
                let _ = send_done_direct(&tx, "failed").await;
            }
        }

        tracing::info!(
            pod = %pod_name,
            agent = agent.as_str(),
            "diagnostic streaming completed"
        );
    });

    let stream = TrackedSseStream::new(ReceiverStream::new(rx));

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Get diagnostic status. With direct API mode, diagnostics are ephemeral
/// SSE streams and don't have persistent Job state. This endpoint is kept
/// for backward compatibility but returns a not-found error since there
/// are no longer K8s Jobs to query.
pub async fn get_diagnostic_status(
    State(_state): State<AppState>,
    Path((_ns, job_name)): Path<(String, String)>,
) -> Result<Json<DiagnosticStatusResponse>, AppError> {
    // In direct API mode, diagnostics are streamed inline and don't
    // persist as Jobs. Return a minimal response for compatibility.
    Err(AppError::NotFound(format!(
        "diagnostic '{job_name}' not found (diagnostics are now streamed inline)"
    )))
}

/// Get diagnostic result. With direct API mode, the result is delivered
/// inline via SSE and not persisted as a Job's pod logs.
pub async fn get_diagnostic_result(
    State(_state): State<AppState>,
    Path((_ns, job_name)): Path<(String, String)>,
) -> Result<Json<DiagnosticResultResponse>, AppError> {
    Err(AppError::NotFound(format!(
        "diagnostic '{job_name}' not found (diagnostics are now streamed inline)"
    )))
}

/// Stream diagnostic output. With direct API mode, this is no longer
/// needed as `create_diagnostic` itself returns an SSE stream. Kept for
/// backward compatibility; returns an immediate "done" event.
pub async fn stream_diagnostic_output(
    State(_state): State<AppState>,
    Path((_ns, job_name)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(1);
    tokio::spawn(async move {
        let _ = send_error(
            &tx,
            &format!(
                "diagnostic '{job_name}' not found; \
                 diagnostics are now streamed inline via POST"
            ),
        )
        .await;
        let _ = send_done_direct(&tx, "failed").await;
    });

    let stream = TrackedSseStream::new(ReceiverStream::new(rx));
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// List diagnostics. With direct API mode, there are no K8s Jobs to list.
/// Returns an empty history.
pub async fn list_diagnostics(
    State(_state): State<AppState>,
    Path(_ns): Path<String>,
) -> Result<Json<DiagnosticHistoryResponse>, AppError> {
    // Direct API mode: diagnostics are ephemeral SSE streams, not
    // persisted Jobs. Return empty history for backward compatibility.
    Ok(Json(DiagnosticHistoryResponse { items: vec![] }))
}

// ---- SSE helpers ----

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

/// Emit a terminal `done` event with a status string so the client can
/// close its EventSource.
async fn send_done_direct(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    status: &str,
) -> Result<(), mpsc::error::SendError<Result<Event, Infallible>>> {
    let payload = serde_json::json!({ "status": status });
    let event = Event::default().event("done").data(payload.to_string());
    tx.send(Ok(event)).await
}

// ---- Stream wrapper for metrics ----

/// Stream wrapper that tracks active SSE connections. Increments the gauge
/// on construction, decrements on drop.
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

// ---- Utility functions ----

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

#[cfg(test)]
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

/// Maximum length for Kubernetes label values.
#[cfg(test)]
const K8S_LABEL_VALUE_MAX: usize = 63;

/// Truncate a string to fit within the K8s label value limit (63 chars).
#[cfg(test)]
fn truncate_label_value(s: &str) -> String {
    if s.len() <= K8S_LABEL_VALUE_MAX {
        return s.to_string();
    }
    s[..K8S_LABEL_VALUE_MAX].trim_end_matches('-').to_string()
}

/// Build a short, K8s-safe resource name. Used for diagnostic identifiers.
#[cfg(test)]
fn make_short_name(prefix: &str, agent: &str, source: &str, ts: i64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    ts.hash(&mut hasher);
    let hash = hasher.finish();
    let short_hash = format!("{:016x}", hash);

    format!("{}-{}-{}", prefix, agent, &short_hash[..8])
}

// ========================================================================
// Legacy K8s Job-based implementation (kept for reference / revert).
//
// The following functions were the original Job lifecycle management:
//   - create_logs_configmap
//   - create_diag_job
//   - find_job_pod_name
//   - wait_for_pod
//   - wait_for_pod_started
//   - patch_configmap_owner
//   - drive_diag_stream
//   - send_done (Job-aware variant)
//   - job_status
//
// They have been replaced by direct Anthropic API calls. If you need to
// revert to the Job-based approach, check the git history for the version
// prior to the "feat: replace K8s Job diagnostics with direct Anthropic
// API calls" commit.
// ========================================================================

#[cfg(test)]
#[path = "../handlers_diagnostics_tests.rs"]
mod handlers_diagnostics_tests;
