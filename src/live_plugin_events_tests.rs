//! Live checks for plugin Kubernetes Events, against a throwaway k3d cluster.
//!
//! Ignored by default. They only run when `DECKWATCH_LIVE_K3D=1` and the
//! current kube context is a `k3d-dw-*` cluster, and never touch anything
//! outside the namespace they create. Run with:
//!
//! ```sh
//! k3d cluster create dw-plugin-events --kubeconfig-update-default=false
//! export KUBECONFIG=$(mktemp) && k3d kubeconfig get dw-plugin-events > "$KUBECONFIG"
//! DECKWATCH_LIVE_K3D=1 cargo test --lib live_cluster_ -- --ignored --nocapture
//! k3d cluster delete dw-plugin-events
//! ```
//!
//! deckwatch acts as a ServiceAccount bound to the Helm chart's own
//! ClusterRole, so a missing RBAC rule fails the test the way it would fail
//! in a real install.

use std::time::Duration;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRole, ClusterRoleBinding};
use kube::api::{DeleteParams, PostParams};
use kube::Api;

use crate::plugin_fixture::{Behavior, FixturePlugin};

pub(crate) struct LiveCluster {
    pub admin: kube::Client,
    /// Acts as the deckwatch ServiceAccount, bound to the chart's ClusterRole.
    pub deckwatch: kube::Client,
    pub namespace: String,
    cluster_role: String,
}

/// `None` (and the test returns early) unless this is a throwaway k3d run.
pub(crate) async fn live_cluster() -> Option<LiveCluster> {
    if std::env::var("DECKWATCH_LIVE_K3D").as_deref() != Ok("1") {
        eprintln!("skipping: set DECKWATCH_LIVE_K3D=1 to run against a throwaway k3d cluster");
        return None;
    }
    let kubeconfig = kube::config::Kubeconfig::read().expect("read KUBECONFIG");
    let context = kubeconfig.current_context.clone().unwrap_or_default();
    assert!(
        context.starts_with("k3d-dw-"),
        "refusing to run against kube context {context:?}: only k3d-dw-* throwaway clusters"
    );
    let _ = rustls::crypto::ring::default_provider().install_default();
    let admin = kube::Client::try_default().await.expect("admin client");

    let suffix = format!("{:08x}", rand::random::<u32>());
    let namespace = format!("dw-plugin-events-{suffix}");
    let cluster_role = format!("dw-plugin-events-{suffix}");
    let ns_api: Api<Namespace> = Api::all(admin.clone());
    ns_api
        .create(
            &PostParams::default(),
            &serde_json::from_value(serde_json::json!({ "metadata": { "name": namespace } }))
                .unwrap(),
        )
        .await
        .expect("create namespace");
    let sa_api: Api<ServiceAccount> = Api::namespaced(admin.clone(), &namespace);
    sa_api
        .create(
            &PostParams::default(),
            &serde_json::from_value(serde_json::json!({ "metadata": { "name": "deckwatch" } }))
                .unwrap(),
        )
        .await
        .expect("create service account");

    // The chart's ClusterRole, exactly as rendered.
    let rendered = std::process::Command::new("helm")
        .args([
            "template",
            "dw",
            concat!(env!("CARGO_MANIFEST_DIR"), "/helm/deckwatch"),
            "--show-only",
            "templates/clusterrole.yaml",
            "--set",
            "image.repository=test",
            "--set",
            "image.tag=test",
        ])
        .output()
        .expect("helm template");
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let mut role: ClusterRole = serde_yaml::from_slice(&rendered.stdout).expect("ClusterRole YAML");
    role.metadata = serde_json::from_value(serde_json::json!({ "name": cluster_role })).unwrap();
    Api::<ClusterRole>::all(admin.clone())
        .create(&PostParams::default(), &role)
        .await
        .expect("create ClusterRole");
    Api::<ClusterRoleBinding>::all(admin.clone())
        .create(
            &PostParams::default(),
            &serde_json::from_value(serde_json::json!({
                "metadata": { "name": cluster_role },
                "roleRef": { "apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": cluster_role },
                "subjects": [{ "kind": "ServiceAccount", "name": "deckwatch", "namespace": namespace }],
            }))
            .unwrap(),
        )
        .await
        .expect("create ClusterRoleBinding");

    let mut config = kube::Config::infer().await.expect("kube config");
    config.auth_info.impersonate = Some(format!("system:serviceaccount:{namespace}:deckwatch"));
    let deckwatch = kube::Client::try_from(config).expect("impersonating client");

    Some(LiveCluster {
        admin,
        deckwatch,
        namespace,
        cluster_role,
    })
}

