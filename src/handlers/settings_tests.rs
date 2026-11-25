// Unit tests for DeckwatchSettings serialization in src/handlers/settings.rs

use super::*;
use crate::outbound::{EndpointBinding, EndpointId, EndpointPolicy, EndpointSettings};

#[test]
fn test_default_settings_values() {
    // Note: DeckwatchSettings derives Default, which uses Rust defaults
    // (false for bool). Serde defaults (default_true) only apply during
    // deserialization.
    let settings = DeckwatchSettings::default();
    assert!(settings.allowed_namespaces.is_empty());
    assert!(settings.default_resource_limits.is_none());
    assert!(settings.auth.is_none());
    assert!(settings.git_repositories.is_empty());
    assert!(settings.oci_registries.is_empty());
    assert!(settings.git_token_secrets.is_empty());
    // Rust Default for bool is false
    assert!(!settings.prometheus_enabled);
    // Rust Default for Vec is empty (serde default gives populated list)
    assert!(settings.build_architectures.is_empty());
}

#[test]
fn test_prometheus_defaults_true() {
    // Deserializing an empty object should yield prometheus_enabled = true
    // because the field has `#[serde(default = "default_true")]`.
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert!(settings.prometheus_enabled);
}

#[test]
fn test_ai_providers_default_true() {
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert!(settings.ai_claude_enabled);
    assert!(settings.ai_codex_enabled);
}

#[test]
fn test_roundtrip_serialization() {
    let settings = DeckwatchSettings {
        allowed_namespaces: vec!["team-a".to_string(), "team-b".to_string()],
        prometheus_enabled: false,
        ai_claude_enabled: false,
        ..Default::default()
    };

    let json = serde_json::to_string(&settings).expect("serialize failed");
    let deserialized: DeckwatchSettings = serde_json::from_str(&json).expect("deserialize failed");

    assert_eq!(deserialized.allowed_namespaces, settings.allowed_namespaces);
    assert!(!deserialized.prometheus_enabled);
    assert!(!deserialized.ai_claude_enabled);
    // DeckwatchSettings::default() uses Rust Default (false for bool), not
    // serde defaults, so ai_codex_enabled round-trips as false.
    assert_eq!(deserialized.ai_codex_enabled, settings.ai_codex_enabled);
}

#[test]
fn test_build_architectures_defaults_from_empty_json() {
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.build_architectures.len(), 2);
    assert_eq!(settings.build_architectures[0].platform, "linux/amd64");
    assert_eq!(settings.build_architectures[0].arch, "amd64");
    assert!(settings.build_architectures[0].enabled);
    assert_eq!(settings.build_architectures[1].platform, "linux/arm64");
    assert_eq!(settings.build_architectures[1].arch, "arm64");
    assert!(settings.build_architectures[1].enabled);
}

#[test]
fn test_build_architecture_enabled_defaults_true() {
    let json = r#"{"platform":"linux/riscv64","arch":"riscv64"}"#;
    let arch: BuildArchitecture = serde_json::from_str(json).unwrap();
    assert!(arch.enabled);
}

#[test]
fn test_build_architectures_disabled_entry_preserved() {
    let json = serde_json::json!({
        "build_architectures": [
            {"platform": "linux/amd64", "arch": "amd64", "enabled": true},
            {"platform": "linux/arm64", "arch": "arm64", "enabled": false}
        ]
    });
    let settings: DeckwatchSettings = serde_json::from_value(json).unwrap();
    assert_eq!(settings.build_architectures.len(), 2);
    assert!(settings.build_architectures[0].enabled);
    assert!(!settings.build_architectures[1].enabled);
}

