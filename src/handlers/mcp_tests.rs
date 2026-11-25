use super::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// JSON-RPC parsing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_initialize_response() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "initialize".to_string(),
        params: json!({}),
    };

    let resp = handle_initialize(&state, &req).await;

    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.id, Some(json!(1)));
    assert!(resp.error.is_none());

    let result = resp.result.expect("should have result");
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["serverInfo"]["name"], "deckwatch");
}

#[tokio::test]
async fn test_tools_list_returns_all_tools() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&state, &req).await;

    assert!(resp.error.is_none());
    let result = resp.result.expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");
    assert!(
        tools.len() > 100,
        "expected 100+ tools (mcp-k8s upstream + deckwatch); got {}",
        tools.len()
    );
}

#[tokio::test]
async fn test_tools_list_tool_names() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&state, &req).await;
    let result = resp.result.expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");

    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    let expected = [
        "create_application",
        "list_templates",
        "set_gitops",
        "get_gitops",
        "list_deployments",
        "get_deployment",
        "get_pod_logs",
        "get_events",
        "list_ingresses",
        "list_pods",
        "list_services",
        "list_configmaps",
        "list_secrets",
    ];

    for name in &expected {
        assert!(
            names.contains(name),
            "expected tool '{}' not found in tools list; got: {:?}",
            name,
            names
        );
    }
}

#[tokio::test]
async fn test_tools_have_input_schema() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&state, &req).await;
    let result = resp.result.expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");

    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        let schema = &tool["inputSchema"];
        assert!(
            schema.is_object(),
            "tool '{}' should have an inputSchema object",
            name
        );
        assert_eq!(
            schema["type"], "object",
            "tool '{}' inputSchema.type should be \"object\"",
            name
        );
    }
}

#[test]
fn test_unknown_method_returns_error() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(5)),
        method: "foo/bar".to_string(),
        params: json!({}),
    };

    let resp = method_not_found(&req);

    assert!(resp.result.is_none());
    let err = resp.error.expect("should have error");
    assert_eq!(err.code, -32601);
    assert!(
        err.message.contains("foo/bar"),
        "error message should mention the unknown method"
    );
}

// ---------------------------------------------------------------------------
// Tool call dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_tool_returns_error() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(10)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "nonexistent",
            "arguments": {}
        }),
    };

    let resp = handle_tool_call(&state, &req).await;

    assert!(resp.result.is_none());
    let err = resp.error.expect("should have error");
    assert!(
        err.message.contains("Unknown tool"),
        "error should mention unknown tool; got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_tool_call_missing_namespace() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(11)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "list_deployments",
            "arguments": {}
        }),
    };

    let resp = handle_tool_call(&state, &req).await;

    assert!(resp.result.is_none());
    let err = resp.error.expect("should have error");
    assert!(
        err.message.contains("namespace"),
        "error should mention missing namespace; got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_configure_gitops_missing_repo_url() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(40)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "set_gitops",
            "arguments": {
                "namespace": "default",
                "deployment_name": "my-app",
                "oci_repository": "ghcr.io/org/app"
            }
        }),
    };

    let resp = handle_tool_call(&state, &req).await;
    let err = resp.error.expect("should error without repo_url");
    assert!(
        err.message.contains("repo_url"),
        "error should mention missing repo_url; got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_upstream_tool_dispatch() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(12)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "list_namespaces",
            "arguments": {}
        }),
    };

    let resp = handle_tool_call(&state, &req).await;

    if let Some(result) = &resp.result {
        let content = result["content"]
            .as_array()
            .expect("content should be array");
        assert!(!content.is_empty());
        assert_eq!(content[0]["type"], "text");
    } else {
        let err = resp
            .error
            .as_ref()
            .expect("should have error when no cluster");
        assert_eq!(err.code, -32000);
        assert!(!err.message.is_empty());
    }
}

