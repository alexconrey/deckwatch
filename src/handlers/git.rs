use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::handlers::settings::DeckwatchSettings;
use crate::state::AppState;

/// Cache TTL for branch listings. The dropdown fires on focus/type, so a
/// short window is enough to keep an interactive edit session from hammering
/// the remote while still letting a page refresh pick up new branches.
const BRANCH_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
pub struct BranchQuery {
    /// Clone URL of the repository. HTTPS only — matches how the poller
    /// speaks to remotes today.
    pub repo_url: String,
    /// Name (matching `git_token_secrets[].name` in settings) of the token
    /// entry to authenticate with. If the user picked a "Custom" repo we
    /// still need a token entry to know which Secret to read.
    pub token_secret: String,
    /// Optional namespace override. When absent the token entry's own
    /// `namespace` field is used.
    pub namespace: Option<String>,
}

#[derive(Serialize)]
pub struct BranchListResponse {
    pub branches: Vec<String>,
    /// Best-effort default branch name. Populated when the remote's
    /// `HEAD` line points at a specific ref; otherwise falls back to
    /// `"main"` if present, else the first branch, else `null`.
    pub default_branch: Option<String>,
}

struct CachedBranches {
    fetched_at: Instant,
    response: BranchListResponse,
}

fn cache() -> &'static Mutex<HashMap<String, CachedBranches>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedBranches>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(repo_url: &str, token_secret: &str) -> String {
    // Key on repo + token name so two callers using different tokens against
    // the same repo don't share stale visibility.
    format!("{repo_url}|{token_secret}")
}

fn cached(key: &str) -> Option<BranchListResponse> {
    let guard = cache().lock().ok()?;
    let entry = guard.get(key)?;
    if entry.fetched_at.elapsed() > BRANCH_CACHE_TTL {
        return None;
    }
    Some(BranchListResponse {
        branches: entry.response.branches.clone(),
        default_branch: entry.response.default_branch.clone(),
    })
}

fn cache_put(key: String, response: &BranchListResponse) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(
            key,
            CachedBranches {
                fetched_at: Instant::now(),
                response: BranchListResponse {
                    branches: response.branches.clone(),
                    default_branch: response.default_branch.clone(),
                },
            },
        );
    }
}

pub async fn list_branches(
    State(state): State<AppState>,
    Query(q): Query<BranchQuery>,
) -> Result<Json<BranchListResponse>, AppError> {
    if q.repo_url.is_empty() {
        return Err(AppError::BadRequest("repo_url is required".to_string()));
    }
    if q.token_secret.is_empty() {
        return Err(AppError::BadRequest(
            "token_secret is required".to_string(),
        ));
    }

    let key = cache_key(&q.repo_url, &q.token_secret);
    if let Some(hit) = cached(&key) {
        return Ok(Json(hit));
    }

    // Resolve the token entry from settings to figure out which Secret in
    // which namespace to read. Explicit ?namespace= wins so power users can
    // still override.
    let settings = load_settings(&state).await;
    let token_entry = settings
        .git_token_secrets
        .iter()
        .find(|s| s.name == q.token_secret)
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "unknown token_secret '{}' — add it under Settings first",
                q.token_secret
            ))
        })?;

    let ns = q
        .namespace
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| token_entry.namespace.clone());
    if ns.is_empty() {
        return Err(AppError::BadRequest(
            "namespace is required (either as query param or on the token secret entry)"
                .to_string(),
        ));
    }

    let secrets_api = state.secrets_api(&ns)?;
    let secret = secrets_api.get(&token_entry.secret_name).await?;
    let token = secret
        .data
        .as_ref()
        .and_then(|d| d.get("token"))
        .map(|v| String::from_utf8_lossy(&v.0).to_string())
        .ok_or_else(|| AppError::BadRequest("secret missing 'token' key".to_string()))?;

    let http = reqwest::Client::new();
    let response = fetch_branches(&http, &q.repo_url, &token)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to list branches: {e}")))?;

    cache_put(key, &response);
    Ok(Json(response))
}

async fn load_settings(state: &AppState) -> DeckwatchSettings {
    let Ok(api) = state.configmaps_api(&state.settings_namespace) else {
        return DeckwatchSettings::default();
    };
    match api.get(&state.settings_configmap_name).await {
        Ok(cm) => cm
            .data
            .as_ref()
            .and_then(|d| d.get("settings"))
            .and_then(|s| serde_json::from_str::<DeckwatchSettings>(s).ok())
            .unwrap_or_default(),
        Err(_) => DeckwatchSettings::default(),
    }
}

async fn fetch_branches(
    http: &reqwest::Client,
    repo_url: &str,
    token: &str,
) -> anyhow::Result<BranchListResponse> {
    // Smart-HTTP advertisement — same endpoint used by `git ls-remote`.
    // Works uniformly across GitHub, GitLab, Bitbucket, Gitea, and any
    // OCI-agnostic HTTPS Git host. Deliberately not using the GitHub REST
    // API here so self-hosted providers keep working.
    let url = format!(
        "{}/info/refs?service=git-upload-pack",
        repo_url.trim_end_matches('/')
    );

    let creds = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("x-token:{token}"),
    );

    let body = http
        .get(&url)
        .header("Authorization", format!("Basic {creds}"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let mut branches: Vec<String> = Vec::new();
    let mut default_branch: Option<String> = None;

    for line in body.lines() {
        // Look for HEAD symref: `symref=HEAD:refs/heads/main`
        if default_branch.is_none() {
            if let Some(idx) = line.find("symref=HEAD:refs/heads/") {
                let rest = &line[idx + "symref=HEAD:refs/heads/".len()..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '\0')
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                if !name.is_empty() {
                    default_branch = Some(name.to_string());
                }
            }
        }

        if let Some(idx) = line.find(" refs/heads/") {
            let rest = &line[idx + " refs/heads/".len()..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '\0')
                .unwrap_or(rest.len());
            let name = &rest[..end];
            if !name.is_empty() && !branches.iter().any(|b| b == name) {
                branches.push(name.to_string());
            }
        }
    }

    branches.sort();

    if default_branch.is_none() {
        default_branch = if branches.iter().any(|b| b == "main") {
            Some("main".to_string())
        } else if branches.iter().any(|b| b == "master") {
            Some("master".to_string())
        } else {
            branches.first().cloned()
        };
    }

    Ok(BranchListResponse {
        branches,
        default_branch,
    })
}
