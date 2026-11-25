//! The mcp-k8s guardrail settings (`mcp_tuning.guardrails`) through the real
//! `/mcp` handler: what `tools/list` offers, what `tools/call` refuses and
//! with which message, and that a settings change applies to the next
//! request.

use std::collections::HashSet;
use std::time::Duration;

use axum::http::HeaderMap;
use sea_orm::{EntityTrait, QueryFilter};
use serde_json::{json, Value};

use super::tests::build_test_state;
use super::tool_telemetry_tests::{call, echo_client, post};
use super::*;
use crate::entities::audit_log;
use crate::handlers::mcp_guardrails::McpGuardrails;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A state with a migrated SQLite database in a temp dir (audit rows are
/// written from background tasks) and `kube_client` as its Kubernetes API.
async fn state_with(kube_client: kube::Client) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = build_test_state().await;
    state.db = crate::db::connect(&format!(
        "sqlite://{}/guardrails.db?mode=rwc",
        dir.path().display()
    ))
    .await
    .unwrap();
    state.kube_client = kube_client;
    (state, dir)
}

/// Store `guardrails` (JSON, as a settings PUT sends it) in the settings.
async fn set_guardrails(state: &AppState, guardrails: Value) {
    let mut s = settings::load_settings_from_db(state).await;
    let parsed: McpGuardrails = serde_json::from_value(guardrails).unwrap();
    s.mcp_tuning.guardrails = Some(parsed.validated().unwrap());
    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .unwrap();
}

async fn tools(state: &AppState) -> Vec<Value> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "tools/list".to_string(),
        params: json!({}),
    };
    let resp = handle_tools_list(state, &req).await;
    resp.result.unwrap()["tools"].as_array().unwrap().clone()
}

fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
    tools.iter().find(|t| t["name"] == name)
}

/// The error a `tools/call` returned, or "" if it succeeded.
async fn call_error(state: &AppState, tool: &str, args: Value) -> String {
    let (_, body) = post(state, HeaderMap::new(), call(tool, args)).await;
    body["error"]["message"].as_str().unwrap_or("").to_string()
}

async fn mcp_rows(state: &AppState) -> Vec<audit_log::Model> {
    audit_log::Entity::find()
        .filter(audit_log::Column::Source.eq(crate::audit::SOURCE_MCP))
        .all(&state.db)
        .await
        .unwrap()
}