#[tokio::test]
async fn test_create_application_missing_name() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(20)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "create_application",
            "arguments": { "namespace": "default" }
        }),
    };

    let resp = handle_tool_call(&state, &req).await;
    let err = resp.error.expect("should error without name");
    assert!(
        err.message.contains("name"),
        "error should mention missing name; got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_application_missing_namespace() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(21)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "create_application",
            "arguments": { "name": "my-app" }
        }),
    };

    let resp = handle_tool_call(&state, &req).await;
    let err = resp.error.expect("should error without namespace");
    assert!(
        err.message.contains("namespace"),
        "error should mention missing namespace; got: {}",
        err.message
    );
}

// ---------------------------------------------------------------------------
// update_deployment — service_account field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_update_deployment_no_fields_returns_error() {
    let state = build_test_state().await;
    let resp = handle_tool_call(
        &state,
        &tool_call_req(
            50,
            "update_deployment",
            json!({ "namespace": "default", "name": "my-app" }),
        ),
    )
    .await;
    let err = resp.error.expect("should error with no fields");
    assert!(
        err.message.contains("At least one field"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_update_deployment_service_account_only_attempts_patch() {
    let state = build_test_state().await;
    let resp = handle_tool_call(
        &state,
        &tool_call_req(
            51,
            "update_deployment",
            json!({
                "namespace": "default",
                "name": "my-app",
                "service_account": "my-sa"
            }),
        ),
    )
    .await;
    // With a dummy cluster this will fail at the kube patch call, not at
    // argument validation — confirming dispatch reached the SA patch path.
    if let Some(err) = resp.error {
        assert_ne!(
            err.message, "At least one field must be provided: image, replicas, env, env_from, command, args, port, ports, resource_limits, resource_requests, liveness_probe, readiness_probe, startup_probe, or service_account",
            "should not hit the no-fields error when service_account is provided"
        );
    }
}

/// Regression: a service_account-only update built its Deployment Api directly
/// and skipped the namespace allowlist, so it could repoint workloads in any
/// namespace (e.g. kube-system) at an arbitrary service account.
#[tokio::test]
async fn test_update_deployment_service_account_respects_namespace_allowlist() {
    let mut state = build_test_state().await;
    state.allowed_namespaces = vec!["team-a".to_string()];
    let resp = handle_tool_call(
        &state,
        &tool_call_req(
            53,
            "update_deployment",
            json!({
                "namespace": "kube-system",
                "name": "coredns",
                "service_account": "attacker-sa"
            }),
        ),
    )
    .await;
    let rendered = serde_json::to_string(&resp).expect("serialize response");
    assert!(
        rendered.contains("Namespace 'kube-system' is not in the allowed list"),
        "service_account patch must be refused outside the allowlist: {rendered}"
    );
}

#[tokio::test]
async fn test_update_deployment_service_account_in_schema() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(52)),
        method: "tools/list".to_string(),
        params: json!({}),
    };
    let resp = handle_tools_list(&state, &req).await;
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();

    let update_tool = tools
        .iter()
        .find(|t| t["name"] == "update_deployment")
        .expect("update_deployment must be in tools list");

    let props = &update_tool["inputSchema"]["properties"];
    assert!(
        props.get("service_account").is_some(),
        "update_deployment schema must include service_account property"
    );

    // Must appear exactly once (no duplicate from mcp-k8s).
    let count = tools
        .iter()
        .filter(|t| t["name"] == "update_deployment")
        .count();
    assert_eq!(
        count, 1,
        "update_deployment must appear exactly once in tools list"
    );
}

// ---------------------------------------------------------------------------
// create_pod — service_account field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_pod_service_account_in_schema() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(60)),
        method: "tools/list".to_string(),
        params: json!({}),
    };
    let resp = handle_tools_list(&state, &req).await;
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();

    let create_pod = tools
        .iter()
        .find(|t| t["name"] == "create_pod")
        .expect("create_pod must be in tools list");

    let props = &create_pod["inputSchema"]["properties"];
    assert!(
        props.get("service_account").is_some(),
        "create_pod schema must include service_account"
    );
    assert!(
        props.get("env").is_some(),
        "create_pod schema must include env"
    );

    let count = tools.iter().filter(|t| t["name"] == "create_pod").count();
    assert_eq!(count, 1, "create_pod must appear exactly once");
}

