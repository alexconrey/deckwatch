# Deckwatch Architecture

**Purpose:** A snapshot of how Deckwatch is wired end-to-end, intended for developers picking up the codebase. Complements `CLAUDE.md` (which is instructions for AI assistants) — this document is prose for humans.

**Snapshot date:** 2026-07-10

## One-liner

Deckwatch is a stateless, single-binary Rust web service that serves a Vue 3 SPA and proxies deploy/manage operations to the Kubernetes API via kube-rs. All state (including per-deployment GitOps config) lives in K8s annotations. A background poller in the same process watches configured Git repos and triggers Kaniko build Jobs on new commits.

## Process topology

```
┌──────────────────────────────────────────────────────────────┐
│                   deckwatch (single pod)                     │
│                                                              │
│  ┌────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ axum HTTP  │  │ tokio task:  │  │ tokio task:          │  │
│  │ server     │  │ git watcher  │  │ build monitor        │  │
│  │ :8080      │  │ (10s tick)   │  │ (10s tick, shares    │  │
│  │            │  │              │  │  poller loop)        │  │
│  └─────┬──────┘  └──────┬───────┘  └──────────┬───────────┘  │
│        │                │                     │              │
│        └────────────────┴──────┬──────────────┘              │
│                                │                             │
│                        ┌───────▼────────┐                    │
│                        │  AppState      │                    │
│                        │  (kube::Client │                    │
│                        │   + ns list)   │                    │
│                        └───────┬────────┘                    │
└────────────────────────────────┼─────────────────────────────┘
                                 │
                                 ▼
                       ┌──────────────────┐
                       │  Kubernetes API  │
                       └──────────────────┘
```

- `src/main.rs:16-59` — process entry, tokio runtime, spawns HTTP server + git watcher.
- `src/watcher.rs:28-43` — the watcher/monitor loop is one tokio task running both `poll_cycle` (detect new commits, trigger builds) and `monitor_builds` (watch running Kaniko Jobs, update image on success) on a 10-second tick.
- No database. No cache. No message broker. Everything is derived from live K8s state.

## Repo layout

```
src/                            # Rust backend
  main.rs                       # process entry
  config.rs                     # clap CLI (--port, --frontend-dir, --namespaces)
  state.rs                      # AppState struct + typed Api factories per resource
  routes.rs                     # axum Router — all HTTP mounted here
  error.rs                      # AppError enum + IntoResponse
  kube_ext.rs                   # K8s object → wire summary/detail structs (Serialize)
  watcher.rs                    # git poller + Kaniko job creation + build monitor
  handlers/                     # HTTP handlers, one file per resource
    health.rs                   # /healthz, /readyz
    namespaces.rs               # list, create
    deployments.rs              # CRUD, scale, restart, yaml, probes, containers
    pods.rs                     # list-for-deployment, get
    logs.rs                     # SSE stream + history JSON
    ingresses.rs                # CRUD (auto-creates backing Service if missing)
    cronjobs.rs                 # list, get
    nodes.rs                    # list (cluster overview)
    gitops.rs                   # per-deployment gitops config + manual trigger
    addons.rs                   # hardcoded addon catalog + attach/detach
frontend/                       # Vite + Vue 3 + Vuetify 3 + TS + Pinia
  src/
    App.vue                     # <router-view> only
    main.ts                     # createApp, plugins (pinia, vuetify, router)
    layouts/AppLayout.vue       # app bar (namespace selector), <router-view>
    router/index.ts             # 5 pages, all lazy-loaded
    plugins/vuetify.ts          # theme config
    styles/                     # global CSS
    stores/                     # Pinia — namespace, deployments
    api/                        # thin fetch wrappers, one per resource
      client.ts                 # ApiError + apiFetch<T>
      deployments.ts, pods.ts, nodes.ts, ...
    types/api.ts                # TS interfaces mirroring backend serde structs
    composables/
      usePolling.ts             # setInterval wrapper (5s default)
      useSse.ts                 # EventSource wrapper (unused; LogViewer inlines)
    components/
      pages/                    # 5 route-level pages
      common/                   # ConfirmDialog, LogViewer, YamlViewer/Editor,
                                # StatusChip, CreateNamespaceDialog
      views/
        deployment/             # DeploymentForm, GitOpsCard, ProbeEditor, ...
        pod/                    # PodTable, PodStatusIcon
helm/deckwatch/                 # Helm chart for deployment
docs/                           # docs; PRODUCT_ROADMAP.md drives strategy
Dockerfile                      # multi-stage: build Rust + Vite → single image
```

