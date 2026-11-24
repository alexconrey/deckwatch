use super::*;
use crate::plugins::{LoadedPlugin, McpTuningField, PluginMetadata};
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

/// Regression: Settings → MCP Tuning only renders plugins whose summary has a
/// non-empty `mcp_tuning_fields`, but the summary never included the field, so
/// no per-plugin section ever appeared.
#[test]
fn summary_includes_mcp_tuning_fields() {
    let mut p = loaded("aws", "0.7.2", b"x");
    p.metadata.mcp_tuning_fields = vec![
        McpTuningField {
            key: "naming".to_string(),
            label: "Naming convention".to_string(),
            description: "How resources are named".to_string(),
            placeholder: "{app}-{env}".to_string(),
            default: Some("{app}".to_string()),
        },
        McpTuningField {
            key: "region".to_string(),
            label: "Region".to_string(),
            ..Default::default()
        },
    ];

    let json = serde_json::to_value(PluginSummary::from(&p)).unwrap();

    // Same shape as the frontend `McpTuningField` type: description and
    // placeholder are always present, `default` is omitted when unset.
    assert_eq!(
        json["mcp_tuning_fields"],
        serde_json::json!([
            {
                "key": "naming",
                "label": "Naming convention",
                "description": "How resources are named",
                "placeholder": "{app}-{env}",
                "default": "{app}"
            },
            { "key": "region", "label": "Region", "description": "", "placeholder": "" }
        ])
    );
}

#[test]
fn summary_sends_empty_mcp_tuning_fields_when_none_declared() {
    // The frontend type marks the field required, so it must always be sent.
    let json = serde_json::to_value(PluginSummary::from(&loaded("aws", "0.7.2", b"x"))).unwrap();
    assert_eq!(json["mcp_tuning_fields"], serde_json::json!([]));
}
/// Known-answer test: plugin checksum pins are compared against this output,
/// so it must stay `shasum -a 256` format across sha2 upgrades.
#[test]
fn sha256_hex_known_answer() {
    assert_eq!(
        crate::plugins::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
