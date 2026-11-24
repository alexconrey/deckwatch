use super::*;

fn policy() -> EndpointPolicy {
    EndpointPolicy::default()
}

fn endpoint(id: EndpointId, raw: &str) -> Endpoint {
    Endpoint::parse(
        id,
        raw,
        &EndpointPolicy {
            allow_insecure: true,
        },
    )
    .expect("valid endpoint")
}

fn bound_to(id: EndpointId, raw: &str) -> CredentialSource {
    let settings = EndpointSettings::default().with(id, raw);
    CredentialSource::Stored(Some(
        EndpointBinding::capture(
            &settings,
            &[id],
            &EndpointPolicy {
                allow_insecure: true,
            },
        )
        .expect("capture"),
    ))
}

#[test]
fn capture_records_the_origin_of_each_endpoint() {
    let settings = EndpointSettings::default()
        .with(EndpointId::GithubApi, "https://ghe.example.com/api/v3")
        .with(EndpointId::GithubRaw, "https://ghe.example.com/raw");
    let binding = EndpointBinding::capture(
        &settings,
        &[
            EndpointId::GithubApi,
            EndpointId::GithubWeb,
            EndpointId::GithubRaw,
        ],
        &policy(),
    )
    .unwrap();
    assert_eq!(
        binding.origin(EndpointId::GithubApi),
        Some("https://ghe.example.com")
    );
    assert_eq!(
        binding.origin(EndpointId::GithubWeb),
        Some("https://github.com")
    );
    assert_eq!(
        binding.origin(EndpointId::GithubRaw),
        Some("https://ghe.example.com")
    );
    assert_eq!(binding.origin(EndpointId::AnthropicApi), None);
    assert_eq!(binding.iter().count(), 3);
}

#[test]
fn capture_refuses_an_invalid_endpoint() {
    let settings =
        EndpointSettings::default().with(EndpointId::AnthropicApi, "http://gateway.example.com");
    assert!(EndpointBinding::capture(&settings, &[EndpointId::AnthropicApi], &policy()).is_err());
}

#[test]
fn defaults_equal_a_capture_of_default_settings() {
    let ids = [EndpointId::VertexToken, EndpointId::VertexApi];
    assert_eq!(
        EndpointBinding::defaults(&ids),
        EndpointBinding::capture(&EndpointSettings::default(), &ids, &policy()).unwrap()
    );
}

#[test]
fn binding_serializes_as_a_map_of_origins() {
    let binding = EndpointBinding::defaults(&[EndpointId::AnthropicApi]);
    assert_eq!(
        serde_json::to_value(&binding).unwrap(),
        serde_json::json!({"anthropic_api": "https://api.anthropic.com"})
    );
    let back: EndpointBinding =
        serde_json::from_value(serde_json::json!({"github_api": "https://ghe.example.com"}))
            .unwrap();
    assert_eq!(
        back.origin(EndpointId::GithubApi),
        Some("https://ghe.example.com")
    );
}

#[test]
fn matching_binding_is_authorized() {
    let ep = endpoint(
        EndpointId::AnthropicApi,
        "https://gateway.example.com/anthropic",
    );
    let source = bound_to(EndpointId::AnthropicApi, "https://gateway.example.com");
    assert!(authorize("Anthropic API key", &source, &ep).is_ok());
}

#[test]
fn binding_is_per_origin_not_per_path() {
    let source = bound_to(
        EndpointId::AnthropicApi,
        "https://gateway.example.com/v1-proxy",
    );
    let ep = endpoint(
        EndpointId::AnthropicApi,
        "https://gateway.example.com/other-path",
    );
    assert!(authorize("Anthropic API key", &source, &ep).is_ok());
}