#[tokio::test]
async fn test_create_pod_missing_namespace_returns_error() {
    let state = build_test_state().await;
    let resp = handle_tool_call(
        &state,
        &tool_call_req(
            61,
            "create_pod",
            json!({ "name": "debug", "image": "busybox", "service_account": "my-sa" }),
        ),
    )
    .await;
    let err = resp.error.expect("should error without namespace");
    assert!(err.message.contains("namespace"), "got: {}", err.message);
}

// ---------------------------------------------------------------------------
// Request deserialization
// ---------------------------------------------------------------------------

#[test]
fn test_jsonrpc_request_deserialize() {
    let raw = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/list",
        "params": { "key": "value" }
    });

    let req: JsonRpcRequest = serde_json::from_value(raw).expect("should deserialize");
    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.id, Some(json!(42)));
    assert_eq!(req.method, "tools/list");
    assert_eq!(req.params["key"], "value");
}

#[test]
fn test_jsonrpc_request_missing_params() {
    let raw = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize"
    });

    let req: JsonRpcRequest =
        serde_json::from_value(raw).expect("should deserialize without params");
    assert_eq!(req.method, "initialize");
    assert!(
        req.params.is_null(),
        "params should default to null when omitted"
    );
}

#[test]
fn test_jsonrpc_response_serialize() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        result: Some(json!({"status": "ok"})),
        error: None,
    };

    let value = serde_json::to_value(&resp).expect("should serialize");
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 1);
    assert_eq!(value["result"]["status"], "ok");
    assert!(
        value.get("error").is_none(),
        "error field should be omitted when None"
    );
}

#[test]
fn test_jsonrpc_error_serialize() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(99)),
        result: None,
        error: Some(JsonRpcError {
            code: -32601,
            message: "Method not found".to_string(),
        }),
    };

    let value = serde_json::to_value(&resp).expect("should serialize");
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 99);
    assert!(value.get("result").is_none());
    assert_eq!(value["error"]["code"], -32601);
    assert_eq!(value["error"]["message"], "Method not found");
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

#[test]
fn test_success_response_structure() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!("abc")),
        method: "test".to_string(),
        params: json!(null),
    };

    let resp = success_response(&req, json!({"data": 123}));

    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.id, Some(json!("abc")));
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["data"], 123);
}

#[test]
fn test_error_response_structure() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(7)),
        method: "test".to_string(),
        params: json!(null),
    };

    let resp = error_response(&req, -32000, "something broke");

    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.id, Some(json!(7)));
    assert!(resp.result.is_none());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32000);
    assert_eq!(err.message, "something broke");
}

#[test]
fn test_method_not_found_response() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(8)),
        method: "bogus/method".to_string(),
        params: json!(null),
    };

    let resp = method_not_found(&req);

    assert!(resp.result.is_none());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("bogus/method"));
}

#[test]
fn test_success_response_preserves_null_id() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: "test".to_string(),
        params: json!(null),
    };

    let resp = success_response(&req, json!("ok"));
    assert!(resp.id.is_none());
}

// ---------------------------------------------------------------------------
// prompts/list + prompts/get
// ---------------------------------------------------------------------------

#[test]
fn prompts_list_returns_deployment_readiness_check() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "prompts/list".to_string(),
        params: json!({}),
    };

    let resp = handle_prompts_list(&req);
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let prompts = result["prompts"].as_array().unwrap();
    assert!(prompts.len() >= 2);
    let names: Vec<&str> = prompts.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"deployment-readiness-check"));
    let readiness = prompts
        .iter()
        .find(|p| p["name"] == "deployment-readiness-check")
        .unwrap();
    assert!(readiness["arguments"].as_array().unwrap().len() >= 2);
}

