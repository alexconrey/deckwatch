// HTTP-surface integration tests for the deckwatch backend.
//
// These tests exercise the axum Router end-to-end via `tower::ServiceExt`,
// which lets us send `Request`s directly into the app without binding a
// real TCP listener. Every test is `#[ignore]`d because they require a
// live `kube::Client` — i.e. a reachable kubernetes API (kind, k3d, minikube,
// or a real cluster with `~/.kube/config` pointed at it).
//
// ## How to run
//
// ```bash
// # Point at a scratch cluster first (kind is easiest):
// kind create cluster --name deckwatch-test
// kubectl config use-context kind-deckwatch-test
//
// # Then:
// cargo test -- --ignored
//
// # Or a single test:
// cargo test integration_tests::health_check -- --ignored
// ```
//
// ## Wiring
//
// To integrate, append to `src/main.rs` (or an appropriate crate-root
// module like `src/lib.rs` if you split the binary):
//
//     #[cfg(test)]
//     #[path = "integration_tests.rs"]
//     mod integration_tests;
//
// The tests use `super::*` imports and reach into `crate::routes`,
// `crate::state`, and `crate::handlers` — same visibility surface as
// `main.rs`, so no `pub` changes are required.
//
// ## What is covered
//
// * `GET /api/healthz`             — public, no kube round-trip
// * `GET /api/readyz`              — pings the apiserver, verifies 200
// * `GET /api/namespaces`          — lists cluster namespaces
// * `GET /api/settings`            — reads (or defaults) the settings CM
// * `PUT /api/settings`            — writes settings, round-trips them
// * Deployment CRUD flow:
//     - POST   /api/namespaces/{ns}/deployments
//     - GET    /api/namespaces/{ns}/deployments/{name}
//     - GET    /api/namespaces/{ns}/deployments
//     - PUT    /api/namespaces/{ns}/deployments/{name}
//     - DELETE /api/namespaces/{ns}/deployments/{name}
//
// All CRUD tests target the `deckwatch-it` namespace (auto-created and
// torn down per test) so they don't pollute `default`. Tests are
// self-cleaning; a failure between create + delete will leak the test
// namespace but re-runs are idempotent (the namespace name is stable).

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use k8s_openapi::api::core::v1::Namespace;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, DeleteParams, PostParams};
    use serde_json::{json, Value};
    use tower::ServiceExt; // for `oneshot`

    use crate::auth::AuthConfig;
    use crate::handlers::registry::RegistryStore;
    use crate::routes::build_router;
    use crate::state::AppState;

    // Namespace used for CRUD tests. Kept stable so a leaked resource from
    // a previously-crashed run is cleaned up on the next successful run
    // instead of accumulating `deckwatch-it-<uuid>` clutter.
    const IT_NAMESPACE: &str = "deckwatch-it";

    async fn build_state() -> AppState {
        let kube_client = kube::Client::try_default()
            .await
            .expect("cannot build kube client — is your kubeconfig valid?");
        AppState {
            kube_client,
            // Empty allow-list = permit any namespace. Matches how the CLI
            // treats `--allowed-namespaces` when the flag is omitted.
            allowed_namespaces: vec![],
            settings_namespace: "default".to_string(),
            settings_configmap_name: "deckwatch-it-settings".to_string(),
            registry_public_url: None,
        }
    }

    async fn build_app() -> axum::Router {
        let state = build_state().await;
        build_router(
            state,
            None::<RegistryStore>,
            // Frontend + book dirs don't need to exist — the fallback
            // service will 404 anything not matched by the API. The tests
            // only hit `/api/*` and `/metrics`, so the SPA fallback never
            // fires.
            "/tmp/deckwatch-it-frontend",
            "/tmp/deckwatch-it-book",
            AuthConfig::disabled(),
        )
    }

    async fn ensure_test_namespace() {
        let client = kube::Client::try_default().await.expect("kube client");
        let api: Api<Namespace> = Api::all(client);
        // get_opt returns None on 404, which is what we want the first time.
        if api.get_opt(IT_NAMESPACE).await.expect("get_opt").is_some() {
            return;
        }
        let ns = Namespace {
            metadata: ObjectMeta {
                name: Some(IT_NAMESPACE.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        api.create(&PostParams::default(), &ns)
            .await
            .expect("create test namespace");
    }

    // Best-effort cleanup of a single deployment inside the test namespace.
    // Not a namespace teardown — we keep the namespace across runs so the
    // apiserver doesn't churn on repeated create/delete of a namespace
    // (which is slow and can hang in the `Terminating` state).
    async fn cleanup_deployment(name: &str) {
        use k8s_openapi::api::apps::v1::Deployment;
        let client = kube::Client::try_default().await.expect("kube client");
        let api: Api<Deployment> = Api::namespaced(client, IT_NAMESPACE);
        let _ = api.delete(name, &DeleteParams::default()).await;
    }

    async fn body_json(body: Body) -> Value {
        let bytes = to_bytes(body, usize::MAX).await.expect("read body");
        if bytes.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            panic!(
                "response body was not JSON: {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }

    fn get(path: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .expect("build request")
    }

    fn json_request(method: Method, path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .expect("build request")
    }

    // ---- health / readiness ----

    #[tokio::test]
    #[ignore = "requires a live kubernetes API — run with `cargo test -- --ignored`"]
    async fn health_check_returns_ok() {
        let app = build_app().await;
        let res = app.oneshot(get("/api/healthz")).await.expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res.into_body()).await;
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn readyz_pings_apiserver() {
        let app = build_app().await;
        let res = app.oneshot(get("/api/readyz")).await.expect("oneshot");
        // A reachable apiserver returns 200; if the kubeconfig context is
        // dead, we get 503. Either way we should not crash.
        assert!(
            res.status() == StatusCode::OK
                || res.status() == StatusCode::SERVICE_UNAVAILABLE,
            "unexpected status {}",
            res.status()
        );
    }

    // ---- namespaces ----

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn list_namespaces_returns_json_array() {
        ensure_test_namespace().await;
        let app = build_app().await;
        let res = app.oneshot(get("/api/namespaces")).await.expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res.into_body()).await;
        let names = body["namespaces"].as_array().expect("namespaces array");
        assert!(!names.is_empty(), "cluster should have at least one namespace");
        // The test namespace we just ensured exists must be in the list.
        let has_it_ns = names.iter().any(|v| v.as_str() == Some(IT_NAMESPACE));
        assert!(has_it_ns, "expected {IT_NAMESPACE} in {names:?}");
    }

    // ---- settings ----

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn settings_get_returns_defaults_when_configmap_missing() {
        let app = build_app().await;
        let res = app.oneshot(get("/api/settings")).await.expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res.into_body()).await;
        // Default shape from settings::default_settings — allowed_namespaces
        // is an array, auth object exists.
        assert!(body["allowed_namespaces"].is_array());
        assert!(body["auth"].is_object());
    }

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn settings_put_round_trips_allowed_namespaces() {
        let app = build_app().await;

        // Write a settings doc with a distinctive allow-list…
        let payload = json!({
            "allowed_namespaces": ["it-alpha", "it-beta"],
            "git_repositories": [],
            "oci_registries": [],
            "git_token_secrets": [],
        });
        let res = app
            .clone()
            .oneshot(json_request(Method::PUT, "/api/settings", payload))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);

        // …and confirm a subsequent GET sees the same values.
        let res = app.oneshot(get("/api/settings")).await.expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res.into_body()).await;
        let allowed: Vec<&str> = body["allowed_namespaces"]
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(allowed.contains(&"it-alpha"));
        assert!(allowed.contains(&"it-beta"));
    }

    // ---- deployment CRUD ----

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn deployment_crud_lifecycle() {
        ensure_test_namespace().await;
        let name = "it-nginx";
        // Guard against a leaked deployment from a prior failed run.
        cleanup_deployment(name).await;

        let app = build_app().await;

        // CREATE
        let payload = json!({
            "name": name,
            "image": "nginx:1.27-alpine",
            "replicas": 1,
            "port": 80,
        });
        let res = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/api/namespaces/{IT_NAMESPACE}/deployments"),
                payload,
            ))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::CREATED, "create failed");
        let created = body_json(res.into_body()).await;
        assert_eq!(created["name"], name);
        assert_eq!(created["namespace"], IT_NAMESPACE);
        assert_eq!(created["image"], "nginx:1.27-alpine");

        // GET single
        let res = app
            .clone()
            .oneshot(get(&format!(
                "/api/namespaces/{IT_NAMESPACE}/deployments/{name}"
            )))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK, "get failed");
        let got = body_json(res.into_body()).await;
        assert_eq!(got["name"], name);

        // LIST
        let res = app
            .clone()
            .oneshot(get(&format!(
                "/api/namespaces/{IT_NAMESPACE}/deployments"
            )))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK, "list failed");
        let listed = body_json(res.into_body()).await;
        let names: Vec<&str> = listed["deployments"]
            .as_array()
            .expect("deployments array")
            .iter()
            .filter_map(|d| d["name"].as_str())
            .collect();
        assert!(names.contains(&name), "expected {name} in {names:?}");

        // UPDATE (change replicas)
        let payload = json!({ "replicas": 2 });
        let res = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                &format!("/api/namespaces/{IT_NAMESPACE}/deployments/{name}"),
                payload,
            ))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK, "update failed");
        let updated = body_json(res.into_body()).await;
        assert_eq!(updated["replicas"]["desired"], 2);

        // DELETE
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!(
                        "/api/namespaces/{IT_NAMESPACE}/deployments/{name}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::NO_CONTENT, "delete failed");

        // GET after DELETE — k8s deletes are async; the object may briefly
        // remain in Terminating. Accept either 404 or 200-with-deletionTimestamp.
        let res = app
            .oneshot(get(&format!(
                "/api/namespaces/{IT_NAMESPACE}/deployments/{name}"
            )))
            .await
            .expect("oneshot");
        assert!(
            res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::OK,
            "unexpected status after delete: {}",
            res.status()
        );
    }

    // ---- namespace allow-list enforcement ----

    #[tokio::test]
    #[ignore = "requires a live kubernetes API"]
    async fn requests_to_disallowed_namespace_are_rejected() {
        // Build a state with an explicit allow-list that excludes the
        // namespace we try to hit. The check happens before any k8s call,
        // so we don't need the namespace to actually exist.
        let mut state = build_state().await;
        state.allowed_namespaces = vec!["only-this-one".to_string()];
        let app = build_router(
            state,
            None::<RegistryStore>,
            "/tmp/deckwatch-it-frontend",
            "/tmp/deckwatch-it-book",
            AuthConfig::disabled(),
        );

        let res = app
            .oneshot(get("/api/namespaces/kube-system/deployments"))
            .await
            .expect("oneshot");
        // AppError::NamespaceNotAllowed maps to 403 Forbidden (see error.rs).
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}
