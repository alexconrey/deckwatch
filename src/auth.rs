//! Microsoft Entra (Azure AD) OIDC authentication plumbing.
//!
//! This module is **opt-in** — the [`require_auth`] middleware is defined but
//! not applied to any routes by default. To activate, layer it onto the API
//! router in `routes.rs`:
//!
//! ```ignore
//! use axum::middleware;
//! use crate::auth;
//!
//! let auth_config = auth::AuthConfig::from_settings(&settings);
//! let api = api.layer(middleware::from_fn_with_state(
//!     auth_config,
//!     auth::require_auth,
//! ));
//! ```
//!
//! When `AuthConfig.enabled` is false the middleware is a no-op — this lets
//! settings toggle auth without a restart.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::handlers::settings::AuthSettings;

/// Runtime auth configuration, cloned into the middleware layer state.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub enabled: bool,
    pub tenant_id: String,
    pub client_id: String,
    pub issuer_url: String,
    /// Shared JWKS cache — refreshed on demand when a `kid` is not found.
    pub jwks: Arc<RwLock<JwksCache>>,
}

impl AuthConfig {
    /// Build an AuthConfig from the persisted [`AuthSettings`]. Returns a
    /// disabled config when settings are missing.
    pub fn from_settings(settings: Option<&AuthSettings>) -> Self {
        match settings {
            Some(s) if s.enabled && !s.tenant_id.is_empty() && !s.client_id.is_empty() => Self {
                enabled: true,
                tenant_id: s.tenant_id.clone(),
                client_id: s.client_id.clone(),
                issuer_url: format!("https://login.microsoftonline.com/{}/v2.0", s.tenant_id),
                jwks: Arc::new(RwLock::new(JwksCache::default())),
            },
            _ => Self::disabled(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            tenant_id: String::new(),
            client_id: String::new(),
            issuer_url: String::new(),
            jwks: Arc::new(RwLock::new(JwksCache::default())),
        }
    }

    fn jwks_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/discovery/v2.0/keys",
            self.tenant_id
        )
    }
}

/// JWKS response from the Entra discovery endpoint.
#[derive(Debug, Clone, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kid: String,
    #[serde(default)]
    kty: String,
    #[serde(default)]
    n: String,
    #[serde(default)]
    e: String,
}

/// In-memory JWKS cache. Entra rotates signing keys periodically, so entries
/// are invalidated after `TTL` and on unknown-kid misses.
#[derive(Debug, Default)]
pub struct JwksCache {
    keys: HashMap<String, Jwk>,
    fetched_at: Option<Instant>,
}

impl JwksCache {
    const TTL: Duration = Duration::from_secs(60 * 60); // 1h

    fn is_fresh(&self) -> bool {
        self.fetched_at
            .map(|t| t.elapsed() < Self::TTL)
            .unwrap_or(false)
    }
}

/// Claims we care about from the Entra ID token. Only `aud`, `iss`, and `exp`
/// are validated by `jsonwebtoken`; the rest are captured for downstream use.
#[derive(Debug, Clone, Deserialize)]
pub struct EntraClaims {
    pub aud: String,
    pub iss: String,
    pub exp: usize,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "preferred_username")]
    pub email: Option<String>,
    #[serde(default)]
    pub oid: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Authenticated user injected into request extensions. Downstream handlers
/// can extract via `Extension<AuthUser>`.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub subject: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
}

impl From<EntraClaims> for AuthUser {
    fn from(c: EntraClaims) -> Self {
        Self {
            subject: c.oid.unwrap_or_default(),
            name: c.name,
            email: c.email,
            groups: c.groups,
            roles: c.roles,
        }
    }
}

/// Axum middleware that validates the incoming JWT against Entra's JWKS.
///
/// When `AuthConfig.enabled` is false, the middleware passes through without
/// touching the request. This is the safe default while the auth flow is
/// still being wired up on the frontend.
pub async fn require_auth(
    State(config): State<AuthConfig>,
    mut req: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(req).await;
    }

    let token = match extract_bearer(&req) {
        Some(t) => t,
        None => return unauthorized("missing bearer token"),
    };

    match validate_token(&config, &token).await {
        Ok(claims) => {
            req.extensions_mut().insert(AuthUser::from(claims));
            next.run(req).await
        }
        Err(msg) => unauthorized(&msg),
    }
}

fn extract_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
}

fn unauthorized(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "unauthorized",
            "message": msg,
        })),
    )
        .into_response()
}

async fn validate_token(config: &AuthConfig, token: &str) -> Result<EntraClaims, String> {
    let header = decode_header(token).map_err(|e| format!("invalid jwt header: {e}"))?;
    let kid = header.kid.ok_or_else(|| "jwt missing kid".to_string())?;

    let jwk = get_or_refresh_key(config, &kid).await?;
    let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| format!("invalid jwk: {e}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[&config.client_id]);
    validation.set_issuer(&[&config.issuer_url]);

    let data = decode::<EntraClaims>(token, &key, &validation)
        .map_err(|e| format!("jwt validation failed: {e}"))?;
    Ok(data.claims)
}

async fn get_or_refresh_key(config: &AuthConfig, kid: &str) -> Result<Jwk, String> {
    {
        let cache = config.jwks.read().await;
        if cache.is_fresh() {
            if let Some(k) = cache.keys.get(kid) {
                return Ok(k.clone());
            }
        }
    }

    let mut cache = config.jwks.write().await;
    // Double-checked: another task may have refreshed while we waited.
    if cache.is_fresh() {
        if let Some(k) = cache.keys.get(kid) {
            return Ok(k.clone());
        }
    }

    let jwks: Jwks = reqwest::get(config.jwks_url())
        .await
        .map_err(|e| format!("jwks fetch failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("jwks fetch non-2xx: {e}"))?
        .json()
        .await
        .map_err(|e| format!("jwks parse failed: {e}"))?;

    cache.keys = jwks.keys.into_iter().map(|k| (k.kid.clone(), k)).collect();
    cache.fetched_at = Some(Instant::now());

    cache
        .keys
        .get(kid)
        .cloned()
        .ok_or_else(|| format!("no jwk found for kid {kid}"))
}
