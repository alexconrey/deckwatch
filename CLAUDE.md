# Deckwatch — Claude Assistant Instructions

## Project Overview

Deckwatch is a Kubernetes deployment lifecycle manager — a web dashboard that
lets users create, update, scale, and delete deployments through a browser
without needing kubectl. It includes ingress management, pod log streaming,
health probe configuration, GitOps pipelines, an embedded OCI container
registry, AI diagnostics, and an MCP server for Claude Code integration.

**Repository:** https://github.com/alexconrey/deckwatch
**Current version:** v0.7.0

## Architecture

- **Backend**: Rust (Axum + kube-rs + SeaORM)
- **Kubernetes**: 1.33 or newer (kube 5, k8s-openapi 0.29 with the `v1_33`
  feature; the Helm chart declares `kubeVersion: ">=1.33.0-0"`)
- **Frontend**: Vue 3 + Vuetify 3 + TypeScript + Vite + Pinia
- **Database**: SeaORM with SQLite (default), PostgreSQL, or MySQL
- **Deployment**: Docker (multi-stage, distroless runtime) + Helm chart
- **CI/CD**: GitHub Actions (CI on PRs and main; a tag-triggered Release
  workflow publishes the image to GHCR and creates the GitHub Release)
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
src/                              # Rust backend (library + thin binary)
  main.rs                         # Binary: calls deckwatch::run()
  lib.rs                          # run(): config, DB connect, watcher spawn; module tree
  outbound/                       # All outbound HTTP: client factory, extra CAs,
                                  #   endpoint settings, credential binding (docs/TLS.md)
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
    mcp.rs                        # MCP server (JSON-RPC 2.0, 34 deckwatch tools + mcp-k8s)
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

### Create the k3d cluster (one-time)

```bash
k3d cluster create deckwatch --port 8080:80@loadbalancer \
  --image rancher/k3s:v1.35.10-k3s1
```

Pin the k3s image: deckwatch needs Kubernetes >= 1.33 and Zeus runs 1.35, but
k3d's default image tracks whatever k3d release is installed (v5.8.3 defaults to
1.33). Bump the tag when Zeus moves to a new minor version.

After creation, configure the node to allow insecure pulls from the
internal registry via NodePort:

```bash
# Tell k3s containerd that localhost:30500 is an HTTP registry
docker exec k3d-deckwatch-server-0 sh -c 'cat > /etc/rancher/k3s/registries.yaml << EOF
mirrors:
  "localhost:30500":
    endpoint:
      - "http://localhost:30500"
EOF'

# Restart k3s to pick up the config
docker exec k3d-deckwatch-server-0 sh -c 'kill 1'
# Wait ~15s, then restart the LB
docker restart k3d-deckwatch-serverlb
```

### Build & Deploy to k3d

```bash
docker build -t deckwatch:dev .
k3d image import deckwatch:dev -c deckwatch
helm upgrade --install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=deckwatch --set image.tag=dev \
  --set image.pullPolicy=Never \
  --set registry.enabled=true \
  --set registry.publicUrl=localhost:30500 \
  --set registry.service.type=NodePort \
  --set registry.service.nodePort=30500 \
  --set registry.internalUrl=deckwatch-registry.deckwatch.svc.cluster.local:5000 \
  --set ingress.enabled=true \
  --set 'ingress.hosts[0].host=' \
  --set 'ingress.hosts[0].paths[0].path=/' \
  --set 'ingress.hosts[0].paths[0].pathType=Prefix'
kubectl -n deckwatch port-forward svc/deckwatch 8080:80
```

#### Registry architecture (local dev)

The embedded OCI registry runs inside the deckwatch pod. Two network
paths reach the same data:

| Consumer | Address | How it resolves |
|----------|---------|-----------------|
| Kaniko (pod) push | `deckwatch-registry.deckwatch.svc.cluster.local:5000` | ClusterIP via CoreDNS |
| Kubelet pull | `localhost:30500` | NodePort via kube-proxy iptables |

