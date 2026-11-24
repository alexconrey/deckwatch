// Unit tests for src/cross_origin.rs

use super::*;
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue};
use axum::routing::{get, post};
use axum::Router;
use tower::ServiceExt;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            HeaderName::from_bytes(k.as_bytes()).expect("header name"),
            HeaderValue::from_str(v).expect("header value"),
        );
    }
    h
}

const EVIL: &[(&str, &str)] = &[
    ("host", "deckwatch.example"),
    ("origin", "https://evil.example"),
    ("sec-fetch-site", "cross-site"),
];
const SAME: &[(&str, &str)] = &[
    ("host", "deckwatch.example"),
    ("origin", "https://deckwatch.example"),
    ("sec-fetch-site", "same-origin"),
];

// ── header classification ───────────────────────────────────────────────────

#[test]
fn fetch_metadata_decides_when_present() {
    assert!(!is_cross_site(&headers(&[(
        "sec-fetch-site",
        "same-origin"
    )])));
    assert!(!is_cross_site(&headers(&[("sec-fetch-site", "none")])));
    assert!(is_cross_site(&headers(&[("sec-fetch-site", "cross-site")])));
    assert!(is_cross_site(&headers(&[("sec-fetch-site", "same-site")])));
    assert!(is_cross_site(&headers(&[("sec-fetch-site", "bogus")])));
}

#[test]
fn fetch_metadata_wins_over_origin_host_comparison() {
    // Vite dev proxy (`changeOrigin: true`) rewrites Host, so Origin and Host
    // disagree even though the browser saw a same-origin request.
    assert!(!is_cross_site(&headers(&[
        ("sec-fetch-site", "same-origin"),
        ("origin", "http://localhost:3000"),
        ("host", "localhost:8080"),
    ])));
    assert!(is_cross_site(&headers(&[
        ("sec-fetch-site", "cross-site"),
        ("origin", "https://deckwatch.example"),
        ("host", "deckwatch.example"),
    ])));
}

#[test]
fn requests_without_browser_headers_are_not_cross_site() {
    // MCP clients, curl, the K8s API server, Git webhooks, docker/kaniko.
    assert!(!is_cross_site(&HeaderMap::new()));
    assert!(!is_cross_site(&headers(&[("host", "deckwatch.example")])));
}

#[test]
fn origin_fallback_accepts_same_host() {
    for (origin, host) in [
        ("https://deckwatch.example", "deckwatch.example"),
        ("http://localhost:8080", "localhost:8080"),
        ("https://Deckwatch.Example:443", "deckwatch.example"),
        ("http://deckwatch.example", "deckwatch.example:80"),
    ] {
        assert!(
            !is_cross_site(&headers(&[("origin", origin), ("host", host)])),
            "{origin} vs {host}"
        );
    }
    // A proxy that rewrote Host but set X-Forwarded-Host.
    assert!(!is_cross_site(&headers(&[
        ("origin", "https://deckwatch.example"),
        ("host", "10.0.0.5:8080"),
        ("x-forwarded-host", "deckwatch.example, internal-lb"),
    ])));
}

#[test]
fn origin_fallback_rejects_other_hosts() {
    for (origin, host) in [
        ("https://evil.example", "deckwatch.example"),
        ("http://localhost:8081", "localhost:8080"),
        ("https://deckwatch.example.evil.com", "deckwatch.example"),
        ("https://evil.deckwatch.example", "deckwatch.example"),
        ("null", "deckwatch.example"),
        ("not a uri", "deckwatch.example"),
    ] {
        assert!(
            is_cross_site(&headers(&[("origin", origin), ("host", host)])),
            "{origin} vs {host}"
        );
    }
    assert!(is_cross_site(&headers(&[(
        "origin",
        "https://evil.example"
    )])));
}

#[test]
fn only_unsafe_methods_and_websocket_upgrades_are_checked() {
    let none = HeaderMap::new();
    let ws = headers(&[("upgrade", "websocket"), ("connection", "Upgrade")]);
    assert!(!needs_origin_check(&Method::GET, &none));
    assert!(!needs_origin_check(&Method::HEAD, &none));
    assert!(!needs_origin_check(&Method::OPTIONS, &none));
    assert!(needs_origin_check(&Method::GET, &ws));
    assert!(needs_origin_check(
        &Method::GET,
        &headers(&[("upgrade", "WebSocket")])
    ));
    for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        assert!(needs_origin_check(&m, &none), "{m}");
    }
}

// ── middleware behaviour ────────────────────────────────────────────────────

fn app() -> Router {
    Router::new()
        .route(
            "/api/thing",
            get(|| async { "read" })
                .post(|| async { "wrote" })
                .delete(|| async { "deleted" }),
        )
        .route("/mcp", post(|| async { "mcp" }))
        .layer(axum::middleware::from_fn(reject_cross_site))
}

async fn status(method: Method, uri: &str, hdrs: &[(&str, &str)]) -> StatusCode {
    let mut req = axum::http::Request::builder().method(method).uri(uri);
    for (k, v) in hdrs {
        req = req.header(*k, *v);
    }
    app()
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("oneshot")
        .status()
}

#[tokio::test]
async fn blocks_cross_site_writes() {
    assert_eq!(
        status(Method::POST, "/api/thing", EVIL).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(Method::DELETE, "/api/thing", EVIL).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(Method::POST, "/mcp", EVIL).await,
        StatusCode::FORBIDDEN
    );
    // Older browser without fetch metadata, form post from another site.
    assert_eq!(
        status(
            Method::POST,
            "/api/thing",
            &[
                ("host", "deckwatch.example"),
                ("origin", "https://evil.example")
            ]
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn allows_cross_site_reads() {
    // Same-origin policy keeps the body from the other origin; navigations and
    // links into the dashboard must keep working.
    assert_eq!(
        status(Method::GET, "/api/thing", EVIL).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn allows_same_origin_and_non_browser_writes() {
    assert_eq!(
        status(Method::POST, "/api/thing", SAME).await,
        StatusCode::OK
    );
    assert_eq!(
        status(Method::DELETE, "/api/thing", SAME).await,
        StatusCode::OK
    );
    assert_eq!(status(Method::POST, "/mcp", &[]).await, StatusCode::OK);
    assert_eq!(
        status(Method::POST, "/api/thing", &[("host", "deckwatch.example")]).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn blocks_cross_site_websocket_upgrade() {
    let upgrade = [("upgrade", "websocket"), ("connection", "Upgrade")];
    let evil_ws: Vec<_> = EVIL.iter().chain(upgrade.iter()).copied().collect();
    let same_ws: Vec<_> = SAME.iter().chain(upgrade.iter()).copied().collect();
    let legacy_evil_ws: Vec<_> = [
        ("host", "deckwatch.example"),
        ("origin", "https://evil.example"),
    ]
    .iter()
    .chain(upgrade.iter())
    .copied()
    .collect();

    assert_eq!(
        status(Method::GET, "/api/thing", &evil_ws).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(Method::GET, "/api/thing", &legacy_evil_ws).await,
        StatusCode::FORBIDDEN
    );
    // Same-origin upgrade reaches the handler.
    assert_eq!(
        status(Method::GET, "/api/thing", &same_ws).await,
        StatusCode::OK
    );
}