#[test]
fn test_build_architectures_roundtrip() {
    let settings = DeckwatchSettings {
        build_architectures: vec![BuildArchitecture {
            platform: "linux/amd64".into(),
            arch: "amd64".into(),
            enabled: true,
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    let deserialized: DeckwatchSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.build_architectures.len(), 1);
    assert_eq!(deserialized.build_architectures[0].arch, "amd64");
    assert!(deserialized.build_architectures[0].enabled);
}

#[test]
fn test_build_architecture_explicit_enabled_false() {
    let json = r#"{"platform":"linux/arm64","arch":"arm64","enabled":false}"#;
    let arch: BuildArchitecture = serde_json::from_str(json).unwrap();
    assert!(!arch.enabled);
    assert_eq!(arch.arch, "arm64");
}

#[test]
fn test_build_architectures_backwards_compat_with_existing_settings() {
    // Simulate an existing production settings blob that has other fields
    // but no build_architectures key -- should get the 2-arch default.
    let json = serde_json::json!({
        "allowed_namespaces": ["prod", "staging"],
        "prometheus_enabled": true,
        "ingress_templates": [{"name": "alb", "is_default": true}]
    });
    let settings: DeckwatchSettings = serde_json::from_value(json).unwrap();
    assert_eq!(settings.allowed_namespaces.len(), 2);
    assert_eq!(settings.build_architectures.len(), 2);
    assert!(settings.build_architectures.iter().all(|a| a.enabled));
}

#[test]
fn test_build_architectures_custom_arch_roundtrip() {
    let json = serde_json::json!({
        "build_architectures": [
            {"platform": "linux/amd64", "arch": "amd64", "enabled": true},
            {"platform": "linux/arm64", "arch": "arm64", "enabled": true},
            {"platform": "linux/riscv64", "arch": "riscv64", "enabled": false}
        ]
    });
    let settings: DeckwatchSettings = serde_json::from_value(json).unwrap();
    assert_eq!(settings.build_architectures.len(), 3);
    assert_eq!(settings.build_architectures[2].platform, "linux/riscv64");
    assert!(!settings.build_architectures[2].enabled);

    let reserialized = serde_json::to_value(&settings).unwrap();
    let arches = reserialized["build_architectures"].as_array().unwrap();
    assert_eq!(arches.len(), 3);
    assert_eq!(arches[2]["enabled"], false);
}

#[test]
fn test_build_settings_defaults_from_empty_json() {
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(
        settings.build_settings.kaniko_image,
        "gcr.io/kaniko-project/executor:v1.24.0"
    );
    assert_eq!(
        settings.build_settings.crane_image,
        "gcr.io/go-containerregistry/crane:latest"
    );
    assert_eq!(settings.build_settings.platform_flag, "--custom-platform");
    assert!(settings.build_settings.cache_enabled);
    assert_eq!(settings.build_settings.snapshot_mode, "redo");
    // crane dropped --docker-media-types, so the field is a no-op kept for
    // backward compat and defaults to false.
    assert!(!settings.build_settings.docker_media_types);
    assert_eq!(settings.build_settings.job_ttl_seconds, 3600);
    assert_eq!(settings.build_settings.kaniko_backoff_limit, 0);
    assert_eq!(settings.build_settings.crane_backoff_limit, 1);
    assert!(settings.build_settings.extra_kaniko_args.is_empty());
}

#[test]
fn test_build_settings_roundtrip() {
    let json = serde_json::json!({
        "build_settings": {
            "kaniko_image": "gcr.io/kaniko-project/executor:v1.23.0",
            "cache_enabled": false,
            "docker_media_types": false,
            "extra_kaniko_args": ["--verbosity=debug"]
        }
    });
    let settings: DeckwatchSettings = serde_json::from_value(json).unwrap();
    assert_eq!(
        settings.build_settings.kaniko_image,
        "gcr.io/kaniko-project/executor:v1.23.0"
    );
    assert!(!settings.build_settings.cache_enabled);
    assert!(!settings.build_settings.docker_media_types);
    assert_eq!(
        settings.build_settings.extra_kaniko_args,
        vec!["--verbosity=debug"]
    );
    // Unspecified fields should get defaults
    assert_eq!(settings.build_settings.platform_flag, "--custom-platform");
    assert_eq!(settings.build_settings.crane_backoff_limit, 1);
}

#[test]
fn test_build_settings_backwards_compat() {
    // Existing settings JSON without build_settings field should get defaults
    let json = serde_json::json!({
        "allowed_namespaces": ["prod"],
        "build_architectures": [
            {"platform": "linux/amd64", "arch": "amd64", "enabled": true}
        ]
    });
    let settings: DeckwatchSettings = serde_json::from_value(json).unwrap();
    assert_eq!(
        settings.build_settings.kaniko_image,
        "gcr.io/kaniko-project/executor:v1.24.0"
    );
    assert!(!settings.build_settings.docker_media_types);
}

#[test]
fn test_build_settings_custom_kaniko_image_roundtrip() {
    let settings = DeckwatchSettings {
        build_settings: BuildSettings {
            kaniko_image: "custom-registry.io/kaniko:v1.20.0".into(),
            ..default_build_settings()
        },
        ..Default::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    let deserialized: DeckwatchSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(
        deserialized.build_settings.kaniko_image,
        "custom-registry.io/kaniko:v1.20.0"
    );
}

// ── secret masking (public GET /api/settings) and restore on PUT ────────────

fn settings_with_secrets() -> DeckwatchSettings {
    serde_json::from_value(serde_json::json!({
        "credentials": {
            "anthropic_api_key": "ciphertext-anthropic",
            "gcp_sa_key": "ciphertext-gcp"
        },
        "git_token_secrets": [
            {"name": "gh", "secret_name": "gh-token", "namespace": "deckwatch", "encrypted_token": "ciphertext-gh"},
            {"name": "unset", "secret_name": "unset-token", "namespace": "deckwatch"}
        ],
        "notifications": {
            "enabled": true,
            "webhook_url": "https://hooks.slack.com/services/T000/B000/XXXXSECRET",
            "event_types": [],
            "namespaces": []
        },
        "plugins": [{
            "name": "aws",
            "source": {"type": "github", "repo": "o/r", "ref": "v1", "path": "p.wasm"},
            "config": {
                "AWS_REGION": "us-gov-west-1",
                "AWS_SECRET_ACCESS_KEY": "plaintext-secret",
                "API_TOKEN": "ciphertext-token",
                "EMPTY": ""
            }
        }]
    }))
    .expect("fixture parses")
}

/// `AWS_REGION` declared non-secret; `API_TOKEN` declared secret;
/// `AWS_SECRET_ACCESS_KEY` undeclared.
fn aws_public_keys() -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    std::collections::HashMap::from([(
        "aws".to_string(),
        std::collections::HashSet::from(["AWS_REGION".to_string(), "EMPTY".to_string()]),
    )])
}

#[test]
fn mask_secrets_hides_every_secret_field() {
    let mut s = settings_with_secrets();
    mask_secrets(&mut s, &aws_public_keys());

    let creds = s.credentials.as_ref().expect("credentials");
    assert_eq!(creds.anthropic_api_key.as_deref(), Some(MASKED_VALUE));
    assert_eq!(creds.gcp_sa_key.as_deref(), Some(MASKED_VALUE));
    assert_eq!(
        s.git_token_secrets[0].encrypted_token.as_deref(),
        Some(MASKED_VALUE)
    );
    assert_eq!(s.git_token_secrets[1].encrypted_token, None);
    assert_eq!(
        s.notifications.as_ref().expect("notifications").webhook_url,
        MASKED_VALUE
    );

    let cfg = &s.plugins[0].config;
    assert_eq!(
        cfg["AWS_REGION"], "us-gov-west-1",
        "declared non-secret stays visible"
    );
    assert_eq!(
        cfg["AWS_SECRET_ACCESS_KEY"], MASKED_VALUE,
        "undeclared key fails closed"
    );
    assert_eq!(cfg["API_TOKEN"], MASKED_VALUE, "declared secret is masked");
    assert_eq!(cfg["EMPTY"], "", "empty stays empty");

    let json = serde_json::to_string(&s).expect("serialize");
    for secret in [
        "ciphertext-anthropic",
        "ciphertext-gcp",
        "ciphertext-gh",
        "XXXXSECRET",
        "plaintext-secret",
        "ciphertext-token",
    ] {
        assert!(!json.contains(secret), "{secret} leaked: {json}");
    }
}

#[test]
fn mask_secrets_masks_all_plugin_config_when_plugin_not_loaded() {
    let mut s = settings_with_secrets();
    mask_secrets(&mut s, &std::collections::HashMap::new());
    assert_eq!(s.plugins[0].config["AWS_REGION"], MASKED_VALUE);
}

#[test]
fn restore_secrets_round_trips_masked_document() {
    let stored = settings_with_secrets();
    let mut incoming = stored.clone();
    mask_secrets(&mut incoming, &aws_public_keys());

    restore_secrets(&mut incoming, &stored).expect("restore");

    assert_eq!(
        serde_json::to_value(&incoming).expect("incoming"),
        serde_json::to_value(&stored).expect("stored"),
    );
}

#[test]
fn restore_secrets_keeps_server_managed_credentials() {
    let stored = settings_with_secrets();

    // SettingsPage omits `credentials` entirely; PluginSettingsPage PUTs the
    // masked GET response back. Neither may touch the stored ciphertext.
    for credentials in [
        None,
        Some(EncryptedCredentials {
            anthropic_api_key: Some(MASKED_VALUE.to_string()),
            gcp_sa_key: Some("attacker-supplied".to_string()),
            ..Default::default()
        }),
    ] {
        let mut incoming = stored.clone();
        incoming.credentials = credentials;
        incoming.git_token_secrets[0].encrypted_token = Some("attacker-supplied".to_string());
        restore_secrets(&mut incoming, &stored).expect("restore");

        let creds = incoming.credentials.expect("credentials kept");
        assert_eq!(
            creds.anthropic_api_key.as_deref(),
            Some("ciphertext-anthropic")
        );
        assert_eq!(creds.gcp_sa_key.as_deref(), Some("ciphertext-gcp"));
        assert_eq!(
            incoming.git_token_secrets[0].encrypted_token.as_deref(),
            Some("ciphertext-gh")
        );
    }
}

#[test]
fn restore_secrets_applies_edited_values() {
    let stored = settings_with_secrets();
    let mut incoming = stored.clone();
    mask_secrets(&mut incoming, &aws_public_keys());
    incoming.plugins[0]
        .config
        .insert("AWS_SECRET_ACCESS_KEY".to_string(), "rotated".to_string());
    incoming
        .notifications
        .as_mut()
        .expect("notifications")
        .webhook_url = "https://hooks.slack.com/services/new".to_string();

    restore_secrets(&mut incoming, &stored).expect("restore");

    assert_eq!(
        incoming.plugins[0].config["AWS_SECRET_ACCESS_KEY"],
        "rotated"
    );
    assert_eq!(incoming.plugins[0].config["API_TOKEN"], "ciphertext-token");
    assert_eq!(
        incoming.notifications.expect("notifications").webhook_url,
        "https://hooks.slack.com/services/new"
    );
}

#[test]
fn restore_secrets_rejects_masked_value_without_stored_original() {
    let stored = settings_with_secrets();
    let mut incoming = stored.clone();
    mask_secrets(&mut incoming, &aws_public_keys());
    // Operator renamed a masked key in the key/value editor.
    let v = incoming.plugins[0]
        .config
        .remove("AWS_SECRET_ACCESS_KEY")
        .expect("key");
    incoming.plugins[0]
        .config
        .insert("AWS_SECRET_KEY".to_string(), v);

    let err = restore_secrets(&mut incoming, &stored).expect_err("must reject");
    assert!(err.contains("AWS_SECRET_KEY"), "{err}");
}

// ── handler-level: GET /api/settings → PUT /api/settings round trips ────────

/// AppState backed by a temp-file SQLite DB. The kube client points at an
/// unroutable address; the settings handlers never reach it once the DB row
/// exists.
async fn handler_test_state(dir: &std::path::Path) -> AppState {
    let url = format!("sqlite://{}?mode=rwc", dir.join("deckwatch.db").display());
    let db = crate::db::connect(&url).await.expect("db");
    // main() installs the process-wide rustls provider; kube::Client needs it.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let kube_config = kube::Config::new("http://127.0.0.1:9".parse().expect("uri"));
    AppState {
        kube_client: kube::Client::try_from(kube_config).expect("kube client"),
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-config".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::new(10),
        db,
        encryption_key: String::new(),
        plugins: Default::default(),
        plugin_patch_fingerprints: Default::default(),
        plugin_telemetry: Default::default(),
    }
}

async fn seeded_state(dir: &std::path::Path) -> (AppState, DeckwatchSettings) {
    let state = handler_test_state(dir).await;
    let mut seeded = settings_with_secrets();
    // Disabled so put_settings' background plugin refresh makes no network calls.
    seeded.plugins[0].enabled = false;
    upsert_settings_to_db(&state.db, &seeded)
        .await
        .expect("seed");
    // Saving records the default binding on the legacy credentials.
    bind_legacy_credentials(&mut seeded);
    (state, seeded)
}

const SECRETS: [&str; 6] = [
    "ciphertext-anthropic",
    "ciphertext-gcp",
    "ciphertext-gh",
    "XXXXSECRET",
    "plaintext-secret",
    "ciphertext-token",
];

fn assert_no_secrets(s: &DeckwatchSettings, what: &str) {
    let json = serde_json::to_string(s).expect("serialize");
    for secret in SECRETS {
        assert!(!json.contains(secret), "{what} leaked {secret}: {json}");
    }
}

/// PluginSettingsPage does `settingsApi.update(await settingsApi.get())`.
#[tokio::test]
async fn get_then_put_masked_document_leaves_stored_secrets_intact() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, seeded) = seeded_state(tmp.path()).await;

    let Json(masked) = get_settings(State(state.clone())).await.expect("get");
    assert_no_secrets(&masked, "GET /api/settings");

    let Json(response) = put_settings(State(state.clone()), Json(masked))
        .await
        .expect("put");
    assert_no_secrets(&response, "PUT /api/settings response");

    let stored = load_settings_from_db(&state).await;
    assert_eq!(
        serde_json::to_value(&stored).expect("stored"),
        serde_json::to_value(&seeded).expect("seeded"),
        "masked round trip must not change stored settings"
    );
}

