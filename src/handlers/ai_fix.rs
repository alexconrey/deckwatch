use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use kube::api::ListParams;
use kube::api::LogParams;
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::anthropic::AnthropicClient;
use crate::error::AppError;
use crate::handlers::diagnostics::{DiagAgent, DiagStatus};
use crate::kube_ext::ApplicationGitConfig;
use crate::log_sanitize::{sanitize_logs, wrap_prompt};
use crate::state::AppState;

// AI-fix reuses the diagnostics status/agent enums so the frontend can
// share UI components for both flows.

const APP_CM_DATA_KEY: &str = "application";
const APP_LABEL: &str = "deckwatch.io/application";

const DEFAULT_CLAUDE_SECRET: &str = "deckwatch-anthropic-api-key";

const AI_FIX_PROMPT: &str = "You are reviewing a Kubernetes application that is managed by Deckwatch. \
Read the context provided (deployment status, crash logs, repository info), identify issues that break \
Kubernetes/Deckwatch compatibility (Dockerfile problems, missing health endpoints, container port \
mismatches, misconfigured probes, image build issues, resource requests, secrets/env expectations), \
and propose concrete file-level fixes. Prefer minimal, surgical edits. Explain WHY each change is \
needed. Print the diagnosis and suggested changes.";

// Bound the crash-log snippet. Same rationale as diagnostics.rs.
const CRASH_LOG_TAIL_BYTES: usize = 32 * 1024;
const CRASH_LOG_MAX_PODS: usize = 3;

const STREAM_CHANNEL_CAPACITY: usize = 64;

fn cm_name(app: &str) -> String {
    format!("deckwatch-app-{app}")
}

fn member_selector(app_name: &str) -> String {
    format!("{APP_LABEL}={app_name}")
}

#[derive(Debug, Deserialize)]
pub struct AiFixRequest {
    pub agent: DiagAgent,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct AiFixResponse {
    pub job_name: String,
    pub status: DiagStatus,
    pub agent: DiagAgent,
}

#[derive(Serialize, Deserialize)]
struct ApplicationData {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    git: Option<ApplicationGitConfig>,
}

/// Read the Anthropic API key from the K8s Secret.
async fn read_api_key(state: &AppState, ns: &str) -> Result<String, AppError> {
    let secret_name = std::env::var("DECKWATCH_AIFIX_CLAUDE_SECRET")
        .or_else(|_| std::env::var("DECKWATCH_DIAG_CLAUDE_SECRET"))
        .unwrap_or_else(|_| DEFAULT_CLAUDE_SECRET.to_string());
    let secrets_api = state.secrets_api(ns)?;
    let secret = secrets_api.get(&secret_name).await.map_err(|_| {
        AppError::BadRequest(format!(
            "Anthropic API key secret not found. Create it with: \
             kubectl create secret generic {secret_name} --from-literal=api-key=sk-ant-..."
        ))
    })?;
    let key = secret
        .data
        .as_ref()
        .and_then(|d| d.get("api-key"))
        .map(|v| String::from_utf8_lossy(&v.0).to_string())
        .ok_or_else(|| AppError::BadRequest("Secret missing 'api-key' key".to_string()))?;
    Ok(key)
}

/// Create an AI fix diagnostic and stream the response via SSE.
///
/// Replaces the previous K8s Job-based approach with a direct Anthropic
/// Messages API call. Gathers crash logs + deployment context (same logic
/// as before), builds the prompt, and streams the AI response back.
pub async fn create_ai_fix(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<AiFixRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    if req.agent == DiagAgent::Codex {
        return Err(AppError::BadRequest(
            "Codex agent is not yet supported with direct API mode".to_string(),
        ));
    }

    // 1. Load the application record from its managed ConfigMap.
    let cm_api = state.configmaps_api(&ns)?;
    let app_cm = cm_api.get(&cm_name(&name)).await?;
    let app_data: ApplicationData = app_cm
        .data
        .as_ref()
        .and_then(|d| d.get(APP_CM_DATA_KEY))
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| {
            AppError::BadRequest(format!("configmap for application '{name}' is malformed"))
        })?;

