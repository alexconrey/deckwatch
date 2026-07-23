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

use sea_orm::EntityTrait;

use crate::anthropic::AnthropicClient;
use crate::entities::applications;
use crate::error::AppError;
use crate::handlers::diagnostics::{DiagAgent, DiagStatus};
use crate::handlers::settings::{load_settings_from_db, AiProviderConfig};
use crate::kube_ext::ApplicationGitConfig;
use crate::log_sanitize::{sanitize_logs, wrap_prompt};
use crate::state::AppState;

const APP_LABEL: &str = "deckwatch.io/application";

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

/// Read a single key from a Kubernetes Secret.
async fn read_secret_key(
    state: &AppState,
    ns: &str,
    secret_name: &str,
    data_key: &str,
) -> Result<String, AppError> {
    let secrets_api = state.secrets_api(ns)?;
    let secret = secrets_api.get(secret_name).await.map_err(|_| {
        AppError::BadRequest(
            "AI provider credentials are not configured. \
             Go to Settings \u{2192} AI Providers to set up your API key."
                .to_string(),
        )
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
/// the Kubernetes Secret if no DB credential is configured.
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
                    let secret_name = std::env::var("DECKWATCH_AIFIX_CLAUDE_SECRET")
                        .or_else(|_| std::env::var("DECKWATCH_DIAG_CLAUDE_SECRET"))
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

    // 1. Load the application record from the database.
    let app_id = format!("{ns}/{name}");
    let app_row = applications::Entity::find_by_id(&app_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("application '{name}' not found")))?;

    let git: ApplicationGitConfig = app_row
        .git_config
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "application '{name}' has no git configuration -- enable GitOps first"
            ))
        })?;

    let branch = git.branch.clone().unwrap_or_else(|| "main".to_string());

    // 2. Build AI client from provider settings.
    let ai_client = build_ai_client(&state, &ns).await?;

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
        &app_row.description,
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

        let client = ai_client;
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