/// SettingsPage's buildPayload() never includes `credentials`.
#[tokio::test]
async fn put_without_credentials_does_not_wipe_them() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.credentials = None;
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    let creds = load_settings_from_db(&state)
        .await
        .credentials
        .expect("credentials survive a settings save");
    assert_eq!(
        creds.anthropic_api_key.as_deref(),
        Some("ciphertext-anthropic")
    );
    assert_eq!(creds.gcp_sa_key.as_deref(), Some("ciphertext-gcp"));
}

/// The UI removes a plugin by saving the document without it. Removing the
/// last one used to start no reload at all, so it kept running.
#[tokio::test]
async fn removing_the_last_plugin_unloads_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    *state.plugins.write().await = vec![crate::plugin_fixture::FixturePlugin::new().loaded("aws")];

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.plugins.clear();
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");
    assert!(
        state.plugins.read().await.is_empty(),
        "the removed plugin is still loaded"
    );
}

/// Disabling unloads before the response, without waiting for a reload.
#[tokio::test]
async fn disabling_a_plugin_in_settings_unloads_it_right_away() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    *state.plugins.write().await = vec![crate::plugin_fixture::FixturePlugin::new().loaded("aws")];

    // The seeded `aws` is configured disabled.
    let Json(payload) = get_settings(State(state.clone())).await.expect("get");
    assert!(!payload.plugins[0].enabled);
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");
    assert!(state.plugins.read().await.is_empty());
}

