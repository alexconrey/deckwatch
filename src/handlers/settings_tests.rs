// Unit tests for DeckwatchSettings serialization in src/handlers/settings.rs

use super::*;

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
