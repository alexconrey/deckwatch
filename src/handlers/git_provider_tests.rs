use super::*;
use axum::http::HeaderValue;

// --------------------------------------------------------- url_host

#[test]
fn url_host_https() {
    assert_eq!(
        url_host("https://github.com/foo/bar"),
        Some("github.com".to_string())
    );
}

#[test]
fn url_host_http() {
    assert_eq!(
        url_host("http://gitlab.example.com/foo/bar"),
        Some("gitlab.example.com".to_string())
    );
}

#[test]
fn url_host_ssh() {
    assert_eq!(
        url_host("git@github.com:foo/bar.git"),
        Some("github.com".to_string())
    );
}

#[test]
fn url_host_with_port() {
    assert_eq!(
        url_host("https://git.local:8443/repo"),
        Some("git.local".to_string())
    );
}

#[test]
fn url_host_with_user_at() {
    assert_eq!(
        url_host("https://user@git.example.com/repo"),
        Some("git.example.com".to_string())
    );
}

#[test]
fn url_host_uppercase_normalized() {
    assert_eq!(
        url_host("https://GitHub.COM/foo/bar"),
        Some("github.com".to_string())
    );
}

#[test]
fn url_host_unknown_scheme_returns_none() {
    assert_eq!(url_host("ftp://example.com/repo"), None);
}

#[test]
fn url_host_empty_returns_none() {
    assert_eq!(url_host(""), None);
}

// ------------------------------------------------- split_github_repo

#[test]
fn split_github_repo_https() {
    assert_eq!(
        split_github_repo("https://github.com/octocat/Hello-World"),
        Some(("octocat".to_string(), "Hello-World".to_string()))
    );
}

#[test]
fn split_github_repo_with_git_suffix() {
    assert_eq!(
        split_github_repo("https://github.com/octocat/Hello-World.git"),
        Some(("octocat".to_string(), "Hello-World".to_string()))
    );
}

#[test]
fn split_github_repo_trailing_slash() {
    assert_eq!(
        split_github_repo("https://github.com/foo/bar/"),
        Some(("foo".to_string(), "bar".to_string()))
    );
}

#[test]
fn split_github_repo_trailing_slash_and_git() {
    assert_eq!(
        split_github_repo("https://github.com/foo/bar.git/"),
        Some(("foo".to_string(), "bar".to_string()))
    );
}

#[test]
fn split_github_repo_bare_string_returns_none() {
    assert_eq!(split_github_repo("justarepo"), None);
}

// ------------------------------------------------- normalize_repo_url

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
fn normalize_repo_url_only_trailing_slash() {
    assert_eq!(
        normalize_repo_url("https://github.com/foo/bar/"),
        "https://github.com/foo/bar"
    );
}

#[test]
fn normalize_repo_url_dot_git_no_slash() {
    assert_eq!(
        normalize_repo_url("https://github.com/foo/bar.git"),
        "https://github.com/foo/bar"
    );
}

#[test]
fn normalize_repo_url_noop() {
    assert_eq!(
        normalize_repo_url("https://github.com/foo/bar"),
        "https://github.com/foo/bar"
    );
}

// ------------------------------------------------------- hex helpers