// ── endpoints and credential bindings ───────────────────────────────────────

fn ghe_endpoints() -> EndpointSettings {
    EndpointSettings::default()
        .with(EndpointId::GithubWeb, "https://ghe.example.com")
        .with(EndpointId::GithubApi, "https://ghe.example.com/api/v3")
        .with(EndpointId::GithubRaw, "https://ghe.example.com/raw")
}

async fn audit_entries(state: &AppState) -> Vec<crate::entities::audit_log::Model> {
    crate::entities::audit_log::Entity::find()
        .all(&state.db)
        .await
        .expect("audit log")
}

#[tokio::test]
async fn get_always_returns_the_endpoints_object() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    let Json(got) = get_settings(State(state)).await.expect("get");
    assert_eq!(got.endpoints, Some(EndpointSettings::default()));
}

#[tokio::test]
async fn put_validates_and_normalizes_endpoints() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.endpoints = Some(
        EndpointSettings::default()
            .with(
                EndpointId::AnthropicApi,
                " https://LLM-Gateway.example.com/anthropic/ ",
            )
            .with(EndpointId::GithubApi, "https://api.github.com/"),
    );
    let Json(saved) = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");
    let expected = EndpointSettings::default().with(
        EndpointId::AnthropicApi,
        "https://llm-gateway.example.com/anthropic",
    );
    assert_eq!(saved.endpoints.as_ref(), Some(&expected));
    assert_eq!(
        load_settings_from_db(&state).await.endpoints,
        Some(expected)
    );
}