    let git = app_data.git.as_ref().ok_or_else(|| {
        AppError::BadRequest(format!(
            "application '{name}' has no git configuration -- enable GitOps first"
        ))
    })?;

    let branch = git.branch.clone().unwrap_or_else(|| "main".to_string());

    // 2. Read API key.
    let api_key = read_api_key(&state, &ns).await?;

    // 3. Gather recent crash-log snippets from the app's member pods.
    let crash_logs = gather_crash_logs(&state, &ns, &name)
        .await
        .unwrap_or_default();

    // 4. Deployment status summary.
    let deployment_status = summarize_deployments(&state, &ns, &name)
        .await
        .unwrap_or_default();

    // 5. Build the prompt.
    let context_md = build_context_markdown(
        &name,
        &ns,
        &app_data.description,
        &git.repo_url,
        &branch,
        &crash_logs,
        &deployment_status,
    );

    let agent = req.agent;
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(STREAM_CHANNEL_CAPACITY);

    // 6. Spawn the API call in a background task.
    tokio::spawn(async move {
        // Send initial status.
        let payload = serde_json::json!({ "phase": "streaming", "app": name });
        let event = Event::default().event("status").data(payload.to_string());
        let _ = tx.send(Ok(event)).await;

        let client = AnthropicClient::new(api_key);
        let (text_tx, mut text_rx) = mpsc::channel::<String>(STREAM_CHANNEL_CAPACITY);

        let stream_handle =
            tokio::spawn(async move { client.message_stream(&context_md, text_tx).await });

        // Forward text chunks as SSE `log` events.
        while let Some(chunk) = text_rx.recv().await {
            let payload = serde_json::json!({ "line": chunk });
            let event = Event::default().event("log").data(payload.to_string());
            if tx.send(Ok(event)).await.is_err() {
                return;
            }
        }

        // Check stream result.
        match stream_handle.await {
            Ok(Ok(())) => {
                let payload = serde_json::json!({ "status": "succeeded" });
                let event = Event::default().event("done").data(payload.to_string());
                let _ = tx.send(Ok(event)).await;
            }
            Ok(Err(e)) => {
                let payload = serde_json::json!({ "message": format!("Anthropic API error: {e}") });
                let event = Event::default().event("error").data(payload.to_string());
                let _ = tx.send(Ok(event)).await;
                let done_payload = serde_json::json!({ "status": "failed" });
                let done_event = Event::default()
                    .event("done")
                    .data(done_payload.to_string());
                let _ = tx.send(Ok(done_event)).await;
            }
            Err(e) => {
                let payload =
                    serde_json::json!({ "message": format!("streaming task panicked: {e}") });
                let event = Event::default().event("error").data(payload.to_string());
                let _ = tx.send(Ok(event)).await;
                let done_payload = serde_json::json!({ "status": "failed" });
                let done_event = Event::default()
                    .event("done")
                    .data(done_payload.to_string());
                let _ = tx.send(Ok(done_event)).await;
            }
        }

        tracing::info!(
            app = %name,
            agent = agent.as_str(),
            "AI fix streaming completed"
        );
    });