## Request flow (typical read)

```
User clicks a deployment in the list
  ↓
Vue router → DeploymentDetailPage.vue (props: namespace, name)
  ↓
usePolling(fetchDetail, 5000) → deploymentsApi.get(ns, name)
  ↓
GET /api/namespaces/{ns}/deployments/{name}
  ↓
axum matches route → handlers::deployments::get(State, Path)
  ↓
state.deployments_api(ns) — checks allowed_namespaces
  ↓
kube::Api::get(name) → Kubernetes API
  ↓
kube_ext::deployment_detail(&dep) — extract summary struct
  ↓
+ list_pods_for_deployment (label selector on app=<name>)
+ list_ingresses_for_service (finds Ingresses pointing to the service)
  ↓
DeploymentDetailResponse (JSON) → back to client
  ↓
Vue reactivity re-renders page
```

Key point: every read path is a live K8s API call. No caching. This is deliberate — makes the app trivially stateless — but means K8s API-server load grows linearly with active user tabs (see UX_IMPROVEMENTS.md §H4).

## Request flow (typical write — e.g., scale)

```
User adjusts replicas in Scale dialog → clicks "Scale"
  ↓
POST /api/namespaces/{ns}/deployments/{name}/scale  body: {replicas: 3}
  ↓
handlers::deployments::scale
  ↓
api.patch(name, PatchParams::default(), Patch::Merge({"spec": {"replicas": 3}}))
  ↓
Kubernetes applies patch → returns updated Deployment
  ↓
handler re-fetches pods + ingresses, returns full DeploymentDetailResponse
  ↓
Client updates UI immediately (doesn't wait for next poll)
```

Write patterns used across handlers:
- **`Patch::Merge`** — for scale, gitops annotations, restart-timestamp (scale, restart, gitops::set_config).
- **`Patch::Strategic`** — for probe updates (needs strategic merge to update a specific container by name; `deployments::update_probes`).
- **`api.replace()`** — for update, update_yaml, add_container, remove_container. Requires up-to-date `resourceVersion`.
- **`api.create()`** — for create, create_namespace, ingress create.
- **`api.delete()`** — for delete deployment/ingress.

## GitOps subsystem

This is the most subtle part of the codebase.

### Config storage
All GitOps configuration is stored as annotations on the target Deployment, prefixed `deckwatch.io/` (helper: `watcher::ann()`, `watcher::get_ann()`):

- `git-enabled` — "true" to watch this deployment
- `git-repo`, `git-branch`, `git-token-secret` — source
- `dockerfile-path`, `docker-context` — build config
- `ecr-repository` — destination image URL prefix
- `include-paths`, `exclude-paths` — comma-separated path filters for detecting whether a commit warrants a build
- `poll-interval`, `webhook-enabled` — currently unused by the watcher (which runs on a fixed 10s tick regardless)
- `last-commit-sha`, `last-build-status`, `last-build-job`, `last-build-time`, `last-build-error` — status tracking

