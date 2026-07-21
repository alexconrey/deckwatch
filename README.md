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

## Quick Start

### Prerequisites

- A Kubernetes cluster (k3d, kind, minikube, EKS, etc.)
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
# Build and load into a local cluster (k3d example)
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

```bash
helm install deckwatch helm/deckwatch \
  --namespace deckwatch --create-namespace \
  --set image.repository=ghcr.io/alexconrey/deckwatch \
  --set image.tag=0.1.0
```

## Configuration

| Variable | CLI Flag | Default | Description |
|----------|----------|---------|-------------|
| `DECKWATCH_NAMESPACES` | `--namespaces` | _(all)_ | Comma-separated namespace allow-list |
| `DECKWATCH_PORT` | `--port` | `8080` | Listen port |
| `DECKWATCH_FRONTEND_DIR` | `--frontend-dir` | `frontend/dist` | Path to built SPA |
| `RUST_LOG` | — | `deckwatch=info,tower_http=info` | Log filter |

## Architecture

Deckwatch is a single stateless Rust binary (Axum + kube-rs) that serves a
Vue 3 + Vuetify SPA. All state lives in Kubernetes — deployment specs,
annotations for GitOps config, and Jobs for builds. A background tokio task
polls configured git repos and monitors Kaniko build Jobs.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum, kube-rs, k8s-openapi |
| Frontend | Vue 3, Vuetify 3, TypeScript, Vite |
| Container build | Kaniko (rootless, in-cluster) |
| Packaging | Docker (multi-stage, distroless runtime), Helm |
| CI | GitHub Actions → GHCR |

## License

See [LICENSE](LICENSE).
