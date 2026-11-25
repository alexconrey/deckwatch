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
- Plugins have no network or filesystem access at runtime (no WASI, no host functions) — this is intentional. Plugins are purely declarative: they receive context and return Kubernetes manifests. Operators like ACK and ESO handle the actual AWS/cloud API calls after those manifests land in the cluster.
- Plugins cannot read Kubernetes secrets or cluster state — all context is passed by deckwatch in the `PluginContext` struct

### Injection rules

1. **User values always win.** If an env var name already exists on the primary container, the plugin's value is silently skipped.
2. **Sidecars don't duplicate.** If a container with the plugin's sidecar name already exists in the pod, the sidecar is skipped.
3. **Opt-in by annotation.** Plugins act on deployments that carry a specific annotation. A deployment with no relevant annotation gets no injection.

### Roadmap

| Capability | Status | Notes |
|---|---|---|
| Env var injection | Shipped | Static values, `secretKeyRef`, `configMapKeyRef` |
| Sidecar injection | Shipped | |
| Kubernetes resource creation | Shipped | CRDs, ExternalSecrets, CronJobs, etc. via server-side apply |
| Kubernetes object deletion on deprovision | Shipped (SDK v0.10.0) | `deprovision()` returns `delete_kubernetes_resources`; deckwatch deletes ServiceAccounts and Jobs in the application's namespace. See [Deleting Kubernetes objects on deprovision](#deleting-kubernetes-objects-on-deprovision) |
| ServiceAccount binding | Shipped (SDK v0.3.0) | Plugin declares `service_account_name`; deckwatch sets it on the pod template |
| Plugin dependency resolution | Shipped (SDK v0.4.0) | `metadata()` export, `provides`/`depends_on`, topological sort, output threading |
| Plugin networking (`allowed_hosts`) | Shipped | Controls which URLs the plugin can reach via extism HTTP host function |
| Plugin config injection (`config` + `inherit_env_keys`) | Shipped | Static config map + live env var passthrough for rotated credentials |
| MCP resource-scoped hints | Shipped | Per-group org guidance injected into tool descriptions at `tools/list` time |
| ServiceAccount management UI | Planned | Create/manage ServiceAccounts from the deckwatch UI, not just via plugins |
| deckwatch UI for plugin management | Shipped | Settings → Plugins |

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