#[tokio::test]
async fn put_rejects_invalid_endpoints_and_keeps_the_stored_ones() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;

    for bad in [
        "http://llm-gateway.example.com",
        "https://user:pass@llm-gateway.example.com",
        "https://llm-gateway.example.com/?key=1",
        "llm-gateway.example.com",
    ] {
        let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
        payload.endpoints = Some(EndpointSettings::default().with(EndpointId::AnthropicApi, bad));
        let err = put_settings(State(state.clone()), Json(payload))
            .await
            .err()
            .unwrap_or_else(|| panic!("{bad} must be rejected"));
        let AppError::BadRequest(msg) = err else {
            panic!("expected 400 for {bad}");
        };
        assert!(msg.contains("Anthropic API"), "{bad}: {msg}");
    }
    assert_eq!(load_settings_from_db(&state).await.endpoints, None);
}

#[tokio::test]
async fn put_without_endpoints_keeps_them() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.endpoints = Some(ghe_endpoints());
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    // SettingsPage builds its payload field by field and omits `endpoints`.
    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.endpoints = None;
    payload.prometheus_enabled = false;
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    let stored = load_settings_from_db(&state).await;
    assert!(!stored.prometheus_enabled);
    assert_eq!(stored.endpoints, Some(ghe_endpoints()));
}

