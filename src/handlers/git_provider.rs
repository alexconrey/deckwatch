//! `GitProvider` abstraction over hosted Git services.
//!
//! Each provider supplies three things:
//!   * `check_head` — resolve a branch to a commit SHA.
//!   * `list_branches` — enumerate branches (best-effort default-branch hint).
//!   * `verify_webhook` — authenticate an incoming webhook delivery and pull
//!     out the (repo_url, branch, commit_sha) tuple.
//!
//! `detect_provider` inspects a repo URL and picks the right impl:
//! GitHub for github.com, GitLab for gitlab.com or any host starting with
//! "gitlab." (self-hosted), Bitbucket for bitbucket.org, otherwise the
//! `GenericGit` fallback that speaks the smart-HTTP protocol (works with any
//! HTTPS Git remote — self-hosted Gitea, cgit, Bitbucket Server, etc.).
//!
//! Webhook verification is provider-specific because each vendor picked a
//! different signature scheme:
//!   * GitHub: HMAC-SHA256 in `X-Hub-Signature-256` (`sha256=<hex>` format).
//!   * GitLab: shared token compared verbatim against `X-Gitlab-Token`.
//!   * Bitbucket: HMAC-SHA256 in `X-Hub-Signature` (same format as GitHub).
//!   * Generic: no signature check — operator opts in by picking this
//!     provider explicitly; useful for internal Git hosts that pre-share a
//!     token in an `Authorization` header.

use async_trait::async_trait;
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Extracted intent from a verified webhook payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEvent {
    /// Canonical repo clone URL (https://...). Normalized to strip a trailing
    /// `.git` because deployment annotations don't include it and we need to
    /// match by string equality.
    pub repo_url: String,
    pub branch: String,
    pub commit_sha: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("missing signature header")]
    MissingSignature,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("unsupported event: {0}")]
    UnsupportedEvent(String),
    #[error("malformed payload: {0}")]
    MalformedPayload(String),
}

/// Providers agree on a common interface so the webhook receiver and poller
/// can be written once against the trait.
#[async_trait]
pub trait GitProvider: Send + Sync {
    /// Human-readable identifier — used in logs and to disambiguate
    /// auto-detection in tests.
    fn name(&self) -> &'static str;

    /// Resolve `branch` on `repo` to a full commit SHA. Blank `token` means
    /// public / unauthenticated.
    async fn check_head(
        &self,
        http: &reqwest::Client,
        repo: &str,
        branch: &str,
        token: &str,
    ) -> anyhow::Result<String>;

    /// List branches on `repo`. Second element is a best-effort default
    /// branch hint (`None` if the remote didn't advertise one).
    async fn list_branches(
        &self,
        http: &reqwest::Client,
        repo: &str,
        token: &str,
    ) -> anyhow::Result<(Vec<String>, Option<String>)>;

    /// Authenticate an incoming webhook delivery and extract the event.
    ///
    /// `secret` is the operator-configured shared signing key (empty string
    /// means "no verification requested" — the receiver decides whether to
    /// allow that, not the provider).
    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &str,
    ) -> Result<WebhookEvent, WebhookError>;
}

// ---------------------------------------------------------------- detection

/// Pick a `GitProvider` for the given repo URL. Falls back to `GenericGit`
/// for anything that doesn't match a known SaaS host.
pub fn detect_provider(repo_url: &str) -> Box<dyn GitProvider> {
    let lower = repo_url.to_ascii_lowercase();
    // Host-based detection: match on the hostname component only so
    // e.g. `https://gitlab.com/foo/bar` and `https://gitlab.example.com/x`
    // both take the GitLab branch, but a URL that happens to contain the
    // word "github" in its path doesn't.
    let host = url_host(&lower).unwrap_or_default();
    if host == "github.com" || host.ends_with(".github.com") {
        Box::new(GitHub)
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        Box::new(GitLab)
    } else if host == "bitbucket.org" || host.starts_with("bitbucket.") {
        Box::new(Bitbucket)
    } else {
        Box::new(GenericGit)
    }
}

/// Extract the host from a repo URL. Handles both `https://host/path` and
/// `git@host:path` forms; returns lowercase host.
fn url_host(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let host = rest.split('/').next()?;
        // Strip user@ prefix and port suffix.
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        return Some(host.to_ascii_lowercase());
    }
    if let Some(rest) = url.strip_prefix("git@") {
        let host = rest.split(':').next()?;
        return Some(host.to_ascii_lowercase());
    }
    None
}