### Watch loop (`watcher::run_poller`)
Every 10 seconds:
1. **`poll_cycle`** — list namespaces (either allowed subset or all), list deployments in each, for each with `git-enabled=true` and `last-build-status != building`:
   - Fetch the token from the K8s Secret named in `git-token-secret` (must have key `token`).
   - HTTP GET `<repo>/info/refs?service=git-upload-pack` with Basic auth (`x-token:<token>`) to read the branch HEAD SHA.
   - If SHA matches `last-commit-sha` → nothing to do.
   - Else if include/exclude paths configured AND `last-commit-sha` set → use GitHub's `/compare` API to diff files; skip build if all changed paths are excluded.
   - Else create a Kaniko Job (see below) and patch annotations to `building`.
2. **`monitor_builds`** — list all Jobs with label `deckwatch.io/build=true`; for each with succeeded or failed count > 0:
   - If succeeded: strategic-patch the Deployment's primary container image to `<ecr-repository>:<short-sha>`, set `last-build-status=success`.
   - If failed: set `last-build-status=failed`, `last-build-error="Kaniko build job failed"`.

### The Kaniko Job
Constructed in `trigger_build` (`src/watcher.rs:271-362`):
- Name: `<deployment>-build-<shortsha>`
- Labels: `deckwatch.io/build=true`, `deckwatch.io/deployment=<name>` (used by list_builds and monitor)
- Single container: `gcr.io/kaniko-project/executor:latest`
- Args: `--dockerfile=<path>`, `--context=git://x-token:<token>@<host>/<repo>#refs/heads/<branch>`, `--destination=<ecr>:<shortsha>`, `--cache=true`
- Env: `GIT_TOKEN` from Secret ref (though the token is *already* baked into --context, so this env is somewhat redundant)
- `ttl_seconds_after_finished=3600`, `backoff_limit=0` (no retry).

### Manual trigger
`POST /gitops/trigger` calls `check_remote_head` and `trigger_build_public` (a re-export of the same trigger fn) — bypasses the path filter check.

## Errors

`AppError` (`src/error.rs`) maps every backend failure to a JSON `{error, message}` shape. Client `ApiError` (`frontend/src/api/client.ts`) parses it back:

| AppError variant | HTTP status | Where it comes from |
|---|---|---|
| `Kube(Api(err))` | passthrough (usually 404, 409, 422) | K8s API rejection |
| `Kube(other)` | 502 | Network / auth to kubernetes |
| `NamespaceNotAllowed` | 403 | `AppState::check_namespace` |
| `NotFound` | 404 | Explicit lookups (addon id, container name) |
| `BadRequest` | 400 | Validation, YAML parse, missing config |

## Frontend state model

Two Pinia stores:
- **`useNamespaceStore`** — the list of allowed namespaces and the currently selected one. Selection is not persisted across sessions.
- **`useDeploymentsStore`** — the deployments list for the currently viewed namespace.

Everything else is component-local state (managed via `ref` in `<script setup>`). No stores for pods, ingresses, cronjobs, nodes, or gitops config — those pages fetch on mount + poll.

The `useDeploymentsStore` is only used by `DeploymentsPage`; `DeploymentDetailPage` fetches its own detail via the api client directly (no shared cache).

## Polling architecture

Every polling page uses the `usePolling(fn, interval)` composable, which:
- Fires `fn` immediately on mount.
- Sets a `setInterval` to call `fn` on the interval.
- Clears the interval on unmount.

Poll intervals in use:
- 5000 ms: DeploymentsPage, DeploymentDetailPage, PodDetailPage, GitOpsCard
- 10000 ms: ClusterOverviewPage (nodes change slowly)

The composable does **not** consider tab visibility — see UX_IMPROVEMENTS.md §H4.

## SSE (logs)

Log streaming is the only non-poll path:
- `GET /api/namespaces/{ns}/pods/{pod}/logs/history?tail_lines=N&container=X` — one-shot JSON of past lines.
- `GET /api/namespaces/{ns}/pods/{pod}/logs?follow=true&tail_lines=1&container=X` — SSE stream, one `log` event per line, plus periodic `keep-alive` text.

