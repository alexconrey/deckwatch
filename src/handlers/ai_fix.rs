use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::Json;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, EnvVar, EnvVarSource, PodSpec, PodTemplateSpec,
    SecretKeySelector, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, LogParams, PostParams};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::diagnostics::{DiagAgent, DiagStatus};
use crate::kube_ext::ApplicationGitConfig;
use crate::log_sanitize::{sanitize_logs, wrap_prompt};
use crate::state::AppState;

// AI-fix jobs reuse the diagnostics status/agent enums so the frontend can
// keep polling through `diagnosticsApi.status()` / `.result()` — the returned
// job_name lives in the same jobs namespace and produces stdout the same way.

const AI_FIX_LABEL_KEY: &str = "deckwatch.io/ai-fix";
const AI_FIX_APP_LABEL_KEY: &str = "deckwatch.io/ai-fix-application";
const AI_FIX_AGENT_LABEL_KEY: &str = "deckwatch.io/ai-fix-agent";

const APP_CM_DATA_KEY: &str = "application";
const APP_LABEL: &str = "deckwatch.io/application";

const DEFAULT_CLAUDE_IMAGE: &str = "node:24-slim";
const DEFAULT_CODEX_IMAGE: &str = "ghcr.io/openai/codex:latest";
const DEFAULT_CLAUDE_SECRET: &str = "deckwatch-anthropic-api-key";
const DEFAULT_CODEX_SECRET: &str = "deckwatch-openai-api-key";

const CONTEXT_MOUNT_DIR: &str = "/fix";
const CONTEXT_FILE_NAME: &str = "prompt.md";
const WORKDIR: &str = "/workspace";

const AI_FIX_PROMPT: &str = "You are reviewing a Kubernetes application that is managed by Deckwatch. \
Read the repository, identify issues that break Kubernetes/Deckwatch compatibility (Dockerfile problems, \
missing health endpoints, container port mismatches, misconfigured probes, image build issues, resource \
requests, secrets/env expectations), and propose concrete file-level fixes. Prefer minimal, surgical \
edits. Explain WHY each change is needed. Do not open a PR — just print the diagnosis and suggested \
changes to stdout.";

// Bound the crash-log snippet embedded in the ConfigMap. Same rationale as
// diagnostics.rs: keep the ConfigMap under the 1 MiB etcd object limit even
// when a pod has been crash-looping for hours.
const CRASH_LOG_TAIL_BYTES: usize = 32 * 1024;
const CRASH_LOG_MAX_PODS: usize = 3;

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

pub async fn create_ai_fix(
    State(state): State<AppState>,
    Path((ns, name)): Path<(String, String)>,
    Json(req): Json<AiFixRequest>,
) -> Result<Json<AiFixResponse>, AppError> {
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
            "application '{name}' has no git configuration — enable GitOps first"
        ))
    })?;

    let branch = git.branch.clone().unwrap_or_else(|| "main".to_string());

    // 2. Resolve the git token from the managed secret (if configured).
    //    Public repos may leave token_secret unset — we tolerate that and
    //    clone anonymously.
    let git_token_secret = git.token_secret.clone().filter(|s| !s.is_empty());

    // 3. Gather recent crash-log snippets from the app's member pods, best
    //    effort. If it fails, we still create the job — the AI can work off
    //    the repo alone.
    let crash_logs = gather_crash_logs(&state, &ns, &name)
        .await
        .unwrap_or_default();

    // 4. Deployment status summary (name + phase-ish signal from replicas).
    let deployment_status = summarize_deployments(&state, &ns, &name)
        .await
        .unwrap_or_default();

    let agent = req.agent;
    let ts = jiff::Timestamp::now().as_second();
    let job_name = make_short_name("dw-aifix", agent.as_str(), &name, ts);
    let context_cm_name = format!("{job_name}-ctx");

    let context_md = build_context_markdown(
        &name,
        &ns,
        &app_data.description,
        &git.repo_url,
        &branch,
        &crash_logs,
        &deployment_status,
    );

    create_context_configmap(
        &state,
        &ns,
        &context_cm_name,
        &job_name,
        &name,
        agent,
        &context_md,
    )
    .await?;

    if let Err(e) = create_ai_fix_job(
        &state,
        &ns,
        &job_name,
        &context_cm_name,
        &name,
        &git.repo_url,
        &branch,
        git_token_secret.as_deref(),
        agent,
    )
    .await
    {
        // Best-effort rollback of the context ConfigMap on job creation failure,
        // matching diagnostics.rs's pattern.
        if let Ok(cm_api) = state.configmaps_api(&ns) {
            let _ = cm_api.delete(&context_cm_name, &Default::default()).await;
        }
        return Err(e);
    }

    Ok(Json(AiFixResponse {
        job_name,
        status: DiagStatus::Pending,
        agent,
    }))
}