/// Strip a trailing `.git` and any trailing slash so two spellings of the
/// same repo compare equal. Webhook payloads carry the `.git` form
/// inconsistently across providers.
pub fn normalize_repo_url(url: &str) -> String {
    let s = url.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_string()
}

// ------------------------------------------------------------------- shared

/// Smart-HTTP `info/refs?service=git-upload-pack` HEAD resolution used by
/// GitLab / generic providers. GitHub gets its own JSON-API path since we
/// want provider-specific rate limits + auth semantics there.
async fn smart_http_head(
    http: &reqwest::Client,
    repo_url: &str,
    branch: &str,
    token: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/info/refs?service=git-upload-pack",
        repo_url.trim_end_matches('/')
    );

    let mut request = http.get(&url);
    if !token.is_empty() {
        let creds = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("x-token:{token}"),
        );
        request = request.header("Authorization", format!("Basic {creds}"));
    }

    let resp = request
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let target_ref = format!("refs/heads/{branch}");
    for line in resp.lines() {
        if line.contains(&target_ref) {
            let sha = line
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_start_matches(|c: char| !c.is_ascii_hexdigit());
            if sha.len() >= 40 {
                return Ok(sha[..40].to_string());
            }
        }
    }
    anyhow::bail!("branch '{branch}' not found in remote refs")
}

async fn smart_http_branches(
    http: &reqwest::Client,
    repo_url: &str,
    token: &str,
) -> anyhow::Result<(Vec<String>, Option<String>)> {
    let url = format!(
        "{}/info/refs?service=git-upload-pack",
        repo_url.trim_end_matches('/')
    );

    let mut request = http.get(&url);
    if !token.is_empty() {
        let creds = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("x-token:{token}"),
        );
        request = request.header("Authorization", format!("Basic {creds}"));
    }

    let body = request.send().await?.error_for_status()?.text().await?;

    let mut branches: Vec<String> = Vec::new();
    let mut default_branch: Option<String> = None;

    for line in body.lines() {
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
    Ok((branches, default_branch))
}

/// Constant-time HMAC-SHA256 verification. `expected_hex` is the hex-encoded
/// digest from a `sha256=<hex>` header value (caller already stripped the
/// prefix). Uses `subtle::ConstantTimeEq` so a timing attack can't leak the
/// signing key one byte at a time.
fn verify_hmac_sha256(secret: &str, body: &[u8], expected_hex: &str) -> Result<(), WebhookError> {
    if secret.is_empty() {
        return Err(WebhookError::InvalidSignature);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| WebhookError::InvalidSignature)?;
    mac.update(body);
    let computed = mac.finalize().into_bytes();

    let expected_bytes = hex_decode(expected_hex).ok_or(WebhookError::InvalidSignature)?;
    if expected_bytes.len() != computed.len() {
        return Err(WebhookError::InvalidSignature);
    }
    if computed.as_slice().ct_eq(&expected_bytes).unwrap_u8() == 1 {
        Ok(())
    } else {
        Err(WebhookError::InvalidSignature)
    }
}

/// Minimal lowercase-hex decoder to avoid a `hex` crate dependency. Returns
/// `None` on any non-hex character or odd length.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// -------------------------------------------------------------------- GitHub

pub struct GitHub;

