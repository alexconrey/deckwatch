//! GitHub REST API calls, against the configured GitHub endpoints
//! (github.com by default, or GitHub Enterprise Server).
//!
//! A git token is only attached after [`authorize`] confirms it may be sent
//! to the configured API origin, so a token is never sent to a host it was
//! not saved for.

use reqwest::header::ACCEPT;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::binding::{authorize, BindingError, CredentialSource};
use super::endpoints::Endpoint;

const GITHUB_JSON: &str = "application/vnd.github+json";

/// A git token and where it came from.
#[derive(Debug, Clone, Copy)]
pub struct GitHubToken<'a> {
    pub token: &'a str,
    /// For error messages, e.g. "git token 'org-token'".
    pub label: &'a str,
    pub source: &'a CredentialSource,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    /// The token may not be sent to the configured API host. Nothing was sent.
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error("GitHub API request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("GitHub API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
}

/// `(owner, repo)` of `repo_url` if it is hosted on the GitHub web host
/// `web` (github.com, or the configured GitHub Enterprise host).
pub fn repo_on_host(repo_url: &str, web: &Endpoint) -> Option<(String, String)> {
    let url = reqwest::Url::parse(repo_url.trim()).ok()?;
    let host = url.host_str()?;
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    if !authority.eq_ignore_ascii_case(web.host()) {
        return None;
    }
    let prefix = web.base().strip_prefix(web.origin()).unwrap_or("");
    let path = url.path().strip_prefix(prefix)?;
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().map(|r| r.trim_end_matches(".git"))?;
    if repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

fn request(
    http: &reqwest::Client,
    api: &Endpoint,
    path: &str,
    token: Option<GitHubToken<'_>>,
) -> Result<reqwest::RequestBuilder, BindingError> {
    let mut req = http.get(api.url(path)).header(ACCEPT, GITHUB_JSON);
    if let Some(t) = token.filter(|t| !t.token.is_empty()) {
        authorize(t.label, t.source, api)?;
        req = req.bearer_auth(t.token);
    }
    Ok(req)
}

async fn send_json<T: DeserializeOwned>(req: reqwest::RequestBuilder) -> Result<T, GitHubError> {
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(GitHubError::Status {
            status: status.as_u16(),
            body: body.chars().take(200).collect(),
        });
    }
    Ok(resp.json().await?)
}

/// Paths changed between two commits
/// (`GET /repos/{owner}/{repo}/compare/{base}...{head}`). GitHub lists up
/// to 300 files.
pub async fn compare_files(
    http: &reqwest::Client,
    api: &Endpoint,
    owner: &str,
    repo: &str,
    base: &str,
    head: &str,
    token: Option<GitHubToken<'_>>,
) -> Result<Vec<String>, GitHubError> {
    #[derive(Deserialize)]
    struct Compare {
        #[serde(default)]
        files: Vec<File>,
    }
    #[derive(Deserialize)]
    struct File {
        filename: String,
    }

    let req = request(
        http,
        api,
        &format!("/repos/{owner}/{repo}/compare/{base}...{head}"),
        token,
    )?;
    let body: Compare = send_json(req).await?;
    Ok(body.files.into_iter().map(|f| f.filename).collect())
}

/// Commit SHA at the tip of `branch`
/// (`GET /repos/{owner}/{repo}/branches/{branch}`).
pub async fn branch_head(
    http: &reqwest::Client,
    api: &Endpoint,
    owner: &str,
    repo: &str,
    branch: &str,
    token: Option<GitHubToken<'_>>,
) -> Result<String, GitHubError> {
    #[derive(Deserialize)]
    struct Branch {
        commit: Commit,
    }
    #[derive(Deserialize)]
    struct Commit {
        sha: String,
    }

    let req = request(
        http,
        api,
        &format!("/repos/{owner}/{repo}/branches/{branch}"),
        token,
    )?;
    let body: Branch = send_json(req).await?;
    Ok(body.commit.sha)
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