impl LiveCluster {
    /// A Deployment in the test namespace that belongs to application `app`.
    pub async fn deployment(&self, name: &str, app: &str) -> Deployment {
        Api::<Deployment>::namespaced(self.admin.clone(), &self.namespace)
            .create(
                &PostParams::default(),
                &serde_json::from_value(serde_json::json!({
                    "metadata": { "name": name, "labels": { "deckwatch.io/application": app } },
                    "spec": {
                        "replicas": 0,
                        "selector": { "matchLabels": { "app": name } },
                        "template": {
                            "metadata": { "labels": { "app": name } },
                            "spec": { "containers": [{ "name": "app", "image": "registry.k8s.io/pause:3.10" }] },
                        },
                    },
                }))
                .unwrap(),
            )
            .await
            .expect("create deployment")
    }

    /// `kubectl describe deployment <name>` output, with the test KUBECONFIG.
    pub fn describe(&self, name: &str) -> String {
        let out = std::process::Command::new("kubectl")
            .args(["-n", &self.namespace, "describe", "deployment", name])
            .output()
            .expect("kubectl describe");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub async fn cleanup(self) {
        let dp = DeleteParams::default();
        let _ = Api::<ClusterRoleBinding>::all(self.admin.clone())
            .delete(&self.cluster_role, &dp)
            .await;
        let _ = Api::<ClusterRole>::all(self.admin.clone())
            .delete(&self.cluster_role, &dp)
            .await;
        let _ = Api::<Namespace>::all(self.admin.clone())
            .delete(&self.namespace, &dp)
            .await;
    }
}

/// Call an MCP tool through deckwatch's `/mcp` handler; returns the text
/// content (or the error message).
pub(crate) async fn mcp_tool(
    state: &crate::state::AppState,
    tool: &str,
    args: serde_json::Value,
) -> String {
    use axum::response::IntoResponse;
    let request: crate::handlers::mcp::JsonRpcRequest = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    }))
    .unwrap();
    let response = crate::handlers::mcp::handle_mcp(
        axum::extract::State(state.clone()),
        axum::http::HeaderMap::new(),
        axum::Json(request),
    )
    .await
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json["result"]["content"][0]["text"]
        .as_str()
        .or_else(|| json["error"]["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Wait for `cond` on `kubectl describe`, which sees new Events within a
/// moment.
pub(crate) async fn eventually(
    cluster: &LiveCluster,
    name: &str,
    cond: impl Fn(&str) -> bool,
) -> String {
    let mut last = String::new();
    for _ in 0..30 {
        last = cluster.describe(name);
        if cond(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("condition never met; last describe:\n{last}");
}

/// A failing plugin's Event shows up in `kubectl describe deployment`
/// (it carries the Deployment's UID), repeats fold into a series, and the
/// chart's ClusterRole allows all of it.
#[tokio::test]
#[ignore = "needs a throwaway k3d cluster; see the module docs"]
async fn live_cluster_plugin_failure_is_a_kubernetes_event_on_the_deployment() {
    let Some(cluster) = live_cluster().await else {
        return;
    };
    cluster.deployment("web", "shop").await;

    let dir = tempfile::tempdir().unwrap();
    let plugin = FixturePlugin::new()
        .export("apply", Behavior::Fail("vault is sealed".into()))
        .loaded("vault");
    let state = crate::plugin_fixture::test_state(
        dir.path(),
        vec![plugin],
        Some(cluster.deckwatch.clone()),
    )
    .await;
    // Republish every time so the series grows within the test.
    crate::plugin_status::spawn_worker_with(
        &state,
        crate::plugin_k8s_events::KubeEventSink::with_interval(
            cluster.deckwatch.clone(),
            Duration::ZERO,
        ),
    );

    let update = || {
        crate::handlers::deployments::update(
            axum::extract::State(state.clone()),
            axum::extract::Path((cluster.namespace.clone(), "web".to_string())),
            axum::Json(serde_json::from_value(serde_json::json!({ "replicas": 0 })).unwrap()),
        )
    };
    let axum::Json(first) = update().await.expect("update succeeds despite the plugin");
    assert_eq!(
        first.plugin_events[0].event.message,
        "apply() returned an error: vault is sealed"
    );
    state.plugin_telemetry.flush().await;

    let described = eventually(&cluster, "web", |d| d.contains("PluginApplyFailed")).await;
    assert!(described.contains("Warning"), "{described}");
    assert!(
        described.contains("deckwatch"),
        "reporting controller: {described}"
    );
    assert!(described.contains("vault is sealed"), "{described}");

    // A repeat is the same Event, counted.
    let _ = update().await.unwrap();
    state.plugin_telemetry.flush().await;
    let described = eventually(&cluster, "web", |d| d.contains("(x2 over")).await;
    eprintln!("{described}");

    // Agents see it through MCP get_events, fenced as untrusted text.
    let events = mcp_tool(
        &state,
        "get_events",
        serde_json::json!({ "namespace": cluster.namespace, "resource_name": "web" }),
    )
    .await;
    eprintln!("{events}");
    assert!(events.contains("UNTRUSTED KUBERNETES EVENTS"), "{events}");
    assert!(
        events.contains("Warning Deployment web: plugin vault"),
        "{events}"
    );
    assert!(events.contains("vault is sealed"), "{events}");
    assert_eq!(
        described.matches("PluginApplyFailed").count(),
        1,
        "{described}"
    );

    cluster.cleanup().await;
}

/// The acceptance story against a real API server: a plugin that keeps
/// failing on the reconcile loop shows up in the status (the application
/// page), as one Kubernetes Event in `kubectl describe`, in MCP
/// `get_events`, and as exactly one notification, with its log lines inside
/// the plugin span; its recovery is reported once too.
#[tokio::test]
#[ignore = "needs a throwaway k3d cluster; see the module docs"]
async fn live_cluster_acceptance_failing_plugin_is_visible_everywhere_and_notifies_once() {
    use crate::plugin_events_acceptance_tests::{get_events, provisioned, sealed_vault};
    use crate::plugin_fixture::capture::capture_async;
    use crate::watcher::{reconcile_application_plugins, PluginFieldSweep};

    let Some(cluster) = live_cluster().await else {
        return;
    };
    let ns = cluster.namespace.clone();
    cluster.deployment("web", "shop").await;

    let dir = tempfile::tempdir().unwrap();
    let mut state = crate::plugin_fixture::test_state(
        dir.path(),
        vec![sealed_vault()],
        Some(cluster.deckwatch.clone()),
    )
    .await;
    // Keep the reconciler's sweep inside the test namespace.
    state.allowed_namespaces = vec![ns.clone()];
    let (webhook, notifications) = crate::plugin_fixture::serve_webhook().await;
    crate::plugin_fixture::enable_notifications(&state, &webhook).await;
    crate::plugin_status::spawn_worker(&state);
    provisioned(&state, &ns).await;
    let mut sweep = PluginFieldSweep::default();

    let ((), logs) = capture_async(reconcile_application_plugins(&state, &mut sweep)).await;
    for _ in 0..4 {
        reconcile_application_plugins(&state, &mut sweep).await;
    }
    state.plugin_telemetry.flush().await;

    let axum::Json(rows) = crate::handlers::plugin_status::for_application(
        axum::extract::State(state.clone()),
        axum::extract::Path((ns.clone(), "shop".to_string())),
    )
    .await
    .unwrap();
    assert_eq!(
        rows[0].health,
        crate::plugin_events::Health::Failing,
        "{rows:#?}"
    );
    assert_eq!(rows[0].consecutive_failures, 5);

    let described = eventually(&cluster, "web", |d| d.contains("PluginApplyFailed")).await;
    eprintln!("{described}");
    assert_eq!(
        described.matches("PluginApplyFailed").count(),
        1,
        "{described}"
    );
    assert!(described.contains("vault is sealed"), "{described}");
    assert!(
        !described.contains("(x"),
        "repeats within 5 minutes are folded: {described}"
    );

    let listed = get_events(&state, &ns).await;
    eprintln!("{listed}");
    assert!(listed.contains("UNTRUSTED KUBERNETES EVENTS"), "{listed}");
    assert!(
        listed.contains("plugin vault: plugin.unspecified: vault is sealed"),
        "{listed}"
    );

    assert_eq!(
        notifications.lock().unwrap().len(),
        1,
        "one notification for five failures"
    );

    let line = logs
        .iter()
        .find(|e| e.target == "extism::pdk" && e.message == "unsealing vault for shop/web")
        .expect("plugin log line");
    assert_eq!(
        line.span("plugin apply")
            .expect("inside the plugin span")
            .fields["deckwatch.plugin.name"],
        "vault"
    );

    *state.plugins.write().await = vec![crate::plugin_fixture::FixturePlugin::new()
        .export("apply", crate::plugin_fixture::Behavior::output("{}"))
        .loaded("vault")];
    reconcile_application_plugins(&state, &mut sweep).await;
    state.plugin_telemetry.flush().await;
    let described = eventually(&cluster, "web", |d| d.contains("PluginRecovered")).await;
    eprintln!("{described}");
    let kinds: Vec<String> = notifications
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["event_type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["plugin_failing", "plugin_recovered"]);

    cluster.cleanup().await;
}
