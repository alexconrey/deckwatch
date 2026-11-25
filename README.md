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
  `v1_33`), and the Helm chart requires `>=1.33`.
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
| `DECKWATCH_CACHE_DIR` | `--cache-dir` | _(unset: no cache)_ | Directory for deckwatch's cache of compiled plugins, kept in `<dir>/wasmtime` (Helm: `cache`, an emptyDir at `/var/cache/deckwatch`). Unset, or a directory deckwatch can't use: plugins are compiled on every call; see [plugin compile cache](#helm-chart-plugin-compile-cache) |
| `RUST_LOG` | — | `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=info,opentelemetry=warn` | Log filter (Helm: `logFilter`). `mcp_k8s` is the embedded library behind most MCP tools; `extism::pdk` carries plugin log lines; `opentelemetry` reports failed OTLP exports ([docs/TRACING.md](docs/TRACING.md#10-deckwatchs-own-telemetry)) |
| `TOKIO_WORKER_THREADS` | — | One per CPU the container may use, at least 2 | Async worker threads (Helm: `extraEnv`); see [extra environment variables](#helm-chart-extra-environment-variables) |

### Helm chart: probes

The chart's `livenessProbe`, `readinessProbe` and `startupProbe` values are
the deckwatch container's probes. Each is a Kubernetes Probe object, passed to
the container unchanged:

| Value | Default | On failure |
|-------|---------|------------|
| `startupProbe` | `GET /api/healthz` every 5 s, 5 s timeout, 60 failures (5 minutes) | Restarts the container |
| `livenessProbe` | `GET /api/healthz` every 10 s, 5 s timeout, 3 failures | Restarts the container |
| `readinessProbe` | `GET /api/readyz` every 10 s, 5 s timeout, 3 failures | Takes the pod out of the Service until it passes again |

deckwatch fetches and compiles every enabled plugin before it starts serving
HTTP. Kubernetes runs the liveness and readiness probes only after the startup
probe succeeds, so neither has an initial delay, and the startup probe sets how
long deckwatch has to start. Plugins load one at a time, and each download can
take up to 30 s; if yours need more than 5 minutes, raise
`startupProbe.failureThreshold`. `/api/readyz` also checks that deckwatch can
reach the Kubernetes API. Liveness and readiness wait 5 s for an answer, rather
than Kubernetes' default of 1 s, so a few slow answers from a busy pod don't
restart it or take it out of the Service.

```yaml
startupProbe:
  failureThreshold: 120   # 10 minutes to start
livenessProbe:
  timeoutSeconds: 10
```

Helm merges your values into these defaults. To switch a probe to another
handler, set `httpGet: null` next to the new one. To remove a probe, set it to
`null` (`--set startupProbe=null`; `{}` keeps the defaults). Without the
startup probe, give `livenessProbe` an `initialDelaySeconds` that covers
startup.

### Helm chart: plugin compile cache

Every plugin call compiles the plugin's WebAssembly before running it. With a
cache, each pod compiles a plugin binary once and later calls load the result
from disk ([#392](https://github.com/alexconrey/deckwatch/issues/392) measured
about 3 ms instead of 45–62 ms per call for a ~700 KB plugin). The chart
mounts an emptyDir and sets `DECKWATCH_CACHE_DIR` to it:

| Value | Default | Description |
|-------|---------|-------------|
| `cache.enabled` | `true` | Mount the emptyDir and set `DECKWATCH_CACHE_DIR` |
| `cache.mountPath` | `/var/cache/deckwatch` | Where the emptyDir is mounted |
| `cache.sizeLimit` | `1Gi` | The emptyDir's size limit; Kubernetes evicts the pod if the cache grows past it. Empty means no limit |

deckwatch keeps wasmtime's cache in `<dir>/wasmtime` and writes its config,
`<dir>/wasmtime-cache.toml`, at every start. wasmtime trims the cache to stay
under 512 MiB, checking at most once an hour, so keep `sizeLimit` well above
that. Each pod has its own cache, so replicas need no shared volume. It lasts
as long as the pod, container restarts included, and a new pod starts empty.

Without `DECKWATCH_CACHE_DIR`, deckwatch caches nothing and logs how to turn
the cache on. If it can't create or write the directory, it logs a warning and
runs without the cache; if the cache stops working later, it warns once and
stops using it. The cache never makes a plugin call fail. Outside the chart,
set `DECKWATCH_CACHE_DIR` to a writable directory yourself.

### Helm chart: extra environment variables

`extraEnv` is a list of Kubernetes `EnvVar` objects (`name`, plus `value` or
`valueFrom`), added to the deckwatch container after the variables the chart
sets. Don't repeat one of those; use the chart's value for it instead
(`logFilter` for `RUST_LOG`, for example), because a duplicate name fails
server-side apply. Environment variable values are strings, so quote numbers.

```yaml
extraEnv:
  - name: TOKIO_WORKER_THREADS
    value: "4"
```

`TOKIO_WORKER_THREADS` sets how many worker threads deckwatch's async runtime
(tokio) runs. The default is one per CPU the container may use, but at least 2.
Rust counts a CPU limit in whole cores, rounded down, so any limit under 2
cores, including the chart's default of `200m`, would otherwise give a single
worker, and while it was busy, deckwatch would answer nothing, its health checks
included. More workers don't raise the CPU limit. Plugin calls don't run on
these workers but on threads of their own, at most 4 at a time. Use a positive
integer; deckwatch doesn't start with anything else.

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
