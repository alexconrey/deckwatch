# Deckwatch — Claude Assistant Instructions

## Project Overview

Deckwatch is a Kubernetes deployment lifecycle manager — a web dashboard that
lets users create, update, scale, and delete deployments through a browser
without needing kubectl. It includes ingress management, pod log streaming,
health probe configuration, GitOps pipelines, an embedded OCI container
registry, AI diagnostics, and an MCP server for Claude Code integration.

**Repository:** https://github.com/alexconrey/deckwatch
**Current version:** v0.1.0

## Architecture

- **Backend**: Rust (Axum + kube-rs + SeaORM)
- **Frontend**: Vue 3 + Vuetify 3 + TypeScript + Vite + Pinia
- **Database**: SeaORM with SQLite (default), PostgreSQL, or MySQL
- **Deployment**: Docker (multi-stage, distroless runtime) + Helm chart
- **CI/CD**: GitHub Actions (cargo check/clippy/fmt, frontend build, helm lint)
- **Local dev**: k3d cluster with port-forward

### Key design decisions
- Native K8s resources (deployments, pods, services, ingresses) are always read
  from the Kubernetes API — never cached in the database
- Deckwatch-owned state (settings, GitOps config, builds, audit log,
  applications) lives in the database so it survives cluster destruction
- The git watcher polls repos from a background tokio task (not a separate process)
- MCP server runs as an HTTP endpoint inside deckwatch (no separate process)

## Directory Layout

```
src/                              # Rust backend
  main.rs                         # Entrypoint, config, DB connect, watcher spawn
  config.rs                       # CLI config (clap) with env var fallbacks
  state.rs                        # AppState: kube::Client + DB + namespace list
  routes.rs                       # Axum router (public_api + private_api)
  error.rs                        # AppError enum + friendly error mapping
  kube_ext.rs                     # K8s type extraction (Summary/Detail structs)
  watcher.rs                      # GitOps polling, build monitor, resource gauges
  db.rs                           # Database connect + migrate + ensure_application
  audit.rs                        # Audit log insert + list endpoint
  metrics.rs                      # Prometheus metrics (deckwatch_* prefix)
  entities/                       # SeaORM entity definitions
    settings.rs                   # settings table (key-value)
    applications.rs               # applications table
    gitops_configs.rs              # gitops_configs table (FK to applications)
    builds.rs                     # builds table (FK to applications)
    audit_log.rs                  # audit_log table
  migrations/                     # SeaORM auto-migrations
    m20260714_000001_initial.rs   # Initial schema (5 tables)
  handlers/                       # Route handlers by resource
    deployments.rs                # CRUD + scale/restart/probes/containers/yaml
    deployments_ux.rs             # History, rollback, validate, clone
    ingresses.rs                  # CRUD + auto-service + IngressClass discovery
    gitops.rs                     # GitOps config CRUD (DB-backed)
    logs.rs                       # Log streaming (SSE) + bulk history
    mcp.rs                        # MCP server (JSON-RPC 2.0, 10 tools)
    monitoring.rs                 # PodMonitor CRD management
    diagnostics.rs                # AI diagnostic job creation
    ai_fix.rs                     # AI fix job creation
    settings.rs                   # Settings CRUD (DB-backed)
    applications.rs               # Application CRUD + members
    promote.rs                    # Cross-namespace promotion
    webhooks.rs                   # Git webhook receiver
    admission.rs                  # K8s validating webhook
    prometheus_query.rs           # PromQL range query proxy
    tracing_handler.rs            # Distributed tracing proxy
    preview.rs                    # Preview environments (NOT YET WIRED)
    templates.rs                  # Deployment template CRUD
    autoscaling.rs                # HPA CRUD
    registry.rs                   # Embedded OCI registry (/v2/*)
    s3_backend.rs                 # S3 storage backend for registry
    license.rs                    # License/entitlements handlers
    ...
frontend/
  src/
    api/                          # API client modules (one per resource)
    components/
      common/                     # ConfirmDialog, LogViewer, StatusChip, YamlEditor, etc.
      pages/                      # Route-level pages
      views/deployment/           # DeploymentForm, GitOpsCard, GitOpsConfigDialog, etc.
      views/pod/                  # PodTable, PodStatusIcon
    composables/                  # usePolling, useSse, useFeatures, useSnackbar, useAiSettings
    layouts/                      # AppLayout (app bar + sidebar nav)
    plugins/                      # Vuetify setup (dark/light themes)
    router/                       # Vue Router config
    stores/                       # Pinia stores (namespace with localStorage persistence)
    types/                        # TypeScript API type definitions
    utils/                        # Shared formatAge, formatMemory, formatTimestamp
helm/deckwatch/                   # Helm chart
  templates/
    deployment.yaml               # Includes DB PVC, registry PVC, env vars
    clusterrole.yaml              # RBAC for all managed resources
    pvc-database.yaml             # SQLite PVC (conditional on storage type)
    servicemonitor.yaml           # Prometheus ServiceMonitor (optional)
    ...
docs/                             # Published documentation (mdBook)
  ARCHITECTURE.md                 # System architecture
  MCP.md                          # MCP integration guide
  SETTINGS.md, GITOPS.md, etc.   # Feature docs
docs/internal/                    # Local-only docs (gitignored)
  PRODUCT_ROADMAP.md, UX_IMPROVEMENTS.md, etc.
```

## Development Workflow

### Build & Deploy to k3d