#[tokio::test]
async fn endpoint_changes_are_audit_logged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    let before = audit_entries(&state).await.len();

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.endpoints = Some(ghe_endpoints());
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    let entries = audit_entries(&state).await;
    let mut endpoint_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.resource_name.starts_with("endpoints."))
        .collect();
    endpoint_entries.sort_by(|a, b| a.resource_name.cmp(&b.resource_name));
    assert_eq!(
        entries.len() - before,
        4,
        "3 endpoint entries + the settings update"
    );
    let names: Vec<&str> = endpoint_entries
        .iter()
        .map(|e| e.resource_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "endpoints.github_api_base",
            "endpoints.github_raw_base",
            "endpoints.github_web_base"
        ]
    );
    let api = endpoint_entries[0];
    assert_eq!(api.action, "update");
    assert_eq!(api.resource_type, "settings");
    assert!(
        api.detail
            .contains("from the default (https://api.github.com)"),
        "{}",
        api.detail
    );
    assert!(
        api.detail.contains("to https://ghe.example.com/api/v3"),
        "{}",
        api.detail
    );

    // Saving the same endpoints again is not a change.
    let Json(payload) = get_settings(State(state.clone())).await.expect("get");
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");
    let again = audit_entries(&state).await;
    assert_eq!(
        again
            .iter()
            .filter(|e| e.resource_name.starts_with("endpoints."))
            .count(),
        3
    );
}

#[test]
fn apply_endpoint_update_reports_resets_to_default() {
    let stored = DeckwatchSettings {
        endpoints: Some(ghe_endpoints()),
        ..Default::default()
    };
    let mut incoming = DeckwatchSettings {
        endpoints: Some(EndpointSettings::default()),
        ..Default::default()
    };
    let changes =
        apply_endpoint_update(&mut incoming, &stored, &EndpointPolicy::default()).expect("valid");
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().all(|c| c.to.is_empty()));
    assert!(changes[0].describe().contains("to the default"));
    assert_eq!(incoming.endpoints, None, "all defaults are stored as None");
}

#[tokio::test]
async fn put_cannot_forge_credential_bindings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, seeded) = seeded_state(tmp.path()).await;
    let attacker = EndpointBinding::capture(
        &EndpointSettings::default()
            .with(EndpointId::AnthropicApi, "https://attacker.example")
            .with(EndpointId::GithubApi, "https://attacker.example"),
        &[EndpointId::AnthropicApi, EndpointId::GithubApi],
        &EndpointPolicy::default(),
    )
    .expect("capture");

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    let creds = payload.credentials.as_mut().expect("credentials");
    creds.anthropic_api_key_binding = Some(attacker.clone());
    payload.git_token_secrets[0].binding = Some(attacker.clone());
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    let stored = load_settings_from_db(&state).await;
    assert_eq!(
        stored
            .credentials
            .expect("credentials")
            .anthropic_api_key_binding,
        seeded
            .credentials
            .expect("seeded")
            .anthropic_api_key_binding
    );
    assert_eq!(
        stored.git_token_secrets[0].binding,
        seeded.git_token_secrets[0].binding
    );
}

