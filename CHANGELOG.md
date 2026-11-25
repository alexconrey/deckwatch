# Changelog

All notable changes to deckwatch are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) (within 0.x, a minor release can
contain breaking changes, so read its upgrade notes before upgrading).

A release is the container image `ghcr.io/alexconrey/deckwatch:<version>`
(linux/amd64 and linux/arm64; the image tag has no `v`) plus the Helm chart in
`helm/deckwatch` at the same version, both built from the `v<version>` tag.
The release workflow refuses a tag that doesn't match `Cargo.toml`,
`helm/deckwatch/Chart.yaml` (`version` and `appVersion`),
`frontend/package.json` and a dated section in this file, and publishes that
section as the GitHub Release notes.

Changes merged since the last release are listed under `[Unreleased]`.

## [Unreleased]

### Upgrade notes

- **The MCP endpoint's Kubernetes tools now have guardrails, on by
  default.** The embedded mcp-k8s moves from v0.5.2 to v0.7.0, and deckwatch
  turns on its secure defaults, except that `apply_manifest` stays available.
  Each switch is in **Settings → MCP Tuning → Kubernetes tool guardrails**
  (`mcp_tuning.guardrails`) and applies from the next MCP request, without a
  restart (clients pick up a changed tool list when they reconnect). What
  agents notice:
  - **Workload guardrails (restrictive).** The typed create and update tools
    (`create_pod`, `create_deployment`, `update_deployment`, `create_service`,
    the RBAC tools, ...) and the custom resource tools refuse privileged
    containers, privilege escalation, host namespaces, `hostPath` volumes,
    host ports, capabilities beyond the runtime's defaults, risky volume
    types, Unconfined profiles, NodePort, LoadBalancer and ExternalName
    Services, ingress snippet annotations, RBAC escalation (escalate, bind,
    impersonate, wildcards, binding cluster-admin) and pod-security labels set
    to `privileged`. The error names the rule and the deckwatch setting. Let a
    rule through with an exception, or switch the policy to permissive (every
    workload guardrail off, as before). Updates are checked with a server-side
    dry run, so an update to a workload that already breaks a rule is refused
    too.
  - **Custom resource writes are refused** (`create_custom_resource`,
    `update_custom_resource`) until their API group is listed under allowed
    custom resource API groups. Cluster-scoped custom resource kinds stay
    refused in restrictive mode (apply them with `apply_manifest`, or use the
    permissive policy). Reads are unaffected.
  - **Secret values are off.** `get_secret` refuses `decode: true` (it used to
    return the values), and `get_resource_json` returns a Secret without its
    `data` and `stringData` and without the
    `kubectl.kubernetes.io/last-applied-configuration` annotation. Turn on
    **Return Secret values** to allow them.
  - **New workloads run as non-root.** The typed create tools fill in
    `runAsNonRoot: true`, the RuntimeDefault seccomp profile, no privilege
    escalation, all capabilities dropped and no service account token (unless
    `service_account_name` is set) when the call doesn't set them. An image
    that runs as root, such as `nginx` or `busybox`, then fails to start
    (`container has runAsNonRoot and image will run as root`) unless the call
    passes `security_context: {"run_as_non_root": false}` or **Secure defaults
    for new workloads** is off.
  - **`approve_csr` refuses privileged identities:** `system:masters`, any
    other `system:` user or group, and requests it can't parse. A node's
    request for its own serving certificate is still approved. Turn on
    **Approve privileged certificate requests** to allow the others.
  - **`apply_manifest` is still on, and now asks first.** It is marked
    destructive, so MCP clients such as Claude Code ask the user before each
    call, and its description tells agents to use it only when asked. It
    still bypasses the workload guardrails. Its `namespace` argument is now
    used: it fills `metadata.namespace` when the manifest has none, and a
    manifest naming another namespace is refused. Turn it off to remove it
    from `tools/list`.
  - **`DECKWATCH_NAMESPACES` reaches more tools.** With an allowlist,
    `create_namespace`, `update_namespace` and `delete_namespace` refuse
    namespaces outside it; `watch_resource` and `list_custom_resources` on a
    namespaced kind, and `list_csistoragecapacities`, refuse to run without
    a namespace; `drain_node` refuses, before cordoning, when it would evict
    pods outside the allowlist; `can_i` and `list_my_permissions` refuse other
    namespaces. `get_custom_resource`, `update_custom_resource` and
    `delete_custom_resource` require `namespace` for namespaced kinds.
  - **Removed and changed tools.** `get_resource_yaml` is gone: use
    `get_resource_json` (same arguments). `list_clusters`, `switch_cluster`
    and `get_active_cluster` are no longer listed; they always answered
    "Unknown tool". `port_forward_check` counts as a write on the pod, so it
    is audited now, and it gives up after 10 seconds. The typed workload,
    Service and Ingress tools refuse unknown arguments (they used to ignore
    them) and accept camelCase spellings. Deckwatch no longer replaces
    `create_pod`: mcp-k8s's takes the whole pod template, and
    `service_account` still works (as an alias of `service_account_name`). A
    pod created with a service account used to skip the guardrails and secure
    defaults, and was labelled `app.kubernetes.io/managed-by: deckwatch`; it
    is now `mcp-k8s`.

  Tool errors are still JSON-RPC errors, as before. The guardrails cover the
  mcp-k8s tools; deckwatch's own tools (`create_ingress`, `update_ingress`,
  ingress templates, `create_application`, plugins) and its `exec_pod` aren't
  checked against them yet. See [docs/MCP.md](docs/MCP.md#guardrails). (#403)
- **Compiled plugins are cached only if `DECKWATCH_CACHE_DIR` is set.**
  extism used to cache them in wasmtime's default directory under `$HOME`
  (`$HOME/.cache/wasmtime`, or `$XDG_CACHE_HOME/wasmtime`), and when it
  couldn't create that directory, every plugin call failed with
  `plugin.load_failed`. Now deckwatch caches nothing unless
  `DECKWATCH_CACHE_DIR` is set. The Helm chart sets it (see Added). If you run
  deckwatch without the chart, set `DECKWATCH_CACHE_DIR` to a writable
  directory, or plugins are compiled on every call. (#399)
- **`tracing.otlp_endpoint` is retired.** It was a gRPC `host:port` for the
  retired OpenTelemetry Collector plugin, and deckwatch never exported to it.
  To export deckwatch's own telemetry, enter your collector's OTLP/HTTP
  endpoint (usually port 4318, not 4317) as the OTLP endpoint in
  **Settings → Observability**. The old value isn't converted. Until you set
  it, or remove the old value from the settings and from the chart's
  `settings.defaults.tracing`, deckwatch logs a warning at startup and the
  Settings page asks for it. `tracing.query_url`, `ui_url` and
  `backend_kind` are unchanged. (#401, #398)

### Security

- **MCP: `DECKWATCH_NAMESPACES` could be bypassed through mcp-k8s tools.**
  With an allowlist set, `delete_namespace` deleted any namespace (for
  example `kube-system`), `drain_node` evicted pods in every namespace on the
  node, and `watch_resource` or `list_custom_resources` without a namespace
  read every namespace. mcp-k8s v0.7.0 refuses them (see Upgrade notes).
  (#403)
- **MCP: Secret values leaked with decoding off.** `get_resource_json`
  returned a Secret's base64 `data`. Secret values are now off by default and
  redacted everywhere unless **Return Secret values** is on. (#403)
- **MCP: `apply_manifest` didn't ask before running.** deckwatch replaced
  mcp-k8s's definition with one that had no `destructiveHint` and no warning,
  so clients ran raw manifests without asking the user. It now carries both.
  (#403)
- kube 5.1.0 → 5.2.0 (RUSTSEC-2026-0600: kube-client's OpenSSL connector also
  trusted the system certificate store). deckwatch uses the rustls connector,
  so it wasn't affected, but the advisory was flagged on every build. (#403)

### Added

- **Settings → MCP Tuning → Kubernetes tool guardrails**
  (`mcp_tuning.guardrails`): the workload policy (`workload_policy`:
  `restrictive` or `permissive`), the rules to let through in restrictive
  mode (`exceptions`, for example `allow_load_balancer`), the custom resource
  API groups agents may write (`allowed_custom_resource_groups`),
  `allow_privileged_csr`, `apply_manifest_enabled`, `secret_decode_enabled`
  and `secure_defaults`. `GET /api/settings` always returns the block, and a
  `PUT` that omits it keeps the stored one. Every change is audit-logged as
  `mcp_tuning.guardrails.<field>`. A refusal by mcp-k8s names the deckwatch
  setting next to the mcp-k8s flag. See
  [docs/MCP.md](docs/MCP.md#guardrails). (#403)
- **OTLP endpoints for deckwatch's own telemetry in Settings →
  Observability:** `observability.otlp_endpoint`, an OpenTelemetry
  Collector's OTLP/HTTP base URL for traces and logs, and
  `observability.otlp_metrics_endpoint`, the full URL of Prometheus's OTLP
  receiver (`/api/v1/otlp/v1/metrics`) for metrics. An
  `OTEL_EXPORTER_OTLP_*ENDPOINT` variable still wins, per signal, and the
  exporter headers (`OTEL_EXPORTER_OTLP_*HEADERS`) are only sent to an
  endpoint from the environment. deckwatch reads the endpoints at startup,
  so restart it after a change. The Settings page and
  `GET /api/settings/observability` show where each signal goes now and
  after a restart. See
  [docs/TRACING.md](docs/TRACING.md#10-deckwatchs-own-telemetry).
  (#401, #398)
- **Helm `extraEnv`:** extra environment variables for the deckwatch
  container, as a list of Kubernetes `EnvVar` objects added after the ones
  the chart sets. See
  [README.md](README.md#helm-chart-extra-environment-variables). (#391)
- **`DECKWATCH_CACHE_DIR`, and a chart volume for it.** Plugin calls cache
  compiled WebAssembly in `<dir>/wasmtime`, so each pod compiles a plugin
  binary once instead of on every call, and plugins no longer need a writable
  `$HOME`. deckwatch writes the cache's config, `<dir>/wasmtime-cache.toml`,
  at startup. If it can't create or write the directory, it logs a warning
  and runs without the cache; the cache never makes a plugin call fail. The
  chart mounts an emptyDir at `/var/cache/deckwatch` for it, one per pod. The
  new `cache` values turn it off (`cache.enabled`) or change
  `cache.mountPath` and `cache.sizeLimit` (default `1Gi`). See
  [README.md](README.md#helm-chart-plugin-compile-cache). (#399)

### Changed

- **The OpenTelemetry Collector plugin is retired.** deckwatch-plugin-otel's
  repository is going away, so remove the plugin under **Settings → Plugins**
  if you installed it. [docs/TRACING.md](docs/TRACING.md) no longer describes
  it. Per-application `OTEL_*` injection is planned to replace it (#398);
  until then, set the `OTEL_*` variables on your workloads yourself.
  deckwatch's own OTLP export (Helm `telemetry.otlp`) works as before, and its
  endpoints can now also be set in **Settings → Observability** (see Added).
  (#396)
- MCP `tools/list` is built from the guardrail settings, so it offers only
  the tools deckwatch serves under them, each once. The `initialize`
  instructions now include mcp-k8s's guidance: prefer the typed tools, report
  a guardrail rejection instead of working around it, and use
  `apply_manifest` only on the user's explicit request. (#403)
- mcp-k8s's `audit` events (target `mcp_k8s::audit`) are never logged or
  exported: deckwatch writes its own audit row for each of those calls. The
  `tool call finished` log line still records every call with its decision.
  A refusal inside mcp-k8s (workload policy, privileged CSR, the namespace
  allowlist cases above) is an `error` row in deckwatch's audit log for now;
  the log line and span say `denied` with the rule. See
  [docs/MCP.md](docs/MCP.md#audit-log). (#403)

### Fixed

- **Plugin calls no longer stall the server, so the pod stops restarting.**
  Plugins ran on the async runtime's worker threads, and a container CPU
  limit under 2 cores leaves the runtime a single worker. While a plugin ran
  (the reconcile loop calls every plugin for each plugin-managed Deployment
  every 10 seconds), nothing else did: `/api/healthz` and `/api/readyz` timed
  out and the kubelet restarted the pod. Plugin calls now run on a separate
  thread pool, at most 4 at a time, and the runtime starts at least 2 worker
  threads (`TOKIO_WORKER_THREADS` still sets the number). Plugin calls still
  have no time limit (#301), but deckwatch now stops waiting for one after 2
  minutes and reports `plugin.timeout`; the call may still finish in the
  background. (#385, #395)
- Saving the Settings page no longer drops the trace viewer's settings
  (`tracing.query_url`, `ui_url` and `backend_kind`) from the settings
  database, where the MCP activity page reads its trace links from. A
  `PUT /api/settings` without `tracing` now keeps the stored block. (#401)
- **The chart's probes give deckwatch time to start, and to answer when
  busy.** deckwatch loads every enabled plugin before it serves HTTP, and the
  liveness probe restarted it after about 30 seconds, so slow plugin
  downloads at startup caused a crash loop. A new startup probe on
  `/api/healthz` allows 5 minutes (60 checks, 5 s apart). Kubernetes starts
  the liveness and readiness probes once it passes, so they no longer have an
  initial delay, and they now wait 5 s for an answer instead of Kubernetes'
  default of 1 s. All three are Helm values now (`startupProbe`,
  `livenessProbe` and `readinessProbe`, each a Kubernetes Probe); raise
  `startupProbe.failureThreshold` if loading your plugins takes longer than
  5 minutes. See [README.md](README.md#helm-chart-probes). (#391)
- The API docs (`/api/docs`) and `/api/openapi.yaml` report the running
  deckwatch version. The spec said 0.2.1 since that release. (#384)
- MCP `tools/list` listed every mcp-k8s resource tool twice, and listed
  `list_clusters`, `switch_cluster` and `get_active_cluster`, which only the
  standalone mcp-k8s server serves. (#403)

### Dependencies

- mcp-k8s v0.5.2 → v0.7.0, and kube 5.1.0 → 5.2.0. mcp-k8s 0.7 deprecates
  the Prometheus metrics `mcp_k8s_tool_calls_total` and
  `mcp_k8s_tool_call_duration_seconds`, which deckwatch's `/metrics` still
  exposes; mcp-k8s 0.8 removes them. Dashboards should move to the
  OpenTelemetry metric `mcp.server.operation.duration`. (#403)

## [0.7.0] - 2026-11-25

A security and hardening release. It also moves to kube 5, which requires
Kubernetes 1.33 or newer; makes outbound HTTPS configurable (extra trusted
CAs, and endpoint overrides that stored credentials can't follow); shows
plugin problems where they happen (in API and MCP responses, a Problems view,
Kubernetes Events and notifications); can export deckwatch's own traces and
logs over OTLP; audits every MCP call that changes something or is refused,
with an MCP activity page;
and fixes several plugin lifecycle bugs. Some changes need action before or
after the upgrade: read the upgrade notes first.

### Upgrade notes

Breaking changes, and other changes that may need action, most important
first.

- **Kubernetes 1.33 or newer is required.** deckwatch builds against kube 5
  and k8s-openapi 0.29 with the Kubernetes 1.33 API (`v1_33`), and the chart
  declares `kubeVersion: ">=1.33.0-0"`, so Helm refuses older clusters. The
  embedded mcp-k8s (now v0.5.2) uses the same API types: its tools for alpha
  and beta APIs (DRA, VolumeAttributesClass, ClusterTrustBundle,
  LeaseCandidate, MutatingAdmissionPolicy, StorageVersion,
  StorageVersionMigration) return the API server's 404 on clusters that don't
  enable those API versions, which is the default. See "404 from DRA,
  VolumeAttributesClass or other alpha/beta tools" in
  [docs/MCP.md](docs/MCP.md). (#252)
- **Back up the database before upgrading.** 0.7.0 migrates it on startup:
  `m20261124_000001_gitops_token_binding` adds `gitops_configs.token_binding`
  (#253), `m20261124_000002_plugin_status` adds the `plugin_status` table
  (#263), and `m20261124_000003_audit_log_mcp` adds the `source`, `tool`,
  `outcome` and `client_name` columns and two indexes to `audit_log` (#293).
  deckwatch 0.6.x refuses to start on a migrated database (sea-orm reports
  those migrations as missing), so rolling back means restoring the backup.
- **The audit log is pruned.** It had no retention; now rows older than 365
  days, and all but the newest 100,000 rows, are deleted at startup and then
  hourly. Set `DECKWATCH_AUDIT_RETENTION_DAYS` and `DECKWATCH_AUDIT_MAX_ROWS`
  (Helm `audit.retentionDays` and `audit.maxRows`; `0` turns a limit off)
  before upgrading if you need to keep more. (#293)
- **`GET /api/audit` returns rows newest first** (`order=asc` for oldest
  first), and `next_cursor` is now an opaque timestamp-and-id position. Before,
  rows came in random order (sorted by a random UUID) and the cursor was a row
  id, which now returns `400`. (#295)
- **Cross-site browser requests are rejected.** The permissive CORS layer is
  gone, so pages on other origins can't read API responses. `POST`, `PUT`,
  `PATCH`, `DELETE` and WebSocket upgrades (pod exec, port-forward) get `403`
  unless the browser reports them as same-origin: `Sec-Fetch-Site` must be
  `same-origin` or `none`, and for browsers that don't send it, `Origin` must
  match `Host` or `X-Forwarded-Host`. The UI and non-browser clients (MCP
  clients, curl and scripts, the Kubernetes API server, Git webhooks, docker
  and kaniko) are unaffected. A browser app on another origin, including a
  sibling subdomain, can no longer call the API. (#205)
- **The settings API masks secrets.** `GET /api/settings` returns
  `"configured"` in place of `notifications.webhook_url`,
  `git_token_secrets[].encrypted_token` and every `plugins[].config` value
  except keys that the loaded plugin declares non-secret (`credentials` were
  already masked). `PUT /api/settings` restores the stored value wherever it
  receives `"configured"`, so saving the masked document back no longer wipes
  anything. It never changes `credentials` or token ciphertext (use
  `POST /api/settings/credentials` and `/api/git-token-secrets`), and it
  returns `400` for a masked plugin value with nothing stored under its key,
  such as a renamed key (#207), and for an invalid endpoint or AI region
  (#253). Scripts that read secrets back from the API need another source.
- **Plugin resource and MCP plugin responses changed.** `DELETE` on an
  application's plugin resource
  (`/api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}`)
  returns `200` with `message`, `plugin_events` and
  `kubernetes_resource_deletions` instead of an empty `204`, and a
  `provision()` that fails outright returns `502` with
  `"error": "plugin_failed"` and the plugin's events. When a plugin reported
  anything, MCP `enable_plugin`, `validate_plugin`, `provision_resource`,
  `refresh_resource` and `deprovision_resource` append it after their JSON
  (`Plugin events (N):` and an untrusted-data fence), so clients that parse
  these results as JSON must read only the JSON part (#261). MCP
  `get_events` output is wrapped in the same kind of fence (#265).
- **Re-enter a plugin's secret config values after changing its code.**
  Saving settings with a new `source` or a changed or removed `sha256` pin
  for a plugin returns `400` until its masked config values are entered
  again (or removed). `POST /api/plugins/{name}/upload` returns `200` with
  `{sha256, cleared_config_keys}` instead of `204`: on a plugin without a
  pin it drops the config values that `GET /api/settings` masks, so enter
  them again after uploading; a pinned plugin only accepts the pinned
  binary. (#342)
- **Helm sets `RUST_LOG` only from the new `logFilter` value.** The chart
  used to hard-code `RUST_LOG=deckwatch=info,tower_http=info`. With
  `logFilter` empty, the default, deckwatch's built-in filter applies, and it
  now also keeps mcp-k8s and plugin log lines and OpenTelemetry export
  warnings (see Changed). To keep the old output, set
  `logFilter: "deckwatch=info,tower_http=info"`. If you patched `RUST_LOG`
  onto the Deployment yourself, set `logFilter` instead. (#256, #260)
- **Installs without the Helm chart need RBAC for Events.** deckwatch now
  publishes plugin failures and notices as Kubernetes Events (see Added),
  which needs `create` and `patch` on `events.k8s.io` `events`. The chart's
  ClusterRole grants both; add them to your own RBAC otherwise. Without them
  deckwatch logs a warning for each Event it can't publish, counts it in
  `deckwatch_plugin_k8s_events_total{result="error"}`, and carries on. (#265)
- **Enabled notifications are actually sent now.** deckwatch read the
  notification settings from the old settings ConfigMap, so installs that
  enabled notifications in Settings never received any. It now reads the
  database, so an enabled webhook starts receiving the new `plugin_failing`
  and `plugin_recovered` notifications, unless `event_types` leaves them out
  (an empty list sends every type). The build, deployment, application and
  pod event types can be selected but still aren't sent. (#270)
- **Leftover plugin fields are removed after the upgrade.** When an
  application's last plugin resource is removed, deckwatch now strips the env
  vars, sidecars, init containers, volumes and annotations that the plugin
  applied to the application's workloads. The first reconcile after startup
  also cleans up workloads that earlier versions left in that state, which
  rolls out those Deployments, but only for applications the database knows:
  fields left on workloads of deleted applications, or removed from an
  application, stay until you remove them. Workloads that deckwatch patches
  get a `plugins.deckwatch.io/fields-owner` annotation identifying the
  database, and later releases need it, so a new or reset database, or
  another deckwatch on the same cluster, never strips fields it didn't apply.
  (#229, #345)
- **Outbound requests have timeouts.** Git (GitOps polling, branch lookups
  and manual builds): 60 s per request. AI calls: 10 minutes. Registry probes
  and Entra JWKS: 15 s. These had none before. The Vertex AI and Bedrock
  region must now be a plain region name such as `us-east-1`. (#253)
- **S3 registry storage (object_store 0.14).** Deletes use the S3 bulk
  `DeleteObjects` API; set `AWS_DISABLE_BULK_DELETE=true` for S3-compatible
  stores that don't support it. TLS to S3 moves from OpenSSL to rustls, so an
  endpoint with a private CA needs that CA in `DECKWATCH_EXTRA_CA_FILE` (see
  Added) and a certificate that names the host in its subject alternative
  names. Object keys are unchanged, so existing buckets stay readable.
  (#240, #253)
- **Registry repository names must follow the OCI grammar:** lowercase
  alphanumeric components separated by `.`, `_`, `__` or dashes, joined by
  `/`, at most 255 characters. References must be a tag or a digest. Docker
  and kaniko already enforce this; pushes of other names are now rejected.
  (#185)
- **Plugins can't inherit `DECKWATCH_ENCRYPTION_KEY` or
  `DECKWATCH_DATABASE_URL`** through `inherit_env_keys` or
  `inherit_env_file_keys`; deckwatch logs a warning and leaves them out.
  (#207)
- **Plugins can't inherit static AWS credentials or the OTLP exporter's
  headers either:** `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
  `AWS_SESSION_TOKEN`, `OTEL_EXPORTER_OTLP_HEADERS` and the per-signal
  `OTEL_EXPORTER_OTLP_*_HEADERS` are deckwatch's own. The plugin call reports
  a `plugin.policy_rejected` warning and runs without them. A plugin that
  read AWS keys from deckwatch's environment needs its own credentials: a
  workload identity token through `inherit_env_file_keys` (for example
  `{"AWS_IDENTITY_TOKEN": "AWS_WEB_IDENTITY_TOKEN_FILE"}`), or keys in its
  `config`. (#298)
- **MCP: deckwatch's own tools refuse arguments their schema doesn't allow.**
  An undeclared argument, a missing required one, or a value of the wrong
  type (such as `"replicas": "3"`) used to be ignored or replaced by a
  default; now the call fails with an error naming it. `validate_plugin` no
  longer takes `token_secret` or `allowed_hosts`, so a plugin in a private
  repository can't be dry-run over MCP: configure it disabled and use
  `enable_plugin`. (#298)
- **The UI needs a recent browser:** Chrome or Edge 111, Firefox 114, or
  Safari 16.4 or newer (vite 8's build target; it was Chrome 87, Firefox 78,
  Safari 14). (#237)

### Security

- **Unauthenticated file write and delete through the registry.** `/v2/*`
  accepted `..` in repository names and references (percent-encoded too), so
  anyone who could reach deckwatch could overwrite or delete files as root in
  the container, such as the SQLite database or the UI's files. Names and
  references are now validated (see Upgrade notes), and storage paths must be
  plain components. The S3 backend wasn't exploitable. Pushes and deletes on
  `/v2/*` still don't require authentication. (#185)
- **The plugin upload route could write outside the uploads directory.**
  `POST /api/plugins/{name}/upload` built the file path from the
  percent-decoded `{name}` and wrote the body before checking that the plugin
  exists. The name must now make a plain file name (letters, digits, `.`, `_`
  and `-`, not starting with `.`) and name a configured plugin before
  anything is written; otherwise the route returns `400` or `404`. Plugins
  with an `upload` source whose file name isn't a plain name in the uploads
  directory are no longer read. (#300)
- **Cross-site request forgery and WebSocket hijacking.** Any web page a user
  had open could drive deckwatch through their browser: read API responses
  (CORS allowed every origin, so for example decoded Secrets through
  `POST /mcp`), make state-changing requests, and open pod exec and
  port-forward sockets. See Upgrade notes. This doesn't add authentication:
  anyone who can reach deckwatch directly can still use the API. (#205)
- **Security headers on every response, and registry manifests can't be
  served as web pages.** Every response, pages opened through the pod proxy
  included, now carries `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: SAMEORIGIN`, `Content-Security-Policy: frame-ancestors
  'self'` and `Referrer-Policy: same-origin`: other sites can't frame
  deckwatch, and browsers don't guess a response's type. The registry served
  a pushed manifest with whatever media type the manifest declared, from
  deckwatch's origin. Manifests are now served only as OCI or Docker
  manifest and index media types, anything else as
  `application/octet-stream`, and every `/v2` response carries a sandboxing
  Content-Security-Policy. (#344)
- **Secrets leaked from the public `GET /api/settings`:** plugin credentials
  in `plugins[].config`, notification webhook URLs and git-token ciphertext
  were returned in clear to any caller. MCP `get_plugin_config` returned
  undeclared plugin keys in clear; they are now `***` with
  `is_secret: true`. And plugin config, which anyone who can reach the API
  can edit, could pass deckwatch's encryption key to a plugin through
  `inherit_env_keys`. (#207)
- **Changing a plugin's code no longer hands it the stored secrets.** A
  settings save restored masked plugin values by plugin name and key alone,
  and an upload kept the config, so pointing a plugin at other code (a new
  `source`, or a binary through `POST /api/plugins/{name}/upload`) passed
  that code the values entered for the old one, and its self-declared schema
  then decided which of them `GET /api/settings` and MCP `get_plugin_config`
  showed in clear. Masked values are now restored only while the plugin's
  `source` is unchanged and its `sha256` pin isn't changed or removed;
  otherwise `PUT /api/settings` returns `400` naming the values to enter
  again, and the change is audited. An upload to
  a pinned plugin must match the pin, which now applies to uploaded binaries
  too; an upload to an unpinned plugin drops the masked config values first.
  See Upgrade notes. (#342)
- **MCP `update_deployment` ignored the namespace allowlist for
  `service_account`.** An update that set only `service_account` could point
  a Deployment in any namespace at any ServiceAccount. Only deployments that
  set `DECKWATCH_NAMESPACES` were affected. (#208)
- **MCP `generate_local_build` quotes the command it returns.** The
  `docker run` command that reproduces a GitOps build on a workstation put
  the stored Dockerfile path, build context and OCI repository (written by
  whoever configures GitOps for the deployment), build settings and the
  caller's arguments into the command line unquoted, so a crafted value
  could run commands on the machine that ran it. Every argument is now
  quoted for a POSIX shell (a leading `~/` in `local_context` still
  expands), and the result also lists the arguments as `argv`, for running
  the command without a shell. Ordinary commands look the same as before.
  (#343)
- **Bounded label cardinality on the public `/api/frontend-metrics`.**
  Free-form labels (`route`, `path`, `kind`) take at most 128 bytes of
  visible ASCII and 512 values each; new values after that are recorded as
  `other`. `method`, the status class and web-vital names come from fixed
  sets. Before, a loop of POSTs could grow memory and the `/metrics` output
  without limit. (#208)
- **Prompt-injection fence hardened (SEC-13).** Lines that imitate the
  untrusted-data fence markers become
  `[deckwatch: removed a line that imitated the untrusted-data fence]` in the
  text that AI diagnostics and AI fixes send to a model and in MCP
  `get_pod_logs` output, so pod logs can't close the fence early. The fence
  nonce is random (128 bits; it was derived from the clock), and the prompt
  names the nonce that closes the section. (#257)
- **Plugin and event text is treated as untrusted.** Before plugin events are
  returned, logged or stored, the values of `secret` config fields are masked
  (in plaintext, stored ciphertext, JSON-escaped and percent-encoded form),
  credentials in URLs are redacted, control characters are stripped, and
  messages and event counts are capped. MCP returns plugin output and
  `get_events` output inside the untrusted-data fence, because event notes
  can come from plugins and from any workload allowed to write Events.
  (#261, #265)
- **MCP tool calls are audited (SEC-22).** Only 12 MCP tools wrote audit
  rows; everything that fell through to mcp-k8s (`apply_manifest`,
  `exec_pod`, RBAC, every `delete_*` but deployments, `drain_node`, …) and
  guardrail denials left none. Now every MCP call that writes, deletes or
  execs, and every call a guardrail refuses, writes exactly one `audit_log`
  row (`source` `mcp`) from the MCP dispatch choke point: the tool, the
  action class, the target (kind, namespace, name), the outcome (`ok`,
  `error`, `denied`, or `cancelled` when the client went away mid-call), the
  argument keys and only the values mcp-k8s's allowlist permits, and the
  client's self-reported name and version. Reads that go through aren't
  audited, and argument values such as Secret data, env values, commands,
  manifests and plugin config are never stored. Audit writes never delay or
  fail a call. `user_identity` stays empty until MCP clients authenticate.
  (#293)
- **MCP calls outside the namespace allowlist are refused up front.** A call
  whose target namespace (its `namespace` argument, or an `apply_manifest`
  manifest's) is outside `DECKWATCH_NAMESPACES` is refused before it runs and
  audited as denied (`denied_rule` `namespace_allowlist`), reads included.
  Every tool already refused these; this records them and covers any tool
  that missed the check. mcp-k8s's permission settings are checked the same
  way. (#293)
- **MCP `validate_plugin` no longer runs caller-supplied code with
  deckwatch's credentials.** It accepted arguments its schema didn't
  declare, so the WASM it dry-runs could be given deckwatch's environment
  variables and identity-token files, a stored git token and network access,
  and its result included the env var values the plugin returned. The dry
  run now gets only the caller's `config` and test context: no git token, no
  network access and nothing from deckwatch's environment or files.
  `token_secret`, `allowed_hosts`, `inherit_env_keys`,
  `inherit_env_file_keys` and `upload` sources are refused with an error
  naming them, and the result lists env var names and Secret or ConfigMap
  references, never values. Every call, refused ones included, is audited as
  `exec`. (#298)
- **deckwatch's own MCP tools enforce their input schemas.** Each call is
  checked against the tool's `inputSchema` before it runs, and undeclared
  arguments are refused (see Upgrade notes), so a handler can no longer act
  on an argument that clients are never told about. The mcp-k8s tools,
  including the four deckwatch replaces, aren't checked this way yet. (#298)
- **More of deckwatch's own secrets are kept from plugins.**
  `inherit_env_keys` and `inherit_env_file_keys` also refuse static AWS
  credentials and the OTLP exporter's headers (see Upgrade notes). This
  remains a denylist; an operator-set allowlist is planned. (#298)
- **Stored credentials can't be redirected:** they are only sent to the
  endpoint they were saved for (see Added), and a crafted Vertex AI or
  Bedrock region can no longer redirect the access token or a signed request.
  (#253)
- **Credentials in URLs are kept out of the logs.** deckwatch logged the
  database URL on every start, password included, and with OTLP log export
  (see Added) log lines also leave the cluster. That URL, the S3 registry
  endpoint, GitOps repository URLs, and the URLs in outbound error messages
  and plugin events are now logged with their userinfo, query values and
  fragment masked (`***`): a token in a plugin source URL's query string is
  masked too, not only `user:password@`. Notification webhook errors no
  longer include the webhook URL. If an earlier version logged a database
  URL that holds a password, rotate that password. (#299)
- **Plugin source URLs are redacted in the missing-checksum warning and in
  plugin download errors**, which logged the URL as configured. (#349)
- **Dependency advisories.** extism 1.41 with wasmtime 49 fixes
  GHSA-mghf-wjfp-p5xm / RUSTSEC-2026-0379 (critical: an aarch64 Cranelift
  miscompile), RUSTSEC-2026-0269 (filesystem sandbox escape),
  RUSTSEC-2026-0327 (native stack buffer overflow) and about ten denial of
  service and soundness issues, all reachable because plugins are untrusted
  WASM run inside deckwatch. Also jsonwebtoken 12 (GHSA-h395-gr6q-cpjc, type
  confusion), rustls 0.23.46 (seven advisories), rustls-webpki 0.103.17 and
  webpki-roots 1.0.12, and h2, tokio, url and zstd fixes (#183). `zip`
  (three advisories, one critical; it was build-time only) and the
  unmaintained `rustls-pemfile` are no longer in the build (#236). The UI
  moves to vue 3.5.46 (a `@vue/server-renderer` XSS), and advisories in the
  frontend build and test tooling are fixed (#183, #233).
- **AES-GCM nonces come straight from the OS RNG** (rand 0.10 dropped the
  fork protection of `ThreadRng`), and a failing RNG returns an error instead
  of panicking. The stored ciphertext format is unchanged. (#216)
- **Releases are published only from `main`.** The release workflow now also
  refuses a tag whose commit isn't on `main` (the tip of `main` or one of its
  ancestors), before it builds anything, so a tag on any other branch
  publishes no image and no GitHub Release. (#297)

### Added

- **Extra trusted CAs for outbound HTTPS:** `DECKWATCH_EXTRA_CA_FILE` (a PEM
  bundle), or Helm `outbound.extraCaBundle` (`secretName` or `configMapName`,
  plus `key`). The CAs are added to the system roots for every outbound call,
  including the S3 registry backend and AWS credential providers. An
  unreadable or invalid bundle stops startup with an error that names the
  file. See [docs/TLS.md](docs/TLS.md). (#253)
- **Endpoint overrides, with credential binding.** New `endpoints` settings
  for the Anthropic API, the Vertex AI token URL and API base, Bedrock, and
  the GitHub API, web and raw bases (for GitHub Enterprise Server), in
  Settings → Advanced → Endpoints. Values must be `https://` without
  credentials, query or fragment; `http://` is accepted for localhost, or for
  any host when `DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true` (Helm
  `outbound.allowInsecureEndpoints`, for development and tests). Changes are
  audit-logged. Each stored credential records the endpoint origins it was
  saved for and is never sent anywhere else, so after changing an endpoint,
  re-enter the credential. Existing credentials are bound to the default
  endpoints, and credentials read from Kubernetes Secrets only go to the
  defaults. `GET /api/settings/outbound` and Settings → Advanced show the
  defaults, the effective values, the CA bundle and where each credential may
  be sent, and the AI provider cards show which host a key was saved for.
  (#253, #254)
- **Amazon Bedrock works.** `InvokeModel` requests are signed with SigV4
  using the pod's AWS identity (IRSA, EKS Pod Identity or environment
  credentials) and are only sent to AWS hosts. Responses don't stream: the
  reply arrives as one chunk. Not yet tested against real AWS. (#253, #254)
- **Helm `logFilter`**, passed to deckwatch as `RUST_LOG`. (#256)
- **One log line per MCP tool call:** `tool call finished` (target
  `mcp_k8s::telemetry`), with the tool, outcome, `duration_ms`, action
  class, decision and target. Requests that reach no tool log
  `request finished`. Argument values beyond mcp-k8s's allowlist and error
  text are never logged. (#256, #266)
- **MCP telemetry.** Every `/mcp` request is one OpenTelemetry server span
  following the MCP conventions, one `mcp.server.operation.duration`
  observation and one log line. This covers deckwatch's own tools and the
  mcp-k8s ones; intercepted tools still count once. Requests join the
  caller's trace (`params._meta.traceparent`, or the `traceparent` header)
  when OTLP export is on. (#266)
- **MCP sessions.** `initialize` returns an `Mcp-Session-Id`. Later requests
  that send it back are labelled with the client's self-reported name and
  version, on spans only and never on metrics. A missing, unknown or
  expired id is never refused. (#266)
- **MCP activity page** (MCP in the top bar, `/mcp-activity`) for the last
  hour, 24 hours or 7 days: every MCP change and denial from the audit log,
  filterable by action, outcome and client (shown as what the client says
  about itself, never as an identity), with a link to the trace when the row
  has a trace ID and a tracing UI is set in Settings; and, from Prometheus,
  tool calls per minute, the share that failed or were denied, the p95
  duration and `apply_manifest` calls over time, and calls, errors, denials
  and p95 per tool. Without Prometheus the page says so and shows the audit
  rows only. (#295)
- **Audit log retention:** `DECKWATCH_AUDIT_RETENTION_DAYS` (default 365) and
  `DECKWATCH_AUDIT_MAX_ROWS` (default 100,000), Helm `audit.retentionDays` and
  `audit.maxRows`; `0` turns a limit off. Deployment configuration rather
  than a setting, so the unauthenticated settings API can't shorten the audit
  trail. See Upgrade notes. (#293)
- **`GET /api/audit` filters and fields.** Entries carry `source` (`api` or
  `mcp`), `tool`, `outcome` and `client_name` (#293), and the endpoint filters
  by `source`, `action`, `tool`, `client`, `outcome` and `since` (#295). MCP
  rows' `detail` is JSON; the OpenAPI spec documents it. (#293, #295)
- **`GET /api/prometheus/mcp_activity?window=1h|24h|7d`** serves the MCP
  activity page's metrics through fixed queries over deckwatch's MCP counters
  and duration histograms, from the Prometheus at `PROMETHEUS_URL`. Like the
  bundled Grafana dashboard, it sums every deckwatch instance that
  Prometheus scrapes. (#295)
- **MCP `get_resource_json`** (from mcp-k8s v0.4.0) returns any resource as
  JSON. `get_resource_yaml`, which always returned JSON too, remains as a
  deprecated alias. (#252)
- **Loaded plugin binaries are identifiable.** `GET /api/plugins` adds
  `sha256` and `loaded_at`. MCP `list_plugins` and `get_plugin_config` add
  `version`, `sha256`, `wasm_size_bytes` and `loaded_at`. Settings → Plugins
  shows the SHA-256, uploads record the file's SHA-256 in the log and the
  audit log, and the OpenAPI spec now covers `/api/plugins*`. (#206)
- **`key_value` plugin config fields** (deckwatch-plugin-sdk v0.10.0), stored
  as a JSON object of strings. `POST /api/plugins/{name}/config` and MCP
  `update_plugin_config` validate them before saving anything, and the UI
  edits them as JSON text for now. MCP `update_plugin_config` also accepts an
  object for them, and `null` removes a key. Plugins that use this type need
  deckwatch 0.7.0: older releases can't parse their metadata and drop all
  their config fields and resources. (#243)
- **A plugin's `deprovision()` can ask deckwatch to delete Kubernetes
  objects** (`delete_kubernetes_resources`, deckwatch-plugin-sdk v0.10.0):
  `v1` ServiceAccounts and `batch/v1` Jobs, in the application's namespace
  only. Jobs are deleted first, with background propagation. A ServiceAccount
  that a Deployment, CronJob or Job still uses is kept, and so is `default`.
  Every request is audit-logged; refusals and failures are counted in
  `deckwatch_plugin_errors_total` (`k8s_delete_refused`, `k8s_delete_failed`)
  and reported by MCP `deprovision_resource` in
  `kubernetes_resource_deletions`. (#235)
- **Plugin events reach the person or agent who triggered the call.**
  Deployment create and update, and provision, refresh and deprovision,
  return what the plugin reported in `plugin_events`, including warnings on
  success; the MCP plugin tools return it fenced; and the UI shows it on the
  deployment pages, the Infrastructure tab and the provision dialog. Every
  plugin call now goes through one path that opens a `plugin <export>` span,
  records the plugin metrics, and turns structured `events`
  (deckwatch-plugin-sdk v0.10.0), the legacy `errors`, `warnings` and
  `message`, and host failures (`plugin.trap`, `plugin.timeout`,
  `plugin.load_failed`, `plugin.output_invalid`, `plugin.fetch_failed`, …)
  into one list. New metrics: `deckwatch_plugin_events_total` and
  `deckwatch_plugin_events_dropped_total`. (#261)
- **Plugin status and a Problems view.** deckwatch keeps the current health
  (`healthy`, `degraded` or `failing`) and latest events of every plugin
  target (each provisioned resource, each plugin's `apply()` per
  application, and each plugin's load) in a new `plugin_status` table. It is
  served by `GET /api/plugin-status` (filters: `health`, `plugin`,
  `namespace`), `GET /api/namespaces/{ns}/applications/{name}/plugin-status`
  and `GET /api/plugins/{name}/status`. The UI shows health badges on
  applications, plugins and resources, and a Problems view
  (`/plugin-problems`), with a Problems button in the top bar while anything
  is failing or degraded. (#263)
- **Plugin failures and notices as Kubernetes Events** on the affected
  Deployments, so they show in `kubectl describe deployment`, deckwatch's
  event views and MCP `get_events`. They are published through kube-rs's
  `Recorder` (events.k8s.io/v1, reporting controller `deckwatch`) with the
  reasons `PluginApplyFailed`, `PluginApplyWarning`, `PluginProvisionFailed`,
  `PluginProvisionWarning`, `PluginProvisioned`, `PluginDeprovisionFailed`,
  `PluginDeprovisioned`, `PluginRecovered` and `PluginFieldsReleased`. The
  same target, reason and action is published at most every 5 minutes, and
  repeats fold into one Event's series count. Publishing never blocks or
  fails the call; `deckwatch_plugin_k8s_events_total{result}` counts
  `published`, `folded`, `dropped` and `error`. Needs new RBAC (see Upgrade
  notes). (#265)
- **`plugin_failing` and `plugin_recovered` notifications**, sent once when a
  plugin target starts failing and once when it recovers, never on every
  reconcile. A target that fails again within 10 minutes of its last
  `plugin_failing` sends nothing, and neither does its next recovery. Plugin
  text is escaped for Slack, so it can't become a mention or a link, and a
  plugin that fails to load ignores the `namespaces` filter. (#270)
- **deckwatch's own telemetry over OTLP/HTTP.** Off unless
  `OTEL_EXPORTER_OTLP_ENDPOINT` or a per-signal endpoint is set. It exports
  traces (a span per HTTP request) and log records that carry their trace
  and span IDs, including MCP tool calls, mcp-k8s and plugin log lines; the
  metrics pipeline is installed but has nothing to export yet, and
  Prometheus `/metrics` is unchanged. Only http/protobuf is supported. It
  reads the standard `OTEL_*` variables at startup, sends through the
  outbound HTTP client (so `DECKWATCH_EXTRA_CA_FILE` applies; the
  `OTEL_EXPORTER_OTLP_*CERTIFICATE`/`*KEY` variables are ignored with a
  warning), and doesn't fall back to the `tracing.otlp_endpoint` setting.
  Helm: `telemetry.otlp.*`; with `telemetry.otlp.enabled`, `tracing.enabled`
  and no endpoint, traces go to the bundled Tempo or Jaeger on port 4318.
  See [docs/TRACING.md](docs/TRACING.md). (#260)
- **Plugins receive `trace_context`**, a W3C `traceparent` for the plugin
  call's span, while deckwatch exports traces, so a plugin can pass it on in
  its own requests (deckwatch-plugin-sdk v0.10.0's `trace_headers()`).
  Plugin events and status rows then carry the trace ID. (#270)
- `User-Agent: deckwatch/<version>` on every outbound request, and outbound
  errors that include the TLS reason (unknown issuer, expired certificate,
  name mismatch). (#253)

### Changed

- **MCP metrics count each call once.** `deckwatch_mcp_tool_calls_total` no
  longer counts intercepted tools that deckwatch hands on to mcp-k8s
  (`apply_manifest`, and `create_pod`/`update_deployment` when they
  delegate), and `create_deployment`, `delete_deployment`,
  `scale_deployment` and `restart_deployment` aren't intercepted at all any
  more (see below). `mcp_k8s_tool_calls_total` counts those, so dashboards
  that add the two no longer count them twice. Calls a guardrail refuses
  before they reach mcp-k8s are counted in `deckwatch_mcp_tool_calls_total`
  with `status="denied"`. `deckwatch_mcp_requests_total` labels methods
  outside the MCP spec `_OTHER` instead of using the raw, client-chosen
  method. (#266, #293)
- **MCP `create_deployment`, `delete_deployment`, `scale_deployment` and
  `restart_deployment` go straight to mcp-k8s.** deckwatch intercepted them
  only to write audit rows, which the MCP choke point now writes for every
  call. Their audit rows use the action classes `write` and `delete` (they
  were `create`, `delete`, `scale` and `restart`), and are written for
  failed calls too. (#293)
- **One audit row per MCP call.** A tool that reuses a REST handler
  (`set_gitops`, `trigger_build`, `create_ingress`, `update_ingress`,
  `deprovision_resource`, …) wrote the handler's row too, or only that one;
  now the MCP row is the only one. The row for MCP `update_deployment`
  records that `service_account` was set, not its value, which isn't on
  mcp-k8s's allowlist. (#293)
- **mcp-k8s and plugin log lines are kept.** The default log filter is now
  `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=info,opentelemetry=warn`;
  a valid `RUST_LOG` still replaces it. `opentelemetry=warn` shows failed
  OTLP exports (#260). `extism::pdk` carries plugin log lines, with a
  `plugin` field. deckwatch-plugin-aws, for example, logs a few lines per
  reconcile for each deployment with AWS resources, roughly 1–2k lines an
  hour each. To keep only plugin warnings, set `logFilter` to
  `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=warn`. (#256)
- **Outbound HTTP clients are built once at startup**, one per purpose, with
  the timeouts listed in [docs/TLS.md](docs/TLS.md). Proxies still come from
  `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`. (#253)
- **GitOps path filtering** treats a repository as GitHub-hosted only when
  its host is exactly the configured GitHub web host (it was a substring
  match on `github.com`), and no longer sends an empty `Bearer` header for
  public repositories. (#253)
- A `PUT /api/settings` without `endpoints` keeps the stored endpoints.
  (#253)
- **mcp-k8s is pinned to the v0.5.2 tag** (it followed `main`) and is built
  without its standalone server's dependencies. Tool names, parameters and
  outputs are unchanged, apart from the API versions in the upgrade notes.
  (#236, #252, #259, #266)
- **MCP `enable_plugin` loads the plugin before it returns** and reports
  fetch, checksum, metadata or version problems; it reloads only that plugin
  instead of all of them. (#261)
- **Plugin logs and metrics:** the reconciler's per-call
  `calling plugin apply()` and `apply() completed` lines are now `debug`;
  plugin events are logged as rate-limited `plugin event:` lines; and
  `deckwatch_plugin_k8s_resources_total` records kinds outside a fixed list
  as `other`, so a plugin can't create label values. (#261)
- **The UI is built with vite 8** (Rolldown and Lightning CSS), which splits
  it into smaller chunks that the page preloads. (#237)
- **web-vitals 6** measures LCP, INP and CLS slightly differently (LCP is
  final only after trusted user input), so expect a small step in the
  `deckwatch_frontend_web_vital` histograms. (#217)
- **S3 registry backend (object_store 0.14):** EKS Pod Identity is
  supported, and retries also cover HTTP 408 and 429. (#240)
- **[docs/TRACING.md](docs/TRACING.md) is rewritten** around
  deckwatch-plugin-otel, the way tracing works today. It notes that the trace
  query proxy reads `tracing` from the settings ConfigMap and works only with
  Jaeger. (#232)

### Fixed

- Saving the Settings page no longer wipes the stored Anthropic and GCP
  credentials, and saving plugin settings no longer overwrites stored secrets
  with `"configured"`. (#207)
- Uploading a plugin no longer unloads every other plugin until the next
  reload, and `deckwatch_plugins_loaded` reports the real count. (#209)
- **Removed and disabled plugins stop running.** Removing the last plugin in
  Settings left it loaded, still running on every reconcile and deployment
  change, until deckwatch restarted; now a settings save unloads every plugin
  it removes or disables before it responds. And a background reload that
  finished after a newer change (another save, MCP `disable_plugin`) put its
  older result back, reviving a disabled plugin or an old config; a reload
  whose plugin settings changed while it ran is now dropped. (#347)
- Removing an application's last plugin resource releases what the plugin
  applied to its Deployments and CronJobs within one reconcile (10 s). Before,
  those fields stayed, and plugins kept acting on them: deckwatch-plugin-aws
  re-created an empty SQS queue from leftover `sqs.deckwatch.io/*`
  annotations. (#229)
- **A new or reset database no longer strips plugin fields from the
  cluster.** The first reconcile after startup released the plugin env vars,
  sidecars and init containers from every workload in scope whose
  application had no plugin resources in the database, which on an empty
  database (a reinstall, a new `DATABASE_URL`, a restored backup) meant every
  plugin-managed Deployment and CronJob, rolling them out broken. deckwatch
  now releases fields only with evidence that this database applied them:
  the `plugins.deckwatch.io/fields-owner` annotation the reconciler adds, or,
  for fields from before it, an application this database knows. And an
  application whose remaining resources add no env vars, sidecars or init
  containers is still reconciled, so the fields of a removed resource go and
  annotation-only resources are applied. A database restored from an older
  backup still releases the fields of resources it has no record of.
  (#345)
- A plugin that declares a config field type deckwatch doesn't know no longer
  loses all its config fields and resources; the field renders as text.
  (#243)
- Settings → MCP Tuning shows each plugin's tuning fields: `GET /api/plugins`
  now returns `mcp_tuning_fields`. (#226)
- Settings → Audit Log showed no rows: the page read a field the API doesn't
  return. It now lists the rows newest first, with their source (UI/API, or
  MCP and the tool). (#295)
- MCP `list_ipaddresses`, `get_ipaddress`, `list_servicecidrs` and
  `get_servicecidr` use GA `networking.k8s.io/v1`, which every supported
  cluster serves (they used `v1beta1`). (#252)
- The GitOps watcher no longer stops for good when one of its steps panics.
  A multi-byte character in the wrong place in a git server's ref list, or in
  more than 64 KiB of build output, panicked the background task that polls
  repositories, monitors builds and reconciles plugins. It then stayed
  stopped until deckwatch restarted, while the API kept serving and the
  health checks kept passing. Ref lists, build logs and commit SHAs are now
  cut on character boundaries, a branch is found by its exact ref name (a
  branch whose name merely contains `refs/heads/<branch>` is no longer
  mistaken for it), and a panic in any watcher step is logged as an error
  and skips only that step for the cycle. (#303)
- The OpenAPI spec (`/api/docs`) no longer lists the removed addon
  endpoints. (#214)
- With `tracing.enabled`, the chart's deckwatch, registry and webhook
  Services also selected the bundled Tempo or Jaeger pod, so some traffic
  went to it. The deckwatch pod now carries `app.kubernetes.io/component:
  server`, and the Services select on it. (#262)
- Notifications work again, including the Test button in Settings →
  Notifications, which reported success while nothing was sent: the
  notification client read the old settings ConfigMap instead of the
  database (see Upgrade notes). (#270)
- The 0.7.0 database migrations can run again after a crash. On SQLite and
  MySQL a migration and its record in `seaql_migrations` aren't applied in
  one transaction, so a deckwatch that stopped part-way through the upgrade
  (killed, OOM, node lost) left columns or indexes behind and then failed
  to start ("duplicate column name"). Each of the three migrations now adds
  a column or index only when it's missing, and sets the default token
  binding only where none is stored. PostgreSQL already applied them in one
  transaction. Rolling back to 0.6.x still means restoring the backup (see
  Upgrade notes). (#346)
- `plugin_failing` and `plugin_recovered` notifications reach the webhook in
  the order the transitions happened. Each was sent from its own task, so a
  receiver could get a target's recovery before its failure. They now go
  through one queue and are sent one at a time; when 64 are already waiting,
  a new one is dropped and counted in
  `deckwatch_plugin_events_dropped_total{reason="notify_queue_full"}`. (#296)
- deckwatch's event views show the time, count and source of events.k8s.io
  Events (from `eventTime`, `series` and `reportingComponent`), so Events
  from deckwatch and other modern controllers no longer sort to the bottom
  without a time. (#265)

### Dependencies

- **Rust:** kube 4 → 5.1 and k8s-openapi 0.28 → 0.29 (#252), with kube's
  `runtime` feature (#265); mcp-k8s v0.5.2 (#236, #252, #259, #266); OpenTelemetry
  0.33 and tracing-opentelemetry 0.34 (#260); extism 1.30 → 1.41 and
  jsonwebtoken 9 → 12 (#183); rand 0.8 → 0.10, aes-gcm 0.10 → 0.11 and
  ed25519-dalek 2 → 3 (#216); hmac 0.12 → 0.13 and sha2 0.10 → 0.11 (#227);
  object_store 0.11 → 0.14 (#240); rcgen 0.13 → 0.14 (#238); tower-http 0.6
  → 0.7 (#197); jiff 0.2 → 1.0 (#202, #221); metrics-exporter-prometheus
  0.17 → 0.18 (#222); base64 0.22 → 0.23 (#224). Known-answer tests pin
  the credential ciphertext format, license signature checks, webhook HMACs,
  OCI digests and plugin checksums across these upgrades, so stored
  credentials still decrypt (#216, #227), and the admission webhook's
  certificates have the same contents (#238).
- **Frontend:** pinia 3 → 4, with `@vue/devtools-api` 8 as a direct
  dependency, and web-vitals 4 → 6 (#217); vite 6 → 8, vitest 2 → 5 and
  happy-dom 15 → 21 (#183, #237); `@vitejs/plugin-vue` 5 → 6 (#219);
  TypeScript 5.7 → 6.0 (#231).
- **Image:** built with `rust:1.99-bookworm` (was 1.91, #186) and
  `node:26-alpine` (was 24, #231), and a pinned, checksum-verified mdBook
  (#258). The runtime base image, `gcr.io/distroless/cc-debian12`, is
  unchanged.

### Internal and CI

- CI runs `cargo test` and `cargo clippy --all-targets` (#228), the frontend
  unit tests (#213) and a type check of the test code; frontend unit tests
  fail on unmocked network requests (#218). Tests that had been failing are
  fixed (#210, #211, #212).
- The Rust toolchain is pinned to 1.99 in `rust-toolchain.toml`, and CI
  fails when the Dockerfile's `rust:` image drifts from it (#215).
- CI also runs on every push to `main`, with a read-only token (#230). A
  Docker Build job builds the release image (amd64, not pushed) on every push
  to `main`, and on PRs only when image-relevant files change; a Docs Build
  job builds the manual the way the image does (#230, #258). Rust caches are
  saved only from `main` (#258), and CI compiles without debug info (#234).
- Dependabot for cargo, npm, Docker and GitHub Actions (#184), and GitHub
  Actions updated to their current major versions (#187).
- The crate is a library plus a thin binary, so tests in `tests/` can call
  real code; `tests/outbound_tls.rs` checks every outbound call against local
  TLS mocks. (#253)
- Stale references to the removed addon system are gone from `CLAUDE.md` and
  the docs. (#214)
- Tests drive real extism plugins, built from WebAssembly text, through every
  plugin call path. (#261)
- **Releases:** this changelog. The release workflow refuses a tag that
  doesn't match the versions and a dated section here, before it builds
  anything, and then creates the GitHub Release from that section. CI's
  Release Check job runs the same check on every PR. (#264)

## Before 0.7.0

Releases up to 0.6.7 have no changelog. See their
[tags](https://github.com/alexconrey/deckwatch/tags) and, for some of them,
[GitHub Releases](https://github.com/alexconrey/deckwatch/releases).

[Unreleased]: https://github.com/alexconrey/deckwatch/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/alexconrey/deckwatch/compare/v0.6.7...v0.7.0