#[test]
fn hex_decode_valid_and_invalid() {
    assert_eq!(hex_decode("deadbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(hex_decode("DEADBEEF"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(hex_decode("odd"), None);
    assert_eq!(hex_decode("zz"), None);
}

#[test]
fn hex_decode_empty_string() {
    assert_eq!(hex_decode(""), Some(vec![]));
}

#[test]
fn hex_decode_mixed_case() {
    assert_eq!(hex_decode("aAbB"), Some(vec![0xaa, 0xbb]));
}

#[test]
fn hex_nibble_digits() {
    for (ch, val) in (b'0'..=b'9').zip(0u8..=9) {
        assert_eq!(hex_nibble(ch), Some(val));
    }
}

#[test]
fn hex_nibble_lowercase() {
    for (ch, val) in (b'a'..=b'f').zip(10u8..=15) {
        assert_eq!(hex_nibble(ch), Some(val));
    }
}

#[test]
fn hex_nibble_uppercase() {
    for (ch, val) in (b'A'..=b'F').zip(10u8..=15) {
        assert_eq!(hex_nibble(ch), Some(val));
    }
}

#[test]
fn hex_nibble_invalid() {
    assert_eq!(hex_nibble(b'g'), None);
    assert_eq!(hex_nibble(b'G'), None);
    assert_eq!(hex_nibble(b' '), None);
    assert_eq!(hex_nibble(b'/'), None);
}

// --------------------------------------------------- detect_provider

#[test]
fn detect_provider_github() {
    assert_eq!(
        detect_provider("https://github.com/foo/bar").name(),
        "github"
    );
    assert_eq!(
        detect_provider("https://GitHub.com/foo/bar.git").name(),
        "github"
    );
    assert_eq!(
        detect_provider("git@github.com:foo/bar.git").name(),
        "github"
    );
}

#[test]
fn detect_provider_gitlab() {
    assert_eq!(
        detect_provider("https://gitlab.com/foo/bar").name(),
        "gitlab"
    );
    assert_eq!(
        detect_provider("https://gitlab.example.com/foo/bar").name(),
        "gitlab"
    );
}

#[test]
fn detect_provider_gitlab_ssh() {
    assert_eq!(
        detect_provider("git@gitlab.com:foo/bar.git").name(),
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
fn detect_provider_bitbucket_self_hosted() {
    assert_eq!(
        detect_provider("https://bitbucket.mycompany.com/scm/proj/repo").name(),
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
    assert_eq!(
        detect_provider("https://git.example.com/org/github-mirror").name(),
        "generic"
    );
}

#[test]
fn detect_provider_github_subdomain() {
    // Enterprise cloud instances like foo.github.com should map to GitHub.
    assert_eq!(
        detect_provider("https://enterprise.github.com/org/repo").name(),
        "github"
    );
}

// ------------------------------------------------ verify_hmac_sha256

#[test]
fn verify_hmac_sha256_valid() {
    let secret = "mysecret";
    let body = b"hello world";
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let hex_sig: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    assert!(verify_hmac_sha256(secret, body, &hex_sig).is_ok());
}

#[test]
fn verify_hmac_sha256_wrong_digest() {
    assert!(matches!(
        verify_hmac_sha256(
            "secret",
            b"body",
            "0000000000000000000000000000000000000000000000000000000000000000"
        ),
        Err(WebhookError::InvalidSignature)
    ));
}

#[test]
fn verify_hmac_sha256_empty_secret() {
    assert!(matches!(
        verify_hmac_sha256("", b"body", "deadbeef"),
        Err(WebhookError::InvalidSignature)
    ));
}

#[test]
fn verify_hmac_sha256_bad_hex() {
    assert!(matches!(
        verify_hmac_sha256("secret", b"body", "zzzz"),
        Err(WebhookError::InvalidSignature)
    ));
}

// ---------------------------------------- GitHub webhook verification

fn compute_hmac_hex(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[test]
fn github_verifies_valid_signature() {
    let secret = "It's a Secret to Everybody";
    let push_body =
        br#"{"ref":"refs/heads/main","after":"abcdef123","repository":{"clone_url":"https://github.com/foo/bar.git"}}"#;
    let hex_sig = compute_hmac_hex(secret, push_body);

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature-256",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-github-event", HeaderValue::from_static("push"));

    let ev = GitHub.verify_webhook(&headers, push_body, secret).unwrap();
    assert_eq!(ev.branch, "main");
    assert_eq!(ev.commit_sha, "abcdef123");
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
    let hex_sig = compute_hmac_hex(secret, body);
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
fn github_tag_push_returns_unsupported() {
    let secret = "s";
    let body = br#"{"ref":"refs/tags/v1.0","after":"abc123","repository":{"clone_url":"https://github.com/foo/bar.git"}}"#;
    let hex_sig = compute_hmac_hex(secret, body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature-256",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-github-event", HeaderValue::from_static("push"));
    // Tag pushes use refs/tags/... which can't be stripped as a branch name.
    assert!(matches!(
        GitHub.verify_webhook(&headers, body, secret),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

#[test]
fn github_malformed_json_returns_error() {
    let secret = "s";
    let body = b"not json";
    let hex_sig = compute_hmac_hex(secret, body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature-256",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-github-event", HeaderValue::from_static("push"));
    assert!(matches!(
        GitHub.verify_webhook(&headers, body, secret),
        Err(WebhookError::MalformedPayload(_))
    ));
}

// ---------------------------------------- GitLab webhook verification

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
fn gitlab_rejects_missing_token() {
    let headers = HeaderMap::new();
    assert!(matches!(
        GitLab.verify_webhook(&headers, b"{}", "secret"),
        Err(WebhookError::MissingSignature)
    ));
}

#[test]
fn gitlab_rejects_empty_secret() {
    let mut headers = HeaderMap::new();
    headers.insert("x-gitlab-token", HeaderValue::from_static("token"));
    headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));
    assert!(matches!(
        GitLab.verify_webhook(&headers, b"{}", ""),
        Err(WebhookError::InvalidSignature)
    ));
}

#[test]
fn gitlab_non_push_event_returns_unsupported() {
    let mut headers = HeaderMap::new();
    headers.insert("x-gitlab-token", HeaderValue::from_static("secret"));
    headers.insert(
        "x-gitlab-event",
        HeaderValue::from_static("Merge Request Hook"),
    );
    assert!(matches!(
        GitLab.verify_webhook(&headers, b"{}", "secret"),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

#[test]
fn gitlab_tag_push_returns_unsupported() {
    let mut headers = HeaderMap::new();
    headers.insert("x-gitlab-token", HeaderValue::from_static("secret"));
    headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));
    let body = br#"{"ref":"refs/tags/v1.0","after":"abc123","project":{"git_http_url":"https://gitlab.com/foo/bar.git"}}"#;
    assert!(matches!(
        GitLab.verify_webhook(&headers, body, "secret"),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

// ------------------------------------- Bitbucket webhook verification

#[test]
fn bitbucket_verifies_valid_signature() {
    let secret = "bb-secret";
    let body = br#"{
        "repository": {"links": {"html": {"href": "https://bitbucket.org/team/repo"}}},
        "push": {"changes": [{"new": {"name": "main", "type": "branch", "target": {"hash": "cafebabe"}}}]}
    }"#;
    let hex_sig = compute_hmac_hex(secret, body);

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-event-key", HeaderValue::from_static("repo:push"));

    let ev = Bitbucket.verify_webhook(&headers, body, secret).unwrap();
    assert_eq!(ev.branch, "main");
    assert_eq!(ev.commit_sha, "cafebabe");
    assert_eq!(ev.repo_url, "https://bitbucket.org/team/repo");
}

#[test]
fn bitbucket_rejects_bad_signature() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature",
        HeaderValue::from_static(
            "sha256=0000000000000000000000000000000000000000000000000000000000000000",
        ),
    );
    headers.insert("x-event-key", HeaderValue::from_static("repo:push"));
    assert!(matches!(
        Bitbucket.verify_webhook(&headers, b"{}", "secret"),
        Err(WebhookError::InvalidSignature)
    ));
}

#[test]
fn bitbucket_rejects_missing_signature() {
    let headers = HeaderMap::new();
    assert!(matches!(
        Bitbucket.verify_webhook(&headers, b"{}", "secret"),
        Err(WebhookError::MissingSignature)
    ));
}

#[test]
fn bitbucket_non_push_event_returns_unsupported() {
    let secret = "s";
    let body = b"{}";
    let hex_sig = compute_hmac_hex(secret, body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-event-key", HeaderValue::from_static("repo:fork"));
    assert!(matches!(
        Bitbucket.verify_webhook(&headers, body, secret),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

#[test]
fn bitbucket_no_branch_change_returns_unsupported() {
    let secret = "s";
    // Push with only tag changes (no branch).
    let body = br#"{
        "repository": {"links": {"html": {"href": "https://bitbucket.org/team/repo"}}},
        "push": {"changes": [{"new": {"name": "v1.0", "type": "tag", "target": {"hash": "abc123"}}}]}
    }"#;
    let hex_sig = compute_hmac_hex(secret, body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hub-signature",
        HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
    );
    headers.insert("x-event-key", HeaderValue::from_static("repo:push"));
    assert!(matches!(
        Bitbucket.verify_webhook(&headers, body, secret),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

// ----------------------------------------------- Generic Git provider

#[test]
fn generic_webhook_unsupported() {
    let headers = HeaderMap::new();
    assert!(matches!(
        GenericGit.verify_webhook(&headers, b"", ""),
        Err(WebhookError::UnsupportedEvent(_))
    ));
}

// ------------------------------------------- WebhookEvent PartialEq

#[test]
fn webhook_event_equality() {
    let a = WebhookEvent {
        repo_url: "https://github.com/foo/bar".to_string(),
        branch: "main".to_string(),
        commit_sha: "abc123".to_string(),
    };
    let b = a.clone();
    assert_eq!(a, b);
}

// -------------------------------------------- Provider trait object

#[test]
fn provider_names_are_distinct() {
    let providers: Vec<Box<dyn GitProvider>> = vec![
        Box::new(GitHub),
        Box::new(GitLab),
        Box::new(Bitbucket),
        Box::new(GenericGit),
    ];
    let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
    // All names should be unique.
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names.len(), sorted.len());
}
