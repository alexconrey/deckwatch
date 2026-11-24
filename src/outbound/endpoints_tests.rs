use super::*;

fn strict() -> EndpointPolicy {
    EndpointPolicy::default()
}

fn insecure() -> EndpointPolicy {
    EndpointPolicy {
        allow_insecure: true,
    }
}

fn parse(id: EndpointId, raw: &str) -> Result<Endpoint, EndpointError> {
    Endpoint::parse(id, raw, &strict())
}

#[test]
fn every_default_is_valid_https() {
    for id in EndpointId::ALL {
        let ep = Endpoint::default_for(id);
        assert!(ep.is_default(), "{id:?}");
        assert_eq!(ep.base(), id.default_url(), "{id:?}");
        assert!(ep.origin().starts_with("https://"), "{id:?}");
        assert!(!ep.is_loopback(), "{id:?}");
    }
}

#[test]
fn defaults_match_the_urls_deckwatch_used_before() {
    assert_eq!(
        Endpoint::default_for(EndpointId::AnthropicApi).url("/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::VertexToken).url(""),
        "https://oauth2.googleapis.com/token"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::VertexApi)
            .url_for_region("us-central1", "/v1")
            .unwrap(),
        "https://us-central1-aiplatform.googleapis.com/v1"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::Bedrock)
            .url_for_region("us-gov-west-1", "")
            .unwrap(),
        "https://bedrock-runtime.us-gov-west-1.amazonaws.com"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::GithubApi).url("/repos/o/r"),
        "https://api.github.com/repos/o/r"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::GithubWeb).base(),
        "https://github.com"
    );
    assert_eq!(
        Endpoint::default_for(EndpointId::GithubRaw).base(),
        "https://raw.githubusercontent.com"
    );
}

#[test]
fn empty_and_whitespace_mean_default() {
    for raw in ["", "   "] {
        assert!(parse(EndpointId::AnthropicApi, raw).unwrap().is_default());
    }
}

#[test]
fn spelling_the_default_differently_still_counts_as_default() {
    let ep = parse(EndpointId::AnthropicApi, " HTTPS://API.Anthropic.com:443/ ").unwrap();
    assert!(ep.is_default());
    assert_eq!(ep.base(), "https://api.anthropic.com");
    let settings =
        EndpointSettings::default().with(EndpointId::AnthropicApi, "https://api.anthropic.com/");
    assert_eq!(
        settings.validated(&strict()).unwrap(),
        EndpointSettings::default()
    );
}

#[test]
fn custom_base_keeps_its_path_without_trailing_slash() {
    let ep = parse(
        EndpointId::AnthropicApi,
        "https://LLM-Gateway.example.com:8443/anthropic/",
    )
    .unwrap();
    assert!(!ep.is_default());
    assert_eq!(ep.base(), "https://llm-gateway.example.com:8443/anthropic");
    assert_eq!(ep.origin(), "https://llm-gateway.example.com:8443");
    assert_eq!(ep.host(), "llm-gateway.example.com:8443");
    assert_eq!(
        ep.url("/v1/messages"),
        "https://llm-gateway.example.com:8443/anthropic/v1/messages"
    );
}

#[test]
fn github_enterprise_bases() {
    let api = parse(EndpointId::GithubApi, "https://ghe.example.com/api/v3").unwrap();
    assert_eq!(
        api.url("/repos/o/r"),
        "https://ghe.example.com/api/v3/repos/o/r"
    );
    let raw = parse(EndpointId::GithubRaw, "https://ghe.example.com/raw").unwrap();
    assert_eq!(raw.origin(), "https://ghe.example.com");
}

