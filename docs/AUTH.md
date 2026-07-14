# Microsoft Entra Authentication

Deckwatch ships with **foundation-only** OIDC plumbing for Microsoft Entra
(formerly Azure AD). The Rust middleware, JWKS cache, and frontend composable
are all in place, but the `require_auth` middleware is **not applied to any
routes by default**. This document explains how to activate it.

## Why plumbing-only?

- Zero-risk of accidentally locking operators out during install.
- Lets teams pre-configure tenant/client IDs via the Settings UI and stage
  the change before enforcement.
- Allows the auth code path to be exercised in staging (`enabled=false`
  behaves as a pass-through no-op) before flipping on in prod.

## Entra app registration

1. **Azure Portal → Microsoft Entra ID → App registrations → New registration**
   - Name: `Deckwatch <cluster-name>`
   - Supported account types: *Accounts in this organizational directory only*
   - Redirect URI: **Single-page application (SPA)** →
     `https://deckwatch.example.com/auth/callback`

2. **Authentication** blade
   - Enable *ID tokens* under Implicit / hybrid flows.
   - Add extra redirect URIs for each environment (localhost, staging, prod).

3. **Token configuration** blade
   - Add optional claim `groups` (or `roles` if using app-role-based RBAC)
     for the ID token. Deckwatch reads either.

4. **API permissions** blade
   - Delegated `openid`, `profile`, `email`, `User.Read`.
   - Grant admin consent.

5. Copy **Tenant ID** and **Application (client) ID** into the Settings page
   in deckwatch. Leave `enabled` off until you're ready to enforce.

## Flow diagram

```
Browser                        Deckwatch backend           Entra ID
   |                                  |                       |
   |-- GET / (no token) ------------->|                       |
   |<-- 401 unauthorized -------------|                       |
   |                                                          |
   |-- login() →→→→→→→→→→→→→→→→→→→→ /oauth2/v2.0/authorize ->|
   |<---------------------- redirect w/ code + state ---------|
   |                                                          |
   |-- POST /api/auth/callback ------>|                       |
   |                                  |-- exchange code ----->|
   |                                  |<-- id_token, access ---|
   |<-- 200 { token, user } ----------|                       |
   |                                                          |
   |-- GET /api/deployments (bearer)->|                       |
   |                                  |-- (first request)     |
   |                                  |-- GET JWKS ---------->|
   |                                  |<-- keys --------------|
   |                                  |-- validate signature  |
   |                                  |-- validate iss, aud, exp
   |                                  |-- extract claims      |
   |<-- 200 [deployments] ------------|                       |
```

## JWT validation

The `require_auth` middleware in `src/auth.rs` performs these checks:

| Check                | How                                                                 |
|----------------------|---------------------------------------------------------------------|
| Bearer token present | `Authorization: Bearer <jwt>` header                                |
| Signature            | RS256 against Entra JWKS at `/discovery/v2.0/keys`                  |
| Issuer               | `https://login.microsoftonline.com/{tenant_id}/v2.0`                |
| Audience             | Matches the configured `client_id`                                  |
| Expiry               | `exp` claim (enforced by `jsonwebtoken` `Validation`)               |

The JWKS cache lives on `AuthConfig.jwks` (Arc<RwLock<...>>) with a 1h TTL
and on-demand refresh when an unknown `kid` is encountered. Refresh writes
take the write lock with a double-check to avoid stampedes.

## Activation

Uncomment / add to `src/routes.rs`:

```rust
use axum::middleware;
use crate::auth::{self, AuthConfig};

// Build once at startup after loading settings. In practice, wire this
// through AppState so the config can be swapped when settings change
// without a restart.
let auth_config = AuthConfig::from_settings(state.current_auth_settings());

let api = api.layer(middleware::from_fn_with_state(
    auth_config,
    auth::require_auth,
));
```

Then set `auth.enabled = true` in the deckwatch settings ConfigMap. When
`enabled` is `false` (or tenant/client ids are blank), the middleware is a
pass-through — safe to leave layered in staging.

## Extracting the caller in handlers

Once activated, handlers can pull the authenticated user via an axum
`Extension<AuthUser>`:

```rust
use axum::Extension;
use crate::auth::AuthUser;

pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<...>, AppError> {
    tracing::info!(user = ?user.email, "list request");
    // ...
}
```

## RBAC mapping (planned)

Entra groups (or app roles) should map to deckwatch permissions:

| Entra group / role       | Deckwatch capability                            |
|--------------------------|-------------------------------------------------|
| `deckwatch-admins`       | Read/write all namespaces, edit settings        |
| `deckwatch-operators`    | Read/write only allowed namespaces              |
| `deckwatch-viewers`      | Read-only across allowed namespaces             |

The group→capability mapping is not yet implemented — `AuthUser.groups` is
captured and injected into requests, but no handler currently reads it.
Follow-up work: add a policy layer that consults `AuthUser.groups` when
answering `is_namespace_allowed` and settings-write requests.

## Local development

To develop against a real Entra tenant:

1. Add `http://localhost:5173/auth/callback` as an SPA redirect URI.
2. Point vite proxy at your backend and log in via the UI.

To develop without a tenant, leave `auth.enabled = false` — the middleware
and composable both no-op cleanly.