    let stream = ReceiverStream::new(rx);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

// ---- Context gathering (kept from original) ----

async fn gather_crash_logs(state: &AppState, ns: &str, app_name: &str) -> Result<String, AppError> {
    let pods_api = state.pods_api(ns)?;
    let lp = ListParams::default().labels(&member_selector(app_name));
    let pods = pods_api.list(&lp).await?;

    let mut ranked: Vec<_> = pods.iter().collect();
    ranked.sort_by_key(|p| {
        let restarts: i32 = p
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .map(|cs| cs.iter().map(|c| c.restart_count).sum())
            .unwrap_or(0);
        std::cmp::Reverse(restarts)
    });

    let mut out = String::new();
    for pod in ranked.into_iter().take(CRASH_LOG_MAX_PODS) {
        let pod_name = pod.name_any();
        let restarts: i32 = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .map(|cs| cs.iter().map(|c| c.restart_count).sum())
            .unwrap_or(0);

        let log_params = LogParams {
            follow: false,
            timestamps: false,
            previous: restarts > 0,
            tail_lines: Some(400),
            ..Default::default()
        };

        let logs = pods_api
            .logs(&pod_name, &log_params)
            .await
            .unwrap_or_default();
        if logs.trim().is_empty() {
            continue;
        }

        let sanitized = sanitize_logs(&logs);
        let tail = truncate_tail(&sanitized, CRASH_LOG_TAIL_BYTES);
        out.push_str(&format!(
            "\n### Pod `{pod_name}` (restarts: {restarts})\n\n```\n{tail}\n```\n"
        ));
    }
    Ok(out)
}

async fn summarize_deployments(
    state: &AppState,
    ns: &str,
    app_name: &str,
) -> Result<String, AppError> {
    let dep_api = state.deployments_api(ns)?;
    let lp = ListParams::default().labels(&member_selector(app_name));
    let deps = dep_api.list(&lp).await?;

    if deps.items.is_empty() {
        return Ok("(no deployments)".to_string());
    }

    let mut out = String::new();
    for d in deps.iter() {
        let dn = d.name_any();
        let spec = d.spec.as_ref();
        let desired = spec.and_then(|s| s.replicas).unwrap_or(0);
        let status = d.status.as_ref();
        let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
        let available = status.and_then(|s| s.available_replicas).unwrap_or(0);
        let image = spec
            .and_then(|s| s.template.spec.as_ref())
            .and_then(|ps| ps.containers.first())
            .and_then(|c| c.image.clone())
            .unwrap_or_else(|| "(unknown)".to_string());
        out.push_str(&format!(
            "- `{dn}`: {ready}/{desired} ready, {available} available, image=`{image}`\n"
        ));
    }
    Ok(out)
}

fn build_context_markdown(
    app_name: &str,
    ns: &str,
    description: &str,
    repo_url: &str,
    branch: &str,
    crash_logs: &str,
    deployment_status: &str,
) -> String {
    let desc = if description.is_empty() {
        "(no description)"
    } else {
        description
    };
    let crash_section = if crash_logs.trim().is_empty() {
        "(no recent crash logs collected)".to_string()
    } else {
        crash_logs.to_string()
    };

    let untrusted_body = format!(
        "**Application:** `{app_name}`\n\
         **Namespace:** `{ns}`\n\
         **Description:** {desc}\n\
         **Repository:** {repo_url}\n\
         **Branch:** {branch}\n\
         \n\
         ## Deployment status\n\
         \n\
         {deployment_status}\n\
         \n\
         ## Recent pod crash logs\n\
         {crash_section}\n"
    );

    let header = format!("# Deckwatch AI Fix Context\n\nApplication: {app_name}\nNamespace: {ns}");

    wrap_prompt(AI_FIX_PROMPT, &header, &untrusted_body)
}

// ---- Utility functions ----

fn truncate_tail(logs: &str, max_bytes: usize) -> String {
    if logs.len() <= max_bytes {
        return logs.to_string();
    }
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
    let s = if trimmed.is_empty() { "app" } else { trimmed };
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

/// Build a short, K8s-safe resource name. Same scheme as diagnostics.rs.
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
//   - create_context_configmap
//   - create_ai_fix_job
//   - agent_image, agent_api_key_env, agent_api_key_secret
//
// They have been replaced by direct Anthropic API calls. If you need to
// revert to the Job-based approach, check the git history for the version
// prior to the "feat: replace K8s Job diagnostics with direct Anthropic
// API calls" commit.
// ========================================================================

#[cfg(test)]
#[path = "../handlers_ai_fix_tests.rs"]
mod handlers_ai_fix_tests;
