# Deckwatch Architecture

**Purpose:** A snapshot of how Deckwatch is wired end-to-end, intended for developers picking up the codebase. Complements `CLAUDE.md` (which is instructions for AI assistants) — this document is prose for humans.

**Snapshot date:** 2026-07-21

## One-liner

Deckwatch is a single-binary Rust web service backed by a SeaORM database (SQLite by default, Postgres/MySQL supported) that serves a Vue 3 SPA and proxies deploy/manage operations to the Kubernetes API via kube-rs. Settings, GitOps configs, build history, and audit logs are persisted in the database. A background poller in the same process watches configured Git repos and triggers Kaniko build Jobs on new commits.

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
│                        ┌───────▼────────────────────────┐    │
│                        │  AppState                      │    │
│                        │  kube::Client, db: DbConn,     │    │
│                        │  registry_enabled/public_url,  │    │
│                        │  ai_rate_limiter, entitlements  │    │
│                        └──┬─────────────────────┬───────┘    │
└───────────────────────────┼─────────────────────┼────────────┘
                            │                     │
                            ▼                     ▼
                  ┌──────────────────┐   ┌─────────────────┐
                  │  Kubernetes API  │   │  Database        │
                  └──────────────────┘   │  (SQLite/PG/MY)  │
                                         └─────────────────┘
```

- `src/main.rs` — process entry, tokio runtime, connects to the database, runs SeaORM migrations, spawns HTTP server + git watcher.
- `src/watcher.rs` — the watcher/monitor loop is one tokio task running both `poll_cycle` (detect new commits, trigger builds) and `monitor_builds` (watch running Kaniko Jobs, update image on success) on a 10-second tick.
- `src/db.rs` — database connection setup (SeaORM), auto-migration on startup, helper functions for ensuring application records exist.
- Live K8s state is still the source of truth for deployments, pods, services, and ingresses. The database stores settings, GitOps configs, build history, and audit logs.

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
  db.rs                         # SeaORM connect + auto-migrate + helpers
  audit.rs                      # audit log recording helpers
  entities/                     # SeaORM entity models (one per DB table)
    mod.rs                      # re-exports all entities
    settings.rs                 # key/value settings store
    applications.rs             # application records (ns/name primary key)
    gitops_configs.rs           # per-application GitOps configuration
    builds.rs                   # build history (FK → applications)
    audit_log.rs                # audit trail entries
  migrations/                   # SeaORM migration definitions
    mod.rs                      # migration registry
    m20260714_000001_initial.rs  # initial schema: all five tables
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
    webhooks.rs                 # POST /api/webhooks/git — inbound Git webhook receiver
    settings.rs                 # GET/PUT /api/settings (DB-backed)
    addons.rs                   # hardcoded addon catalog + attach/detach
    templates.rs                # deployment templates CRUD
    monitoring.rs               # Prometheus integration toggle
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

Key point: every K8s resource read is a live API call (no caching), which means K8s API-server load grows linearly with active user tabs (see UX_IMPROVEMENTS.md §H4). Settings, GitOps configs, and build history reads are served from the database.

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
GitOps configuration is stored in the `gitops_configs` database table (one row per application), keyed by `application_id` (format `{namespace}/{name}`). See `src/entities/gitops_configs.rs` for the full schema. Key fields: `repo_url`, `branch`, `token_secret`, `dockerfile_path`, `docker_context`, `oci_repository`, `include_paths`, `exclude_paths`, `poll_interval_seconds`, `webhook_enabled`, plus status columns (`last_commit_sha`, `last_build_status`, `last_build_job`, `last_build_time`, `last_build_error`).

Build history is persisted in the `builds` table (FK to `applications`), recording `job_name`, `commit_sha`, `image_tag`, `status`, timestamps, and error messages.

**Legacy migration:** Deployments that still carry the old `deckwatch.io/` annotation-based config are automatically migrated to the database on first read. The annotations are left in place for backwards compatibility but the database is the source of truth going forward.

### Watch loop (`watcher::run_poller`)
Every 10 seconds:
1. **`poll_cycle`** — list namespaces (either allowed subset or all), list deployments in each, for each with `git-enabled=true` and `last-build-status != building`:
   - Fetch the token from the K8s Secret named in `git-token-secret` (must have key `token`).
   - HTTP GET `<repo>/info/refs?service=git-upload-pack` with Basic auth (`<auth_user>:<token>`) to read the branch HEAD SHA. The auth username is auto-detected from the hostname (`oauth2` for GitLab, `x-access-token` for GitHub, `x-token-auth` for Bitbucket) or can be set explicitly via `git_auth_user` in the GitOps config.
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
- Args: `--dockerfile=<path>`, `--context=git://<auth_user>:<token>@<host>/<repo>#refs/heads/<branch>`, `--destination=<oci_repo>:<shortsha>`, `--cache=true`
- Env: `GIT_TOKEN` from Secret ref (though the token is *already* baked into --context, so this env is somewhat redundant)
- `ttl_seconds_after_finished=3600`, `backoff_limit=0` (no retry).