#[test]
fn bind_legacy_credentials_binds_only_unbound_secrets_to_defaults() {
    let mut s = settings_with_secrets();
    let custom = EndpointBinding::capture(
        &ghe_endpoints(),
        GIT_TOKEN_ENDPOINTS,
        &EndpointPolicy::default(),
    )
    .expect("capture");
    s.credentials.as_mut().expect("creds").gcp_sa_key_binding = Some(custom.clone());

    // anthropic key + git token "gh"; the GCP key is already bound and the
    // "unset" token has no stored secret.
    assert_eq!(bind_legacy_credentials(&mut s), 2);
    let creds = s.credentials.as_ref().expect("creds");
    assert_eq!(
        creds.anthropic_api_key_binding,
        Some(EndpointBinding::defaults(ANTHROPIC_KEY_ENDPOINTS))
    );
    assert_eq!(creds.gcp_sa_key_binding, Some(custom));
    assert_eq!(
        s.git_token_secrets[0].binding,
        Some(EndpointBinding::defaults(GIT_TOKEN_ENDPOINTS))
    );
    assert_eq!(s.git_token_secrets[1].binding, None);
    // Idempotent.
    assert_eq!(bind_legacy_credentials(&mut s), 0);
}

#[tokio::test]
async fn startup_migration_records_default_bindings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = handler_test_state(tmp.path()).await;
    // A settings row written by an earlier version: no bindings.
    let legacy = serde_json::to_string(&settings_with_secrets()).expect("json");
    crate::entities::settings::Entity::insert(crate::entities::settings::ActiveModel {
        key: sea_orm::ActiveValue::Set(DB_SETTINGS_KEY.to_string()),
        value: sea_orm::ActiveValue::Set(legacy),
        updated_at: sea_orm::ActiveValue::Set(now_utc()),
    })
    .exec(&state.db)
    .await
    .expect("insert legacy row");

    bind_legacy_credentials_at_startup(&state).await;

    let stored = load_settings_from_db(&state).await;
    let creds = stored.credentials.expect("creds");
    assert_eq!(
        creds.anthropic_api_key_binding,
        Some(EndpointBinding::defaults(ANTHROPIC_KEY_ENDPOINTS))
    );
    assert_eq!(
        creds.gcp_sa_key_binding,
        Some(EndpointBinding::defaults(GCP_KEY_ENDPOINTS))
    );
    assert_eq!(
        creds.anthropic_api_key.as_deref(),
        Some("ciphertext-anthropic")
    );
}

#[tokio::test]
async fn saving_a_credential_binds_it_to_the_current_endpoints() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (mut state, _) = seeded_state(tmp.path()).await;
    state.encryption_key = "test-encryption-key".to_string();

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.endpoints = Some(
        EndpointSettings::default()
            .with(EndpointId::AnthropicApi, "https://llm-gateway.example.com")
            .with(EndpointId::GithubApi, "https://ghe.example.com/api/v3"),
    );
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("put");

    let Json(resp) = set_credentials(
        State(state.clone()),
        Json(SetCredentialsRequest {
            anthropic_api_key: Some("sk-ant-new".to_string()),
            gcp_sa_key: None,
        }),
    )
    .await
    .expect("set credentials");
    let binding = resp.anthropic_api_key_binding.expect("binding returned");
    assert_eq!(
        binding.origin(EndpointId::AnthropicApi),
        Some("https://llm-gateway.example.com")
    );
    let stored = load_settings_from_db(&state).await;
    let creds = stored.credentials.as_ref().expect("creds");
    assert_eq!(creds.anthropic_api_key_binding, Some(binding));
    // The untouched GCP key keeps its (default) binding.
    assert_eq!(
        creds.gcp_sa_key_binding,
        Some(EndpointBinding::defaults(GCP_KEY_ENDPOINTS))
    );

    let Json(_) = put_git_token_secret(
        State(state.clone()),
        Json(GitTokenSecretRequest {
            name: "ghe".to_string(),
            secret_name: None,
            token: "ghp_new".to_string(),
        }),
    )
    .await
    .expect("git token");
    let stored = load_settings_from_db(&state).await;
    let token = stored
        .git_token_secrets
        .iter()
        .find(|t| t.name == "ghe")
        .expect("token");
    let binding = token.binding.as_ref().expect("token binding");
    assert_eq!(
        binding.origin(EndpointId::GithubApi),
        Some("https://ghe.example.com")
    );
    assert_eq!(
        binding.origin(EndpointId::GithubWeb),
        Some("https://github.com")
    );

    // Clearing a credential clears its binding.
    let Json(resp) = set_credentials(
        State(state.clone()),
        Json(SetCredentialsRequest {
            anthropic_api_key: Some(String::new()),
            gcp_sa_key: None,
        }),
    )
    .await
    .expect("clear");
    assert!(resp.anthropic_api_key.is_none());
    assert!(resp.anthropic_api_key_binding.is_none());
}