fn agent_image(agent: DiagAgent) -> String {
    match agent {
        DiagAgent::Claude => std::env::var("DECKWATCH_AIFIX_CLAUDE_IMAGE")
            .or_else(|_| std::env::var("DECKWATCH_DIAG_CLAUDE_IMAGE"))
            .unwrap_or_else(|_| DEFAULT_CLAUDE_IMAGE.to_string()),
        DiagAgent::Codex => std::env::var("DECKWATCH_AIFIX_CODEX_IMAGE")
            .or_else(|_| std::env::var("DECKWATCH_DIAG_CODEX_IMAGE"))
            .unwrap_or_else(|_| DEFAULT_CODEX_IMAGE.to_string()),
    }
}

fn agent_api_key_env(agent: DiagAgent) -> &'static str {
    match agent {
        DiagAgent::Claude => "ANTHROPIC_API_KEY",
        DiagAgent::Codex => "OPENAI_API_KEY",
    }
}

fn agent_api_key_secret(agent: DiagAgent) -> String {
    match agent {
        DiagAgent::Claude => std::env::var("DECKWATCH_AIFIX_CLAUDE_SECRET")
            .or_else(|_| std::env::var("DECKWATCH_DIAG_CLAUDE_SECRET"))
            .unwrap_or_else(|_| DEFAULT_CLAUDE_SECRET.to_string()),
        DiagAgent::Codex => std::env::var("DECKWATCH_AIFIX_CODEX_SECRET")
            .or_else(|_| std::env::var("DECKWATCH_DIAG_CODEX_SECRET"))
            .unwrap_or_else(|_| DEFAULT_CODEX_SECRET.to_string()),
    }
}

