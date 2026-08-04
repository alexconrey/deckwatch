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
    let mut settings = DeckwatchSettings::default();
    settings.allowed_namespaces = vec!["team-a".to_string(), "team-b".to_string()];
    settings.prometheus_enabled = false;
    settings.ai_claude_enabled = false;

    let json = serde_json::to_string(&settings).expect("serialize failed");
    let deserialized: DeckwatchSettings = serde_json::from_str(&json).expect("deserialize failed");

    assert_eq!(deserialized.allowed_namespaces, settings.allowed_namespaces);
    assert_eq!(deserialized.prometheus_enabled, false);
    assert_eq!(deserialized.ai_claude_enabled, false);
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
    let mut settings = DeckwatchSettings::default();
    settings.build_architectures = vec![BuildArchitecture {
        platform: "linux/amd64".into(),
        arch: "amd64".into(),
        enabled: true,
    }];
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