Every call compiles the plugin's WebAssembly before instantiating it. With
`DECKWATCH_CACHE_DIR` set, as the Helm chart does by default (its `cache`
values), compiled code is cached on disk in that directory: each pod compiles
a plugin binary once, and later calls load it from the cache. Without it,
every call compiles. A cache directory deckwatch can't use is logged and the
cache turned off; the cache never makes a plugin call fail.

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
      "token_secret": "my-github-pat",

      // Optional. Hosts the plugin can reach via extism's HTTP host function.
      // Supports glob patterns. Empty = deny all outbound HTTP from the plugin.
      "allowed_hosts": ["*.amazonaws.com"],

      // Optional. Key-value config injected into the plugin's extism namespace.
      // The plugin reads these via extism_pdk::config::get("KEY").
      // Use for static credentials, endpoints, or plugin-specific settings.
      "config": { "AWS_REGION": "us-east-1" },

      // Optional. Environment variable names to read from the deckwatch process
      // environment and inject into the plugin config at invocation time.
      // These OVERRIDE same-named "config" entries so live/rotated values
      // always win. deckwatch's own secrets are never passed (see
      // SETTINGS.md, "Plugin inherit_env_keys").
      "inherit_env_keys": ["AWS_ROLE_ARN"],

      // Optional. Config key -> environment variable holding a file path;
      // the plugin receives the file's contents (e.g. a workload identity token).
      "inherit_env_file_keys": { "AWS_IDENTITY_TOKEN": "AWS_WEB_IDENTITY_TOKEN_FILE" }
    }
  ]
}
```

### Config field types

A plugin declares its settings in `metadata().config_schema`, and the plugin
settings page renders one control per field. Every value is stored as a string.

| `field_type` | Control | Stored value |
|---|---|---|
| `string` | Text input | As entered |
| `secret` | Masked input | Encrypted at rest |
| `bool` | Switch | `"true"` or `"false"` |
| `key_value` | Text input of the JSON value (a row-per-pair editor is planned) | JSON object of string keys to string values, e.g. `{"Team":"platform"}`; empty means no pairs |
| `select` | Dropdown of `options` | The chosen option |
| anything else | Text input | As entered |

- **`key_value` values are validated on save**, both in the UI and in MCP
  `update_plugin_config`. Anything but an object of strings is rejected with a
  400 or an error, and nothing in that save is written. Over MCP you can pass the
  object itself (`{"RESOURCE_TAGS": {"Team": "platform"}}`) or its JSON string.
- **An unrecognised `field_type`**, from a plugin built against a newer SDK, is
  treated as plain text instead of failing the whole `metadata()` parse. Before
  this, one unknown type made deckwatch drop every config field and resource
  the plugin declares.

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

## Deleting Kubernetes objects on deprovision

A plugin can't delete Kubernetes objects itself. When a provisioned resource is removed, deckwatch calls the plugin's `deprovision()` export, and the plugin can list objects for deckwatch to delete in `delete_kubernetes_resources` (SDK v0.10.0):

```json
{
  "errors": [],
  "message": "Deleted IAM role team-shop-rds",
  "delete_kubernetes_resources": [
    { "api_version": "batch/v1", "kind": "Job", "name": "shop-db-bootstrap-1a2b3c4d" },
    { "api_version": "v1", "kind": "ServiceAccount", "name": "shop-db-bootstrap" }
  ]
}
```

Deckwatch deletes them after `deprovision()` returns, whether the resource was removed through the API or the MCP `deprovision_resource` tool:

- Only in the application's namespace. An entry has no namespace field; a `namespace` key is ignored.
- Only `v1` `ServiceAccount` and `batch/v1` `Job`. Other kinds, and invalid object names, are refused.
- Jobs first, with background propagation so their pods are deleted too; then ServiceAccounts.
- A ServiceAccount that a Deployment, CronJob or Job in the namespace still uses is kept, because deleting it would stop new pods from being created. The namespace's `default` ServiceAccount is never deleted.
- An object that's already gone counts as deleted.
- Every object gets an audit entry (action `delete`, outcome in the detail). Refusals and failures are also logged as warnings and counted in `deckwatch_plugin_errors_total` (`k8s_delete_refused`, `k8s_delete_failed`). None of them stops the resource record from being removed.

The `DELETE …/resources/{plugin}/{resource_id}` response and the MCP tool's response report each object's outcome in `kubernetes_resource_deletions`, next to the plugin's `message` and events.

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

## Events, warnings and errors

Everything a plugin call reports is turned into **plugin events**, each with a
severity (`info`, `warning` or `error`), a code, a message, and optionally a
resource id, a Kubernetes object, a `retryable` flag and a hint:

- **From the plugin.** Structured `events` (SDK 0.10.0, `PluginEvent`) on
  `PluginResult`, `ResourceProvisionResult` and `ResourceDeprovisionResult`.
  Use codes of the form `<plugin-name>.<code>`. The legacy `errors`,
  `warnings` and deprovision `message` strings still work and become events
  with code `plugin.unspecified`; the SDK's copies of structured events in
  those fields are recognised and reported once.
- **From deckwatch.** Failures it detects itself, with reserved codes:
  `plugin.fetch_failed`, `plugin.checksum_mismatch`, `plugin.load_failed`
  (bad module, missing export, `min_deckwatch_version`), `plugin.trap` (a
  panic), `plugin.timeout`, `plugin.output_invalid` (the message names the
  field, e.g. `env_vars[1].name`), `plugin.k8s_apply_failed` and
  `plugin.policy_rejected`, plus the informational `plugin.fields_released`
  (the reconciler removed what an application's provisioned resources had
  added to a workload, because none are left). A plugin can't use the
  `plugin.*` namespace: such codes are moved under the plugin's name.

Where they show up:

- **The response of the action that ran the plugin**, warnings included even
  when it succeeds: `plugin_events` on deployment create and update, and on
  provision and refresh; `{message, plugin_events,
  kubernetes_resource_deletions}` from deprovision (the `message` is what the
  plugin says it did and didn't clean up). A
  `provision()` that fails outright returns 502 with the events. The MCP
  tools `provision_resource`, `refresh_resource`, `deprovision_resource`,
  `validate_plugin` and `enable_plugin` return them too, inside a fence that
  marks them as untrusted text.
- **Plugin status**: the current state of every call target (a provisioned
  resource, a plugin's `apply()` for an application, or the plugin's own
  load): healthy, degraded or failing, the latest events, when the current
  run of failures started, how many calls in a row failed, and the last
  success. The application and plugin pages show it as badges, and
  **Problems** in the top bar (shown while anything is failing or degraded)
  lists every failing or degraded target. The API is `GET /api/plugin-status`
  (filter with `health=failing,degraded`, `plugin`, `namespace`),
  `GET /api/namespaces/{ns}/applications/{name}/plugin-status` and
  `GET /api/plugins/{name}/status`. It holds current state only: a row is
  replaced on every call, and removed with its resource, its application, or
  when the plugin is disabled or removed. The reconcile loop records one
  `apply()` outcome per plugin and application per cycle, the worst across
  the application's Deployments.
- **Kubernetes Events** on the affected Deployments, from reporting
  controller `deckwatch` (events.k8s.io/v1, through kube-rs `Recorder`).
  They show up in `kubectl describe deployment`, deckwatch's event views and
  MCP `get_events` (fenced as untrusted text there). Reasons:

  | Reason | Type | When |
  |---|---|---|
  | `PluginApplyFailed` | Warning | `apply()` failed or reported an error (deployment create/update, reconcile) |
  | `PluginApplyWarning` | Warning | `apply()` reported a warning |
  | `PluginProvisionFailed` | Warning | `provision()` failed or reported an error (provision, refresh, status polling) |
  | `PluginProvisionWarning` | Warning | `provision()` reported a warning |
  | `PluginProvisioned` | Normal | an operator's provision or refresh worked |
  | `PluginDeprovisionFailed` | Warning | `deprovision()` failed or reported an error |
  | `PluginDeprovisioned` | Normal | `deprovision()` worked; the note is its message |
  | `PluginRecovered` | Normal | a failing target works again |
  | `PluginFieldsReleased` | Normal | the reconciler removed what provisioned resources had added to a workload |

  The action carries the event code (e.g. `Apply plugin.trap`). The same
  Event on the same workload is published at most every 5 minutes, and the
  Recorder folds repeats into a series, so a persistent failure is one Event
  whose count goes up (`kubectl describe` shows `(x12 over 1h)`). Provision
  and deprovision Events go on every Deployment of the application. The
  chart's ClusterRole grants `create` and `patch` on `events.k8s.io` events;
  set `POD_NAME` (the chart does) to fill `reportingInstance`.
- **Notifications** (`plugin_failing`, `plugin_recovered`), once per
  transition into or out of failing, never on every reconcile. See
  [notifications](SETTINGS.md#configuring-notifications).
- **The logs**, as `plugin event:` lines inside the call's `plugin {export}`
  span, next to the plugin's own `log_*` output (target `extism::pdk`).
- **Traces**: when deckwatch exports traces (`OTEL_EXPORTER_OTLP_*`), each
  call's `plugin {export}` span is exported, and the plugin gets its
  `trace_context`.

Plugin text is untrusted. Before deckwatch returns, logs or stores it, it
masks the values it passed the plugin for `secret` config fields (in any form:
plain, the stored ciphertext, JSON-escaped or percent-encoded), redacts
credentials in URLs, strips control characters, puts each message on one line
and caps it at 2 KB, and keeps at most 20 events per call. Logging is rate
limited per plugin; what's over the limit is dropped and counted in
`deckwatch_plugin_events_dropped_total`.

When deckwatch exports traces, `PluginContext`, `ResourceProvisionRequest`
and `ResourceDeprovisionRequest` carry `trace_context`, a W3C `traceparent`
for the call. Forward it with the SDK's `trace_headers()` so the plugin's own
API calls join the trace.

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

### deckwatch restarts while plugins load or run

deckwatch fetches and compiles every enabled plugin before it starts serving
HTTP (the `listening` log line). The Helm chart's startup probe gives it 5
minutes; plugins load one at a time, and each download can take up to 30 s. If
the pod's events show `failed startup probe, will be restarted` before
deckwatch has logged `listening`, raise the budget:

```yaml
startupProbe:
  failureThreshold: 120   # 10 minutes, checked every 5 s
