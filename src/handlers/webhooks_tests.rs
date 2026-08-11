// Unit tests for src/handlers/webhooks.rs — webhook payload parsing and helpers.

use super::*;
use axum::http::HeaderMap;

// ---- extract_event_unverified: GitHub push ----

#[test]
fn extract_github_push_event() {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-event", "push".parse().unwrap());

    let body = serde_json::json!({
        "ref": "refs/heads/main",
        "after": "abc123def456",
        "repository": {
            "clone_url": "https://github.com/org/repo.git"
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let event = extract_event_unverified("github", &headers, &bytes).unwrap();
    assert_eq!(event.branch, "main");
    assert_eq!(event.commit_sha, "abc123def456");
    // normalize_repo_url strips .git
    assert!(!event.repo_url.ends_with(".git"));
}

#[test]
fn extract_github_push_feature_branch() {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-event", "push".parse().unwrap());

    let body = serde_json::json!({
        "ref": "refs/heads/feature/my-feature",
        "after": "deadbeef",
        "repository": {
            "clone_url": "https://github.com/org/repo.git"
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let event = extract_event_unverified("github", &headers, &bytes).unwrap();
    assert_eq!(event.branch, "feature/my-feature");
}

#[test]
fn extract_github_rejects_non_branch_ref() {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-event", "push".parse().unwrap());

    let body = serde_json::json!({
        "ref": "refs/tags/v1.0.0",
        "after": "abc",
        "repository": {
            "clone_url": "https://github.com/org/repo.git"
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let result = extract_event_unverified("github", &headers, &bytes);
    assert!(result.is_err());
}

#[test]
fn extract_github_rejects_non_push_event() {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-event", "pull_request".parse().unwrap());

    let body = serde_json::json!({});
    let bytes = serde_json::to_vec(&body).unwrap();

    let result = extract_event_unverified("github", &headers, &bytes);
    assert!(result.is_err());
}

// ---- extract_event_unverified: GitLab push ----

#[test]
fn extract_gitlab_push_event() {
    let mut headers = HeaderMap::new();
    headers.insert("x-gitlab-event", "Push Hook".parse().unwrap());

    let body = serde_json::json!({
        "ref": "refs/heads/develop",
        "after": "gitlab_sha_abc",
        "project": {
            "git_http_url": "https://gitlab.com/org/repo.git"
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let event = extract_event_unverified("gitlab", &headers, &bytes).unwrap();
    assert_eq!(event.branch, "develop");
    assert_eq!(event.commit_sha, "gitlab_sha_abc");
}

#[test]
fn extract_gitlab_rejects_non_push_hook() {
    let mut headers = HeaderMap::new();
    headers.insert("x-gitlab-event", "Merge Request Hook".parse().unwrap());

    let body = serde_json::json!({});
    let bytes = serde_json::to_vec(&body).unwrap();

    let result = extract_event_unverified("gitlab", &headers, &bytes);
    assert!(result.is_err());
}

// ---- extract_event_unverified: Bitbucket push ----

#[test]
fn extract_bitbucket_push_event() {
    let mut headers = HeaderMap::new();
    headers.insert("x-event-key", "repo:push".parse().unwrap());

    let body = serde_json::json!({
        "repository": {
            "links": {
                "html": {
                    "href": "https://bitbucket.org/org/repo"
                }
            }
        },
        "push": {
            "changes": [{
                "new": {
                    "name": "main",
                    "type": "branch",
                    "target": {
                        "hash": "bb_sha_123"
                    }
                }
            }]
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let event = extract_event_unverified("bitbucket", &headers, &bytes).unwrap();
    assert_eq!(event.branch, "main");
    assert_eq!(event.commit_sha, "bb_sha_123");
}

#[test]
fn extract_bitbucket_rejects_tag_push() {
    let mut headers = HeaderMap::new();
    headers.insert("x-event-key", "repo:push".parse().unwrap());

    let body = serde_json::json!({
        "repository": {
            "links": { "html": { "href": "https://bitbucket.org/org/repo" } }
        },
        "push": {
            "changes": [{
                "new": {
                    "name": "v1.0",
                    "type": "tag",
                    "target": { "hash": "abc" }
                }
            }]
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap();

    let result = extract_event_unverified("bitbucket", &headers, &bytes);
    assert!(result.is_err());
}

#[test]
fn extract_bitbucket_rejects_non_push_event() {
    let mut headers = HeaderMap::new();
    headers.insert("x-event-key", "repo:fork".parse().unwrap());

    let body = serde_json::json!({});
    let bytes = serde_json::to_vec(&body).unwrap();

    let result = extract_event_unverified("bitbucket", &headers, &bytes);
    assert!(result.is_err());
}

// ---- extract_event_unverified: unknown provider ----

#[test]
fn extract_unknown_provider_errors() {
    let headers = HeaderMap::new();
    let result = extract_event_unverified("svn", &headers, b"{}");
    assert!(result.is_err());
}

// ---- WebhookResponse serialization ----

#[test]
fn webhook_response_serializes_triggered_and_skipped() {
    let resp = WebhookResponse {
        triggered: vec![TriggeredBuild {
            namespace: "prod".to_string(),
            deployment: "api".to_string(),
            job_name: "build-api-abc".to_string(),
        }],
        skipped: vec![SkippedDeployment {
            namespace: "staging".to_string(),
            deployment: "web".to_string(),
            reason: "signature mismatch".to_string(),
        }],
        provider: "github".to_string(),
        commit_sha: "deadbeef".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["provider"], "github");
    assert_eq!(json["commit_sha"], "deadbeef");
    assert_eq!(json["triggered"][0]["namespace"], "prod");
    assert_eq!(json["triggered"][0]["job_name"], "build-api-abc");
    assert_eq!(json["skipped"][0]["reason"], "signature mismatch");
}

#[test]
fn webhook_response_serializes_empty_lists() {
    let resp = WebhookResponse {
        triggered: Vec::new(),
        skipped: Vec::new(),
        provider: "gitlab".to_string(),
        commit_sha: "abc".to_string(),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["triggered"].as_array().unwrap().is_empty());
    assert!(json["skipped"].as_array().unwrap().is_empty());
}
