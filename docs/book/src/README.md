# Deckwatch

**Deckwatch** is a Kubernetes deployment lifecycle manager. It gives operators
a friendly web UI on top of the raw kube API and augments it with the
capabilities most day-to-day platform work needs — without asking the
operator to know kubectl or hand-write YAML.

## What Deckwatch does

- **Application & Deployment CRUD** — create, edit, restart, scale, roll back
  from a form UI. Templates jumpstart common workload shapes (web app, worker,
  cron job, static site) with sensible defaults for probes, resources, and
  ingress.
- **GitOps** — attach a Git repo to a deployment; a background watcher polls
  for new commits and Kaniko builds run inside the cluster. No external CI
  system required.
- **Embedded OCI Registry** — optional Distribution Spec v1.1 endpoint under
  `/v2/*` so in-cluster builds have somewhere to push without a third-party
  registry (ECR, Docker Hub, GHCR).
- **Observability** — Prometheus text-format metrics on `/metrics`, per-pod
  and per-node resource metrics (CPU/memory sparklines), and AI-assisted
  diagnostics that hand pod logs to Claude or Codex when a pod crashes.
- **Templates & Addons** — jumpstart new deployments from the template
  catalog; attach shared infrastructure (databases, message queues, ...) as
  addons.
- **Rollout History & Rollback** — every mutation becomes a first-class
  revision so a non-engineer can see "what shipped when" and undo a bad
  rollout without touching `kubectl rollout undo`.

## Who this manual is for

This book is organised so **operators** can find "how do I do X" quickly, and
**developers** extending Deckwatch have a separate section for architecture,
design proposals, and contribution notes.

### If you are an operator

Start here:

- [Settings](../../SETTINGS.md) — how runtime config is stored and edited.
- [Authentication](../../AUTH.md) — Microsoft Entra OIDC plumbing.
- [Deployment Templates](../../TEMPLATES.md) — the built-in template catalog.
- [Rollout History & Rollback](../../ROLLBACK.md) — undo a bad rollout.
- [GitOps](../../GITOPS.md) — wire a Git repo to a Deployment.
- [Embedded OCI Registry](../../REGISTRY.md) — turn on the in-cluster registry.
- [Metrics](../../METRICS.md) — what Deckwatch exposes for Prometheus.
- [AI Diagnostics](../../AI_DIAGNOSTICS.md) — the Diagnose-with-AI button.

### If you are a developer

Start here:

- [Architecture](../../ARCHITECTURE.md) — end-to-end wiring, snapshotted for
  the current codebase state.
- [Product Roadmap](../../PRODUCT_ROADMAP.md) — priority order and
  motivation for upcoming work.
- [Testing](../../TESTING.md) — the three test suites and how to run them.
- [State Management design](../../ARCHITECTURE_DECISION.md) — why
  ConfigMaps + annotations for K8s state, embedded SQLite for the new
  concerns on the roadmap.
- [Metrics Visualization](../../METRICS_VISUALIZATION.md) and
  [Prometheus Integration](../../PROMETHEUS_INTEGRATION.md) — design
  proposals for the observability roadmap.
- [Licensing Strategy](../../LICENSING_STRATEGY.md) — proposed tier
  structure and the "cluster resources always free" guiding constraint.
- [UX Review Findings](../../UX_IMPROVEMENTS.md) — full-pass review with
  severity-tagged findings.

## Where things live

| Concern | Location |
|---|---|
| UI | Vue 3 / Vuetify — `frontend/` |
| API | Rust / axum + kube-rs — `src/` |
| Book | this manual — `docs/book/` |
| Raw markdown | canonical source — `docs/` |
| API reference | Swagger UI served by the running binary at `/api/docs` |
| OpenAPI spec | `/api/openapi.yaml` |

Editing docs? Edit the canonical files under `docs/*.md` directly — the book
just re-uses them via relative paths in the SUMMARY.