#[test]
fn different_host_is_refused_with_a_re_enter_message() {
    let source = bound_to(EndpointId::AnthropicApi, "https://gateway-a.example.com");
    let ep = endpoint(EndpointId::AnthropicApi, "https://gateway-b.example.com");
    let err = authorize("Anthropic API key", &source, &ep).unwrap_err();
    assert!(matches!(err, BindingError::HostMismatch { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("saved for gateway-a.example.com"), "{msg}");
    assert!(msg.contains("now gateway-b.example.com"), "{msg}");
    assert!(msg.contains("re-enter the Anthropic API key"), "{msg}");
}

#[test]
fn different_port_or_scheme_is_a_different_host() {
    let source = bound_to(EndpointId::GithubApi, "https://ghe.example.com");
    for raw in ["https://ghe.example.com:8443", "http://ghe.example.com"] {
        let ep = endpoint(EndpointId::GithubApi, raw);
        let err = authorize("git token 'gh'", &source, &ep).expect_err(raw);
        assert!(matches!(err, BindingError::HostMismatch { .. }), "{raw}");
    }
    // A downgrade to http is visible in the message.
    let ep = endpoint(EndpointId::GithubApi, "http://ghe.example.com");
    let msg = authorize("git token 'gh'", &source, &ep)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("now http://ghe.example.com"), "{msg}");
}

#[test]
fn legacy_credentials_are_bound_to_the_defaults() {
    let legacy = CredentialSource::Stored(None);
    assert!(authorize(
        "Anthropic API key",
        &legacy,
        &Endpoint::default_for(EndpointId::AnthropicApi)
    )
    .is_ok());
    let err = authorize(
        "Anthropic API key",
        &legacy,
        &endpoint(EndpointId::AnthropicApi, "https://gateway.example.com"),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("saved for api.anthropic.com"),
        "{err}"
    );
}

#[test]
fn a_binding_without_the_endpoint_falls_back_to_its_default() {
    // Recorded before the endpoint existed.
    let source =
        CredentialSource::Stored(Some(EndpointBinding::defaults(&[EndpointId::GithubApi])));
    assert!(authorize(
        "git token 'gh'",
        &source,
        &Endpoint::default_for(EndpointId::GithubRaw)
    )
    .is_ok());
    assert!(authorize(
        "git token 'gh'",
        &source,
        &endpoint(EndpointId::GithubRaw, "https://ghe.example.com/raw")
    )
    .is_err());
}

#[test]
fn kubernetes_secrets_only_go_to_the_default_endpoint() {
    let source = CredentialSource::KubernetesSecret("deckwatch-anthropic-api-key".to_string());
    assert!(authorize(
        "Anthropic API key",
        &source,
        &Endpoint::default_for(EndpointId::AnthropicApi)
    )
    .is_ok());
    let err = authorize(
        "Anthropic API key",
        &source,
        &endpoint(EndpointId::AnthropicApi, "https://gateway.example.com"),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindingError::SecretOnlyForDefault { .. }),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("deckwatch-anthropic-api-key"), "{msg}");
    assert!(msg.contains("api.anthropic.com"), "{msg}");
    assert!(msg.contains("gateway.example.com"), "{msg}");
}

#[test]
fn templated_endpoints_bind_on_the_template() {
    let source = CredentialSource::Stored(None);
    assert!(authorize(
        "GCP service account key",
        &source,
        &Endpoint::default_for(EndpointId::VertexApi)
    )
    .is_ok());
    let psc = endpoint(
        EndpointId::VertexApi,
        "https://{region}-aiplatform-psc.p.googleapis.com",
    );
    assert!(authorize("GCP service account key", &source, &psc).is_err());
}

#[test]
fn aws_credentials_only_go_to_aws_hosts_or_loopback() {
    let default = Endpoint::default_for(EndpointId::Bedrock);
    let host = default.host_for_region("us-east-1").unwrap();
    assert!(authorize_aws(&default, &host).is_ok());

    let vpce = endpoint(
        EndpointId::Bedrock,
        "https://vpce-1.bedrock-runtime.us-east-1.vpce.amazonaws.com:443",
    );
    assert!(authorize_aws(&vpce, vpce.host()).is_ok());

    let local = endpoint(EndpointId::Bedrock, "https://localhost:8443");
    assert!(authorize_aws(&local, local.host()).is_ok());

    let attacker = endpoint(EndpointId::Bedrock, "https://bedrock.attacker.example");
    let err = authorize_aws(&attacker, attacker.host()).unwrap_err();
    assert!(matches!(err, BindingError::NotAwsHost { .. }), "{err:?}");
    assert!(
        err.to_string().contains("bedrock.attacker.example"),
        "{err}"
    );
}

#[test]
fn host_without_port_handles_ipv6() {
    assert_eq!(host_without_port("[::1]:8443"), "::1");
    assert_eq!(host_without_port("example.com:443"), "example.com");
    assert_eq!(host_without_port("example.com"), "example.com");
}

#[test]
fn display_origin_hides_only_https() {
    assert_eq!(
        display_origin("https://api.anthropic.com"),
        "api.anthropic.com"
    );
    assert_eq!(
        display_origin("http://localhost:8080"),
        "http://localhost:8080"
    );
}
