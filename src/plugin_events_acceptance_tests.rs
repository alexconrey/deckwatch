//! Acceptance for plugin events (OBS-1, phase Q2): a plugin that keeps
//! failing on the reconcile loop shows up on the application page, as a
//! Kubernetes Event, in MCP `get_events` and as exactly one notification,
//! and its log lines sit inside the plugin span. Recovering is reported
//! once too.
//!
//! The cluster is a fake API server that stores Events the way the real one
//! does (`plugin_fixture::fake_events`); `live_plugin_events_tests` runs the
//! same story against a throwaway k3d cluster.

use axum::extract::{Path, State};
use axum::Json;
use sea_orm::{ActiveModelTrait, ActiveValue::Set};

use crate::entities::application_plugin_resources;
use crate::plugin_events::{codes, Health};
use crate::plugin_fixture::capture::capture_async;
use crate::plugin_fixture::fake_events::FakeEventsApi;
use crate::plugin_fixture::{
    enable_notifications, serve_webhook, test_state, Behavior, FixturePlugin, LogLevel,
};
use crate::state::AppState;
use crate::watcher::{reconcile_application_plugins, PluginFieldSweep};

/// A plugin that logs through the PDK, then reports an error.
pub(crate) fn sealed_vault() -> crate::plugins::LoadedPlugin {
    FixturePlugin::new()
        .export(
            "apply",
            Behavior::logs_then_output(
                &[(LogLevel::Warn, "unsealing vault for shop/web")],
                r#"{"errors":["vault is sealed"]}"#,
            ),
        )
        .loaded("vault")
}

/// Give application `shop` in `ns` a provisioned resource, so the
/// reconcile loop visits it.
pub(crate) async fn provisioned(state: &AppState, ns: &str) {
    crate::db::ensure_application(&state.db, ns, "shop")
        .await
        .unwrap();
    let now = sea_orm::prelude::DateTimeUtc::from_timestamp(1_800_000_000, 0).unwrap();
    application_plugin_resources::ActiveModel {
        id: Set("row".into()),
        application_id: Set(format!("{ns}/shop")),
        plugin_name: Set("vault".into()),
        resource_id: Set("kv".into()),
        fields: Set("{}".into()),
        state: Set(r#"{"VAULT_ADDR":"https://vault.internal"}"#.into()),
        annotations: Set("{}".into()),
        sidecars: Set("[]".into()),
        init_containers: Set("[]".into()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.db)
    .await
    .unwrap();
}

pub(crate) async fn get_events(state: &AppState, ns: &str) -> String {
    use axum::response::IntoResponse;
    let request: crate::handlers::mcp::JsonRpcRequest = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "get_events", "arguments": { "namespace": ns } },
    }))
    .unwrap();
    let response = crate::handlers::mcp::handle_mcp(State(state.clone()), Json(request))
        .await
        .into_response();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_failing_plugin_is_visible_everywhere_and_notifies_once() {
    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let kube = api.kube();
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![sealed_vault()], Some(kube.client())).await;
    let (webhook, notifications) = serve_webhook().await;
    enable_notifications(&state, &webhook).await;
    crate::plugin_status::spawn_worker(&state);
    provisioned(&state, "shop").await;
    let mut sweep = PluginFieldSweep::default();

    // Five reconcile cycles (50 seconds of the real loop); the first one
    // with its log lines captured.
    let ((), logs) = capture_async(reconcile_application_plugins(&state, &mut sweep)).await;
    for _ in 0..4 {
        reconcile_application_plugins(&state, &mut sweep).await;
    }
    state.plugin_telemetry.flush().await;

    // 1. The application page's status: failing, five calls in a row.
    let Json(rows) = crate::handlers::plugin_status::for_application(
        State(state.clone()),
        Path(("shop".into(), "shop".into())),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].plugin, "vault");
    assert_eq!(rows[0].health, Health::Failing);
    assert_eq!(rows[0].consecutive_failures, 5);
    assert_eq!(rows[0].last_events[0].event.message, "vault is sealed");
    assert_eq!(rows[0].last_events[0].event.code, codes::UNSPECIFIED);

    // 2. One Kubernetes Event on the Deployment (the repeats were folded).
    let events = api.events();
    assert_eq!(events.len(), 1, "{events:#?}");
    assert_eq!(events[0]["type"], "Warning");
    assert_eq!(events[0]["reason"], "PluginApplyFailed");
    assert_eq!(events[0]["regarding"]["name"], "web");
    assert_eq!(events[0]["regarding"]["uid"], "uid-web");
    assert_eq!(events[0]["reportingController"], "deckwatch");
    assert_eq!(
        events[0]["note"],
        "plugin vault: plugin.unspecified: vault is sealed"
    );

    // 3. Agents see it through MCP get_events, fenced as untrusted text.
    let listed = get_events(&state, "shop").await;
    assert!(listed.contains("UNTRUSTED KUBERNETES EVENTS"), "{listed}");
    assert!(
        listed
            .contains("Warning Deployment web: plugin vault: plugin.unspecified: vault is sealed"),
        "{listed}"
    );

    // 4. Exactly one notification for five failing cycles.
    {
        let sent = notifications.lock().unwrap();
        assert_eq!(sent.len(), 1, "{sent:#?}");
        assert_eq!(sent[0]["event_type"], "plugin_failing");
        assert_eq!(sent[0]["event"]["application"], "shop");
        assert_eq!(sent[0]["event"]["message"], "vault is sealed");
    }

    // 5. The plugin's own log line sits inside the plugin span.
    let line = logs
        .iter()
        .find(|e| e.target == "extism::pdk" && e.message == "unsealing vault for shop/web")
        .expect("plugin log line");
    let span = line.span("plugin apply").expect("inside the plugin span");
    assert_eq!(span.fields["deckwatch.plugin.name"], "vault");
    assert_eq!(span.fields["deckwatch.plugin.trigger"], "reconcile");
    assert_eq!(span.fields["deckwatch.application"], "shop");
    assert_eq!(span.fields["k8s.deployment.name"], "web");

    // The plugin is fixed: one recovery, everywhere.
    *state.plugins.write().await = vec![FixturePlugin::new()
        .export("apply", Behavior::output("{}"))
        .loaded("vault")];
    for _ in 0..3 {
        reconcile_application_plugins(&state, &mut sweep).await;
    }
    state.plugin_telemetry.flush().await;
    let Json(rows) = crate::handlers::plugin_status::for_application(
        State(state.clone()),
        Path(("shop".into(), "shop".into())),
    )
    .await
    .unwrap();
    assert_eq!(rows[0].health, Health::Healthy);
    let reasons: Vec<String> = api
        .events()
        .iter()
        .map(|e| e["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(reasons, vec!["PluginApplyFailed", "PluginRecovered"]);
    let kinds: Vec<String> = notifications
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["event_type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["plugin_failing", "plugin_recovered"]);
}
