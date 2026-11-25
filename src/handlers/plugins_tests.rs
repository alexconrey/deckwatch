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

// ---------------------------------------------------------------------------
// Uploads
// ---------------------------------------------------------------------------

/// The WASM magic bytes and version: all the upload route checks.
const WASM: &[u8] = b"\0asm\x01\0\0\0";

/// Configure disabled plugins named `names`, so an upload's background
/// reload doesn't try to load them.
async fn configure(state: &AppState, names: &[&str]) {
    let mut settings = crate::handlers::settings::load_settings_from_db(state).await;
    settings.plugins = names
        .iter()
        .map(|name| crate::handlers::settings::PluginConfig {
            name: name.to_string(),
            enabled: false,
            source: PluginSource::Url {
                url: "https://example.com/plugin.wasm".into(),
            },
            token_secret: None,
            allowed_hosts: vec![],
            config: Default::default(),
            inherit_env_keys: vec![],
            inherit_env_file_keys: Default::default(),
            sha256: None,
            mcp_tuning: Default::default(),
        })
        .collect();
    crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
        .await
        .unwrap();
}

/// Every file under `dir`, relative to it.
fn files_under(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(dir).unwrap().display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// `{name}` is percent-decoded, so `..%2F` and absolute paths used to write
/// the body outside the uploads directory.
#[tokio::test]
async fn upload_refuses_a_name_that_is_not_a_plain_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let uploads = dir.path().join("data").join("uploads");
    let absolute = dir.path().join("absolute").display().to_string();
    let names = [
        "../escape",
        "../../escape",
        absolute.as_str(),
        "sub/escape",
        "..",
        ".hidden",
        "",
        "back\\slash",
        "white space",
    ];
    // Configured, so it's the name check that refuses them.
    configure(&state, &names).await;
    let before = files_under(dir.path());

    for name in names {
        let err = store_upload(
            state.clone(),
            &uploads,
            name.to_string(),
            Bytes::from_static(WASM),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, AppError::BadRequest(m) if m.contains("plugin name")),
            "{name:?}: {err:?}"
        );
    }
    assert_eq!(files_under(dir.path()), before, "nothing was written");
}

#[tokio::test]
async fn upload_for_an_unconfigured_plugin_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let uploads = dir.path().join("uploads");

    let err = store_upload(
        state,
        &uploads,
        "ghost".to_string(),
        Bytes::from_static(WASM),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
    assert!(!uploads.join("ghost.wasm").exists());
}

#[tokio::test]
async fn upload_for_a_configured_plugin_lands_in_the_uploads_dir() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let uploads = dir.path().join("uploads");
    configure(&state, &["demo"]).await;

    let status = store_upload(
        state.clone(),
        &uploads,
        "demo".to_string(),
        Bytes::from_static(WASM),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(std::fs::read(uploads.join("demo.wasm")).unwrap(), WASM);
    let settings = crate::handlers::settings::load_settings_from_db(&state).await;
    assert!(
        matches!(&settings.plugins[0].source, PluginSource::Upload { filename } if filename == "demo.wasm"),
        "{:?}",
        settings.plugins[0].source
    );
}