`DECKWATCH_REGISTRY_INTERNAL_URL` is the Kaniko push address.
`DECKWATCH_REGISTRY_PUBLIC_URL` is the kubelet pull address (used in
deployment image references). On production (EKS) the kubelet can
resolve `.svc.cluster.local` natively, so no split is needed.

### Frontend dev (standalone)

```bash
cd frontend && pnpm install && pnpm dev
# Proxies /api to localhost:8080
```

### Running tests

```bash
cargo test                                    # Backend (~880+ tests)
cargo test --test outbound_tls                # Outbound HTTPS contract tests (TLS mocks)
cd frontend && npx vitest run                 # Frontend (~70+ tests)
```

### CI

CI (`.github/workflows/ci.yml`) runs on every PR to main and every push to
main:
- **Rust Check:** `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked`, `cargo fmt --check`
- **Frontend Build:** `pnpm install --frozen-lockfile`, `pnpm run build`,
  `pnpm run typecheck:test`, `pnpm test`
- **Docs Build:** the mdBook manual, built the way the image builds it
- **Helm Lint:** `helm lint`
- **Release Check:** the tests for `.github/scripts/release-notes.sh`, and
  that script run against the current version (see "Cutting a release")
- **Docker Build:** the release image (linux/amd64, not pushed), on every
  push to main, and on PRs that change the files in the `changes` job's
  pattern

The Rust toolchain is pinned in `rust-toolchain.toml` and must match the
`rust:<version>` image tag in the `Dockerfile` (CI fails if they differ), so
bump both in the same PR.

The Release workflow (`.github/workflows/publish.yml`) runs on `v*` tags; see
"Cutting a release".

### Cutting a release

A release is the image `ghcr.io/alexconrey/deckwatch:X.Y.Z` (no `v`) plus the
Helm chart in `helm/deckwatch` at version X.Y.Z, both built from the tag
`vX.Y.Z`. Its notes are the `## [X.Y.Z]` section of `CHANGELOG.md` (Keep a
Changelog 1.1.0 and semver; within 0.x a minor release may break things).

**CRITICAL:** The Helm chart version MUST stay in lockstep with the application
version. A mismatched `appVersion` in `Chart.yaml` causes the deployed UI to
show the wrong version and breaks ArgoCD drift detection. The Release workflow
refuses a tag that doesn't match all of the versions.

1. **Changes accumulate under `## [Unreleased]`** in `CHANGELOG.md`: every PR
   with a user-visible change adds its entry there (see Git Workflow). The
   version doesn't change between releases.
2. **The release PR** (branch `release/vX.Y.Z`; collect everything planned for
   the release on main first) does two things:
   - **Dates the section.** Rename `## [Unreleased]` to
     `## [X.Y.Z] - YYYY-MM-DD` with the PR's date (the maintainer updates it if
     the tag slips), add a new empty `## [Unreleased]` above it, and update
     the compare links at the bottom. Sections are ordered Upgrade notes,
     Security, Added, Changed, Fixed, Dependencies, Internal and CI.
   - **Bumps the five version spots in one commit:**
     1. `Cargo.toml` → `version`
     2. `Cargo.lock` → deckwatch's own `[[package]]` entry only. Edit that one
        line and check it with `cargo metadata --locked`. Don't run
        `cargo update`: it re-resolves unrelated packages.
     3. `helm/deckwatch/Chart.yaml` → `version` AND `appVersion`
     4. `frontend/package.json` → `version` (the UI footer reads it)
     5. `CLAUDE.md` → `Current version`

   Preview the notes, or the refusal, with
   `.github/scripts/release-notes.sh vX.Y.Z`; CI's Release Check job runs it
   for the current version on every PR. If more PRs land on main before the
   tag, merge `origin/main` into the release PR and add their entries; don't
   rebase it.
