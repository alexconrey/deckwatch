//! Deployment create and update return what the plugins reported.

use super::*;
use crate::plugin_events::codes;
use crate::plugin_fixture::fake_kube::FakeKube;
use crate::plugin_fixture::{test_state, Behavior, FixturePlugin};
use serde_json::{json, Value};

/// A namespace with one Deployment that answers create, get, replace and
/// the pod/ingress lists the handlers make.
fn cluster() -> FakeKube {
    let stored = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
    FakeKube::new(move |method, path, body| {
        let list =
            |kind: &str| json!({"apiVersion": "v1", "kind": kind, "metadata": {}, "items": []});
        match (method, path) {
            ("POST", "/apis/apps/v1/namespaces/shop/deployments") => {
                *stored.lock().unwrap() = body.clone();
                Some((201, body.clone()))
            }
            ("GET", "/apis/apps/v1/namespaces/shop/deployments/web") => {
                Some((200, stored.lock().unwrap().clone()))
            }
            ("PUT", "/apis/apps/v1/namespaces/shop/deployments/web") => {
                *stored.lock().unwrap() = body.clone();
                Some((200, body.clone()))
            }
            ("GET", "/api/v1/namespaces/shop/pods") => Some((200, list("PodList"))),
            ("GET", "/apis/networking.k8s.io/v1/namespaces/shop/ingresses") => {
                Some((200, list("IngressList")))
            }
            _ => None,
        }
    })
}

fn plugin() -> crate::plugins::LoadedPlugin {
    FixturePlugin::new()
        .export(
            "apply",
            Behavior::output(
                r#"{"env_vars":[{"name":"FROM_PLUGIN","value":"1"}],
                    "warnings":["sidecar image is not pinned"]}"#,
            ),
        )
        .loaded("vault")
}

#[tokio::test]
async fn create_returns_plugin_warnings_with_the_deployment() {
    let fake = cluster();
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![plugin()], Some(fake.client())).await;
    let req: CreateDeploymentRequest = serde_json::from_value(json!({
        "name": "web",
        "image": "nginx:1.27",
        "labels": { "deckwatch.io/application": "shop" },
    }))
    .unwrap();

    let (status, Json(response)) = create(State(state), Path("shop".into()), Json(req))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(response.plugin_events.len(), 1);
    let report = &response.plugin_events[0];
    assert_eq!(report.plugin, "vault");
    assert_eq!(report.export, "apply");
    assert_eq!(report.event.code, codes::UNSPECIFIED);
    assert_eq!(report.event.message, "sidecar image is not pinned");

    // The plugin ran before the object reached the API server.
    let posted = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(
        posted.body["spec"]["template"]["spec"]["containers"][0]["env"][0]["name"],
        "FROM_PLUGIN"
    );

    // Flattened: the deployment fields plus `plugin_events`.
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["name"], "web");
    assert_eq!(json["plugin_events"][0]["severity"], "warning");
}

#[tokio::test]
async fn update_returns_plugin_failures_without_failing_the_update() {
    let fake = cluster();
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![plugin()], Some(fake.client())).await;
    let req: CreateDeploymentRequest =
        serde_json::from_value(json!({ "name": "web", "image": "nginx:1.27" })).unwrap();
    let _ = create(State(state.clone()), Path("shop".into()), Json(req))
        .await
        .unwrap();

    // The plugin now traps; the update still goes through.
    *state.plugins.write().await = vec![FixturePlugin::new()
        .export("apply", Behavior::Trap)
        .loaded("vault")];
    let req: UpdateDeploymentRequest =
        serde_json::from_value(json!({ "image": "nginx:1.28" })).unwrap();
    let Json(response) = update(State(state), Path(("shop".into(), "web".into())), Json(req))
        .await
        .unwrap();
    assert_eq!(response.deployment.detail.image, "nginx:1.28");
    assert_eq!(response.plugin_events[0].event.code, codes::TRAP);
}

#[tokio::test]
async fn without_plugins_the_response_has_no_plugin_events_field() {
    let fake = cluster();
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], Some(fake.client())).await;
    let req: CreateDeploymentRequest =
        serde_json::from_value(json!({ "name": "web", "image": "nginx:1.27" })).unwrap();
    let (_, Json(response)) = create(State(state), Path("shop".into()), Json(req))
        .await
        .unwrap();
    assert!(serde_json::to_value(&response)
        .unwrap()
        .get("plugin_events")
        .is_none());
}