```

Once deckwatch is running, plugin calls run on threads of their own, at most 4
at a time, not on the async worker threads that answer HTTP, so a slow or stuck
plugin doesn't hold up the health checks. deckwatch stops waiting for a call
after 2 minutes and reports `plugin.timeout`. Plugin calls have no time limit
yet ([#301](https://github.com/alexconrey/deckwatch/issues/301)), so the call
itself may still be running.

### Plugin loaded but not injecting

- Verify the deployment has the annotation the plugin checks for
- Check that the annotation value matches exactly (case-sensitive, e.g. `"enabled"` not `"true"`)
- Check the `plugin_events` in the deployment create/update response (the UI shows them after saving), or look for `plugin event:` lines in the deckwatch logs. A `plugin.trap`, `plugin.timeout` or `plugin.output_invalid` code means the plugin failed and was skipped.

### Env vars not appearing after update

If a deployment was created before the plugin was loaded, the next update via deckwatch will trigger injection. Alternatively, do a no-op update (e.g. add/remove a label) to force the plugin to run.

### Duplicate injection not happening

This is intentional. If `deckwatch.plugin-env/<name>` or `deckwatch.plugin-sidecar/<name>` annotations already exist on the deployment, deckwatch knows what was previously injected and the "skip if already exists" guard prevents duplicates.

---

## Implementation notes

For developers working on deckwatch itself:

- **`src/plugins.rs`** — plugin fetching, WASM execution via extism (every call goes through `call_export`, the funnel), env/sidecar merge logic
- **`src/plugin_events.rs`** — turning results and failures into plugin events: normalization, secret masking, caps, the LLM fence and the log rate limit
- **`src/handlers/settings.rs`** — `PluginConfig` and `PluginSource` types, background refresh on settings PUT
- **`src/state.rs`** — `AppState.plugins: Arc<RwLock<Vec<LoadedPlugin>>>`
- **`src/handlers/deployments.rs`** — `apply_plugins()` called in both `create` and `update` handlers

Plugins are instantiated fresh per deployment operation (not pooled). This avoids thread-safety concerns with extism `Plugin` instances and keeps memory bounded since WASM binaries for a single-function plugin are typically 50–200 KB.

A plugin call blocks its thread: extism runs the export synchronously, and the plugin's HTTP requests wait on that thread. So calls never run on Tokio's async workers, where they would stall every other task, health checks included. The entry points in `src/plugins.rs` (`run_plugin`, `run_provision`, `run_deprovision`, `apply_plugins`, `fetch_plugins` and `fetch_and_validate`) are async and run each call on Tokio's blocking pool, at most 4 at a time, inside the caller's span; the blocking functions are private to the module. A caller waits for a call up to the call timeout plus 5 seconds, or 2 minutes when no timeout is set, then reports `plugin.timeout`; the abandoned call may still be running.
