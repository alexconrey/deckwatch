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

For a remote deckwatch instance (Zeus, EKS, etc.):

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
| `get_namespaces` | List all namespaces visible to deckwatch | — |
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
| `list_addons` | List available sidecar addons (Redis, PostgreSQL, Memcached, etc.) with default config | — |
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

### Discovering available addons

```
> what database sidecars can I attach to my deployment?
```

Claude Code will:
1. Call `list_addons` to list the addon catalog
2. Show you available options (Redis, PostgreSQL, Memcached, etc.)
3. Explain what each addon provides and its default configuration

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
will only return data from those namespaces.

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

## Protocol Details

Deckwatch implements the [MCP 2025-11-25 specification](https://modelcontextprotocol.io/specification/2025-11-25)
using Streamable HTTP transport:

- **Endpoint:** `POST /mcp`
- **Protocol:** JSON-RPC 2.0
- **Content-Type:** `application/json`
- **Methods:** `initialize`, `notifications/initialized`, `tools/list`, `tools/call`

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
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_namespaces","arguments":{}}}'

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
was added in v0.1.0.

### Tools not appearing in Claude Code

Run `claude mcp list` to verify the server is registered. If it shows but
tools don't appear, try removing and re-adding:

```bash
claude mcp remove deckwatch-local
claude mcp add --transport http deckwatch-local http://localhost:8080/mcp
```

### Namespace access denied

The MCP server respects deckwatch's namespace allowlist. If a tool returns
"namespace not allowed", check the `DECKWATCH_NAMESPACES` environment
variable on the deckwatch deployment.