#[async_trait]
impl GitProvider for GitHub {
    fn name(&self) -> &'static str {
        "github"
    }

    async fn check_head(
        &self,
        http: &reqwest::Client,
        repo: &str,
        branch: &str,
        token: &str,
    ) -> anyhow::Result<String> {
        // Use the REST API when a token is available so we get proper rate
        // limits + private-repo support. Fall back to smart-HTTP for
        // unauthenticated calls (avoids the 60/hour unauth REST limit).
        if token.is_empty() {
            return smart_http_head(http, repo, branch, "").await;
        }
        let (owner, name) = split_github_repo(repo)
            .ok_or_else(|| anyhow::anyhow!("not a github repo url: {repo}"))?;
        let url = format!("https://api.github.com/repos/{owner}/{name}/branches/{branch}");
        let resp: serde_json::Value = http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "deckwatch")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let sha = resp
            .get("commit")
            .and_then(|c| c.get("sha"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("branch '{branch}' not found"))?;
        Ok(sha.to_string())
    }

    async fn list_branches(
        &self,
        http: &reqwest::Client,
        repo: &str,
        token: &str,
    ) -> anyhow::Result<(Vec<String>, Option<String>)> {
        // Smart-HTTP works fine for GitHub too and gives us the HEAD symref
        // for the default branch in the same round-trip — cheaper than
        // paginating the REST /branches endpoint.
        smart_http_branches(http, repo, token).await
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &str,
    ) -> Result<WebhookEvent, WebhookError> {
        let sig = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .ok_or(WebhookError::MissingSignature)?;
        let hex = sig
            .strip_prefix("sha256=")
            .ok_or(WebhookError::InvalidSignature)?;
        verify_hmac_sha256(secret, body, hex)?;

        // Ignore anything but `push` — pull_request, ping, etc. are fine but
        // don't drive a build. `ping` is common on webhook creation; return
        // UnsupportedEvent so the receiver can return 200 without acting.
        let event = headers
            .get("x-github-event")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if event != "push" {
            return Err(WebhookError::UnsupportedEvent(event.to_string()));
        }

        #[derive(Deserialize)]
        struct GhPush {
            #[serde(rename = "ref")]
            ref_: String,
            after: String,
            repository: GhRepo,
        }
        #[derive(Deserialize)]
        struct GhRepo {
            clone_url: String,
        }

        let payload: GhPush = serde_json::from_slice(body)
            .map_err(|e| WebhookError::MalformedPayload(e.to_string()))?;
        let branch = payload
            .ref_
            .strip_prefix("refs/heads/")
            .ok_or_else(|| WebhookError::UnsupportedEvent(payload.ref_.clone()))?
            .to_string();
        Ok(WebhookEvent {
            repo_url: normalize_repo_url(&payload.repository.clone_url),
            branch,
            commit_sha: payload.after,
        })
    }
}

fn split_github_repo(url: &str) -> Option<(String, String)> {
    let s = url.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = s.rsplitn(3, '/').collect();
    if parts.len() < 2 {
        return None;
    }
    Some((parts[1].to_string(), parts[0].to_string()))
}

// -------------------------------------------------------------------- GitLab

pub struct GitLab;

#[async_trait]
impl GitProvider for GitLab {
    fn name(&self) -> &'static str {
        "gitlab"
    }

    async fn check_head(
        &self,
        http: &reqwest::Client,
        repo: &str,
        branch: &str,
        token: &str,
    ) -> anyhow::Result<String> {
        // Smart-HTTP works for both gitlab.com and self-hosted, and doesn't
        // require URL-encoding the project path the way the REST API does.
        smart_http_head(http, repo, branch, token).await
    }

    async fn list_branches(
        &self,
        http: &reqwest::Client,
        repo: &str,
        token: &str,
    ) -> anyhow::Result<(Vec<String>, Option<String>)> {
        smart_http_branches(http, repo, token).await
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &str,
    ) -> Result<WebhookEvent, WebhookError> {
        // GitLab doesn't sign the payload; it echoes back the operator-set
        // secret in `X-Gitlab-Token` verbatim. Constant-time compare against
        // the configured secret so an attacker can't diff response timings.
        let token = headers
            .get("x-gitlab-token")
            .and_then(|v| v.to_str().ok())
            .ok_or(WebhookError::MissingSignature)?;
        if secret.is_empty() {
            return Err(WebhookError::InvalidSignature);
        }
        if token.as_bytes().ct_eq(secret.as_bytes()).unwrap_u8() != 1 {
            return Err(WebhookError::InvalidSignature);
        }

        let event = headers
            .get("x-gitlab-event")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if event != "Push Hook" {
            return Err(WebhookError::UnsupportedEvent(event.to_string()));
        }

        #[derive(Deserialize)]
        struct GlPush {
            #[serde(rename = "ref")]
            ref_: String,
            after: String,
            project: GlProject,
        }
        #[derive(Deserialize)]
        struct GlProject {
            // GitLab exposes both `git_http_url` and `web_url`; `git_http_url`
            // is the pushable clone URL and matches what a user would paste
            // into the repo dropdown.
            git_http_url: String,
        }

        let payload: GlPush = serde_json::from_slice(body)
            .map_err(|e| WebhookError::MalformedPayload(e.to_string()))?;
        let branch = payload
            .ref_
            .strip_prefix("refs/heads/")
            .ok_or_else(|| WebhookError::UnsupportedEvent(payload.ref_.clone()))?
            .to_string();
        Ok(WebhookEvent {
            repo_url: normalize_repo_url(&payload.project.git_http_url),
            branch,
            commit_sha: payload.after,
        })
    }
}

// ----------------------------------------------------------------- Bitbucket

pub struct Bitbucket;

