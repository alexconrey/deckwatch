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
    assert!(!settings.registry_enabled);
}

#[test]
fn test_prometheus_defaults_true() {
    // Deserializing an empty object should yield prometheus_enabled = true
    // because the field has `#[serde(default = "default_true")]`.
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert!(settings.prometheus_enabled);
}

#[test]
fn test_registry_defaults_false() {
    let settings: DeckwatchSettings = serde_json::from_str("{}").unwrap();
    assert!(!settings.registry_enabled);
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
    settings.registry_enabled = true;
    settings.ai_claude_enabled = false;

    let json = serde_json::to_string(&settings).expect("serialize failed");
    let deserialized: DeckwatchSettings = serde_json::from_str(&json).expect("deserialize failed");

    assert_eq!(deserialized.allowed_namespaces, settings.allowed_namespaces);
    assert_eq!(deserialized.prometheus_enabled, false);
    assert_eq!(deserialized.registry_enabled, true);
    assert_eq!(deserialized.ai_claude_enabled, false);
    // DeckwatchSettings::default() uses Rust Default (false for bool), not
    // serde defaults, so ai_codex_enabled round-trips as false.
    assert_eq!(deserialized.ai_codex_enabled, settings.ai_codex_enabled);
}