async fn wait_for_row(state: &AppState, tool: &str) -> audit_log::Model {
    for _ in 0..500 {
        if let Some(row) = mcp_rows(state).await.into_iter().find(|r| r.tool == tool) {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no audit row for {tool}: {:?}", mcp_rows(state).await);
}

const MANIFEST: &str =
    "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: web\n  namespace: prod\ndata:\n  a: b\n";

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

/// The multi-cluster tools were listed and then answered "Unknown tool";
/// mcp-k8s's resource tools were listed twice.
#[tokio::test]
async fn tools_list_offers_each_served_tool_once() {
    let state = build_test_state().await;
    let listed = tools(&state).await;
    let names = names(&listed);
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "a tool is listed twice: {names:?}"
    );
    for tool in ["list_clusters", "switch_cluster", "get_active_cluster"] {
        assert!(!names.iter().any(|n| n == tool), "{tool} is listed");
    }

    // Every tool deckwatch doesn't serve itself is one mcp-k8s's handle_tool
    // answers (with no arguments it refuses, but it answers).
    let client = mcp_k8s_client(&state, &McpGuardrails::default().permissions());
    for name in &names {
        if mcp_telemetry::deckwatch_tool(name).is_some() {
            continue;
        }
        let answered = Box::pin(mcp_k8s::mcp::handle_tool(&client, name, &json!({})))
            .await
            .is_some();
        assert!(answered, "{name} is listed but nothing serves it");
    }
}

#[tokio::test]
async fn apply_manifest_is_listed_by_default_and_asks_before_each_call() {
    let state = build_test_state().await;
    let listed = tools(&state).await;
    let apply = find(&listed, "apply_manifest").expect("apply_manifest is listed by default");
    assert_eq!(apply["annotations"]["destructiveHint"], true);
    // The same annotations as mcp-k8s's own definition.
    let upstream = mcp_k8s::mcp::tool_definitions(&McpGuardrails::default().permissions())
        .into_iter()
        .find(|t| t["name"] == "apply_manifest")
        .unwrap();
    assert_eq!(apply["annotations"], upstream["annotations"]);
    let description = apply["description"].as_str().unwrap();
    assert!(description.contains("UNRESTRICTED"), "{description}");
    assert!(description.contains("YAML or JSON"), "{description}");

    // And a call isn't refused: it reaches the (unreachable) API.
    let (state, _dir) = state_with(echo_client()).await;
    let error = call_error(&state, "apply_manifest", json!({ "manifest": MANIFEST })).await;
    assert!(!error.contains("turned off"), "{error}");
    assert!(!error.is_empty(), "the echo API fails every request");
}

#[tokio::test]
async fn turning_apply_manifest_off_unlists_and_refuses_it() {
    let (state, _dir) = state_with(echo_client()).await;
    set_guardrails(&state, json!({ "apply_manifest_enabled": false })).await;

    assert!(find(&tools(&state).await, "apply_manifest").is_none());
    let error = call_error(&state, "apply_manifest", json!({ "manifest": MANIFEST })).await;
    assert!(
        error.contains("mcp_tuning.guardrails.apply_manifest_enabled"),
        "{error}"
    );
    let row = wait_for_row(&state, "apply_manifest").await;
    assert_eq!(row.outcome, "denied");
    let detail: Value = serde_json::from_str(&row.detail).unwrap();
    assert_eq!(detail["denied_rule"], "apply_manifest:disabled");
}

/// Settings are read per request: no restart.
#[tokio::test]
async fn tools_list_follows_the_settings_from_the_next_request() {
    let (state, _dir) = state_with(echo_client()).await;
    assert!(find(&tools(&state).await, "apply_manifest").is_some());
    set_guardrails(&state, json!({ "apply_manifest_enabled": false })).await;
    assert!(find(&tools(&state).await, "apply_manifest").is_none());
    set_guardrails(&state, json!({ "apply_manifest_enabled": true })).await;
    assert!(find(&tools(&state).await, "apply_manifest").is_some());
}

/// deckwatch no longer replaces create_pod: mcp-k8s's takes a service
/// account (`service_account` still works, as an alias) and the whole pod
/// template, and says how to opt out of the secure defaults.
#[tokio::test]
async fn create_pod_is_mcp_k8s_with_the_secure_defaults_opt_out() {
    let state = build_test_state().await;
    let listed = tools(&state).await;
    let create_pod = find(&listed, "create_pod").expect("create_pod");
    let props = &create_pod["inputSchema"]["properties"];
    assert!(props.get("service_account_name").is_some(), "{props}");
    assert!(props.get("security_context").is_some(), "{props}");
    let description = create_pod["description"].as_str().unwrap();
    assert!(description.contains("run_as_non_root"), "{description}");
    assert!(mcp_telemetry::deckwatch_tool("create_pod").is_none());
}

// ---------------------------------------------------------------------------
// Refusals name the setting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_guardrail_rejection_names_the_flag_and_the_deckwatch_setting() {
    let (state, _dir) = state_with(echo_client()).await;
    let lb = json!({ "namespace": "prod", "name": "web", "type": "LoadBalancer" });

    let error = call_error(&state, "create_service", lb.clone()).await;
    assert!(
        error.contains("rejected by the mcp-k8s workload policy"),
        "{error}"
    );
    assert!(error.contains("--allow-loadbalancer"), "{error}");
    assert!(
        error.contains(
            "allow_load_balancer is an exception to add (mcp_tuning.guardrails.exceptions)"
        ),
        "{error}"
    );

    set_guardrails(&state, json!({ "exceptions": ["allow_load_balancer"] })).await;
    let error = call_error(&state, "create_service", lb).await;
    assert!(!error.contains("workload policy"), "allowed now: {error}");
    assert!(error.contains("not found"), "it reached the API: {error}");
}

