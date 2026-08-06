# Deckwatch Plugin System

Deckwatch supports external plugins — compiled WASM binaries hosted in Git repositories — that can inject environment variables and sidecar containers into Kubernetes Deployments at create/update time. Plugins live entirely outside the deckwatch codebase, letting organizations add org-specific behaviour without forking deckwatch.

---

## Overview

### What plugins can do

| Capability | Use case |
|---|---|
| **Env var injection** | Wire external services into deployments: database URLs, secret manager addresses, feature flag endpoints, service discovery config |
| **Sidecar injection** | Add org-managed containers to pods: service mesh agents, log shippers, monitoring collectors, security scanners |

### What plugins cannot do

- Plugins cannot deny or block deployments (they can only add; they cannot remove or replace user-supplied values)
- Plugins have no network or filesystem access at runtime (no WASI, no host functions)
- Plugins cannot read Kubernetes secrets or cluster state — all context is passed by deckwatch in the `PluginContext` struct

### Injection rules

1. **User values always win.** If an env var name already exists on the primary container, the plugin's value is silently skipped.
2. **Sidecars don't duplicate.** If a container with the plugin's sidecar name already exists in the pod, the sidecar is skipped.
3. **Opt-in by annotation.** Plugins act on deployments that carry a specific annotation. A deployment with no relevant annotation gets no injection.

---

## Lifecycle

```
deckwatch startup
  └─ load settings from DB
      └─ for each enabled plugin:
           fetch .wasm from configured source (GitHub raw / GitHub Release / HTTPS URL)
           cache wasm bytes in AppState.plugins

deckwatch settings PUT
  └─ save new settings to DB
      └─ background task: re-fetch plugins from new config
           update AppState.plugins (atomic swap via RwLock)

deployment create or update (POST /api/:ns/deployments, PATCH /...)
  └─ apply user-supplied values to Deployment object
      └─ for each loaded plugin:
           instantiate plugin (fresh WASM instance per call)
           call plugin.apply(PluginContext{namespace, name, annotations, labels})
           merge returned env_vars into primary container (skipping existing names)
           merge returned sidecars into pod spec (skipping existing container names)
           record injected names in deployment annotations
      └─ apply merged Deployment to Kubernetes API
```

---

## Configuring plugins

Plugins are configured as part of `DeckwatchSettings`, stored in the deckwatch database and editable via Settings → Plugins in the UI or via `PUT /api/settings`.

### Schema

```jsonc
{
  "plugins": [
    {
      // Required. Unique name, used as annotation key suffix.
      // Must be lowercase alphanumeric + hyphens.
      "name": "org-vault-provider",

      // Optional. Defaults to true. Set false to disable without removing config.
      "enabled": true,

      // Required. Where to fetch the .wasm binary.
      "source": {
        // "github" or "url"
        "type": "github",

        // For type "github":
        "repo": "myorg/deckwatch-plugin-vault",  // "owner/repo"
        "ref": "v1.2.0",                          // tag, branch, or full SHA
        "path": "plugin.wasm",                    // path to .wasm in repo or release
        "use_release": true                        // true = GitHub Release asset
                                                   // false = raw file in repo
      },

      // Optional. Name of a git_token_secrets entry for private repos.
      "token_secret": "my-github-pat"
    }
  ]
}
```

### Source types

#### `github` — GitHub repository

Fetches from `raw.githubusercontent.com` (when `use_release: false`) or from GitHub Release assets (when `use_release: true`).

```jsonc
// Raw file in repo (good for development, pinning to a branch)
{
  "type": "github",
  "repo": "myorg/my-plugin",
  "ref": "main",
  "path": "dist/plugin.wasm",
  "use_release": false
}

// GitHub Release asset (recommended for production — immutable per tag)
{
  "type": "github",
  "repo": "myorg/my-plugin",
  "ref": "v1.0.0",
  "path": "plugin.wasm",
  "use_release": true
}
```

#### `url` — Arbitrary HTTPS URL

Useful for self-hosted Gitea, Forgejo, S3, or any HTTPS-accessible binary.

```jsonc
{
  "type": "url",
  "url": "https://artifacts.myorg.internal/deckwatch-plugins/vault-provider-v1.2.0.wasm"
}
```

### Private repository authentication

For private repositories, create a `git_token_secrets` entry pointing at a Kubernetes Secret that holds a personal access token with `repo` scope (or `contents:read` for fine-grained tokens):

```jsonc
// In git_token_secrets:
{
  "name": "my-github-pat",
  "secret_name": "deckwatch-github-token",
  "namespace": "deckwatch"
}
```

The Secret must have a `token` key:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: deckwatch-github-token
  namespace: deckwatch
stringData:
  token: ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

Reference the entry by name in the plugin config:

```jsonc
{
  "name": "my-private-plugin",
  "enabled": true,
  "source": { "type": "github", "repo": "myorg/private-plugin", "ref": "v1.0.0", "path": "plugin.wasm", "use_release": true },
  "token_secret": "my-github-pat"
}
```

---

## Opt-in annotations

Plugins act only on deployments that carry specific annotations. The annotation keys are entirely defined by the plugin — deckwatch does not prescribe them. Operators add annotations to deployments through:

**deckwatch UI** — Deployment → YAML editor:
```yaml
metadata:
  annotations:
    myorg.io/vault: "enabled"
    myorg.io/mesh: "enabled"
```