#[test]
fn outbound_status_flags_credentials_bound_to_another_host() {
    let mut s = settings_with_secrets();
    bind_legacy_credentials(&mut s);
    s.endpoints = Some(
        EndpointSettings::default()
            .with(EndpointId::AnthropicApi, "https://llm-gateway.example.com"),
    );
    let status = outbound_status(&s, &crate::outbound::OutboundHttp::default());

    assert!(!status.allow_insecure_endpoints);
    assert!(status.extra_ca.is_none());
    assert_eq!(status.endpoints.len(), EndpointId::ALL.len());
    let anthropic = status
        .endpoints
        .iter()
        .find(|e| e.id == EndpointId::AnthropicApi)
        .expect("anthropic endpoint");
    assert_eq!(anthropic.default, "https://api.anthropic.com");
    assert_eq!(anthropic.value, "https://llm-gateway.example.com");
    assert_eq!(anthropic.host, "llm-gateway.example.com");
    assert!(anthropic.error.is_none());

    let key = status
        .credentials
        .iter()
        .find(|c| c.credential == "anthropic_api_key")
        .expect("key status");
    assert!(!key.usable);
    assert_eq!(key.bindings[0].bound_host, "api.anthropic.com");
    assert_eq!(key.bindings[0].configured_host, "llm-gateway.example.com");

    let gcp = status
        .credentials
        .iter()
        .find(|c| c.credential == "gcp_sa_key")
        .expect("gcp status");
    assert!(gcp.usable);
    assert_eq!(gcp.bindings.len(), 2);

    let token = status
        .credentials
        .iter()
        .find(|c| c.credential == "git_token:gh")
        .expect("git token status");
    assert!(token.usable);
    assert_eq!(token.label, "git token 'gh'");
    // Only tokens with a stored secret are listed.
    assert!(!status
        .credentials
        .iter()
        .any(|c| c.credential == "git_token:unset"));
}

#[test]
fn outbound_status_reports_endpoints_invalid_under_the_current_policy() {
    let s = DeckwatchSettings {
        endpoints: Some(
            EndpointSettings::default().with(EndpointId::GithubApi, "http://ghe.lab.internal"),
        ),
        ..Default::default()
    };
    let status = outbound_status(&s, &crate::outbound::OutboundHttp::default());
    let api = status
        .endpoints
        .iter()
        .find(|e| e.id == EndpointId::GithubApi)
        .expect("github api");
    assert!(
        api.error.as_deref().unwrap_or("").contains("http://"),
        "{api:?}"
    );
}

#[tokio::test]
async fn put_rejects_a_region_that_would_change_the_host() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (state, _) = seeded_state(tmp.path()).await;
    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.ai_provider = AiProviderConfig::VertexAi {
        project_id: "p".to_string(),
        region: "evil.example.com/".to_string(),
        sa_key_secret: "s".to_string(),
    };
    assert!(matches!(
        put_settings(State(state.clone()), Json(payload)).await,
        Err(AppError::BadRequest(_))
    ));

    let Json(mut payload) = get_settings(State(state.clone())).await.expect("get");
    payload.ai_provider = AiProviderConfig::Bedrock {
        region: "us-gov-west-1".to_string(),
        model_id: String::new(),
    };
    let _ = put_settings(State(state.clone()), Json(payload))
        .await
        .expect("a real region is fine");
}
