# Deckwatch Settings

Runtime configuration for deckwatch is stored in the database (SQLite by
default, PostgreSQL or MySQL via `DATABASE_URL`). The Settings page in the UI
provides a sidebar tree navigation with categorized sections, letting operators
edit settings without needing to `helm upgrade` for every change.

## Storage model

- **Backend:** SeaORM `settings` table (key/value pairs with `updated_at`
  timestamps). See `src/entities/settings.rs`.
- **Migration from ConfigMap:** Older installations stored settings in a
  `deckwatch-config` ConfigMap. On upgrade, existing ConfigMap settings are
  automatically migrated to the database on first access. The ConfigMap is
  retained by Helm for backwards compatibility but is no longer the source of
  truth.

## Settings page sections

The Settings page renders a sidebar tree with the following sections:

| Section | Sidebar ID | What it controls |
|---------|------------|-----------------|
| **General** | `general` | Allowed namespaces, default resource limits |
| **Authentication** | `auth` | Entra ID tenant/client config, auth toggle |
| **AI Providers** | `ai_providers` | Server-side toggles for AI provider integrations (not browser-local) |
| **Observability** | `observability` | Prometheus runtime toggle (`prometheus_enabled`), metrics config |
| **Templates** | `templates` | Deployment templates (CRUD, per-category defaults) |
| **Git Repositories** | `git_repositories` | Managed list of Git repos for GitOps dropdowns |
| **Container Registries** | `container_registries` | Managed list of OCI registries for GitOps dropdowns |
| **Audit Log** | `audit` | Read-only view of the `audit_log` table |

## Schema

```jsonc
{
  "allowed_namespaces": ["default", "team-a"],
  "default_resource_limits": {
    "cpu_request": "100m",
    "memory_request": "128Mi",
    "cpu_limit": "500m",
    "memory_limit": "512Mi"
  },
  "auth": {
    "enabled": false,
    "tenant_id": "00000000-0000-0000-0000-000000000000",
    "client_id": "00000000-0000-0000-0000-000000000000",
    "redirect_uri": null,
    "scopes": "openid profile email"
  },
  "notifications": {
    "enabled": false,
    "webhook_url": ""
  },
  "prometheus_enabled": true,
  "git_repositories": [],
  "oci_registries": [],
  "git_token_secrets": []
}
```

Fields marked optional in the Rust struct (`Option<T>`) are omitted from the
JSON payload when unset.

## API surface

| Method | Path            | Purpose                                                                  |
|--------|-----------------|--------------------------------------------------------------------------|
| `GET`  | `/api/settings` | Returns the parsed settings from the database. Returns defaults if empty |
| `PUT`  | `/api/settings` | Writes the full settings body to the database                            |

## Key behaviors

### Prometheus toggle

Prometheus metrics collection is a runtime toggle controlled by the
`prometheus_enabled` field in settings (default: `true`). Toggling this in the
Observability section enables or disables the `/metrics` scrape endpoint
without requiring a pod restart.

### Container registry

The embedded OCI registry is controlled by the Helm chart
(`registry.enabled` in `values.yaml`), not by the settings page. The Helm
chart passes `DECKWATCH_REGISTRY_ENABLED` and `DECKWATCH_REGISTRY_PUBLIC_URL`
environment variables to the pod, which populate `AppState.registry_enabled`
and `AppState.registry_public_url`. The settings page displays registry
status but cannot toggle it.

### AI provider toggles

AI provider configuration (which providers are active, API keys, rate limits)
is managed server-side in the database via the AI Providers settings section.
These are not browser-local preferences -- they are shared across all users
and take effect immediately.

## Configuring auth (Coming Soon)

The Authentication section in the Settings page persists Entra tenant/client
identifiers but the toggle is disabled -- the backend middleware is defined but
not layered onto any routes. See [AUTH.md](AUTH.md) for the plumbing.

To activate once ready:

1. Register a Deckwatch app in Entra ID (see AUTH.md).
2. Fill in tenant ID, client ID, and redirect URI on the Settings page.
3. Flip `auth.enabled = true` via the toggle.
4. Restart the deckwatch pod so it re-reads settings at startup and layers
   the `require_auth` middleware onto the API router.

## Configuring notifications (Coming Soon)

Same pattern -- persist the webhook URL now, activate later.

## Local development

When running the backend locally (e.g. `cargo run`), the database defaults to
a SQLite file. No ConfigMap setup is required. Settings are persisted
automatically on first `PUT` via the Settings page.

For legacy ConfigMap compatibility during local dev:

```bash
kubectl create namespace deckwatch
kubectl create configmap deckwatch-config -n deckwatch \
  --from-literal=settings='{"allowed_namespaces":[]}'
```
