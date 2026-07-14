//! Axum middleware that gates Pro/Enterprise routes on an [`Entitlements`]
//! check. Sits alongside `src/license.rs`; kept in its own file so the pure
//! entitlements logic can be unit-tested without pulling in Axum.
//!
//! ## Usage
//!
//! In `routes.rs`, wrap a route (or a whole sub-router) with the layer
//! returned by [`require_entitlement`]:
//!
//! ```ignore
//! use crate::license_middleware::require_entitlement;
//!
//! .route("/api/namespaces/{ns}/diagnostics",
//!     post(diagnostics::create_diagnostic)
//!         .layer(require_entitlement("ai_diagnostics", state.entitlements.clone())))
//! ```
//!
//! Community routes are never wrapped. The middleware short-circuits with a
//! 403 JSON body when the feature is not granted; on grant it passes the
//! request through untouched.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::license::{Entitlements, Tier};

/// State carried into [`entitlement_middleware`]. Cloned per request; the
/// inner `Entitlements` is an `Arc` so this is cheap.
#[derive(Clone)]
pub struct EntitlementGuard {
    pub feature: &'static str,
    pub entitlements: Entitlements,
}

/// Build a middleware layer that enforces `feature`. Attach to any route or
/// sub-router that is Pro or Enterprise. Community routes MUST NOT be layered
/// with this — the pledge in `docs/LICENSING_STRATEGY.md` §1 requires that
/// cluster-control endpoints never gate on tier.
///
/// The return type is `impl Layer` so callers can `.layer(...)` it directly
/// on an axum `Router` or `MethodRouter` without naming the concrete type.
pub fn require_entitlement(
    feature: &'static str,
    entitlements: Entitlements,
) -> axum::middleware::FromFnLayer<
    fn(
        State<EntitlementGuard>,
        Request,
        Next,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>,
    EntitlementGuard,
    (),
> {
    let guard = EntitlementGuard {
        feature,
        entitlements,
    };
    from_fn_with_state(guard, entitlement_middleware)
}

/// Middleware body. Returns a boxed future so its type is nameable in the
/// `from_fn_with_state` return signature above — an `async fn` produces an
/// anonymous future that couldn't be used as a fn-pointer type parameter.
fn entitlement_middleware(
    State(guard): State<EntitlementGuard>,
    req: Request,
    next: Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> {
    Box::pin(async move {
        if guard.entitlements.has(guard.feature) {
            return next.run(req).await;
        }
        license_required_response(guard.feature, guard.entitlements.tier())
    })
}

/// Response body shape:
///
/// ```json
/// {
///   "error": "license_required",
///   "feature": "ai_diagnostics",
///   "tier_required": "pro",
///   "current_tier": "community",
///   "upgrade_url": "https://deckwatch.io/pricing?feature=ai_diagnostics&tier=pro"
/// }
/// ```
///
/// Frontend keys on `error: "license_required"` to render the upgrade overlay.
fn license_required_response(feature: &str, current_tier: Tier) -> Response {
    let required = Entitlements::required_tier(feature);
    let body = serde_json::json!({
        "error": "license_required",
        "feature": feature,
        "tier_required": required.as_str(),
        "current_tier": current_tier.as_str(),
        "upgrade_url": format!(
            "https://deckwatch.io/pricing?feature={feature}&tier={}",
            required.as_str()
        ),
    });
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{License, LicensePayload, Limits};
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use axum::Router;
    use jiff::Timestamp;
    use tower::ServiceExt;

    async fn ok() -> &'static str {
        "ok"
    }

    fn pro_entitlements() -> Entitlements {
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        Entitlements::from_license(&license, Timestamp::from_second(1_800_000_000).unwrap())
    }

    #[tokio::test]
    async fn allows_when_feature_granted() {
        let app = Router::new().route(
            "/ai",
            get(ok).layer(require_entitlement("ai_diagnostics", pro_entitlements())),
        );
        let resp = app
            .oneshot(HttpRequest::get("/ai").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn returns_403_when_feature_denied() {
        let app = Router::new().route(
            "/ai",
            get(ok).layer(require_entitlement(
                "ai_diagnostics",
                Entitlements::community(),
            )),
        );
        let resp = app
            .oneshot(HttpRequest::get("/ai").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"], "license_required");
        assert_eq!(body["feature"], "ai_diagnostics");
        assert_eq!(body["tier_required"], "pro");
        assert_eq!(body["current_tier"], "community");
        assert!(body["upgrade_url"]
            .as_str()
            .unwrap()
            .contains("ai_diagnostics"));
    }

    #[tokio::test]
    async fn enterprise_feature_advertises_enterprise_tier() {
        let app = Router::new().route(
            "/mc",
            get(ok).layer(require_entitlement("multi_cluster", pro_entitlements())),
        );
        let resp = app
            .oneshot(HttpRequest::get("/mc").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["tier_required"], "enterprise");
        assert_eq!(body["current_tier"], "pro");
    }
}
