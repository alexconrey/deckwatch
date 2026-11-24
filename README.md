# Deckwatch

A web-based Kubernetes deployment lifecycle manager. Create, update, scale, and
delete deployments through a browser — no `kubectl` required. Includes ingress
management, pod log streaming, health probe configuration, and an optional
GitOps pipeline that watches git repos, builds container images with Kaniko, and
auto-deploys on push.

## Features

- **Deployment CRUD** — create, update, scale, restart, and delete deployments
  with a form-driven UI
- **Ingress management** — configure routing rules per deployment; backing
  Services are auto-created
- **Pod health** — view pod conditions, container states, and restart counts at
  a glance with pass/fail indicators
- **Health probes** — configure liveness, readiness, and startup probes
  (HTTP, TCP, exec) from the deployment form
- **Log viewer** — full log history loaded in bulk, then live-streamed via SSE
- **GitOps** — per-deployment git repo watcher with path-based include/exclude
  filters, Kaniko builds, and automatic image tag rollouts
- **Namespace scoping** — restrict which namespaces deckwatch can manage via
  env var or CLI flag
- **Database persistence** — pluggable storage via SeaORM (SQLite for
  single-node, Postgres or MySQL for production)
- **Audit logging** — records who changed what and when, queryable from the UI
  and exposed as Prometheus metrics
- **Deployment history with rollback** — tracks every revision and lets you
  roll back to any prior state
- **Cross-namespace promotion** — promote a deployment from one namespace to
  another with a single action
- **Distributed tracing proxy** — forwards trace context headers through API
  calls for end-to-end observability
- **Git webhook receiver** — accepts push webhooks from GitHub/GitLab to
  trigger builds immediately instead of polling
- **Validating admission webhook** — optional Kubernetes webhook that enforces
  deckwatch policies on cluster resources
- **Prometheus query proxy** — proxies PromQL queries so the frontend can
  render live metrics without direct Prometheus access
- **Licensing system** — optional license key validation for gating
  enterprise features

## Quick Start

### Prerequisites

- A Kubernetes cluster running **1.33 or newer** (k3d, kind, minikube, EKS,
  etc.). Deckwatch is built against the Kubernetes 1.33 API (k8s-openapi
  `v1_33`), and the Helm chart requires `>=1.33`. Production (Zeus, EKS) runs
  1.35.
- `helm` 3.x
- `docker` (for building the image locally)

### Run Locally

```bash
# Backend (uses your local kubeconfig)
cargo run -- --namespaces default

# Frontend (proxies /api to the backend)
cd frontend && pnpm install && pnpm dev
```

Open [http://localhost:3000](http://localhost:3000).

### Deploy to Kubernetes

```bash
# Build and load into a local cluster (k3d example; k3d's default k3s image
# may be older than 1.33, so pin one)
k3d cluster create my-cluster --image rancher/k3s:v1.35.10-k3s1
docker build -t deckwatch:dev .
k3d image import deckwatch:dev -c my-cluster

# Install with Helm
helm install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=deckwatch \
  --set image.tag=dev \
  --set image.pullPolicy=Never

# Access
kubectl port-forward svc/deckwatch 9090:80 -n deckwatch
```

Open [http://localhost:9090](http://localhost:9090).

### Pull from GHCR

Each release is the image `ghcr.io/alexconrey/deckwatch:<version>` plus the
chart in `helm/deckwatch` at the same version. From a checkout of the release
tag (for example `v0.7.0`), the chart's image tag defaults to its
`appVersion`, so the chart and the image match:

```bash
helm install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=ghcr.io/alexconrey/deckwatch
```

### Upgrading

Read [CHANGELOG.md](CHANGELOG.md) before you upgrade. Each release lists what
changed, and its **Upgrade notes** say what you may have to change first: the
minimum Kubernetes version, Helm values, API behaviour.

## Configuration

| Variable | CLI Flag | Default | Description |
|----------|----------|---------|-------------|
| `DECKWATCH_NAMESPACES` | `--namespaces` | _(all)_ | Comma-separated namespace allow-list |
| `DECKWATCH_PORT` | `--port` | `8080` | Listen port |
| `DECKWATCH_FRONTEND_DIR` | `--frontend-dir` | `frontend/dist` | Path to built SPA |
| `DECKWATCH_DATABASE_URL` | `--database-url` | `sqlite://deckwatch.db` | Database connection string (SQLite, Postgres, or MySQL) |
| `RUST_LOG` | — | `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=info` | Log filter (Helm: `logFilter`). `mcp_k8s` is the embedded library behind most MCP tools; `extism::pdk` carries plugin log lines |

## Architecture

Deckwatch is a Rust binary (Axum + kube-rs + SeaORM) that serves a
Vue 3 + Vuetify SPA. Kubernetes remains the source of truth for workload
specs, but operational state — audit logs, deployment history, settings, and
license data — is persisted in a relational database (SQLite by default,
Postgres or MySQL for production). A background tokio task polls configured
git repos, monitors Kaniko build Jobs, and accepts webhook-triggered builds.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum, kube-rs, k8s-openapi, SeaORM |
| Frontend | Vue 3, Vuetify 3, TypeScript, Vite |
| Database | SQLite (default), Postgres, MySQL via SeaORM |
| Container build | Kaniko (rootless, in-cluster) |
| Packaging | Docker (multi-stage, distroless runtime), Helm |
| CI | GitHub Actions → GHCR |

## License

See [LICENSE](LICENSE).
