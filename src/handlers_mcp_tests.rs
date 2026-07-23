use super::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// JSON-RPC parsing
// ---------------------------------------------------------------------------

#[test]
fn test_initialize_response() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "initialize".to_string(),
        params: json!({}),
    };

    let resp = handle_initialize(&req);

    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.id, Some(json!(1)));
    assert!(resp.error.is_none());

    let result = resp.result.expect("should have result");
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["serverInfo"]["name"], "deckwatch");
    assert_eq!(result["serverInfo"]["version"], "0.1.0");
}

#[test]
fn test_tools_list_returns_all_tools() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&req);

    assert!(resp.error.is_none());
    let result = resp.result.expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");
    assert_eq!(tools.len(), 14);
}

#[test]
fn test_tools_list_tool_names() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&req);
    let result = resp.result.expect("should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools should be an array");

    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    let expected = [
        "get_namespaces",
        "list_deployments",
        "get_deployment",
        "get_pod_logs",
        "get_events",
        "get_deployment_history",
        "get_gitops_status",
        "get_build_logs",
        "list_ingresses",
        "get_metrics",
        "create_application",
        "list_addons",
        "list_templates",
        "configure_gitops",
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

#[test]
fn test_tools_have_input_schema() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/list".to_string(),
        params: json!({}),
    };

    let resp = handle_tools_list(&req);
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
            "name": "configure_gitops",
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
async fn test_configure_gitops_missing_oci_repository() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(41)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "configure_gitops",
            "arguments": {
                "namespace": "default",
                "deployment_name": "my-app",
                "repo_url": "https://github.com/org/repo"
            }
        }),
    };

    let resp = handle_tool_call(&state, &req).await;
    let err = resp.error.expect("should error without oci_repository");
    assert!(
        err.message.contains("oci_repository"),
        "error should mention missing oci_repository; got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_tool_call_get_namespaces_shape() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(12)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "get_namespaces",
            "arguments": {}
        }),
    };

    let resp = handle_tool_call(&state, &req).await;

    // get_namespaces talks to the cluster; with our dummy kube client it will
    // error, but the dispatch itself must still produce a valid JSON-RPC
    // response (error envelope, not a panic).
    if let Some(result) = &resp.result {
        // If it somehow succeeds (e.g. a real kubeconfig is present), verify shape.
        let content = result["content"]
            .as_array()
            .expect("content should be array");
        assert!(
            !content.is_empty(),
            "content should have at least one entry"
        );
        assert_eq!(content[0]["type"], "text");
    } else {
        // Error path: verify it's a well-formed JSON-RPC error, not a panic.
        let err = resp
            .error
            .as_ref()
            .expect("should have error when no cluster");
        assert_eq!(err.code, -32000);
        assert!(!err.message.is_empty());
    }
}

#[tokio::test]
async fn test_list_addons_returns_catalog() {
    let state = build_test_state().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(30)),
        method: "tools/call".to_string(),
        params: json!({
            "name": "list_addons",
            "arguments": {}
        }),
    };

    let resp = handle_tool_call(&state, &req).await;
    assert!(
        resp.error.is_none(),
        "list_addons should succeed without a cluster"
    );
    let result = resp.result.expect("should have result");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("should have text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("should be valid JSON");
    let addons = parsed["addons"]
        .as_array()
        .expect("should have addons array");
    let ids: Vec<&str> = addons.iter().filter_map(|a| a["id"].as_str()).collect();
    assert!(
        ids.contains(&"postgres"),
        "catalog should include postgres addon"
    );
    assert!(ids.contains(&"redis"), "catalog should include redis addon");
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
    // params should default to null via #[serde(default)]
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
    // error should be omitted (skip_serializing_if = "Option::is_none")
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
    // result should be omitted
    assert!(
        value.get("result").is_none(),
        "result field should be omitted when None"
    );
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
// Test helpers
// ---------------------------------------------------------------------------

/// Build an AppState for dispatch-level tests. Constructs a dummy kube client
/// from an in-memory kubeconfig pointing at an unreachable server. Tool calls
/// that actually hit the cluster will return connection errors, but dispatch
/// routing and parameter validation are fully testable without a live cluster.
async fn build_test_state() -> crate::state::AppState {
    use crate::rate_limit::RateLimiter;

    // Build a minimal kubeconfig YAML and parse it — avoids constructing
    // non-exhaustive kube config structs field-by-field.
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
        registry_enabled: false,
        ai_rate_limiter: RateLimiter::default(),
        db,
        encryption_key: String::new(),
    }
}