**kubectl**:
```bash
kubectl annotate deployment my-app \
  myorg.io/vault=enabled \
  myorg.io/mesh=enabled \
  -n my-namespace
```

**GitOps** — commit the annotation to the deployment manifest in the GitOps repo.

---

## Annotation tracking

Deckwatch records what each plugin injected using deployment-level annotations. These are written automatically and should not be edited manually.

| Annotation key | Value | Purpose |
|---|---|---|
| `deckwatch.plugin-env/<plugin-name>` | `VAR_ONE,VAR_TWO` | Comma-separated env var names injected by this plugin |
| `deckwatch.plugin-sidecar/<plugin-name>` | `mesh-agent,log-shipper` | Comma-separated sidecar names injected by this plugin |

These annotations are how deckwatch knows what to clean up if a plugin is disabled and the deployment is next updated.

---

## Writing a plugin

See [deckwatch-plugin-sdk](https://github.com/alexconrey/deckwatch-plugin-sdk) for the SDK crate and full development guide.

### Minimal plugin

```rust
use deckwatch_plugin_sdk::{EnvVarSpec, PluginContext, PluginResult};
use extism_pdk::*;

#[plugin_fn]
pub fn apply(Json(ctx): Json<PluginContext>) -> FnResult<Json<PluginResult>> {
    let mut result = PluginResult::default();

    if ctx.annotations.get("myorg.io/vault").map(|v| v == "enabled").unwrap_or(false) {
        result.env_vars.push(EnvVarSpec {
            name: "VAULT_ADDR".into(),
            value: "https://vault.myorg.internal:8200".into(),
        });
    }

    Ok(Json(result))
}
```

### Required Cargo.toml settings

```toml
[lib]
crate-type = ["cdylib"]   # produces a WASM module; "rlib" can be added for tests

[dependencies]
extism-pdk = "1"
deckwatch-plugin-sdk = { git = "https://github.com/alexconrey/deckwatch-plugin-sdk", tag = "v0.1.0" }
```

### Required `.cargo/config.toml`

```toml
[build]
target = "wasm32-unknown-unknown"
```

### Build

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

### Example repo

[alexconrey/deckwatch-plugin-example](https://github.com/alexconrey/deckwatch-plugin-example) is a reference implementation with both env var and sidecar injection, unit tests, and a GitHub Actions release workflow.

---

## Security considerations

### WASM sandboxing

Plugins run inside a `wasmtime` WASM instance with no WASI (no filesystem, no network, no host system calls). A plugin cannot:
- Make network requests
- Read or write files
- Access environment variables of the deckwatch process
- Call arbitrary host functions

The only communication channel is JSON in (the `PluginContext`) and JSON out (the `PluginResult`).

### Supply chain

- **Pin to immutable refs.** Use GitHub Release assets with a specific version tag rather than a branch name. A branch can be force-pushed; a published release asset cannot be changed once uploaded.
- **Audit plugin code before deploying.** Deckwatch executes whatever WASM you configure. Treat plugin repos with the same scrutiny as a direct dependency.
- **Private repos for sensitive logic.** If the plugin encodes org-specific topology (internal hostnames, service naming conventions), host it in a private repo with a scoped token.

### Token scope

The `token_secret` used for authenticated fetches only needs read access to the repository contents. For GitHub:
- Classic PAT: `repo` scope (or `public_repo` for public repos)
- Fine-grained PAT: `Contents: Read` permission on the specific repo

Deckwatch uses the token only at startup and on settings PUT — never during deployment operations.

---

## Troubleshooting

### Plugin not loading at startup

Check the deckwatch logs for `failed to fetch plugin WASM`:

```bash
kubectl logs -n deckwatch deploy/deckwatch | grep plugin
```

Common causes:
- `repo`, `ref`, or `path` is wrong — verify the URL manually: `curl -L https://github.com/<repo>/releases/download/<ref>/<path>`
- Private repo with no `token_secret` configured
- Token has insufficient scope
- `use_release: true` but no release exists for the given tag

### Plugin loaded but not injecting

- Verify the deployment has the annotation the plugin checks for
- Check that the annotation value matches exactly (case-sensitive, e.g. `"enabled"` not `"true"`)
- Look for `plugin apply() failed` in deckwatch logs, which indicates a runtime error inside the WASM

### Env vars not appearing after update

If a deployment was created before the plugin was loaded, the next update via deckwatch will trigger injection. Alternatively, do a no-op update (e.g. add/remove a label) to force the plugin to run.

### Duplicate injection not happening

This is intentional. If `deckwatch.plugin-env/<name>` or `deckwatch.plugin-sidecar/<name>` annotations already exist on the deployment, deckwatch knows what was previously injected and the "skip if already exists" guard prevents duplicates.

---

## Implementation notes

For developers working on deckwatch itself:

- **`src/plugins.rs`** — plugin fetching, WASM execution via extism, env/sidecar merge logic
- **`src/handlers/settings.rs`** — `PluginConfig` and `PluginSource` types, background refresh on settings PUT
- **`src/state.rs`** — `AppState.plugins: Arc<RwLock<Vec<LoadedPlugin>>>`
- **`src/handlers/deployments.rs`** — `apply_plugins()` called in both `create` and `update` handlers

Plugins are instantiated fresh per deployment operation (not pooled). This avoids thread-safety concerns with extism `Plugin` instances and keeps memory bounded since WASM binaries for a single-function plugin are typically 50–200 KB.