#[test]
fn prompts_get_returns_messages_with_arguments() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "prompts/get".to_string(),
        params: json!({
            "name": "deployment-readiness-check",
            "arguments": {
                "namespace": "test-ns",
                "deployment_name": "my-app"
            }
        }),
    };

    let resp = handle_prompts_get(&req);
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let text = messages[0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("test-ns"));
    assert!(text.contains("my-app"));
    assert!(text.contains("get_deployment"));
    assert!(text.contains("get_gitops"));
}

#[test]
fn prompts_list_includes_pre_deployment_check() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "prompts/list".to_string(),
        params: json!({}),
    };

    let resp = handle_prompts_list(&req);
    let result = resp.result.unwrap();
    let prompts = result["prompts"].as_array().unwrap();
    let names: Vec<&str> = prompts.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"pre-deployment-check"));
    assert!(names.contains(&"deployment-readiness-check"));
}

#[test]
fn prompts_get_pre_deployment_check_returns_codebase_analysis() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "prompts/get".to_string(),
        params: json!({"name": "pre-deployment-check"}),
    };

    let resp = handle_prompts_get(&req);
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    let text = messages[0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("Dockerfile"));
    assert!(text.contains("git"));
    assert!(text.contains("Health endpoint"));
    assert!(text.contains("Database"));
    assert!(text.contains("secrets"));
}

#[test]
fn prompts_get_unknown_returns_error() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "prompts/get".to_string(),
        params: json!({"name": "nonexistent"}),
    };

    let resp = handle_prompts_get(&req);
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("Unknown prompt"));
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn tool_call_req(id: u64, tool: &str, arguments: serde_json::Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(id)),
        method: "tools/call".to_string(),
        params: json!({ "name": tool, "arguments": arguments }),
    }
}