### Manual trigger
`POST /gitops/trigger` calls `check_remote_head` and `trigger_build_public` (a re-export of the same trigger fn) — bypasses the path filter check.

## State (database)

Deckwatch uses SeaORM with SQLite by default (configurable to PostgreSQL or MySQL via the `DATABASE_URL` environment variable). Migrations run automatically on startup (`src/db.rs`). The database holds:

| Table | Entity file | Purpose |
|-------|------------|---------|
| `settings` | `entities/settings.rs` | Key/value pairs for all runtime settings (auth, notifications, AI providers, Prometheus toggle, resource limits, managed lists) |
| `applications` | `entities/applications.rs` | Canonical registry of tracked applications, keyed by `{namespace}/{name}` |
| `gitops_configs` | `entities/gitops_configs.rs` | Per-application GitOps configuration (repo, branch, token, paths, build status). FK to `applications` |
| `builds` | `entities/builds.rs` | Build history records (job name, commit, image tag, status, timestamps). FK to `applications` |
| `audit_log` | `entities/audit_log.rs` | Immutable audit trail (action, resource, namespace, user identity, detail) |

**ConfigMap fallback migration path:** Settings were originally stored in a `deckwatch-config` ConfigMap. On upgrade, existing ConfigMap settings are read and migrated to the database on first access. The ConfigMap is no longer the source of truth but is still created by Helm for backwards compatibility with older releases.

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

Addons (`src/handlers/addons.rs`) are a hardcoded catalog in Rust:
- redis (redis:7-alpine)
- memcached (memcached:1.6-alpine)
- nginx-proxy (nginx:1.27-alpine)
- fluent-bit (fluent/fluent-bit:3.1)
- otel-collector (otel/opentelemetry-collector:latest)
- postgres (postgres:16-alpine) — **persistent storage addon**

Attaching an addon appends a container to the Deployment's PodSpec and annotates the pod template with `deckwatch.addon/<container-name>=<addon-id>` (used by detach to find the container by addon id).

The **postgres** addon additionally provisions a PersistentVolumeClaim (`{deployment}-{container}-data`) and mounts it at `/var/lib/postgresql/data`. The PVC size defaults to `1Gi` and can be customized via the `storage` field on `AttachAddonRequest`. Detaching the addon removes both the volume from the pod spec and deletes the PVC from the cluster. Injected env vars: `PG_HOST=localhost`, `PGDATA=/var/lib/postgresql/data/pgdata`, `POSTGRES_DB=app`, `POSTGRES_USER=app`, `POSTGRES_PASSWORD=changeme`.

## What is NOT in the codebase (deliberate scope)

- No StatefulSet / DaemonSet management.
- No standalone PersistentVolumeClaim UI (PVCs are managed automatically by the postgres addon).
- No Service / ConfigMap / Secret CRUD (Secrets are read-only via the gitops trigger; there's no way to create them in-app — see UX_IMPROVEMENTS.md §C2).
- No Events feed (see §H6).
- No RBAC / user auth (see §C1).
- No pod exec / port-forward / port-forward proxy (roadmap item).
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