#[async_trait]
impl GitProvider for Bitbucket {
    fn name(&self) -> &'static str {
        "bitbucket"
    }

    async fn check_head(
        &self,
        http: &reqwest::Client,
        repo: &str,
        branch: &str,
        token: &str,
    ) -> anyhow::Result<String> {
        smart_http_head(http, repo, branch, token).await
    }

    async fn list_branches(
        &self,
        http: &reqwest::Client,
        repo: &str,
        token: &str,
    ) -> anyhow::Result<(Vec<String>, Option<String>)> {
        smart_http_branches(http, repo, token).await
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &str,
    ) -> Result<WebhookEvent, WebhookError> {
        // Bitbucket Cloud webhooks use X-Hub-Signature with the same
        // sha256=<hex> encoding as GitHub. Bitbucket Server ("Stash") uses
        // X-Hub-Signature too, so we accept either.
        let sig = headers
            .get("x-hub-signature")
            .and_then(|v| v.to_str().ok())
            .ok_or(WebhookError::MissingSignature)?;
        let hex = sig
            .strip_prefix("sha256=")
            .ok_or(WebhookError::InvalidSignature)?;
        verify_hmac_sha256(secret, body, hex)?;

        let event = headers
            .get("x-event-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if event != "repo:push" {
            return Err(WebhookError::UnsupportedEvent(event.to_string()));
        }

        #[derive(Deserialize)]
        struct BbPush {
            repository: BbRepo,
            push: BbPushChanges,
        }
        #[derive(Deserialize)]
        struct BbRepo {
            links: BbLinks,
        }
        #[derive(Deserialize)]
        struct BbLinks {
            html: BbHref,
        }
        #[derive(Deserialize)]
        struct BbHref {
            href: String,
        }
        #[derive(Deserialize)]
        struct BbPushChanges {
            changes: Vec<BbChange>,
        }
        #[derive(Deserialize)]
        struct BbChange {
            new: Option<BbRef>,
        }
        #[derive(Deserialize)]
        struct BbRef {
            name: String,
            #[serde(rename = "type")]
            kind: String,
            target: BbTarget,
        }
        #[derive(Deserialize)]
        struct BbTarget {
            hash: String,
        }

        let payload: BbPush = serde_json::from_slice(body)
            .map_err(|e| WebhookError::MalformedPayload(e.to_string()))?;

        // Take the first branch change; Bitbucket batches multiple ref
        // updates in one delivery but the receiver only needs one to fan
        // out to matching deployments.
        let change = payload
            .push
            .changes
            .into_iter()
            .find(|c| {
                c.new
                    .as_ref()
                    .map(|r| r.kind == "branch")
                    .unwrap_or(false)
            })
            .and_then(|c| c.new)
            .ok_or_else(|| WebhookError::UnsupportedEvent("no branch change".to_string()))?;

        Ok(WebhookEvent {
            repo_url: normalize_repo_url(&payload.repository.links.html.href),
            branch: change.name,
            commit_sha: change.target.hash,
        })
    }
}

// ------------------------------------------------------------------- Generic

pub struct GenericGit;

#[async_trait]
impl GitProvider for GenericGit {
    fn name(&self) -> &'static str {
        "generic"
    }

    async fn check_head(
        &self,
        http: &reqwest::Client,
        repo: &str,
        branch: &str,
        token: &str,
    ) -> anyhow::Result<String> {
        smart_http_head(http, repo, branch, token).await
    }

    async fn list_branches(
        &self,
        http: &reqwest::Client,
        repo: &str,
        token: &str,
    ) -> anyhow::Result<(Vec<String>, Option<String>)> {
        smart_http_branches(http, repo, token).await
    }

    fn verify_webhook(
        &self,
        _headers: &HeaderMap,
        _body: &[u8],
        _secret: &str,
    ) -> Result<WebhookEvent, WebhookError> {
        // Generic Git hosts don't share a webhook payload format. If an
        // operator's host uses GitHub-shaped payloads (Gitea does), they
        // should register it with a github.com hostname alias or extend
        // `detect_provider` — we deliberately don't guess here because
        // guessing wrong would silently accept unsigned payloads.
        Err(WebhookError::UnsupportedEvent(
            "generic git host does not support webhooks; use polling".to_string(),
        ))
    }
}