#[test]
fn http_is_rejected_for_remote_hosts() {
    let err = parse(EndpointId::AnthropicApi, "http://llm-gateway.example.com").unwrap_err();
    assert!(matches!(err, EndpointError::Insecure { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("Anthropic API"), "{msg}");
    assert!(msg.contains("DECKWATCH_ALLOW_INSECURE_ENDPOINTS"), "{msg}");
    assert!(msg.contains("https://llm-gateway.example.com"), "{msg}");
}

#[test]
fn http_is_allowed_for_loopback() {
    for raw in [
        "http://localhost:8080",
        "http://LOCALHOST",
        "http://127.0.0.1:9000",
        "http://127.1.2.3",
        "http://[::1]:8443",
    ] {
        let ep = parse(EndpointId::AnthropicApi, raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
        assert!(ep.is_loopback(), "{raw}");
    }
}

#[test]
fn loopback_lookalikes_are_not_loopback() {
    for raw in [
        "http://localhost.example.com",
        "http://127.0.0.1.nip.io",
        "http://10.0.0.1",
    ] {
        assert!(
            matches!(
                parse(EndpointId::AnthropicApi, raw),
                Err(EndpointError::Insecure { .. })
            ),
            "{raw}"
        );
    }
}

#[test]
fn allow_insecure_accepts_http_anywhere() {
    let ep = Endpoint::parse(
        EndpointId::GithubApi,
        "http://ghe.lab.internal/api/v3",
        &insecure(),
    )
    .unwrap();
    assert_eq!(ep.origin(), "http://ghe.lab.internal");
}

#[test]
fn userinfo_is_rejected() {
    for raw in [
        "https://user:pass@api.example.com",
        "https://token@api.example.com",
    ] {
        assert!(
            matches!(
                parse(EndpointId::AnthropicApi, raw),
                Err(EndpointError::UserInfo { .. })
            ),
            "{raw}"
        );
    }
}

#[test]
fn query_and_fragment_are_rejected() {
    for raw in [
        "https://api.example.com/?x=1",
        "https://api.example.com/#frag",
        "https://api.example.com?",
    ] {
        assert!(
            matches!(
                parse(EndpointId::AnthropicApi, raw),
                Err(EndpointError::QueryOrFragment { .. })
            ),
            "{raw}"
        );
    }
}

#[test]
fn other_schemes_and_relative_values_are_rejected() {
    assert!(matches!(
        parse(EndpointId::AnthropicApi, "ftp://api.example.com"),
        Err(EndpointError::Scheme { .. })
    ));
    assert!(matches!(
        parse(EndpointId::AnthropicApi, "file:///etc/passwd"),
        Err(EndpointError::Scheme { .. })
    ));
    assert!(matches!(
        parse(EndpointId::AnthropicApi, "api.example.com"),
        Err(EndpointError::Invalid { .. })
    ));
    assert!(matches!(
        parse(EndpointId::AnthropicApi, "https://"),
        Err(EndpointError::Invalid { .. })
    ));
}

#[test]
fn overlong_values_are_rejected() {
    let raw = format!("https://example.com/{}", "a".repeat(MAX_ENDPOINT_LEN));
    assert!(matches!(
        parse(EndpointId::AnthropicApi, &raw),
        Err(EndpointError::TooLong { .. })
    ));
}

#[test]
fn region_placeholder_rules() {
    let vertex = parse(
        EndpointId::VertexApi,
        "https://{region}-aiplatform.example.com",
    )
    .unwrap();
    assert!(vertex.is_templated());
    assert_eq!(vertex.origin(), "https://{region}-aiplatform.example.com");
    assert_eq!(vertex.host(), "{region}-aiplatform.example.com");
    assert_eq!(
        vertex.host_for_region("europe-west4").unwrap(),
        "europe-west4-aiplatform.example.com"
    );

    let fixed = parse(EndpointId::Bedrock, "https://bedrock.vpce.amazonaws.com").unwrap();
    assert!(!fixed.is_templated());

    assert!(matches!(
        parse(EndpointId::AnthropicApi, "https://{region}.example.com"),
        Err(EndpointError::RegionPlaceholder { .. })
    ));
    assert!(matches!(
        parse(EndpointId::VertexApi, "https://example.com/{region}"),
        Err(EndpointError::RegionPlaceholder { .. })
    ));
    assert!(matches!(
        parse(
            EndpointId::VertexApi,
            "https://{region}.{region}.example.com"
        ),
        Err(EndpointError::RegionPlaceholder { .. })
    ));
}

#[test]
fn regions_must_be_one_dns_label() {
    for ok in [
        "us-east-1",
        "us-central1",
        "europe-west4",
        "us-gov-west-1",
        "global",
    ] {
        assert!(validate_region(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "US-EAST-1",
        "-us",
        "us-",
        "us.east",
        "evil.example.com/",
        "x#",
        "a b",
        &"a".repeat(64),
    ] {
        assert!(validate_region(bad).is_err(), "{bad:?}");
    }
    // Even for endpoints without {region}, a malformed region never reaches
    // the URL.
    assert!(Endpoint::default_for(EndpointId::AnthropicApi)
        .url_for_region("../x", "")
        .is_err());
}

#[test]
fn validated_reports_every_bad_field() {
    let settings = EndpointSettings::default()
        .with(EndpointId::AnthropicApi, "http://gateway.example.com")
        .with(EndpointId::GithubApi, "https://user@ghe.example.com")
        .with(EndpointId::GithubWeb, "https://ghe.example.com/");
    let errors = settings.validated(&strict()).unwrap_err();
    assert_eq!(errors.0.len(), 2, "{errors}");
    let msg = errors.to_string();
    assert!(msg.contains("Anthropic API"), "{msg}");
    assert!(msg.contains("GitHub API"), "{msg}");
}

#[test]
fn validated_normalizes_values() {
    let settings = EndpointSettings::default()
        .with(EndpointId::GithubWeb, " https://GHE.example.com/ ")
        .with(EndpointId::GithubApi, "https://ghe.example.com/api/v3/");
    let normalized = settings.validated(&strict()).unwrap();
    assert_eq!(normalized.github_web_base, "https://ghe.example.com");
    assert_eq!(normalized.github_api_base, "https://ghe.example.com/api/v3");
    assert!(!normalized.is_default());
    assert!(EndpointSettings::default().is_default());
}

#[test]
fn settings_serde_round_trip() {
    let parsed: EndpointSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed, EndpointSettings::default());
    let s =
        EndpointSettings::default().with(EndpointId::GithubApi, "https://ghe.example.com/api/v3");
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["github_api_base"], "https://ghe.example.com/api/v3");
    assert_eq!(json["anthropic_api_base"], "");
    assert_eq!(
        serde_json::to_value(EndpointId::AnthropicApi).unwrap(),
        "anthropic_api"
    );
}

#[test]
fn every_endpoint_has_a_matching_setting_field() {
    let json = serde_json::to_value(EndpointSettings::default()).unwrap();
    for id in EndpointId::ALL {
        assert!(json.get(id.setting_name()).is_some(), "{id:?}");
    }
}

#[test]
fn aws_host_detection() {
    for host in [
        "bedrock-runtime.us-east-1.amazonaws.com",
        "bedrock-runtime-fips.us-gov-west-1.amazonaws.com",
        "vpce-0a1b.bedrock-runtime.us-east-1.vpce.amazonaws.com",
        "bedrock-runtime.cn-north-1.amazonaws.com.cn",
        "bedrock-runtime.us-east-1.api.aws",
        "BEDROCK-RUNTIME.US-EAST-1.AMAZONAWS.COM.",
    ] {
        assert!(is_aws_host(host), "{host}");
    }
    for host in [
        "amazonaws.com",
        "evilamazonaws.com",
        "amazonaws.com.evil.example",
        "bedrock.example.com",
        "api.aws.evil.example",
        "localhost",
    ] {
        assert!(!is_aws_host(host), "{host}");
    }
}
