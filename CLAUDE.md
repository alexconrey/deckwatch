# Deckwatch — Claude Assistant Instructions

## Project Overview

Deckwatch is a Kubernetes management dashboard designed for **non-software engineers** to deploy and manage applications on K8s clusters without needing kubectl or YAML knowledge.

## Architecture

- **Backend**: Rust (Axum web framework) + kube-rs for K8s API communication
- **Frontend**: Vue 3 + Vuetify 3 + TypeScript + Vite + Pinia
- **Deployment**: Docker (multi-stage) + Helm chart + k3d for local dev
- **No database** — all state comes from K8s API directly; config stored in K8s annotations

## Directory Layout

```
src/                          # Rust backend
  main.rs                     # Entrypoint, config, watcher spawn
  config.rs                   # CLI config (clap)
  state.rs                    # AppState with kube API factories
  routes.rs                   # Axum router definition
  error.rs                    # AppError enum + IntoResponse
  kube_ext.rs                 # K8s type extraction (Summary/Detail structs)
  watcher.rs                  # GitOps polling watcher
  handlers/                   # Route handlers by resource
    deployments.rs            # CRUD + scale/restart/probes/containers/yaml
    pods.rs                   # Pod list/get
    namespaces.rs             # List + create namespaces
    ingresses.rs              # CRUD ingresses
    cronjobs.rs               # List/get cronjobs
    nodes.rs                  # List cluster nodes
    addons.rs                 # Addon catalog + attach/detach
    gitops.rs                 # GitOps config + build triggers
    logs.rs                   # Log streaming (SSE) + history
    health.rs                 # Health/readiness probes
frontend/
  src/
    api/                      # API client modules (one per resource)
    components/
      common/                 # Reusable: ConfirmDialog, LogViewer, StatusChip, YamlViewer/Editor, CreateNamespaceDialog
      pages/                  # Route-level pages
      views/                  # Domain-specific sub-components
        deployment/           # DeploymentForm, GitOps, ProbeEditor, AddonsCard, SidecarManager, etc.
        pod/                  # PodTable, PodStatusIcon
    composables/              # usePolling, useSse
    layouts/                  # AppLayout (app bar + nav)
    plugins/                  # Vuetify setup
    router/                   # Vue Router config
    stores/                   # Pinia stores (namespace, deployments)
    types/                    # TypeScript API type definitions
    styles/                   # Global CSS
helm/deckwatch/               # Helm chart
docs/                         # Documentation
  PRODUCT_ROADMAP.md          # Strategic product roadmap
```

## Development Workflow

### Build & Run (local k3d)

```bash
# Build Docker image
docker build -t deckwatch:dev .

# Import into k3d
k3d image import deckwatch:dev -c deckwatch

# Deploy with Helm
helm upgrade --install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=deckwatch --set image.tag=dev \
  --set ingress.enabled=true

# Restart deployment to pick up new image
kubectl rollout restart deployment/deckwatch -n deckwatch
```

### Frontend dev (standalone)

```bash
cd frontend && pnpm install && pnpm dev
```

### Backend dev (standalone)

```bash
cargo run -- --port 8080 --frontend-dir frontend/dist
```

## Coding Conventions

### Rust Backend
- All handlers in `src/handlers/` follow the pattern: `State(state)`, `Path(...)`, `Json(...)` extractors → `Result<..., AppError>`
- Type extraction from K8s objects lives in `kube_ext.rs` (not in handlers)
- API factory methods on `AppState` in `state.rs` — always check namespace allowlist
- Use `serde_json` for JSON, `serde_yaml` for YAML serialization
- Error handling via `AppError` enum in `error.rs`

### Vue Frontend
- Components use `<script setup lang="ts">` with Composition API
- Vuetify 3 components with Material Design Icons (`mdi-*`)
- API clients in `frontend/src/api/` use `apiFetch` from `client.ts`
- Types in `frontend/src/types/api.ts` mirror backend serde structs
- Polling via `usePolling` composable (default 5s for deployment views)
- State management via Pinia stores

### Testing (to be established)
- Backend: Rust unit tests with `#[tokio::test]`
- Frontend: Vitest for unit tests
- E2E: Playwright

## Write Permission Note

Agent subprocesses cannot write directly to this directory due to sandbox restrictions. Agents should stage files to `/tmp/deckwatch-staging/<agent-name>/` mirroring the project tree structure. The team lead will copy files into place. For shell-escaped content, use `python3 << 'PYEOF'` with `open().write()` instead of heredocs.

## TODO

See `TODO.md` for current backlog. See `docs/PRODUCT_ROADMAP.md` for strategic direction.