// --------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn detect_provider_github() {
        assert_eq!(detect_provider("https://github.com/foo/bar").name(), "github");
        assert_eq!(detect_provider("https://GitHub.com/foo/bar.git").name(), "github");
        assert_eq!(detect_provider("git@github.com:foo/bar.git").name(), "github");
    }

    #[test]
    fn detect_provider_gitlab() {
        assert_eq!(detect_provider("https://gitlab.com/foo/bar").name(), "gitlab");
        assert_eq!(
            detect_provider("https://gitlab.example.com/foo/bar").name(),
            "gitlab"
        );
    }

    #[test]
    fn detect_provider_bitbucket() {
        assert_eq!(
            detect_provider("https://bitbucket.org/foo/bar").name(),
            "bitbucket"
        );
    }

    #[test]
    fn detect_provider_generic() {
        assert_eq!(
            detect_provider("https://git.mycompany.io/foo/bar").name(),
            "generic"
        );
    }

    #[test]
    fn detect_provider_path_containing_github_word_is_generic() {
        // Regression: earlier version used substring match and this URL
        // would incorrectly select GitHub because the string "github"
        // appears in the path.
        assert_eq!(
            detect_provider("https://git.example.com/org/github-mirror").name(),
            "generic"
        );
    }

    #[test]
    fn normalize_repo_url_strips_git_suffix_and_slash() {
        assert_eq!(
            normalize_repo_url("https://github.com/foo/bar.git/"),
            "https://github.com/foo/bar"
        );
        assert_eq!(
            normalize_repo_url("https://github.com/foo/bar"),
            "https://github.com/foo/bar"
        );
    }

    #[test]
    fn hex_decode_valid_and_invalid() {
        assert_eq!(hex_decode("deadbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(hex_decode("DEADBEEF"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(hex_decode("odd"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn github_verifies_valid_signature() {
        let secret = "It's a Secret to Everybody";
        let push_body =
            br#"{"ref":"refs/heads/main","after":"abcdef123","repository":{"clone_url":"https://github.com/foo/bar.git"}}"#;

        // Compute the sig over the actual payload we'll deliver — the
        // GitHub docs example vector is a nice sanity check for the
        // primitive but doesn't cover the payload-parse path.
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(push_body);
        let hex_sig: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
        );
        headers.insert("x-github-event", HeaderValue::from_static("push"));

        let ev = GitHub.verify_webhook(&headers, push_body, secret).unwrap();
        assert_eq!(ev.branch, "main");
        assert_eq!(ev.commit_sha, "abcdef123");
        // normalize_repo_url strips the `.git` from the clone URL.
        assert_eq!(ev.repo_url, "https://github.com/foo/bar");
    }

    #[test]
    fn github_rejects_bad_signature() {
        let secret = "topsecret";
        let body = b"{}";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_static("sha256=deadbeef"),
        );
        headers.insert("x-github-event", HeaderValue::from_static("push"));
        assert!(matches!(
            GitHub.verify_webhook(&headers, body, secret),
            Err(WebhookError::InvalidSignature)
        ));
    }

    #[test]
    fn github_rejects_missing_signature() {
        let headers = HeaderMap::new();
        assert!(matches!(
            GitHub.verify_webhook(&headers, b"{}", "s"),
            Err(WebhookError::MissingSignature)
        ));
    }

    #[test]
    fn github_non_push_event_returns_unsupported() {
        let secret = "s";
        let body = b"{}";
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let hex_sig: String = mac.finalize().into_bytes().iter().map(|b| format!("{:02x}", b)).collect();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
        );
        headers.insert("x-github-event", HeaderValue::from_static("ping"));
        assert!(matches!(
            GitHub.verify_webhook(&headers, body, secret),
            Err(WebhookError::UnsupportedEvent(_))
        ));
    }

    #[test]
    fn gitlab_verifies_matching_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", HeaderValue::from_static("t0psecret"));
        headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));
        let body = br#"{"ref":"refs/heads/main","after":"deadbeef","project":{"git_http_url":"https://gitlab.com/foo/bar.git"}}"#;
        let ev = GitLab.verify_webhook(&headers, body, "t0psecret").unwrap();
        assert_eq!(ev.branch, "main");
        assert_eq!(ev.repo_url, "https://gitlab.com/foo/bar");
    }

    #[test]
    fn gitlab_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", HeaderValue::from_static("wrong"));
        headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));
        assert!(matches!(
            GitLab.verify_webhook(&headers, b"{}", "right"),
            Err(WebhookError::InvalidSignature)
        ));
    }

    #[test]
    fn generic_webhook_unsupported() {
        let headers = HeaderMap::new();
        assert!(matches!(
            GenericGit.verify_webhook(&headers, b"", ""),
            Err(WebhookError::UnsupportedEvent(_))
        ));
    }
}
