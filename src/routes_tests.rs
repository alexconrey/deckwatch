// Unit tests for src/routes.rs

use super::*;
use axum::body::Body;
use axum::http::{Method, Request};
use tower::ServiceExt;

// `oci_router` is the `/v2` router exactly as `build_router` mounts it; the
// rest of the app needs a kube client.

async fn fs_store(root: &std::path::Path) -> RegistryStore {
    let store = RegistryStore::filesystem(root);
    if let RegistryStore::Filesystem(fs) = &store {
        fs.ensure_dirs().await.expect("ensure_dirs");
    }
    store
}

async fn send(store: &RegistryStore, method: Method, uri: &str, body: &str) -> StatusCode {
    oci_router(store.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("oneshot")
        .status()
}

/// Regression: repository names containing `..` (raw or percent-encoded,
/// which axum decodes before the handler sees it) used to pass
/// `validate_name` and escape `<root>/manifests/`, giving unauthenticated
/// arbitrary file write and delete (e.g. over the SQLite DB that sits next to
/// the registry root in the Helm layout).
#[tokio::test]
async fn oci_rejects_path_traversal_in_repository_name_and_reference() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("registry");
    let store = fs_store(&root).await;

    // A file outside the registry root standing in for /data/deckwatch.db.
    let victim = tmp.path().join("deckwatch.db");
    std::fs::write(&victim, b"original").expect("write victim");

    let traversal_uris = [
        // manifests/../.. == registry root's parent, so the reference lands next to it.
        "/v2/%2e%2e/%2e%2e/manifests/deckwatch.db",
        "/v2/../../manifests/deckwatch.db",
        "/v2/ok/%2e%2e/%2e%2e/%2e%2e/manifests/deckwatch.db",
        "/v2/%2E%2E/%2E%2E/manifests/deckwatch.db",
        "/v2/./%2e%2e/%2e%2e/manifests/deckwatch.db",
        // `..` as the reference.
        "/v2/library/app/manifests/%2e%2e",
        "/v2/library/app/manifests/..",
    ];

    for uri in traversal_uris {
        let put = send(&store, Method::PUT, uri, "{\"schemaVersion\":2}").await;
        assert!(put.is_client_error(), "PUT {uri} returned {put}");
        let del = send(&store, Method::DELETE, uri, "").await;
        assert!(del.is_client_error(), "DELETE {uri} returned {del}");
    }

    assert_eq!(
        std::fs::read(&victim).expect("victim still readable"),
        b"original",
        "file outside the registry root was modified"
    );
    let escaped: Vec<_> = std::fs::read_dir(tmp.path())
        .expect("read tmp")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        {
            let mut names = escaped.clone();
            names.sort();
            names
        },
        vec!["deckwatch.db".to_string(), "registry".to_string()],
        "unexpected files created outside the registry root: {escaped:?}"
    );
}

/// The tightened validation must not break normal pushes and pulls.
#[tokio::test]
async fn oci_put_and_get_manifest_for_valid_name_still_works() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = fs_store(&tmp.path().join("registry")).await;

    let manifest = r#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{},"layers":[]}"#;
    for uri in [
        "/v2/team-a/my_app.web/manifests/abc123-amd64",
        "/v2/library/app/manifests/latest",
    ] {
        assert_eq!(
            send(&store, Method::PUT, uri, manifest).await,
            StatusCode::CREATED,
            "PUT {uri}"
        );
        assert_eq!(
            send(&store, Method::GET, uri, "").await,
            StatusCode::OK,
            "GET {uri}"
        );
    }
}

/// The whole app as `run()` serves it, with a test database, a registry in
/// `dir` and a frontend holding only `index.html`.
async fn app(dir: &std::path::Path) -> Router {
    let state = crate::plugin_fixture::test_state(dir, vec![], None).await;
    let store = fs_store(&dir.join("registry")).await;
    let frontend = dir.join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(
        frontend.join("index.html"),
        "<!doctype html><title>deckwatch</title>",
    )
    .unwrap();
    build_router(
        state,
        Some(store),
        frontend.to_str().unwrap(),
        dir.join("no-book").to_str().unwrap(),
        AuthConfig::from_settings(None),
    )
}

async fn call(app: &Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("oneshot")
}

fn header_values(response: &axum::response::Response, name: &str) -> Vec<String> {
    response
        .headers()
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

fn assert_security_headers(response: &axum::response::Response, what: &str) {
    assert_eq!(
        header_values(response, "x-content-type-options"),
        ["nosniff"],
        "{what}"
    );
    assert_eq!(
        header_values(response, "x-frame-options"),
        ["SAMEORIGIN"],
        "{what}"
    );
    assert_eq!(
        header_values(response, "referrer-policy"),
        ["same-origin"],
        "{what}"
    );
    assert!(
        header_values(response, "content-security-policy")
            .contains(&"frame-ancestors 'self'".to_string()),
        "{what}"
    );
}

#[tokio::test]
async fn every_kind_of_response_carries_the_security_headers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = app(tmp.path()).await;

    let api = call(
        &app,
        Request::get("/api/healthz").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(api.status(), StatusCode::OK);
    assert_security_headers(&api, "API");

    let asset = call(
        &app,
        Request::get("/index.html").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_security_headers(&asset, "static file");

    // A client-side route: the SPA's index.html, from the fallback.
    let spa = call(
        &app,
        Request::get("/deployments/web")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_security_headers(&spa, "SPA fallback");

    // Refused by the cross-site guard before any handler runs.
    let refused = call(
        &app,
        Request::post("/api/namespaces")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_security_headers(&refused, "cross-site refusal");

    let v2 = call(&app, Request::get("/v2/").body(Body::empty()).unwrap()).await;
    assert_eq!(v2.status(), StatusCode::OK);
    assert_security_headers(&v2, "registry");
}

/// A pushed manifest names its own media type; only manifest types may come
/// back as `Content-Type`, so a manifest can't be served as a web page.
#[tokio::test]
async fn manifests_are_served_only_as_manifest_media_types() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = app(tmp.path()).await;

    let html = r#"{"schemaVersion":2,"mediaType":"text/html","config":{},"layers":[],"annotations":{"x":"<b>not a page</b>"}}"#;
    let oci = r#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{},"layers":[]}"#;
    let docker = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{},"layers":[]}"#;
    for (tag, body, served) in [
        ("html", html, "application/octet-stream"),
        ("oci", oci, "application/vnd.oci.image.manifest.v1+json"),
        (
            "docker",
            docker,
            "application/vnd.docker.distribution.manifest.v2+json",
        ),
    ] {
        let uri = format!("/v2/team/web/manifests/{tag}");
        let put = call(
            &app,
            Request::put(uri.as_str()).body(Body::from(body)).unwrap(),
        )
        .await;
        assert_eq!(put.status(), StatusCode::CREATED, "PUT {tag}");

        for method in [Method::GET, Method::HEAD] {
            let response = call(
                &app,
                Request::builder()
                    .method(method.clone())
                    .uri(uri.as_str())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{method} {tag}");
            assert_eq!(
                header_values(&response, "content-type"),
                [served],
                "{method} {tag}"
            );
            assert_security_headers(&response, tag);
            assert!(
                header_values(&response, "content-security-policy")
                    .contains(&crate::security_headers::REGISTRY_POLICY.to_string()),
                "{method} {tag}"
            );
        }
    }
}
