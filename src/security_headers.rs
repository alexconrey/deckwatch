//! Security headers on every response, and a stricter policy for the
//! embedded registry.
//!
//! [`apply`] wraps the whole router, so API responses, the SPA and its
//! assets, the docs, `/v2`, proxied pod responses and the middleware's own
//! errors all get:
//!
//! - `X-Content-Type-Options: nosniff`: browsers go by the declared
//!   `Content-Type` and never guess, so a response that doesn't declare HTML
//!   or script isn't run as either.
//! - `X-Frame-Options: SAMEORIGIN` and `Content-Security-Policy:
//!   frame-ancestors 'self'`: no other site can frame deckwatch, so it can't
//!   be overlaid with a decoy to trick clicks. The UI frames nothing itself;
//!   same-origin framing stays allowed for pages opened through the pod
//!   proxy that frame their own pages.
//! - `Referrer-Policy: same-origin`: links to other sites don't carry
//!   deckwatch URLs, which name namespaces and workloads.
//!
//! The first three replace whatever a handler or a proxied response set. The
//! CSP is appended instead, so a response's own policy still applies and the
//! browser enforces both.
//!
//! Not here: a CSP that restricts scripts and styles. The SPA and the
//! Swagger page (which loads its bundle from a CDN) need one designed and
//! tested for them.

use axum::extract::Request;
use axum::http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// Middleware for the whole router: adds the headers to every response.
pub async fn apply(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    set(response.headers_mut());
    response
}

/// Add the headers described in the module docs to `headers`.
pub fn set(headers: &mut HeaderMap) {
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("same-origin"));
    headers.append(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
}

/// The registry's own policy, for every `/v2` response: no scripts, no
/// subresources, a unique origin. Its clients (docker, kaniko, the kubelet)
/// ignore CSP; a browser that opens a registry URL gets an inert document.
pub const REGISTRY_POLICY: &str = "sandbox; default-src 'none'";

/// Middleware for the `/v2` router: adds [`REGISTRY_POLICY`].
pub async fn registry(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response.headers_mut().append(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(REGISTRY_POLICY),
    );
    response
}

#[cfg(test)]
#[path = "security_headers_tests.rs"]
mod tests;
