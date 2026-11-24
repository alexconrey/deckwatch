//! Cross-site request protection.
//!
//! The dashboard is a same-origin SPA, so no other website should ever be able
//! to drive the API from a user's browser. Not sending CORS headers stops
//! other origins from *reading* responses, but two request types reach the
//! handlers anyway and still cause side effects:
//!
//! * **"Simple" cross-site writes.** A form post or `fetch(url, {method:
//!   "POST", mode: "no-cors"})` is sent without a preflight. The response is
//!   opaque to the attacker, but restarts, deletes, scaling and so on still
//!   happen. Deckwatch's API is reachable without credentials while auth is
//!   disabled, so the victim does not need to be logged in. Their network
//!   position, e.g. on the VPN in front of an internal ALB, is enough.
//! * **WebSocket upgrades.** These are not subject to CORS at all, so any page
//!   could open the pod exec / port-forward sockets (cross-site WebSocket
//!   hijacking).
//!
//! [`reject_cross_site`] refuses both when the browser reports a cross-site
//! initiator. Browsers attach `Sec-Fetch-Site` (all evergreen browsers) and/or
//! `Origin` to these requests. Non-browser clients send neither and are
//! unaffected: MCP clients, curl/scripts, the Kubernetes API server calling the
//! admission webhook, Git providers calling the webhook receiver, and
//! docker/kaniko against `/v2`. Validating `Origin` on the MCP endpoint is
//! also required by the MCP Streamable HTTP transport spec.

use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Axum middleware: reject cross-site state-changing requests and WebSocket
/// upgrades with `403 Forbidden`. Same-origin and non-browser requests pass
/// through untouched.
pub async fn reject_cross_site(req: Request, next: Next) -> Response {
    if needs_origin_check(req.method(), req.headers()) && is_cross_site(req.headers()) {
        tracing::warn!(
            method = %req.method(),
            path = %req.uri().path(),
            origin = ?req.headers().get(header::ORIGIN),
            sec_fetch_site = ?req.headers().get("sec-fetch-site"),
            "blocked cross-site request"
        );
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "forbidden",
                "message": "cross-site request blocked",
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// Safe methods are exempt (the same-origin policy already keeps their
/// responses away from other origins), except WebSocket upgrades, which ride
/// on a `GET` but bypass CORS entirely.
fn needs_origin_check(method: &Method, headers: &HeaderMap) -> bool {
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    !safe || is_websocket_upgrade(headers)
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket"))
}

/// True when the browser tells us the request was initiated by another site.
fn is_cross_site(headers: &HeaderMap) -> bool {
    // Fetch metadata is set by the browser, can't be forged by page script,
    // and describes the initiator relative to the URL the browser requested.
    // It therefore stays correct behind proxies that rewrite `Host`, such as
    // the Vite dev server's `changeOrigin` proxy. `none` means user-initiated
    // (typed URL / bookmark). `same-site` (a sibling subdomain) is still a
    // different origin and is rejected.
    if let Some(site) = headers.get("sec-fetch-site") {
        return !matches!(site.to_str(), Ok("same-origin" | "none"));
    }
    // Older browsers without fetch metadata: compare Origin with Host.
    match headers.get(header::ORIGIN).map(|v| v.to_str()) {
        // No Origin at all: not a browser-initiated cross-site request.
        None => false,
        Some(Ok(origin)) => !origin_matches_host(origin, headers),
        Some(Err(_)) => true,
    }
}

/// Compare the `Origin` authority with the request's `Host`, also accepting
/// `X-Forwarded-Host` when a proxy rewrote `Host`. A page can't set
/// `X-Forwarded-Host` on a cross-origin request without a CORS preflight,
/// which we never approve, so trusting it here doesn't let an attacker
/// through. An opaque origin (`null`, from sandboxed iframes or `data:` URLs)
/// never matches.
fn origin_matches_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(origin_authority) = origin
        .parse::<Uri>()
        .ok()
        .and_then(|u| u.authority().map(|a| normalize_authority(a.as_str())))
    else {
        return false;
    };
    [header::HOST.as_str(), "x-forwarded-host"]
        .iter()
        .filter_map(|name| headers.get(*name).and_then(|v| v.to_str().ok()))
        // X-Forwarded-Host may be a comma-separated chain; the first entry is
        // the host the client originally asked for.
        .filter_map(|v| v.split(',').next())
        .any(|host| normalize_authority(host) == origin_authority)
}

/// Lowercase and drop default ports so `Host: example.com` matches
/// `Origin: https://example.com:443`.
fn normalize_authority(authority: &str) -> String {
    let a = authority.trim().to_ascii_lowercase();
    a.strip_suffix(":443")
        .or_else(|| a.strip_suffix(":80"))
        .map(str::to_string)
        .unwrap_or(a)
}

#[cfg(test)]
#[path = "cross_origin_tests.rs"]
mod cross_origin_tests;
