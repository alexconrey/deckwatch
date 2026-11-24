use super::*;
use crate::outbound::{EndpointBinding, EndpointId, EndpointPolicy, EndpointSettings};

fn web(raw: &str) -> Endpoint {
    Endpoint::parse(EndpointId::GithubWeb, raw, &EndpointPolicy::default()).expect("valid")
}

fn api(raw: &str) -> Endpoint {
    Endpoint::parse(EndpointId::GithubApi, raw, &EndpointPolicy::default()).expect("valid")
}

#[test]
fn repo_on_host_parses_github_com_urls() {
    let github = Endpoint::default_for(EndpointId::GithubWeb);
    for url in [
        "https://github.com/acme/api",
        "https://github.com/acme/api.git",
        "https://github.com/acme/api/",
        "https://GitHub.com/acme/api.git",
        "https://x-access-token@github.com/acme/api.git",
    ] {
        assert_eq!(
            repo_on_host(url, &github),
            Some(("acme".to_string(), "api".to_string())),
            "{url}"
        );
    }
}

#[test]
fn repo_on_host_ignores_other_hosts() {
    let github = Endpoint::default_for(EndpointId::GithubWeb);
    for url in [
        "https://gitlab.com/acme/api",
        "https://github.com.evil.example/acme/api",
        "https://evil.example/github.com/acme/api",
        "https://github.com/acme",
        "not a url",
    ] {
        assert_eq!(repo_on_host(url, &github), None, "{url}");
    }
}

#[test]
fn repo_on_host_follows_github_enterprise() {
    let ghe = web("https://ghe.example.com");
    assert_eq!(
        repo_on_host("https://ghe.example.com/platform/deckwatch.git", &ghe),
        Some(("platform".to_string(), "deckwatch".to_string()))
    );
    assert_eq!(repo_on_host("https://github.com/acme/api", &ghe), None);

    let with_port = web("https://ghe.example.com:8443");
    assert!(repo_on_host("https://ghe.example.com:8443/o/r", &with_port).is_some());
    assert!(repo_on_host("https://ghe.example.com/o/r", &with_port).is_none());
}

#[test]
fn token_is_attached_when_bound_to_the_api_host() {
    let http = crate::outbound::OutboundHttp::default().client(crate::outbound::Purpose::Git);
    let endpoint = api("https://ghe.example.com/api/v3");
    let binding = EndpointBinding::capture(
        &EndpointSettings::default().with(EndpointId::GithubApi, "https://ghe.example.com/api/v3"),
        &[EndpointId::GithubApi],
        &EndpointPolicy::default(),
    )
    .unwrap();
    let source = CredentialSource::Stored(Some(binding));
    let token = GitHubToken {
        token: "ghp_secret",
        label: "git token 'gh'",
        source: &source,
    };
    let req = request(&http, &endpoint, "/repos/o/r", Some(token))
        .expect("bound")
        .build()
        .expect("request");
    assert_eq!(
        req.url().as_str(),
        "https://ghe.example.com/api/v3/repos/o/r"
    );
    assert_eq!(req.headers()["authorization"], "Bearer ghp_secret");
    assert_eq!(req.headers()["accept"], GITHUB_JSON);
}

#[test]
fn token_bound_elsewhere_is_refused_before_a_request_exists() {
    let http = crate::outbound::OutboundHttp::default().client(crate::outbound::Purpose::Git);
    let endpoint = api("https://ghe.example.com/api/v3");
    // Saved while the default (api.github.com) was configured.
    let source = CredentialSource::Stored(None);
    let token = GitHubToken {
        token: "ghp_secret",
        label: "git token 'gh'",
        source: &source,
    };
    let err = request(&http, &endpoint, "/repos/o/r", Some(token)).expect_err("refused");
    let msg = err.to_string();
    assert!(msg.contains("api.github.com"), "{msg}");
    assert!(msg.contains("ghe.example.com"), "{msg}");
}

#[test]
fn empty_token_is_not_sent_or_checked() {
    let http = crate::outbound::OutboundHttp::default().client(crate::outbound::Purpose::Git);
    let endpoint = api("https://ghe.example.com/api/v3");
    let source = CredentialSource::Stored(None);
    let token = GitHubToken {
        token: "",
        label: "git token",
        source: &source,
    };
    let req = request(&http, &endpoint, "/repos/o/r", Some(token))
        .expect("anonymous")
        .build()
        .expect("request");
    assert!(req.headers().get("authorization").is_none());
}