```bash
docker build -t deckwatch:dev .
k3d image import deckwatch:dev -c deckwatch
helm upgrade --install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=deckwatch --set image.tag=dev \
  --set image.pullPolicy=Never \
  --set registry.enabled=true \
  --set ingress.enabled=true \
  --set 'ingress.hosts[0].host=' \
  --set 'ingress.hosts[0].paths[0].path=/' \
  --set 'ingress.hosts[0].paths[0].pathType=Prefix'
kubectl -n deckwatch rollout restart deploy/deckwatch
kubectl -n deckwatch port-forward svc/deckwatch 8080:80
```

### Frontend dev (standalone)

```bash
cd frontend && pnpm install && pnpm dev
# Proxies /api to localhost:8080
```

### Running tests

```bash
cargo test                                    # Backend (~580+ tests)
cd frontend && npx vitest run                 # Frontend (~70+ tests)
```

### CI

CI runs on PRs to main via `.github/workflows/ci.yml`:
- `cargo check --locked`
- `cargo clippy -- -D warnings`
- `cargo fmt --check`
- Frontend: `pnpm install && vite build`
- `helm lint`

Publish workflow (`.github/workflows/publish.yml`) builds multi-arch image to
GHCR on tag push.

## Coding Conventions

### Rust Backend
- Handlers in `src/handlers/` use: `State(state)`, `Path(...)`, `Json(...)` → `Result<..., AppError>`
- Type extraction from K8s objects lives in `kube_ext.rs` (not handlers)
- API factory methods on `AppState` — always check namespace allowlist
- Friendly error mapping in `error.rs` (maps common K8s errors to human text)
- Audit logging: fire-and-forget `audit::log_action()` calls in mutation handlers
- Test pattern: `#[cfg(test)] #[path = "../<file>_tests.rs"] mod tests;` at bottom of source files
- Tests use `use super::*;` — no nested `mod tests {}` wrapper
- Clippy enforced as `-D warnings` in CI

### Vue Frontend
- `<script setup lang="ts">` with Composition API
- Vuetify 3 with Material Design Icons (`mdi-*`)
- Dark/light theme toggle (persisted to localStorage)
- API clients in `api/` use `apiFetch` from `client.ts`
- Types in `types/api.ts` mirror backend serde structs
- Polling via `usePolling` composable (5s default, pauses on hidden tab)
- Namespace selector persisted to localStorage
- Vitest + happy-dom + @vue/test-utils (vuetify inlined via vite-plugin-vuetify)

### Git Workflow
- Iterative small commits, not monolithic version bumps
- Concise commit messages; larger context in PR descriptions
- Do NOT include `Co-Authored-By: Claude` in commits or PRs
- Do NOT automatically merge PRs — leave for user review
- Run `cargo fmt` before committing to avoid CI failures
- Create separate branches per body of work

## Infrastructure

### Zeus (production)
- ArgoCD Application on mgmt cluster targets Zeus cluster
- Manifest: `~/git/k2/kore/keystone/src/aws/govcloud/shared-services/clusters/mgmt/apps/deckwatch.yaml`
- Image: `ghcr.io/alexconrey/deckwatch:<tag>`
- Hostname: `deckwatch.zeus.gc.aws.notskunk.works`
- Storage: gp3 PVCs for SQLite DB and registry
- Internal ALB via `alb.ingress.kubernetes.io/group.name: zeus`
- SSO: `AWS_PROFILE=SHARED-SERVICES-GOV`

### k3d (local dev)
- Cluster name: `deckwatch`
- Port 8080 mapped to loadbalancer
- MinIO deployed in `minio` namespace for S3 registry testing
- MCP server registered: `claude mcp add --transport http deckwatch-localhost http://localhost:8080/mcp`

## MCP Server

Deckwatch exposes an MCP endpoint at `POST /mcp` (JSON-RPC 2.0, MCP 2025-11-25 spec).
Claude Code connects via `claude mcp add --transport http <name> <url>/mcp`.

10 tools: get_namespaces, list_deployments, get_deployment, get_pod_logs,
get_events, get_deployment_history, get_gitops_status, get_build_logs,
list_ingresses, get_metrics.

## Feature Flags

- **Prometheus monitoring**: runtime setting in DB (Settings → Observability)
- **Container registry**: controlled by `registry.enabled` in Helm values (not settings page)
- **AI providers**: server-side toggles in DB (Settings → AI Providers)

## Key Files for Common Tasks

| Task | Files |
|------|-------|
| Add a new API endpoint | `src/handlers/<module>.rs`, `src/routes.rs`, `src/handlers/mod.rs` |
| Add a new K8s resource type | `src/state.rs` (API accessor), `src/kube_ext.rs` (summary/detail), handler |
| Add a frontend page | `frontend/src/components/pages/`, `frontend/src/router/index.ts`, `frontend/src/layouts/AppLayout.vue` (nav) |
| Add a DB table | `src/migrations/`, `src/entities/`, `src/db.rs` |
| Add a new MCP tool | `src/handlers/mcp.rs` (add to TOOLS array + tool function) |
| Modify settings | `src/handlers/settings.rs` (DeckwatchSettings struct), `frontend/src/components/pages/SettingsPage.vue` |

## TODO

See `TODO.md` for current backlog. Key upcoming items:
- Switch AI diagnostics from K8s pods to direct Anthropic API calls
- Wire preview environments handler into routes
- Remaining UX polish (M9, M11, M12, L1-L12)
