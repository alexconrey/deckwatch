# MCP Integration — Claude Code + Deckwatch

Deckwatch exposes an [MCP (Model Context Protocol)](https://modelcontextprotocol.io)
server that lets AI coding assistants like Claude Code query your Kubernetes
cluster directly. Connect once, then ask natural-language questions about
deployments, pods, logs, events, and builds — all from your terminal.

## Why MCP?

When you debug a production issue, you typically need two things:

1. **Cluster context** — pod logs, events, deployment status, build history
2. **Source code context** — the actual code causing the issue

Claude Code already has access to your local filesystem. By adding deckwatch
as an MCP server, Claude Code gains access to your cluster context too. It
can correlate a `CrashLoopBackOff` event with the actual code that changed,
read the build logs to understand what was deployed, and suggest fixes that
reference both the error and the source.

## Quick Start

### 1. Register the MCP server

For a local deckwatch instance (k3d, kind, minikube):

```bash
claude mcp add --transport http deckwatch-local http://localhost:8080/mcp
```

For a remote deckwatch instance (on EKS, for example):

```bash
claude mcp add --transport http deckwatch https://deckwatch.your-domain.com/mcp
```

You can register multiple deckwatch instances — one per cluster:

```bash
claude mcp add --transport http deckwatch-staging https://deckwatch.staging.example.com/mcp
claude mcp add --transport http deckwatch-prod https://deckwatch.prod.example.com/mcp
```

### 2. Verify the connection

```bash
claude mcp list
```

You should see your deckwatch server listed with its tools.

### 3. Start using it

In any Claude Code session, ask questions that require cluster context:

```
> why is my-app in the default namespace crashing?

> show me the last 50 lines of logs for pod nginx-abc123 in staging

> what deployments are running in the production namespace?

> show me the gitops build history for hello-world

> what events happened in the default namespace in the last hour?
```

Claude Code will automatically call the appropriate deckwatch MCP tools to
fetch the information, then combine it with your local source code to provide
a complete diagnosis.

## Available Tools

Deckwatch exposes the following MCP tools:

### Cluster Discovery

| Tool | Description | Parameters |
|------|-------------|------------|
| `list_namespaces` | List all namespaces visible to deckwatch | — |
| `list_deployments` | List deployments in a namespace | `namespace` |
| `list_ingresses` | List ingresses with hosts, classes, and addresses | `namespace` |

### Deployment Details

| Tool | Description | Parameters |
|------|-------------|------------|
| `get_deployment` | Full deployment detail — status, replicas, conditions, probes, pods, and ingresses | `namespace`, `name` |
| `get_deployment_history` | ReplicaSet revision history with image/config diffs | `namespace`, `name` |
| `get_events` | Recent Kubernetes events, optionally filtered by resource | `namespace`, `resource_name` (optional) |

### Logs

| Tool | Description | Parameters |
|------|-------------|------------|
| `get_pod_logs` | Fetch pod log history | `namespace`, `pod_name`, `tail_lines` (optional), `container` (optional) |
| `get_build_logs` | Fetch logs from a GitOps build Job | `namespace`, `job_name` |

### GitOps & Metrics

| Tool | Description | Parameters |
|------|-------------|------------|
| `get_gitops_status` | GitOps config, last commit SHA, build status | `namespace`, `name` |
| `get_metrics` | Pod CPU and memory usage from metrics-server | `namespace`, `label_selector` (optional) |

### Catalog & Management

| Tool | Description | Parameters |
|------|-------------|------------|
| `create_application` | Create a new deckwatch application with optional seed deployment | `namespace`, `name`, `description` (optional), `template_id` (optional), `create_deployment` (optional, default true) |
| `list_plugins` | List configured plugins with enabled/loaded state, source, and the loaded binary's version and SHA-256 | — |
| `validate_plugin` | Dry-run a plugin from a public GitHub or URL source: checks that it loads, and reports the env var names (never values), sidecars and Kubernetes objects its `apply()` would add. It runs with no git token, no network access and nothing from deckwatch's environment or files | `source`, `config` (optional), `test_namespace`, `test_deployment`, `test_annotations`, `test_labels` (optional) |
| `provision_resource` | Provision a plugin-declared resource for an application (e.g. a Redis or PostgreSQL sidecar); its env vars and sidecars are injected into every deployment in the application | `namespace`, `application_name`, `plugin_name`, `resource_id`, `fields` (optional) |
| `list_application_resources` | List an application's provisioned plugin resources: input fields, state (injected env vars), annotations, and sidecars | `namespace`, `application_name` |
| `list_templates` | List deployment templates (web-app, worker, cron-job, static-site, custom) with payloads | — |
| `configure_gitops` | Enable GitOps for a deployment — polls a git repo, builds images with Kaniko, auto-deploys | `namespace`, `deployment_name`, `repo_url`, `oci_repository`, `branch` (optional), `dockerfile_path` (optional), `docker_context` (optional), `token_secret` (optional), `poll_interval_seconds` (optional) |
| `create_ingress` | Create an Ingress resource (auto-creates backing Service if missing) | `namespace`, `name`, `service_name`, `host` (optional), `service_port` (optional, default 80), `path` (optional, default /), `ingress_class` (optional), `annotations` (optional) |
| `update_ingress` | Update an existing Ingress resource | `namespace`, `name`, `service_name`, `host` (optional), `service_port` (optional), `path` (optional), `ingress_class` (optional), `annotations` (optional) |
| `create_service` | Create a ClusterIP Service with app label selector | `namespace`, `name`, `port` (optional, default 80), `target_port` (optional) |

## Example Workflows

### Diagnosing a crashing deployment

```
> the hello-world deployment in default is showing CrashLoopBackOff. what's wrong?
```

Claude Code will:
1. Call `get_deployment` to see the deployment status and conditions
2. Call `get_pod_logs` to read the crash logs
3. Call `get_events` to see recent events (image pull failures, OOM kills, etc.)
4. Read your local source code to correlate the error with recent changes
5. Suggest a fix with full context

### Investigating a failed GitOps build

```
> the last gitops build for api-server in staging failed. what happened?
```

Claude Code will:
1. Call `get_gitops_status` to see the build status and error message
2. Call `get_build_logs` to read the Kaniko build output
3. Read your local `Dockerfile` and source code
4. Identify the build failure (missing dependency, syntax error, etc.)

### Reviewing deployment history before rollback

```
> show me what changed between the last two revisions of web-frontend in production
```

Claude Code will:
1. Call `get_deployment_history` to list revisions with diffs
2. Show the image, env, and config changes between revisions
3. Help you decide whether to roll back

### Checking resource usage

```
> are any pods in the staging namespace close to their memory limits?
```

Claude Code will:
1. Call `get_metrics` to fetch CPU/memory usage
2. Call `list_deployments` to get the configured limits
3. Compare usage against limits and flag any at-risk pods

### Creating an application from Claude Code

```
> create a new web app called "api-gateway" in the staging namespace
```

Claude Code will:
1. Call `list_templates` to see available templates
2. Call `create_application` with the chosen template
3. Confirm the application was created with its seed deployment

### Adding a sidecar to an application

```
> add a redis cache to the checkout application in staging
```

Claude Code will:
1. Call `list_plugins` to check that a Redis plugin (built from [deckwatch-plugin-redis](https://github.com/alexconrey/deckwatch-plugin-redis)) is configured and loaded
2. Call `provision_resource` with that plugin's name and the resource ID it declares (`cache` for deckwatch-plugin-redis)
3. Call `list_application_resources` to confirm the resource and the `REDIS_URL` env var it injects

Resource IDs come from each plugin's metadata; `GET /api/plugins` lists them
under `resources[].id`. The available sidecar plugins are listed in
[Deployments](DEPLOYMENTS.md#sidecar-containers-plugins).

### Setting up GitOps for a deployment

```
> enable gitops for my-api in staging, watch https://github.com/org/my-api on the main branch, push images to ghcr.io/org/my-api
```

Claude Code will:
1. Call `configure_gitops` with the repo URL, branch, and OCI repository
2. Confirm the GitOps pipeline is configured
3. Deckwatch will begin polling for new commits and building images automatically

## Configuration

### Authentication

The MCP endpoint is currently unauthenticated — it relies on network-level
access control (the same as the deckwatch UI). If deckwatch has Entra
authentication enabled, MCP clients will need to provide a bearer token.

### Namespace Scoping

The MCP server respects deckwatch's namespace restrictions. If deckwatch is
configured with `DECKWATCH_NAMESPACES=staging,production`, the MCP tools
will only return data from those namespaces. With an allowlist, the tools
also refuse anything that would reach other namespaces:

- `create_namespace`, `update_namespace` and `delete_namespace` refuse
  namespaces outside the list;
- `watch_resource` and `list_custom_resources` on a namespaced kind, and
  `list_csistoragecapacities`, refuse to run without a `namespace` (it would
  mean every namespace);
- `drain_node` refuses, before cordoning, when it would evict pods outside
  the list;
- `can_i` and `list_my_permissions` refuse other namespaces;
- `get_custom_resource`, `update_custom_resource` and
  `delete_custom_resource` require `namespace` for namespaced kinds.

### Multiple Clusters

Register one MCP server per deckwatch instance. Use descriptive names to
distinguish them:

```bash
claude mcp add --transport http dw-dev http://localhost:8080/mcp
claude mcp add --transport http dw-staging https://deckwatch.staging.example.com/mcp
claude mcp add --transport http dw-prod https://deckwatch.prod.example.com/mcp
```

When asking Claude Code a question, specify which cluster you mean:

```
> use dw-staging to check if api-server is healthy
```

### Removing a Server

```bash
claude mcp remove deckwatch-local
```

## Guardrails

The Kubernetes tools come from the embedded
[mcp-k8s](https://github.com/alexconrey/mcp-k8s) library (v0.7.0), which
checks what agents create, keeps Secret values back, and refuses privileged
certificate approvals. Deckwatch turns these guardrails on, but keeps
`apply_manifest` available. An admin changes them in **Settings → MCP Tuning
→ Kubernetes tool guardrails**; they are stored as `mcp_tuning.guardrails`
in the settings (`GET`/`PUT /api/settings`). `tools/list` and every
`tools/call` read them, so a change applies from the next request, without a
restart. MCP clients pick up a changed tool list when they reconnect
(deckwatch doesn't send `notifications/tools/list_changed`). Every change is
audit-logged as `mcp_tuning.guardrails.<field>`.

| Setting | Default | What it does |
|---|---|---|
| `workload_policy` | `restrictive` | `restrictive`: the typed create and update tools (`create_pod`, `create_deployment`, `update_deployment`, `create_service`, the RBAC tools, ...) and the custom resource tools refuse privileged containers, privilege escalation, host namespaces, `hostPath` volumes, host ports, capabilities beyond the container runtime's defaults, risky volume types (nfs, inline csi, gitRepo, ...), Unconfined seccomp or AppArmor profiles, NodePort, LoadBalancer and ExternalName Services and `externalIPs`, ingress snippet annotations, RBAC escalation and pod-security namespace labels set to `privileged`. An update is checked with a server-side dry run, so updating a workload that already breaks a rule is refused too. `permissive`: every workload guardrail off, as with mcp-k8s 0.5. |
| `exceptions` | none | Rules `restrictive` lets through anyway (see below). Ignored in permissive mode. |
| `allowed_custom_resource_groups` | none | API groups whose custom resources `create_custom_resource` and `update_custom_resource` may write, such as `cert-manager.io`, or `*` for any. With none, custom resource writes are refused: an operator can turn a custom resource into pods or RBAC objects. Built-in groups such as `apps` are refused here, since the typed tools write those kinds. Cluster-scoped custom resource kinds stay refused in restrictive mode. Ignored in permissive mode, which allows every group. Reads are never affected. |
| `allow_privileged_csr` | `false` | Lets `approve_csr` approve CertificateSigningRequests for `system:masters`, any other `system:` user or group (nodes, control-plane components, service accounts, `system:anonymous`), and requests it can't parse. Such a certificate can act as cluster-admin or as a node. A node's request for its own serving certificate is approved either way. Applies in both modes. |
| `apply_manifest_enabled` | `true` | Lists and serves `apply_manifest`, a server-side apply of any manifest (YAML or JSON). It **bypasses the workload guardrails**: only the namespace allowlist applies, so it can create anything deckwatch's service account may. It carries the MCP `destructiveHint`, so clients such as Claude Code ask the user before each call, and its description tells agents to use it only when the user asks. Off: it isn't listed, and a call is refused with an error naming this setting. |
| `secret_decode_enabled` | `false` | Lets `get_secret` return decoded values (`decode: true`) and `get_resource_json` return a Secret's `data`. The values then end up in the agent's context and the MCP client's transcript. Off: `get_secret` refuses `decode: true` with an error naming this setting, and `get_resource_json` leaves out `data`, `stringData` and the `kubectl.kubernetes.io/last-applied-configuration` annotation. |
| `secure_defaults` | `true` | The typed create tools fill in `runAsNonRoot: true`, the RuntimeDefault seccomp profile, `allowPrivilegeEscalation: false`, all capabilities dropped, and `automountServiceAccountToken: false` unless `service_account_name` is set, when the call doesn't set them. These are defaults, not rules: an image that must run as root needs `security_context: {"run_as_non_root": false}`. Applies in both modes. |

The `exceptions` are mcp-k8s's names for the rules, the same names its error
messages use:

| Exception | Lets through |
|---|---|
| `allow_privileged` | Privileged containers, and namespaces labelled with the privileged Pod Security level |
| `allow_privilege_escalation` | `allowPrivilegeEscalation: true` |
| `allow_host_namespaces` | `hostNetwork`, `hostPID` and `hostIPC` |
| `allow_host_path` | `hostPath` volumes |
| `allow_host_ports` | `hostPort` on container ports |
| `allow_unconfined` | Unconfined seccomp or AppArmor, custom SELinux options, unsafe sysctls |
| `allow_node_port` | NodePort Services |
| `allow_load_balancer` | LoadBalancer Services and cloud load-balancer annotations |
| `allow_external_services` | ExternalName Services and `externalIPs` |
| `allow_ingress_snippets` | ingress-nginx `*-snippet` annotations and nginx configuration in annotation values |
| `allow_rbac_escalation` | `escalate`, `bind` and `impersonate` verbs, wildcards, aggregation, binding cluster-admin or anonymous subjects, service account token Secrets |

mcp-k8s has more settings than deckwatch exposes: lists of extra
capabilities, volume types, allowed image registries, ingress annotation
allow and deny lists, and kinds the custom resource tools may write despite
being refused by default (cluster-scoped kinds, for example). A rejection
that names one of them can only be lifted with the permissive policy, or by
applying the object with `apply_manifest`. mcp-k8s's per-resource action
switches (its read-only mode) aren't exposed either: every action is
allowed.

### When a call is refused

The call returns an error (JSON-RPC code `-32000`, as for every tool error;
mcp-k8s 0.7 returns MCP `isError` results instead, which deckwatch doesn't
yet) that names the rule. mcp-k8s names its own server's flag and setting;
deckwatch adds the setting to change on deckwatch:

```
rejected by the mcp-k8s workload policy (1 violation):
- spec.type: LoadBalancer Services provision an externally reachable load balancer; an operator can allow this with --allow-loadbalancer (env MCP_K8S_ALLOW_LOADBALANCER=true; library: WorkloadPolicy.allow_load_balancer)
These guardrails apply to every typed tool and the custom resource tools. Don't work around a rejection with another tool (including apply_manifest): report it to the user.
On deckwatch, an admin sets these guardrails in Settings → MCP Tuning → Kubernetes tool guardrails (mcp_tuning.guardrails), not with the mcp-k8s server flags above: allow_load_balancer is an exception to add (mcp_tuning.guardrails.exceptions).
```

The `initialize` instructions tell agents to report a refusal to the user
rather than work around it, and to use `apply_manifest` only when the user
asks for a raw manifest to be applied.

### What the guardrails don't cover

- **Deckwatch's own tools.** `create_ingress` and `update_ingress` (deckwatch's
  versions, with ingress templates), `create_ingress_template`,
  `create_application`, the plugin and GitOps tools create objects through
  deckwatch's own code, which the workload policy doesn't check. In
  particular, ingress annotations from agents or templates aren't checked for
  snippets.
- **`exec_pod`.** Deckwatch serves its own non-interactive `exec_pod`, so
  mcp-k8s 0.7's exec limits (a 60 s timeout, a 1 MiB cap per output stream,
  and refusing pods with node-level access) don't apply to it yet.
- **`update_deployment` with only `service_account`.** Deckwatch's
  `update_deployment` hands every other field to mcp-k8s, which checks the
  result; a `service_account`-only update sets `serviceAccountName` itself.
- **Kubernetes RBAC is the hard boundary.** The guardrails keep the
  agent-facing tools within sensible limits; they don't replace the RBAC of
  deckwatch's service account, Pod Security Admission or an admission policy
  engine.

## Protocol Details

Deckwatch implements the [MCP 2025-11-25 specification](https://modelcontextprotocol.io/specification/2025-11-25)
using Streamable HTTP transport:

- **Endpoint:** `POST /mcp`
- **Protocol:** JSON-RPC 2.0
- **Content-Type:** `application/json`
- **Methods:** `initialize`, `notifications/initialized`, `tools/list`, `tools/call`
- **Arguments:** deckwatch's own tools check a call's `arguments` against the
  tool's `inputSchema` before running it. An argument the schema doesn't
  declare, a missing required argument, or a value of the wrong type or
  outside its allowed values is refused with an error naming it (`null` counts
  as leaving an optional argument out). The mcp-k8s tools, including the
  three deckwatch replaces (`update_deployment`, `apply_manifest`,
  `exec_pod`), aren't checked this way; mcp-k8s's typed workload, Service and
  Ingress tools refuse unknown arguments themselves.
- **Tools offered:** `tools/list` offers the tools deckwatch serves under the
  current [guardrail settings](#guardrails), each once (`apply_manifest` only
  while it is on). mcp-k8s's multi-cluster tools (`list_clusters`,
  `switch_cluster`, `get_active_cluster`) aren't offered: only the standalone
  mcp-k8s server serves them, and a deckwatch instance manages the cluster it
  runs in.
- **Sessions:** a successful `initialize` returns an `Mcp-Session-Id` header,
  which clients send back on later requests. Deckwatch only uses it to label
  those requests with the name and version the client gave in `initialize`
  (see [Which tools were called](#which-tools-were-called)). A missing,
  unknown or expired session id is never refused; sessions are kept in memory
  (up to 1024, forgotten after 7 days unused).

### Testing with curl

```bash
# Initialize
curl -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'

# List tools
curl -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

# Call a tool
curl -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_namespaces","arguments":{}}}'

# Get pod logs
curl -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_pod_logs","arguments":{"namespace":"default","pod_name":"my-pod","tail_lines":50}}}'
```

## Troubleshooting

### "Connection refused"

Ensure deckwatch is running and accessible at the URL you registered. For
local instances, check that the port-forward is active:

```bash
kubectl port-forward svc/deckwatch 8080:80 -n deckwatch
```

### "Unknown tool" error

Ensure your deckwatch instance is running v0.1.0 or later. The MCP endpoint
was added in v0.1.0. `get_resource_yaml` was removed with mcp-k8s 0.7: use
`get_resource_json`, which takes the same arguments. `list_clusters`,
`switch_cluster` and `get_active_cluster` aren't served (see
[Protocol Details](#protocol-details)).

### "rejected by the mcp-k8s workload policy"

A [guardrail](#guardrails) refused the call. The message names the rule and
the deckwatch setting that allows it. Ask an admin to add the exception, or
change the request.

### A pod created through MCP doesn't start: "container has runAsNonRoot and image will run as root"

The typed create tools run containers as non-root unless told otherwise
([secure defaults](#guardrails)). For an image that must run as root, pass
`security_context: {"run_as_non_root": false}` in the call, or turn off
**Secure defaults for new workloads**.

### Tools not appearing in Claude Code

Run `claude mcp list` to verify the server is registered. If it shows but
tools don't appear, try removing and re-adding:

```bash
claude mcp remove deckwatch-local
claude mcp add --transport http deckwatch-local http://localhost:8080/mcp
```

### 404 from DRA, VolumeAttributesClass or other alpha/beta tools

The upstream mcp-k8s tools for alpha and beta APIs use the newest version in
the Kubernetes 1.33 API types that deckwatch is built with: DRA
(`list_resourceclaims`, `list_resourceslices`, `list_deviceclasses`, …) uses
`resource.k8s.io/v1beta2`, VolumeAttributesClass `storage.k8s.io/v1beta1`,
ClusterTrustBundle and LeaseCandidate `v1beta1`, and MutatingAdmissionPolicy,
StorageVersion and StorageVersionMigration `v1alpha1`. Clusters serve those
versions only when they are enabled with `--runtime-config`. A default 1.35
cluster serves only the GA versions, so these tools return the API server's
404 there. IPAddress and ServiceCIDR tools use the GA
`networking.k8s.io/v1` and work on every supported cluster.

### Namespace access denied

The MCP server respects deckwatch's namespace allowlist. If a tool returns
"namespace not allowed", check the `DECKWATCH_NAMESPACES` environment
variable on the deckwatch deployment.

### Which tools were called

Every MCP request is one span and one log line, written by the embedded
mcp-k8s library with the target `mcp_k8s::telemetry`. Deckwatch's own tools
report exactly like the mcp-k8s tools. A `tools/call` logs
`tool call finished` with:

- `gen_ai.tool.name`, `mcp_k8s.outcome` (`ok`, `error`, `denied` or
  `cancelled`) and `duration_ms`;
- `mcp_k8s.action` (`read`, `write`, `delete` or `exec`) and
  `mcp_k8s.decision` (`allowed` or `denied`);
- the target: `k8s.namespace.name`, `mcp_k8s.resource.kind` and
  `mcp_k8s.resource.name`.

Other methods log `request finished`. Argument values and error text are
never logged, except the allowlisted target fields (such as the namespace,
name, kind, image and container): the request span also carries the
argument keys (`mcp_k8s.tool.argument_keys`) and those allowlisted values
(`mcp_k8s.tool.arguments`).

```bash
kubectl logs -n deckwatch deploy/deckwatch | grep '"tool call finished"'
```

Each JSON line includes the fields of its request span. In a session, that
includes the client's name and version from `initialize`
(`mcp_k8s.client.name` and `mcp_k8s.client.version`). The client reports
these itself, so treat them as a label, not as the caller's identity.

### Audit log

Every MCP tool call that writes, deletes or execs, and every call a
guardrail refuses, adds one row to deckwatch's audit log (Settings → Audit
Log, `GET /api/audit`), whether the tool is deckwatch's own or one of
mcp-k8s's. Reads that go through are not audited. A row records:

| Column | Value |
|---|---|
| `source` | `mcp` (`api` for changes made through the UI or REST API) |
| `tool` | The tool called |
| `action` | `write`, `delete`, `exec`, or `read` for a refused read |
| `outcome` | `ok`, `error`, `denied`, or `cancelled` when the client went away mid-call (the change may still have been made) |
| `resource_type`, `resource_name`, `namespace` | The target, such as `deployment` `web` in `prod`. For `apply_manifest`, from the manifest |
| `client_name` | The client's name from `initialize`. **The client reports it itself**, so it labels a call; it doesn't identify the caller |
| `detail` | JSON: the argument keys, the allowlisted argument values (namespace, name, kind, image, container, replicas, ...), the refusing rule, the client's name and version (`"self_reported": true`), the session id, the trace id when deckwatch exports traces, and the duration |
| `user_identity` | Empty until MCP clients authenticate |

Argument values outside that allowlist are never stored: no Secret data, env
values, commands, manifest bodies or plugin config.

The guardrails checked before a call runs are deckwatch's namespace allowlist
(`DECKWATCH_NAMESPACES`, for the namespace a call targets) and mcp-k8s's
permission settings, which include `apply_manifest` being turned off. A
refused call returns the reason as an error and is recorded with `outcome`
`denied` and the rule (`namespace_allowlist`, `apply_manifest:disabled`, or
mcp-k8s's `global:<action>` / `override:<resource>-<action>`).

mcp-k8s refuses some calls itself, while running them: the workload policy,
`approve_csr`'s identity check, and the namespace allowlist for namespace
objects, calls without a namespace and `drain_node` (see
[Guardrails](#guardrails)). These also return the reason as an error, but
their row has `outcome` `error` for now, not `denied`. The request's span and
its `tool call finished` log line do record them as `mcp_k8s.decision`
`denied`, with the rule in `mcp_k8s.denied_rule` (`policy:allow_load_balancer`,
`policy:allow_privileged_csr`, `namespace_allowlist`, ...).

mcp-k8s also emits an `audit` event (`tracing` target `mcp_k8s::audit`) for
each call that writes, deletes or execs, or that was refused. Deckwatch never
logs or exports it, whatever `RUST_LOG` says: it records those calls in
these rows already, with more context (the client, session and trace ids,
allowlisted argument values, cancelled calls), and a second, differently
shaped audit record of every call in the logs would only compete with them.
To follow MCP changes in the logs, select the `tool call finished` lines
whose `mcp_k8s.action` isn't `read` or whose `mcp_k8s.decision` is `denied`.

`port_forward_check` opens a tunnel into the pod, so mcp-k8s counts it as a
write on the pod: its calls are audited, as `write`.

Rows are kept for 365 days and up to 100,000 rows, then pruned hourly. Set
`DECKWATCH_AUDIT_RETENTION_DAYS` and `DECKWATCH_AUDIT_MAX_ROWS` (Helm
`audit.retentionDays`, `audit.maxRows`; `0` turns a limit off).

### MCP activity page

**MCP** in the top bar opens the MCP activity page. It shows the last hour,
24 hours or 7 days:

- **Changes and denials**: the MCP audit rows above, newest first, filterable
  by action, outcome and client. The client column says what the client said
  about itself (`client says claude-code 2.1.0`), never who made the call. A
  **View trace** link appears when deckwatch exports traces (the row has a
  trace id) and a tracing UI is set in Settings → Observability
  (`tracing.ui_url`).
- **Tool calls**: calls per minute, the share that failed or were denied, the
  p95 duration and `apply_manifest` calls per minute over time, and calls,
  errors, denials and p95 per tool. These come from Prometheus (deckwatch's own
  `/metrics`, scraped by the `PROMETHEUS_URL` Prometheus) through fixed queries
  (`GET /api/prometheus/mcp_activity`). Without Prometheus the page says so and
  shows only the audit rows. Like the bundled Grafana dashboard, it sums every
  deckwatch instance that Prometheus scrapes.

The audit rows come from `GET /api/audit`, which filters on `source`, `action`,
`tool`, `client`, `outcome`, `resource_type`, `namespace` and `since`, and
returns pages newest first (`order=asc` for oldest first) linked by
`next_cursor`.

Prometheus `/metrics` counts each tool call once:
`deckwatch_mcp_tool_calls_total{tool,status}` counts the calls deckwatch
serves itself (and calls a guardrail refused, as `status="denied"`), and
`mcp_k8s_tool_calls_total{tool,status}` counts the ones mcp-k8s runs,
including those deckwatch intercepts and then hands on (such as
`apply_manifest`). `deckwatch_mcp_requests_total{method,status}` labels
methods outside the MCP specification as `_OTHER`.

---

## MCP Tuning — org-specific guidance

Deckwatch can inject operator-defined guidance into MCP tool descriptions so
the AI follows your organization's conventions automatically — without you
having to repeat them in every prompt.

### How it works

At `tools/list` time, deckwatch appends configured hints to the `description`
field of tools in that resource group. The AI only sees the namespace hint when
namespace tools are in scope, deployment hints when deployment tools are in
scope, and so on. Nothing is injected into unrelated interactions.

### Configuring hints

Go to **Settings → MCP Tuning** and fill in the groups you want to enforce.
The same page holds the Kubernetes tool [guardrails](#guardrails).

| Group | When it fires |
|---|---|
| Global | Every MCP session (via `initialize` instructions) |
| Namespaces | Namespace creation, listing, application creation |
| Deployments | Deployment CRUD, scaling, restarts |
| Applications | Application creation, listing, restarts, scaling, templates |
| GitOps & Builds | GitOps config, build triggers, build logs |
| Ingresses | Ingress CRUD, ingress template management |
| Pods | Pod listing, log streaming, exec |
| Secrets | Secret CRUD |
| Nodes | Node listing, cordon/drain operations |
| Storage | PVC and StorageClass management |
| Plugins | Plugin lifecycle (enable, disable, validate), application associations, provisioned resources |

### Example: team-based namespace enforcement

In **Settings → MCP Tuning → Namespaces**:

> *"This org uses team-based Kubernetes namespaces (e.g. team-platform,
> team-rad, team-avionics). When creating applications or suggesting where to
> deploy, always use an existing team namespace rather than creating a new one.
> Ask which team owns this work before proceeding."*

After saving, Claude will follow this convention any time it touches
`create_namespace`, `create_application`, or `list_namespaces` — without
the user needing to mention it.