`LogViewer.vue` implements this by:
1. Fetching history JSON first, populating `lines[]`.
2. Opening an EventSource on the follow endpoint with `skipFirst=true` to avoid duplicating the last history line (buggy — see UX_IMPROVEMENTS.md §H5).
3. Appending new lines. Capping at 50k, slicing to 25k on overflow.

The `useSse` composable exists (with better error handling and 10k-line cap) but is unused — LogViewer inlines its own EventSource logic.

## Auth / Security

**None currently.** The `helm/deckwatch/values.yaml` has no auth config; no middleware exists. CORS is permissive (`CorsLayer::permissive()` in `routes.rs:109`). Any client that can reach the pod can call any endpoint.

The only access control is `--namespaces` (env: `DECKWATCH_NAMESPACES`). If unset, all namespaces are managed. If set (comma-separated list), the `AppState::check_namespace` gate returns 403 for anything else.

## Deployment model

- **Docker:** Multi-stage `Dockerfile` — Vite build in one stage, Cargo build in another, both copied into a slim runtime image. See `.dockerignore` and `Dockerfile`.
- **Helm:** `helm/deckwatch/` — templates for Deployment (single replica by default), Service, Ingress (optional), ServiceAccount + ClusterRoleBinding (for kube API access).
- **Local dev:** k3d + `docker build` + `k3d image import` + `helm upgrade`. Frontend can also be run standalone via `pnpm dev` (proxies `/api` to the backend via Vite dev server config).

## Add-on catalog

Addons (`src/handlers/addons.rs:48-75`) are a hardcoded list in Rust:
- redis (redis:7-alpine)
- memcached (memcached:1.6-alpine)
- nginx-proxy (nginx:1.27-alpine)
- fluent-bit (fluent/fluent-bit:3.1)

Attaching an addon appends a container to the Deployment's PodSpec and annotates the pod template with `deckwatch.addon/<container-name>=<addon-id>` (used by detach to find the container by addon id).

## What is NOT in the codebase (deliberate scope)

- No StatefulSet / DaemonSet management.
- No PersistentVolumeClaim UI.
- No Service / ConfigMap / Secret CRUD (Secrets are read-only via the gitops trigger; there's no way to create them in-app — see UX_IMPROVEMENTS.md §C2).
- No Events feed (see §H6).
- No RBAC / user auth (see §C1).
- No pod exec / port-forward / port-forward proxy (roadmap item).
- No metrics-server integration; no Prometheus scrape endpoint.
- No deployment history / rollback UI (see §C5).
- No cluster-selector — one deckwatch pod = one target cluster (implicit via kube_client's default kubeconfig).
- No websocket support anywhere.

## Testing

Currently: none in the repo. Both backend and frontend are untested. `TODO.md` lists this as backlog. `CLAUDE.md` notes "Testing (to be established)" with target frameworks Rust `#[tokio::test]`, Vitest, Playwright.

## Extension points

For future contributors:
- **New K8s resource type:** add a factory in `state.rs`, a summary/detail in `kube_ext.rs`, a handler file in `handlers/`, a route in `routes.rs`, an api client in `frontend/src/api/`, a types stanza in `frontend/src/types/api.ts`.
- **New addon:** append to the `catalog()` vec in `src/handlers/addons.rs:48`.
- **New polled page:** call `usePolling(refreshFn, intervalMs)` in `<script setup>`.
- **New app-template (once P0 lands):** likely a YAML/JSON in a ConfigMap loaded on startup, mapped to a form defaults preset in `DeploymentForm.vue`.

## References

- `CLAUDE.md` — AI assistant directives.
- `TODO.md` — active backlog.
- `docs/PRODUCT_ROADMAP.md` — strategic direction and P0/P1/P2 slate.
- `UX_IMPROVEMENTS.md` (sibling of this file) — detailed UX audit findings.
