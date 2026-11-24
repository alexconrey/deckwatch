use super::*;
use crate::plugins::{LoadedPlugin, PluginMetadata};
use std::collections::BTreeMap;
use std::sync::Arc;

fn loaded(name: &str, version: &str, bytes: &[u8]) -> LoadedPlugin {
    LoadedPlugin {
        name: name.to_string(),
        wasm_bytes: Arc::new(bytes.to_vec()),
        allowed_hosts: vec![],
        config: BTreeMap::new(),
        inherit_env_keys: vec![],
        inherit_env_file_keys: BTreeMap::new(),
        metadata: PluginMetadata {
            name: name.to_string(),
            version: version.to_string(),
            ..Default::default()
        },
        sha256: crate::plugins::sha256_hex(bytes),
        loaded_at: "2026-11-24T12:00:00+00:00".to_string(),
    }
}

#[test]
fn summary_reports_version_size_checksum_and_load_time() {
    let p = loaded("aws", "0.7.2", b"\0asm-build-a");
    let json = serde_json::to_value(PluginSummary::from(&p)).unwrap();

    assert_eq!(json["name"], "aws");
    assert_eq!(json["version"], "0.7.2");
    assert_eq!(json["wasm_size_bytes"], 12);
    assert_eq!(json["sha256"], crate::plugins::sha256_hex(b"\0asm-build-a"));
    assert_eq!(json["loaded_at"], "2026-11-24T12:00:00+00:00");
}

#[test]
fn summary_keeps_existing_fields() {
    // Additive change: every field clients relied on before must still be present.
    let json = serde_json::to_value(PluginSummary::from(&loaded("aws", "0.7.2", b"x"))).unwrap();
    for key in [
        "name",
        "version",
        "description",
        "provides",
        "depends_on",
        "config_schema",
        "resources",
        "wasm_size_bytes",
    ] {
        assert!(json.get(key).is_some(), "missing existing field `{key}`");
    }
}

#[test]
fn same_version_different_binary_is_distinguishable() {
    let published = PluginSummary::from(&loaded("aws", "0.7.2", b"\0asm-release"));
    let fix_build = PluginSummary::from(&loaded("aws", "0.7.2", b"\0asm-fix"));

    assert_eq!(published.version, fix_build.version);
    assert_ne!(published.sha256, fix_build.sha256);
}
