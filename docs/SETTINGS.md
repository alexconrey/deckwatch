# Deckwatch Settings

Runtime configuration for deckwatch lives in a Kubernetes ConfigMap. This lets
operators edit settings through the Settings page in the UI without needing to
`helm upgrade` for every change, while still keeping settings declarative and
inspectable via `kubectl`.

## Storage model

- **Kind:** `ConfigMap`
- **Default name:** `deckwatch-config` (override via `settings.configMapName`
  in `values.yaml` or `DECKWATCH_SETTINGS_CONFIGMAP` env var)
- **Namespace:** the pod's own namespace (via the downward-API `POD_NAMESPACE`
  env var), overridable with `DECKWATCH_SETTINGS_NAMESPACE`. Falls back to
  `deckwatch` when neither is set (local dev).
- **Payload:** a single key `data.settings` holding pretty-printed JSON. Kept
  as JSON (not individual keys) so nested structures survive round-tripping
  without brittle key flattening.

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
  }
}
```

Fields marked optional in the Rust struct (`Option<T>`) are omitted from the
JSON payload when unset.

## API surface

| Method | Path            | Purpose                                                                                       |
|--------|-----------------|-----------------------------------------------------------------------------------------------|
| `GET`  | `/api/settings` | Returns the parsed settings. If the ConfigMap does not exist, returns a defaults payload.     |
| `PUT`  | `/api/settings` | Server-side apply of the full settings body. Creates the ConfigMap if missing.                |

The `PUT` uses `Patch::Apply` with field-manager `deckwatch`, so helm and the
app own disjoint fields of the object: helm owns the initial seed metadata,
the app owns `data.settings`. The `helm.sh/resource-policy: keep` annotation
prevents `helm uninstall` from destroying operator-authored settings.

## Configuring auth (Coming Soon)

The Authentication tab in the Settings page persists Entra tenant/client
identifiers but the toggle is disabled — the backend middleware is defined but
not layered onto any routes. See [AUTH.md](AUTH.md) for the plumbing.

To activate once ready:

1. Register a Deckwatch app in Entra ID (see AUTH.md).
2. Fill in tenant ID, client ID, and redirect URI on the Settings page.
3. Flip `auth.enabled = true` in the ConfigMap (or via the toggle once
   enabled by a future release).
4. Restart the deckwatch pod so it re-reads settings at startup and layers
   the `require_auth` middleware onto the API router.

## Configuring notifications (Coming Soon)

Same pattern — persist the webhook URL now, activate later.

## Local development

When running the backend outside a pod (e.g. `cargo run`), the settings
namespace defaults to `deckwatch`. Create it and seed the ConfigMap with:

```bash
kubectl create namespace deckwatch
kubectl create configmap deckwatch-config -n deckwatch \
  --from-literal=settings='{"allowed_namespaces":[]}'
```

Or just let the app create it on the first `PUT`.
