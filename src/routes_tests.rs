// Unit tests for src/routes.rs

use super::*;
use axum::body::Body;
use axum::http::{Method, Request};
use tower::ServiceExt;

/// The `/v2/{*rest}` router exactly as `build_router` mounts it, minus the
/// rest of the app (which needs a kube client).
fn oci_router(store: RegistryStore) -> Router {
    Router::new()
        .route(
            "/v2/{*rest}",
            get(oci_dispatch_get)
                .head(oci_dispatch_head)
                .post(oci_dispatch_post)
                .patch(oci_dispatch_patch)
                .put(oci_dispatch_put)
                .delete(oci_dispatch_delete),
        )
        .with_state(store)
}

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