pub(super) async fn build_test_state() -> crate::state::AppState {
    use crate::rate_limit::RateLimiter;

    let _ = rustls::crypto::ring::default_provider().install_default();

    let kubeconfig_yaml = r#"
apiVersion: v1
kind: Config
current-context: dummy
clusters:
  - name: dummy
    cluster:
      server: https://127.0.0.1:1
      insecure-skip-tls-verify: true
contexts:
  - name: dummy
    context:
      cluster: dummy
      user: dummy
      namespace: default
users:
  - name: dummy
    user: {}
"#;
    let kubeconfig: kube::config::Kubeconfig =
        serde_yaml::from_str(kubeconfig_yaml).expect("parse dummy kubeconfig");

    let config = kube::Config::from_custom_kubeconfig(
        kubeconfig,
        &kube::config::KubeConfigOptions::default(),
    )
    .await
    .expect("config from custom kubeconfig");

    let kube_client = kube::Client::try_from(config).expect("dummy kube client");

    let db = crate::db::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");

    crate::state::AppState {
        kube_client,
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-settings".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        plugin_telemetry: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Plugin binary identity (version + sha256) in list_plugins / get_plugin_config
// ---------------------------------------------------------------------------

async fn state_with_plugins() -> crate::state::AppState {
    let state = build_test_state().await;
    let s = settings::DeckwatchSettings {
        plugins: serde_json::from_value(json!([
        { "name": "aws", "enabled": true, "source": { "type": "upload", "filename": "aws.wasm" } },
        { "name": "gcp", "enabled": true, "source": { "type": "url", "url": "https://example.com/gcp.wasm" } }
    ]))
        .expect("plugin configs"),
        ..Default::default()
    };
    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .expect("save settings");

    let bytes = b"\0asm-fix-build".to_vec();
    state
        .plugins
        .write()
        .await
        .push(crate::plugins::LoadedPlugin {
            name: "aws".to_string(),
            sha256: crate::plugins::sha256_hex(&bytes),
            wasm_bytes: std::sync::Arc::new(bytes),
            allowed_hosts: vec![],
            config: Default::default(),
            inherit_env_keys: vec![],
            inherit_env_file_keys: Default::default(),
            metadata: crate::plugins::PluginMetadata {
                name: "aws".to_string(),
                version: "0.7.2".to_string(),
                ..Default::default()
            },
            loaded_at: "2026-11-24T12:00:00+00:00".to_string(),
        });
    state
}

#[tokio::test]
async fn test_list_plugins_reports_loaded_binary_identity() {
    let state = state_with_plugins().await;
    let out: serde_json::Value =
        serde_json::from_str(&tool_list_plugins(&state).await.expect("ok")).unwrap();
    let plugins = out["plugins"].as_array().expect("plugins array");

    let aws = plugins.iter().find(|p| p["name"] == "aws").expect("aws");
    assert_eq!(aws["loaded"], true);
    assert_eq!(aws["version"], "0.7.2");
    assert_eq!(
        aws["sha256"],
        crate::plugins::sha256_hex(b"\0asm-fix-build")
    );
    assert_eq!(aws["wasm_size_bytes"], 14);
    assert_eq!(aws["loaded_at"], "2026-11-24T12:00:00+00:00");
    // Existing fields are still present.
    assert_eq!(aws["enabled"], true);
    assert_eq!(aws["source"], "upload: aws.wasm");

    let gcp = plugins.iter().find(|p| p["name"] == "gcp").expect("gcp");
    assert_eq!(gcp["loaded"], false);
    for key in ["version", "sha256", "wasm_size_bytes", "loaded_at"] {
        assert!(gcp[key].is_null(), "{key} should be null when not loaded");
    }
}

#[tokio::test]
async fn test_get_plugin_config_reports_loaded_binary_identity() {
    let state = state_with_plugins().await;
    let out: serde_json::Value = serde_json::from_str(
        &tool_get_plugin_config(&state, &json!({ "name": "aws" }))
            .await
            .expect("ok"),
    )
    .unwrap();
    assert_eq!(out["status"], "loaded");
    assert_eq!(out["version"], "0.7.2");
    assert_eq!(
        out["sha256"],
        crate::plugins::sha256_hex(b"\0asm-fix-build")
    );
    assert_eq!(out["loaded_at"], "2026-11-24T12:00:00+00:00");
}

// ---------------------------------------------------------------------------
// update_plugin_config: key_value fields
// ---------------------------------------------------------------------------

async fn state_with_key_value_schema() -> crate::state::AppState {
    let state = state_with_plugins().await;
    let field = |key: &str, field_type| crate::plugins::ConfigField {
        key: key.to_string(),
        label: key.to_string(),
        description: String::new(),
        field_type,
        default: None,
        required: false,
        options: vec![],
        env_source: None,
    };
    state.plugins.write().await[0].metadata.config_schema = vec![
        field("RESOURCE_TAGS", crate::plugins::ConfigFieldType::KeyValue),
        field("AWS_REGION", crate::plugins::ConfigFieldType::String),
    ];
    state
}

async fn stored_aws_config(
    state: &crate::state::AppState,
) -> std::collections::BTreeMap<String, String> {
    let s = settings::load_settings_from_db(state).await;
    s.plugins
        .into_iter()
        .find(|p| p.name == "aws")
        .expect("aws plugin")
        .config
        .into_iter()
        .collect()
}

#[tokio::test]
async fn test_update_plugin_config_accepts_key_value_objects_strings_and_null() {
    let state = state_with_key_value_schema().await;

    // A JSON object is stored as its JSON string; strings pass through.
    tool_update_plugin_config(
        &state,
        &json!({ "name": "aws", "config": {
            "RESOURCE_TAGS": { "Team": "platform" },
            "AWS_REGION": "us-gov-west-1"
        }}),
    )
    .await
    .expect("update ok");
    let cfg = stored_aws_config(&state).await;
    let tags: serde_json::Value = serde_json::from_str(&cfg["RESOURCE_TAGS"]).unwrap();
    assert_eq!(tags, json!({ "Team": "platform" }));
    assert_eq!(cfg["AWS_REGION"], "us-gov-west-1");

    // The same value as a JSON string is accepted too.
    tool_update_plugin_config(
        &state,
        &json!({ "name": "aws", "config": { "RESOURCE_TAGS": "{\"Team\":\"data\"}" } }),
    )
    .await
    .expect("string form ok");
    let tags: serde_json::Value =
        serde_json::from_str(&stored_aws_config(&state).await["RESOURCE_TAGS"]).unwrap();
    assert_eq!(tags, json!({ "Team": "data" }));

    // null removes a key, as the tool description documents.
    tool_update_plugin_config(
        &state,
        &json!({ "name": "aws", "config": { "AWS_REGION": null } }),
    )
    .await
    .expect("null ok");
    assert!(!stored_aws_config(&state).await.contains_key("AWS_REGION"));
}

#[tokio::test]
async fn test_update_plugin_config_rejects_invalid_key_value_without_partial_writes() {
    let state = state_with_key_value_schema().await;
    for bad in [
        json!("Team=platform"),
        json!(["Team"]),
        json!({ "Count": 3 }),
        json!(7),
    ] {
        let err = tool_update_plugin_config(
            &state,
            &json!({ "name": "aws", "config": {
                "AWS_REGION": "us-gov-west-1",
                "RESOURCE_TAGS": bad
            }}),
        )
        .await
        .expect_err("invalid key_value must be rejected");
        assert!(
            err.contains("RESOURCE_TAGS"),
            "error names the field: {err}"
        );
        // The valid AWS_REGION in the same call must not have been applied.
        assert!(
            !stored_aws_config(&state).await.contains_key("AWS_REGION"),
            "no partial update for {bad}"
        );
    }
}

// ---------------------------------------------------------------------------
// generate_local_build
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generate_local_build_shell_quotes_the_stored_gitops_fields() {
    use sea_orm::{ActiveModelTrait as _, IntoActiveModel as _};

    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let app_id = crate::db::ensure_application(&state.db, "team", "web")
        .await
        .unwrap();
    let now = sea_orm::prelude::DateTimeUtc::default();
    // Whoever configures GitOps for the deployment writes these fields.
    gitops_configs::Model {
        id: "gitops-1".into(),
        application_id: app_id,
        repo_url: "https://git.example/team/web".into(),
        branch: "main".into(),
        token_secret: String::new(),
        encrypted_token: None,
        token_binding: None,
        git_auth_user: String::new(),
        dockerfile_path: "build/Dockerfile$(touch canary)".into(),
        docker_context: "app;touch canary".into(),
        oci_repository: "registry.example/team/web`touch canary`".into(),
        include_paths: String::new(),
        exclude_paths: String::new(),
        poll_interval_seconds: 60,
        webhook_enabled: false,
        last_commit_sha: None,
        last_build_status: None,
        last_build_job: None,
        last_build_time: None,
        last_build_error: None,
        created_at: now,
        updated_at: now,
    }
    .into_active_model()
    .reset_all()
    .insert(&state.db)
    .await
    .unwrap();

    let out = tool_generate_local_build(
        &state,
        &json!({"namespace": "team", "deployment_name": "web", "no_push": false}),
    )
    .await
    .unwrap();
    let result: serde_json::Value = serde_json::from_str(&out).unwrap();
    let command = result["command"].as_str().unwrap();
    for quoted in [
        "-v './app;touch canary:/workspace'",
        "'--dockerfile=/workspace/Dockerfile$(touch canary)'",
        "'--destination=registry.example/team/web`touch canary`:local-test'",
    ] {
        assert!(command.contains(quoted), "{quoted} in\n{command}");
    }
    let argv: Vec<&str> = result["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[..3], ["docker", "run", "--rm"]);
    assert!(argv.contains(&"./app;touch canary:/workspace"), "{argv:?}");
    assert!(argv.contains(&"--dockerfile=/workspace/Dockerfile$(touch canary)"));
}