#[tokio::test]
async fn secret_values_are_refused_until_turned_on() {
    let (state, _dir) = state_with(echo_client()).await;
    let args = json!({ "namespace": "prod", "name": "db", "decode": true });

    let error = call_error(&state, "get_secret", args.clone()).await;
    assert!(error.contains("secret decoding is disabled"), "{error}");
    assert!(
        error.contains("mcp_tuning.guardrails.secret_decode_enabled"),
        "{error}"
    );

    set_guardrails(&state, json!({ "secret_decode_enabled": true })).await;
    let error = call_error(&state, "get_secret", args).await;
    assert!(!error.contains("secret decoding is disabled"), "{error}");
    assert!(error.contains("not found"), "it reached the API: {error}");
}

/// The echo API puts the request body in its error, so the pod create_pod
/// sent can be read from it.
#[tokio::test]
async fn create_pod_fills_in_the_secure_defaults_unless_turned_off() {
    let (state, _dir) = state_with(echo_client()).await;
    let args = json!({ "namespace": "prod", "name": "debug", "image": "busybox" });

    let sent = call_error(&state, "create_pod", args.clone()).await;
    assert!(sent.contains(r#""runAsNonRoot":true"#), "{sent}");
    assert!(
        sent.contains(r#""automountServiceAccountToken":false"#),
        "{sent}"
    );

    set_guardrails(&state, json!({ "secure_defaults": false })).await;
    let sent = call_error(&state, "create_pod", args).await;
    assert!(sent.contains("not found"), "{sent}");
    assert!(!sent.contains("runAsNonRoot"), "{sent}");
}

/// The settings are read on every call, from the database only. A row that
/// can't be read leaves the guardrails at their strictest: apply_manifest off.
#[tokio::test]
async fn unreadable_settings_never_relax_a_guardrail() {
    use sea_orm::ActiveValue::Set;
    let (state, _dir) = state_with(echo_client()).await;
    crate::entities::settings::Entity::insert(crate::entities::settings::ActiveModel {
        key: Set("main".to_string()),
        value: Set("{ not json".to_string()),
        updated_at: Set(sea_orm::prelude::DateTimeUtc::from_timestamp(0, 0).unwrap()),
    })
    .exec(&state.db)
    .await
    .unwrap();

    let error = call_error(&state, "apply_manifest", json!({ "manifest": MANIFEST })).await;
    assert!(
        error.contains("mcp_tuning.guardrails.apply_manifest_enabled"),
        "{error}"
    );
}

/// deckwatch's `service_account` argument goes through mcp-k8s, which takes it
/// as an alias of `service_account_name` (its typed tools refuse unknown
/// arguments since 0.6).
#[tokio::test]
async fn service_account_still_works_with_mcp_k8s_refusing_unknown_arguments() {
    let (state, _dir) = state_with(echo_client()).await;

    // update_deployment hands the other fields, service_account included, to
    // mcp-k8s, which reads the deployment first (the echo API's 404).
    let error = call_error(
        &state,
        "update_deployment",
        json!({ "namespace": "prod", "name": "web", "image": "nginx:2", "service_account": "builder" }),
    )
    .await;
    assert!(!error.contains("unknown argument"), "{error}");
    assert!(error.contains("not found"), "{error}");

    // create_pod with a service account gets the secure defaults too now.
    let sent = call_error(
        &state,
        "create_pod",
        json!({ "namespace": "prod", "name": "job", "image": "busybox", "service_account": "builder" }),
    )
    .await;
    assert!(sent.contains(r#""serviceAccountName":"builder""#), "{sent}");
    assert!(sent.contains(r#""runAsNonRoot":true"#), "{sent}");
}