3. **The maintainer merges the release PR, then tags** `vX.Y.Z` on main and
   pushes the tag. Agents never create tags or GitHub Releases.
4. **The Release workflow** (`.github/workflows/publish.yml`) runs on the tag,
   one run per tag, never cancelled:
   - **Verify tag** runs `release-notes.sh`, which refuses the tag unless it
     equals `Cargo.toml`'s version, `Chart.yaml`'s `version` and
     `appVersion`, and `frontend/package.json`'s version, and `CHANGELOG.md`
     has a dated, non-empty `## [X.Y.Z] - YYYY-MM-DD` section. Every other
     job needs this one, so a refused tag publishes nothing. Fix main with a
     PR; the maintainer then deletes the tag and tags the fixed commit.
   - **build** and **merge** push the linux/amd64 and linux/arm64 image to
     GHCR, tagged `X.Y.Z`, `X.Y`, `X`, the short commit SHA and `latest`.
   - **Create GitHub Release**, the only job with `contents: write`, creates
     the `vX.Y.Z` release from the CHANGELOG section, plus a pull command and
     the image digest.

`bash .github/scripts/release-notes-test.sh` tests the script (CI runs it).

## Coding Conventions

### Rust Backend
- Handlers in `src/handlers/` use: `State(state)`, `Path(...)`, `Json(...)` → `Result<..., AppError>`
- deckwatch is the binary, so it chooses the k8s-openapi version feature
  (`v1_33` in `Cargo.toml`); k8s-openapi allows only one per build, and
  mcp-k8s deliberately sets none. `kube` and `k8s-openapi` must stay on the
  same versions as mcp-k8s, because deckwatch passes its `kube::Client` into
  `mcp_k8s::K8sClient::new`. Upgrade them together with an mcp-k8s release
  (a Dependabot PR that bumps only `kube` can't compile). Raising the feature
  raises the minimum Kubernetes version: update the chart's `kubeVersion`, the
  README and the k3d image too.
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
- **NEVER commit directly to main** — always create a branch and open a PR
- **NEVER push tags directly** — only tag after a PR has been merged to main
- **PRs with user-visible changes add an entry under `## [Unreleased]` in
  `CHANGELOG.md`**, in the same PR, with the PR number. Write for an operator
  upgrading a deployment: what they must change and what behaves differently.
  Anything that needs action goes under `### Upgrade notes`. Changes only to
  CI, tests or internals may go under `### Internal and CI` or be left out.
- Iterative small commits, not monolithic version bumps
- Concise commit messages; larger context in PR descriptions
- Do NOT include `Co-Authored-By: Claude` in commits or PRs
- **NEVER merge a PR without CI passing** — always wait for all checks to go green first
- Do NOT automatically merge PRs — open the PR, wait for CI, then let the user merge
- Run `cargo fmt` before committing to avoid CI failures
- Create separate branches per body of work

## Infrastructure

### Zeus (production)
- EKS, Kubernetes 1.35
- ArgoCD Application on mgmt cluster targets Zeus cluster
- Manifest: `~/git/k2/kore/keystone/src/aws/govcloud/shared-services/clusters/mgmt/apps/deckwatch.yaml`
- Image: `ghcr.io/alexconrey/deckwatch:<tag>`
- Hostname: `deckwatch.zeus.gc.aws.notskunk.works`
- Storage: gp3 PVCs for SQLite DB and registry
- Internal ALB via `alb.ingress.kubernetes.io/group.name: zeus`
- SSO: `AWS_PROFILE=SHARED-SERVICES-GOV`

### k3d (local dev)
- Cluster name: `deckwatch`
- k3s image pinned to `rancher/k3s:v1.35.10-k3s1` (matches Zeus's 1.35)
- Port 8080 mapped to loadbalancer
- Registry: NodePort 30500 → ClusterIP 5000 (see "Registry architecture" above)
- `registries.yaml` on node maps `localhost:30500` → `http://localhost:30500` (insecure)
- MinIO deployed in `minio` namespace for S3 registry testing
- MCP server registered: `claude mcp add --transport http deckwatch-localhost http://localhost:8080/mcp`

## MCP Server

Deckwatch exposes an MCP endpoint at `POST /mcp` (JSON-RPC 2.0, MCP 2025-11-25 spec).
Claude Code connects via `claude mcp add --transport http <name> <url>/mcp`.

34 deckwatch-specific tools (`deckwatch_tool_definitions()` in
`src/handlers/mcp.rs`) plus 200+ upstream K8s tools from mcp-k8s (deployments,
pods, services, ingresses, secrets, configmaps, RBAC, CRDs, etc.):

- **Applications:** `create_application`, `list_applications`, `get_application`,
  `restart_application`, `scale_application`, `list_templates`
- **GitOps & builds:** `set_gitops`, `get_gitops`, `trigger_build`, `list_builds`,
  `get_build_log`, `watch_build`, `generate_local_build`
- **Ingress:** `create_ingress`, `update_ingress`, `list_ingress_templates`,
  `create_ingress_template`, `update_ingress_template`, `delete_ingress_template`
- **Monitoring:** `get_monitoring`, `enable_monitoring`, `disable_monitoring`
- **Plugins:** `list_plugins`, `enable_plugin`, `disable_plugin`,
  `update_plugin_config`, `get_plugin_config`, `validate_plugin`,
  `associate_plugin`, `disassociate_plugin`
- **Plugin resources:** `list_application_resources`, `provision_resource`,
  `deprovision_resource`, `refresh_resource`

`submit_agent_feedback` is listed only when agent feedback is enabled in
settings. Deckwatch also replaces the upstream `update_deployment`,
`create_pod`, `apply_manifest` and `exec_pod` with its own implementations, and
intercepts `create_deployment`, `delete_deployment`, `scale_deployment` and
`restart_deployment` to audit-log them.

## Plugins

Sidecars and other workload injection come from external WASM plugins built on
[deckwatch-plugin-sdk](https://github.com/alexconrey/deckwatch-plugin-sdk). They
replaced the built-in addon system, which was removed in #171 (there is no
`/api/addons`). The old sidecars are now the
`alexconrey/deckwatch-plugin-{redis,memcached,nginx-proxy,fluent-bit,postgresql,otel}`
plugins.

- **Configured** in settings `plugins[]` (Settings → Plugins, `PUT /api/settings`,
  or the MCP plugin tools). The WASM comes from a GitHub file or release, a URL,
  or `POST /api/plugins/{name}/upload`. Enabled plugins load at startup and on
  settings changes. They run in extism with WASI disabled, and outbound HTTP is
  limited to each plugin's `allowed_hosts`.
- **Per deployment:** every loaded plugin's `apply()` runs when a deployment is
  created or updated through the REST API (`POST`/`PUT` handlers in
  `src/handlers/deployments.rs`; MCP `create_deployment` does not run them).
  Each plugin decides whether to act, usually based on deployment annotations
  that it defines.
- **Per application:** plugins that declare resources are provisioned from the
  application's Infrastructure tab or with MCP `provision_resource`.
  `reconcile_application_plugins()` in `src/watcher.rs` then server-side-applies
  their env vars and sidecars to every Deployment and CronJob labelled
  `deckwatch.io/application=<app>`, every 10 s. Associations (`associate_plugin`,
  Plugins tab) are stored, but neither path reads them.
- Plugin output can include Kubernetes objects of any kind. Deckwatch
  server-side-applies them with its own credentials.

Key files: `src/plugins.rs` (fetch, execute, merge), `src/handlers/plugins.rs`
(`/api/plugins`), `src/handlers/applications.rs` (associations),
`src/handlers/application_resources.rs` (provisioning), `src/watcher.rs`
(reconciler), `PluginConfig` in `src/handlers/settings.rs`. Guide:
`docs/PLUGINS.md`.

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
