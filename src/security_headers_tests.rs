use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt as _;

use super::*;

fn values(response: &Response, name: impl axum::http::header::AsHeaderName) -> Vec<String> {
    response
        .headers()
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

async fn get_path(router: Router, path: &str) -> Response {
    router
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn every_response_gets_the_headers() {
    let router = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(apply));
    for path in ["/ok", "/missing"] {
        let response = get_path(router.clone(), path).await;
        assert_eq!(
            values(&response, X_CONTENT_TYPE_OPTIONS),
            ["nosniff"],
            "{path}"
        );
        assert_eq!(values(&response, X_FRAME_OPTIONS), ["SAMEORIGIN"], "{path}");
        assert_eq!(
            values(&response, REFERRER_POLICY),
            ["same-origin"],
            "{path}"
        );
        assert_eq!(
            values(&response, CONTENT_SECURITY_POLICY),
            ["frame-ancestors 'self'"],
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_response_cannot_weaken_them_but_keeps_its_own_csp() {
    // What a proxied pod response (or a careless handler) might send.
    let router = Router::new()
        .route(
            "/proxied",
            get(|| async {
                (
                    [
                        (X_CONTENT_TYPE_OPTIONS, "sniff"),
                        (X_FRAME_OPTIONS, "ALLOWALL"),
                        (REFERRER_POLICY, "unsafe-url"),
                        (CONTENT_SECURITY_POLICY, "frame-ancestors *"),
                    ],
                    "<html></html>",
                )
            }),
        )
        .layer(axum::middleware::from_fn(apply));
    let response = get_path(router, "/proxied").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(values(&response, X_CONTENT_TYPE_OPTIONS), ["nosniff"]);
    assert_eq!(values(&response, X_FRAME_OPTIONS), ["SAMEORIGIN"]);
    assert_eq!(values(&response, REFERRER_POLICY), ["same-origin"]);
    // Both policies apply: the browser allows framing only where both do.
    assert_eq!(
        values(&response, CONTENT_SECURITY_POLICY),
        ["frame-ancestors *", "frame-ancestors 'self'"]
    );
}

#[tokio::test]
async fn registry_responses_get_the_registry_policy() {
    let router = Router::new()
        .route("/v2/", get(|| async { "{}" }))
        .layer(axum::middleware::from_fn(registry));
    let response = get_path(router, "/v2/").await;
    assert_eq!(
        values(&response, CONTENT_SECURITY_POLICY),
        [REGISTRY_POLICY]
    );
}