async fn gather_crash_logs(state: &AppState, ns: &str, app_name: &str) -> Result<String, AppError> {
    let pods_api = state.pods_api(ns)?;
    // Same selector convention applications.rs uses for its own member queries.
    let lp = ListParams::default().labels(&member_selector(app_name));
    let pods = pods_api.list(&lp).await?;

    // Prefer pods with a non-zero restart count — those are the ones that
    // actually have something interesting for the AI to look at. Fall back
    // to the most recently started pods if nothing is crashing.
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

        // Ask for the previous-instance log when the container has restarted;
        // that's where the crash trace lives.
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

        // Sanitize every pod's logs before embedding. Each pod is a
        // separate untrusted source; strip escapes/controls and cap line
        // length here rather than in one big blob at the end so the
        // per-pod fenced sections stay well-formed.
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

    // The whole crash-log block is untrusted input (see AI_SAFETY.md).
    // Wrap the entire markdown context in the hardened fence: the operator-
    // supplied fields (app name, ns, repo URL, branch) are trusted; the
    // pod logs and application description are not. `wrap_prompt` marks
    // the whole thing as untrusted for the agent, which is the safest
    // over-approximation — the agent still gets to *see* the trusted bits,
    // it just doesn't act on directives inside them either.
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

async fn create_context_configmap(
    state: &AppState,
    ns: &str,
    cm_name_str: &str,
    job_name: &str,
    app_name: &str,
    agent: DiagAgent,
    context_md: &str,
) -> Result<(), AppError> {
    let mut labels = BTreeMap::new();
    labels.insert(AI_FIX_LABEL_KEY.to_string(), "true".to_string());
    labels.insert(
        AI_FIX_AGENT_LABEL_KEY.to_string(),
        agent.as_str().to_string(),
    );
    labels.insert(
        AI_FIX_APP_LABEL_KEY.to_string(),
        truncate_label_value(&sanitize_name_segment(app_name)),
    );
    labels.insert("deckwatch.io/ai-fix-job".to_string(), job_name.to_string());

    let mut annotations = BTreeMap::new();
    annotations.insert("deckwatch.io/application".to_string(), app_name.to_string());

    let mut data = BTreeMap::new();
    data.insert(CONTEXT_FILE_NAME.to_string(), context_md.to_string());

    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(cm_name_str.to_string()),
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

#[allow(clippy::too_many_arguments)]
async fn create_ai_fix_job(
    state: &AppState,
    ns: &str,
    job_name: &str,
    context_cm_name: &str,
    app_name: &str,
    repo_url: &str,
    branch: &str,
    git_token_secret: Option<&str>,
    agent: DiagAgent,
) -> Result<(), AppError> {
    let mut labels = BTreeMap::new();
    labels.insert(AI_FIX_LABEL_KEY.to_string(), "true".to_string());
    labels.insert(
        AI_FIX_AGENT_LABEL_KEY.to_string(),
        agent.as_str().to_string(),
    );
    labels.insert(
        AI_FIX_APP_LABEL_KEY.to_string(),
        truncate_label_value(&sanitize_name_segment(app_name)),
    );

    let mut annotations = BTreeMap::new();
    annotations.insert("deckwatch.io/application".to_string(), app_name.to_string());

    let mut env = vec![
        EnvVar {
            name: agent_api_key_env(agent).to_string(),
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: agent_api_key_secret(agent),
                    key: "api-key".to_string(),
                    optional: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_AIFIX_APP".to_string(),
            value: Some(app_name.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_AIFIX_REPO".to_string(),
            value: Some(repo_url.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_AIFIX_BRANCH".to_string(),
            value: Some(branch.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "DECKWATCH_AIFIX_CONTEXT_PATH".to_string(),
            value: Some(format!("{CONTEXT_MOUNT_DIR}/{CONTEXT_FILE_NAME}")),
            ..Default::default()
        },
    ];

    if let Some(secret) = git_token_secret {
        env.push(EnvVar {
            name: "GIT_TOKEN".to_string(),
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: secret.to_string(),
                    key: "token".to_string(),
                    optional: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
    }

    // Agent CLI selection.
    //
    // AI-fix intentionally runs the agent WITH `--dangerously-skip-permissions`
    // for Claude (and Codex's `--sandbox workspace-write`) because the whole
    // point is to let it read the repo, run linters, and try edits. That
    // makes the injection defenses upstream (log sanitizer + fenced prompt)
    // load-bearing: an attacker who slipped a prompt past the fence could
    // now run tools. We accept that trade-off because:
    //   - the job runs in a short-lived Pod with no cluster-wide RBAC,
    //   - it can only touch its own volumes and the cloned repo,
    //   - egress is normal outbound HTTPS (whatever NetPol the cluster has).
    // Operators who want stricter isolation should attach a NetworkPolicy
    // to the diagnostic/ai-fix Job pods. See docs/AI_SAFETY.md.
    // Clone URL construction matches watcher::trigger_build: strip scheme,
    // then re-inject via `https://x-token:$GIT_TOKEN@host/path` when a token
    // is present. Anonymous clone otherwise.
    //
    // The prompt is read from a file, never interpolated into the shell —
    // the fenced/sanitized prompt lands verbatim on the CLI's stdin.
    //
    // Claude: installed on-the-fly via `npx` from a Node.js base image
    // (the ghcr.io/anthropics/claude-code image is not publicly pullable).
    // The node:24-slim image doesn't ship git, so we install it first.
    // Codex: expected to be pre-installed in the image.
    let clone_script = match agent {
        DiagAgent::Claude => format!(
            r#"set -eu

# node:24-slim doesn't include git; install it for the clone step.
apt-get update -qq && apt-get install -y -qq git >/dev/null 2>&1

if ! command -v npx >/dev/null 2>&1; then
  echo "error: npx not found in image PATH — is this a Node.js base image?" >&2
  exit 127
fi

REPO="$DECKWATCH_AIFIX_REPO"
BRANCH="$DECKWATCH_AIFIX_BRANCH"

if [ -n "${{GIT_TOKEN:-}}" ]; then
  CLONE_URL="$(printf '%s' "$REPO" | sed -E 's#^https?://#&x-token:'"$GIT_TOKEN"'@#')"
else
  CLONE_URL="$REPO"
fi

mkdir -p {WORKDIR}
cd {WORKDIR}
git clone --depth 50 --branch "$BRANCH" "$CLONE_URL" repo 2>&1 | sed "s#${{GIT_TOKEN:-__no_token__}}#***#g" || true
cd repo

echo "=== Deckwatch AI Fix ==="
echo "Application: $DECKWATCH_AIFIX_APP"
echo "Repository:  $REPO"
echo "Branch:      $BRANCH"
echo "Agent:       claude (via npx)"
echo

cat "$DECKWATCH_AIFIX_CONTEXT_PATH" | npx -y @anthropic-ai/claude-code@latest --print --dangerously-skip-permissions
"#
        ),
        DiagAgent::Codex => {
            let cli = "codex";
            let cli_flags = "exec --sandbox workspace-write --";
            format!(
                r#"set -eu
if ! command -v git >/dev/null 2>&1; then
  echo "error: git not found in agent image PATH" >&2
  exit 127
fi
if ! command -v {cli} >/dev/null 2>&1; then
  echo "error: {cli} CLI not found in image PATH" >&2
  exit 127
fi

REPO="$DECKWATCH_AIFIX_REPO"
BRANCH="$DECKWATCH_AIFIX_BRANCH"

if [ -n "${{GIT_TOKEN:-}}" ]; then
  CLONE_URL="$(printf '%s' "$REPO" | sed -E 's#^https?://#&x-token:'"$GIT_TOKEN"'@#')"
else
  CLONE_URL="$REPO"
fi

mkdir -p {WORKDIR}
cd {WORKDIR}
git clone --depth 50 --branch "$BRANCH" "$CLONE_URL" repo 2>&1 | sed "s#${{GIT_TOKEN:-__no_token__}}#***#g" || true
cd repo

echo "=== Deckwatch AI Fix ==="
echo "Application: $DECKWATCH_AIFIX_APP"
echo "Repository:  $REPO"
echo "Branch:      $BRANCH"
echo "Agent:       {cli}"
echo

cat "$DECKWATCH_AIFIX_CONTEXT_PATH" | {cli} {cli_flags}
"#
            )
        }
    };

    let container_spec = Container {
        name: "agent".to_string(),
        image: Some(agent_image(agent)),
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string()]),
        args: Some(vec![clone_script]),
        env: Some(env),
        volume_mounts: Some(vec![VolumeMount {
            name: "fix-context".to_string(),
            mount_path: CONTEXT_MOUNT_DIR.to_string(),
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
            // Repo clones + agent think time can exceed the 10-minute
            // diagnostics budget. Give AI-fix jobs a longer deadline.
            active_deadline_seconds: Some(1800),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some({
                        let mut l = BTreeMap::new();
                        l.insert(AI_FIX_LABEL_KEY.to_string(), "true".to_string());
                        l.insert(
                            AI_FIX_AGENT_LABEL_KEY.to_string(),
                            agent.as_str().to_string(),
                        );
                        l
                    }),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    containers: vec![container_spec],
                    volumes: Some(vec![Volume {
                        name: "fix-context".to_string(),
                        config_map: Some(ConfigMapVolumeSource {
                            name: context_cm_name.to_string(),
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
    jobs_api.create(&PostParams::default(), &job).await?;
    Ok(())
}

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
const K8S_LABEL_VALUE_MAX: usize = 63;

/// Truncate a string to fit within the K8s label value limit (63 chars).
fn truncate_label_value(s: &str) -> String {
    if s.len() <= K8S_LABEL_VALUE_MAX {
        return s.to_string();
    }
    s[..K8S_LABEL_VALUE_MAX].trim_end_matches('-').to_string()
}

/// Build a short, K8s-safe resource name. Same scheme as diagnostics.rs.
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

#[cfg(test)]
#[path = "../handlers_ai_fix_tests.rs"]
mod handlers_ai_fix_tests;
