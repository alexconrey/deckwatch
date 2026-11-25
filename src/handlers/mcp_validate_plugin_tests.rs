//! MCP `validate_plugin` runs caller-chosen WASM, so it gets nothing the
//! caller doesn't supply: arguments that would hand it a git token, network
//! access or deckwatch's environment are refused, and its result names env
//! vars without their values. Also the argument check every
//! deckwatch-native tool goes through.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::plugin_fixture::{serve_bytes, test_state, Behavior, FixturePlugin};

async fn call(state: &AppState, tool: &str, args: serde_json::Value) -> Result<String, String> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: "tools/call".into(),
        params: json!({ "name": tool, "arguments": args }),
    };
    let resp = handle_tool_call(state, &req).await;
    match (resp.result, resp.error) {
        (Some(result), _) => Ok(result["content"][0]["text"]
            .as_str()
            .expect("text content")
            .to_string()),
        (None, Some(error)) => Err(error.message),
        (None, None) => panic!("empty JSON-RPC response"),
    }
}

/// Serve `bytes` and count the downloads; returns the URL and the count.
async fn serve_counted(bytes: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/plugin.wasm",
        axum::routing::get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let bytes = bytes.clone();
            async move { bytes }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/plugin.wasm"), hits)
}

fn echo_plugin() -> FixturePlugin {
    FixturePlugin::new().export("apply", Behavior::output("{}"))
}

#[tokio::test]
async fn refuses_a_git_token_hosts_and_inherited_env_before_fetching() {
    let (url, hits) = serve_counted(echo_plugin().wasm()).await;
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;

    for (key, value) in [
        ("token_secret", json!("shared-token")),
        ("allowed_hosts", json!(["*"])),
        ("inherit_env_keys", json!(["OTEL_EXPORTER_OTLP_HEADERS"])),
        (
            "inherit_env_file_keys",
            json!({ "T": "AWS_WEB_IDENTITY_TOKEN_FILE" }),
        ),
    ] {
        let mut args = json!({ "source": { "type": "url", "url": url } });
        args[key] = value;
        let err = call(&state, "validate_plugin", args).await.expect_err(key);
        assert!(
            err.contains(&format!("unknown argument `{key}`")),
            "{key}: {err}"
        );
        assert!(
            err.contains(
                "no git token, no network access and nothing from deckwatch's environment"
            ),
            "{key}: {err}"
        );
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was fetched");

    // Arguments no schema declares are refused too.
    let err = call(
        &state,
        "validate_plugin",
        json!({ "source": { "type": "url", "url": url }, "sha256": "0" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown argument `sha256`"), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    // Without them, the same call runs.
    let text = call(
        &state,
        "validate_plugin",
        json!({ "source": { "type": "url", "url": url } }),
    )
    .await
    .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(text.contains(r#""valid": true"#), "{text}");
}

/// Defense in depth: the handler refuses them even if the schema check were
/// skipped.
#[tokio::test]
async fn the_handler_itself_refuses_them() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;
    for key in [
        "token_secret",
        "allowed_hosts",
        "inherit_env_keys",
        "inherit_env_file_keys",
    ] {
        let mut args = json!({ "source": { "type": "url", "url": "http://127.0.0.1:9/p.wasm" } });
        args[key] = json!(null);
        let err = tool_validate_plugin(&state, &args).await.unwrap_err();
        assert!(
            err.starts_with(&format!("validate_plugin refuses `{key}`: ")),
            "{key}: {err}"
        );
    }
    let err = tool_validate_plugin(
        &state,
        &json!({ "source": { "type": "upload", "filename": "../../etc/hostname" } }),
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("invalid source: "), "{err}");
}

#[tokio::test]
async fn refuses_upload_sources() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;
    let err = call(
        &state,
        "validate_plugin",
        json!({ "source": { "type": "upload", "filename": "x.wasm" } }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown argument `source.filename`"), "{err}");
}

#[tokio::test]
async fn reports_env_var_names_and_references_never_values() {
    const CANARY: &str = "canary-plain-env-value";
    let plugin = FixturePlugin::new().export(
        "apply",
        Behavior::output(format!(
            r#"{{"env_vars":[
                {{"name":"PLAIN","value":"{CANARY}"}},
                {{"name":"FROM_SECRET","value_from":{{"secret_key_ref":{{"name":"db","key":"password"}}}}}},
                {{"name":"FROM_CM","value_from":{{"config_map_key_ref":{{"name":"cfg","key":"host"}}}}}}
            ]}}"#
        )),
    );
    let url = serve_bytes(plugin.wasm()).await;
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;

    let text = call(
        &state,
        "validate_plugin",
        json!({ "source": { "type": "url", "url": url } }),
    )
    .await
    .unwrap();
    assert!(!text.contains(CANARY), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).expect("JSON result");
    assert_eq!(json["valid"], true);
    assert_eq!(
        json["result"]["env_vars"],
        json!([
            { "name": "PLAIN" },
            { "name": "FROM_SECRET", "from_secret": "db", "key": "password" },
            { "name": "FROM_CM", "from_configmap": "cfg", "key": "host" },
        ])
    );
}

// ---------------------------------------------------------------------------
// The argument check, through the real dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_tools_refuse_arguments_their_schema_does_not_allow() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;

    let err = call(
        &state,
        "get_plugin_config",
        json!({ "name": "aws", "include_secrets": true }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        "invalid arguments for get_plugin_config: unknown argument `include_secrets`"
    );

    let err = call(
        &state,
        "scale_application",
        json!({ "namespace": "shop", "name": "web", "replicas": "3" }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        "invalid arguments for scale_application: `replicas` must be an integer"
    );

    let err = call(&state, "enable_plugin", json!({})).await.unwrap_err();
    assert_eq!(
        err,
        "invalid arguments for enable_plugin: missing required argument `name`"
    );

    // Declared arguments still reach the handler.
    let err = call(&state, "enable_plugin", json!({ "name": "nope" }))
        .await
        .unwrap_err();
    assert!(!err.starts_with("invalid arguments"), "{err}");
}
