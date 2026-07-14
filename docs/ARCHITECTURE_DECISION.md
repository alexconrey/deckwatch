# Deckwatch State Management: Database vs ConfigMaps

**Status:** Proposed
**Date:** 2026-07-10
**Author:** arch-research agent (for Alex Conrey)
**Scope:** State persistence strategy for Deckwatch as it scales from a stateless K8s dashboard toward a PaaS-lite platform.

---

## TL;DR

**Recommendation: Option D (Hybrid) — keep the current ConfigMap + annotation model as the source of truth for K8s-adjacent state, and introduce embedded SQLite (via `rusqlite`) as a purpose-built store for the *new* concerns on the roadmap (audit log, metrics ring buffer, notification rules, cached remote data) — behind a PVC with graceful degradation if the PVC is unavailable.**

The current model is not broken for what it does. It breaks precisely at the point where the roadmap wants to go: audit history, metrics history, user preferences, and durable non-K8s state. Migrating existing K8s state into a DB would be a lot of churn for a "make it worse" outcome (loses restart-safety, loses `kubectl` inspectability, loses zero-ops). But refusing to add a DB *at all* would push us into ugly workarounds — ring-buffer ConfigMaps for audit logs, base64-blobbed metrics timeseries, "one ConfigMap per audit event" patterns that all fail badly.

Do both. Right tool for each concern.

---

## 1. Current State Inventory

What's stored, where, and why.

### 1.1 K8s-native state (this is fine as-is)

Deckwatch does not store this — Kubernetes does. Deckwatch reads live via `kube::Api`.

| Concern | Storage | Where |
|---|---|---|
| Deployments, Pods, Services, Ingresses, CronJobs, Nodes, Namespaces | K8s API | live reads in `handlers/*.rs` |
| Deployment history (revisions) | ReplicaSet objects | K8s-native, unused today (roadmap P0) |
| Secrets (git tokens, LLM API keys) | K8s `Secret` | `handlers/gitops.rs:246`, `diagnostics.rs:322` |

### 1.2 Application configuration state (ConfigMap)

Small, restart-safe, cluster-scoped app config.

| Concern | Storage | Location | Size ceiling |
|---|---|---|---|
| Global settings (allowed_namespaces, resource defaults, auth config, notification webhook) | ConfigMap `deckwatch-config` in release ns | `handlers/settings.rs:65-113` | trivial, ~few KB |
| Application groups (name, description, git config, timestamps) | one ConfigMap per app: `deckwatch-app-<name>` | `handlers/applications.rs:48-58` | ~1 KB per app |
| AI diagnostic prompt + truncated pod logs (passed into diagnostic Job as a volume) | one ConfigMap per diagnostic: `<job>-logs` | `handlers/diagnostics.rs:271-305` | up to 256 KiB (truncated) |

### 1.3 Per-deployment state (annotations)

Deckwatch stashes state directly on the resources it manages — no separate store.

| Concern | Storage | Location |
|---|---|---|
| GitOps config: repo, branch, token-secret name, dockerfile path, ecr repo, poll interval, path filters | annotations on `Deployment` (prefix `deckwatch.io/`) | `handlers/gitops.rs:117-187`, `watcher.rs:14-26` |
| GitOps status: last commit SHA, last build status, last build job, last build error, last build time | annotations on `Deployment` (prefix `deckwatch.io/`) | `watcher.rs:150+`, `gitops.rs:104-114` |
| Addon attachment tracking (which container came from which addon, and which env vars the addon injected into the primary) | annotations on the pod template inside the Deployment (prefix `deckwatch.addon/`, `deckwatch.addon-env/`) | `handlers/addons.rs:17-21, 115-161` |
| Application membership | label `deckwatch.io/application=<app>` on member Deployments and CronJobs | `handlers/applications.rs:26-32` |
| Diagnostic job provenance (source pod, agent) | labels on Job + ConfigMap: `deckwatch.io/diagnostic*` | `handlers/diagnostics.rs:18-20` |

### 1.4 Ephemeral / in-memory state

None persisted. `AppState` holds only the `kube::Client` and static config. There is no in-process cache, no message queue, no session store — every request re-hits the K8s API.

### 1.5 Total picture

Deckwatch today has **~4 kinds of write patterns**:
1. Patch annotations on a K8s object (GitOps, addons, poller state)
2. Apply/patch a ConfigMap owned by Deckwatch (settings, application defs, diagnostic prompts)
3. Create ephemeral K8s objects (Jobs, Deployments) via the K8s API
4. Read secrets (never writes them)

The persistence model is: **"delegate everything to Kubernetes"**. This is the right instinct for a K8s dashboard, and the reason Deckwatch survives pod restarts trivially.

---

## 2. Upcoming Features That Need State

Cross-referencing `TODO.md`, `docs/PRODUCT_ROADMAP.md`, and `docs/AUTH.md`.

### 2.1 Features whose state naturally lives in K8s (no DB needed)

| Feature | Why K8s is fine |
|---|---|
| **P0 #2 Deployment History + Rollback** | ReplicaSets are the history; K8s already retains `revisionHistoryLimit` of them |
| **P0 #3 Container Exec / Port-Forward** | Zero persistent state — WebSocket → Api::exec |
| **P0 #5 Metrics (Prometheus / metrics-server)** for the live-panel case | External Prometheus is the DB; deckwatch is a query proxy |
| **P1 Secrets/ConfigMaps first-class UI** | These are K8s resources; CRUD via kube-rs |
| **P1 Events feed** | K8s `Event` resource; live-list |
| **P1 Namespace CRUD** | K8s Namespace + LimitRange + ResourceQuota |
| **P2 HPA config** | K8s HPA resource |
| **App Templates (P0 #1)** | Templates themselves live in a ConfigMap (per roadmap §Extension points) |

### 2.2 Features that need durable *new* state

These are the load-bearing use cases for a DB.

| Feature | State shape | Volume | Access pattern | Why ConfigMap fails |
|---|---|---|---|---|
| **Audit log** (P0 #4) | append-only rows: `(ts, user, action, ns, resource, diff_json)` | 100s–1000s of rows/day; retention months | filter by user, ns, resource, time range; sort by ts desc | ConfigMaps have a 1 MB limit and no querying — need indexes on 4 dimensions; ring-buffer CM = fragile |
| **Metrics history ring buffer** (Phase 2, `docs/METRICS_VISUALIZATION.md`) | timeseries: `(pod, container, ts, cpu, mem)` | 15-min buffer × N pods × 15s samples = ~60 samples/pod | time-range scan per pod | ConfigMap = base64 blob of the whole buffer, rewrite on every sample; unusable |
| **RBAC policy** (P0 #4 auth) | rows: `(subject_group, namespace, role)` | 10s–100s of rows | lookup by user's groups × ns for every request | Fits in ConfigMap today, but breaks the "readable by the app but not editable outside it" invariant — needs transactional multi-row updates |
| **User preferences / session state** | rows: `(user_id, key, value)` | small | keyed by user | Per-user CM is silly; a single CM would race on concurrent writes |
| **Notification rules** (P1) | rows: `(ns, event_pattern, channel, target)` | 10s–100s of rows | lookup by matching event | Fits in CM but needs versioning + audit history for changes → back to needing a DB anyway |
| **Cached git branch list** (TODO §GitOps: "Branch selector should be a dynamic dropdown querying actual branches from the remote repo") | rows: `(repo, branch, cached_at)` | 10s of rows per repo | keyed lookup with TTL | Not worth a CM; wants LRU eviction and TTL |
| **Cached OCI/registry queries** (TODO §GitOps: "Support ALL OCI-compliant registries") | rows: `(registry, image, tag, cached_at)` | 10s of rows | keyed lookup with TTL | Same as above |
| **Managed repository list** (TODO §GitOps: "Repositories should be settings-configurable — users select from a managed list") | rows: `(repo_url, token_secret_name, added_by, added_at)` | 10s of rows | list + lookup | Fits in CM today; will grow into "who added it and when" → audit-adjacent |
| **Managed token secret registry** (TODO §GitOps: "Git token secrets should be settings-configurable shared resources") | rows: `(secret_name, ns, purpose, added_by)` | 10s of rows | list + lookup | Same as above |
| **Diagnostic history list** (TODO §AI Diagnostics: "Add diagnostic history list endpoint") | rows: `(job_name, source_pod, agent, status, started, finished, summary)` | 10s–100s of rows | list, filter by ns/pod | Today the state is *in* the Jobs themselves — but Jobs have `ttl_seconds_after_finished=3600` (`diagnostics.rs:414`), so **the history disappears after an hour**. Need to persist metadata longer than the Job. |
| **Cost snapshots** (P2 Cost awareness) | timeseries: `(ns, deployment, ts, cost/hr)` | small daily rollups | time-range per ns/deployment | Same shape as metrics history |
| **Multi-cluster kubeconfig registry** (P2) | rows: `(cluster_name, api_endpoint, credentials_ref, added_by)` | small | list + lookup | Would work in K8s Secret+CM, but cluster-selector state + last-used tracking is per-user session data |

### 2.3 The pattern

Everything with **audit semantics** (who did what when), **timeseries semantics** (history over time), or **per-user semantics** (session, preference) needs something ConfigMaps can't give: **indexed, queryable, transactional, small-write-amplification storage**.

Everything else — K8s resources and lightweight config — the current model handles fine.

---

## 3. Option Analysis

### Option A: Continue with ConfigMaps + Annotations only

**Pros**
- Zero infrastructure, zero operational burden. Works today.
- Restart-safe by default (K8s owns the storage).
- `kubectl get configmap` works — inspectable without deckwatch running.
- Single-pod and HA look identical (all pods read the same K8s state).
- No migrations, no schema drift, no backup story to build.
- Aligns with the stateless design philosophy in `docs/ARCHITECTURE.md`.

**Cons**
- **1 MiB hard limit per ConfigMap** (etcd request size). Audit log or metrics history will hit this in weeks.
- **No querying / indexing.** Every "show me audit events for user X" requires reading and JSON-parsing the whole ConfigMap.
- **No transactions.** Two writers racing on the same ConfigMap either overwrite each other or one gets a `409 Conflict` and has to retry. Optimistic concurrency via `resourceVersion` works but is not fun to program against for multi-key writes.
- **No relationships.** Foreign keys and joins must be done in application code, badly.
- **No time-series primitives.** Ring buffers must be rewritten in full on every sample.
- **etcd is not designed for high-write-volume small objects.** Metrics at 15s sample rate × 100 pods = 4× writes/sec of a full ConfigMap rewrite. Cluster operators will notice.
- **Eventual consistency across CM watch caches.** Fine for config, bad for "audit event just recorded, must appear in list within 100ms".
- **Access control leaks.** Every ConfigMap is visible to anyone with `configmaps:get` in the namespace. Audit logs, RBAC policy — probably shouldn't be there.

**When it breaks:** the moment the first roadmap feature that needs history (audit log, metrics buffer, diagnostic history persistence beyond 1h TTL) lands.

**Effort to adopt:** zero (status quo).

---

### Option B: Embedded SQLite (via `rusqlite`)

**Pros**
- Single file, no network, no separate process. Add one crate.
- ACID transactions, full SQL, indexes, foreign keys.
- Fast: SQLite handles 10s of MB/s of writes on a decent disk; reads are trivial.
- Tiny footprint (adds ~1 MB to the binary; a few MB of disk at steady state).
- Battle-tested, boring technology. First Resonance / K2 SRE won't be paged at 3am about SQLite.
- Excellent Rust ecosystem: `rusqlite` (raw), `sqlx` (compile-time-checked, async), `sea-orm`, `diesel`. Recommend `rusqlite` + `refinery` for migrations — matches the project's "no more machinery than necessary" style.

**Cons**
- **Not HA.** One writer at a time. If deckwatch ever scales to `replicaCount > 1`, only one pod can own the SQLite file, the others must proxy writes or be read-only.
- **PVC required for persistence.** Without a PVC the DB dies with the pod. This is a *deployment* change — the Helm chart is currently `replicaCount: 1` and stateless, so this is a real shift.
- **Backup story is on us.** Either PVC snapshots (cluster-dependent), a periodic `VACUUM INTO` to S3, or accept that a lost PVC means lost audit history (defensible for metrics buffer, not for audit log).
- **Migrations are on us.** `refinery` or hand-rolled SQL migrations that run on startup. Fine, but net-new complexity.
- **`kubectl get` no longer shows the state.** Ops loses the ability to inspect without hitting an API endpoint or `kubectl exec` + `sqlite3`.
- **Graceful degradation matters.** If the PVC fails to mount, the app should still boot in "no-DB mode" (K8s-native features work, DB-backed features return 503) rather than crash-looping.

**When it's the right call:** for the new state concerns identified in §2.2. Not for the existing state.

**Effort to adopt:** ~1-2 weeks for foundation:
- Add `rusqlite` + `refinery` + `tokio-rusqlite` (async wrapper) to `Cargo.toml`
- Add `DbState` to `AppState`, initialize with PVC-backed path (env: `DECKWATCH_DB_PATH=/data/deckwatch.db`)
- Add `helm/deckwatch/templates/pvc.yaml` (opt-in via `values.yaml` — default off for backward-compat)
- Update `deployment.yaml` to mount the PVC at `/data`
- Write first migration (audit_events table) as the pilot
- Add graceful-degrade: on init failure, log warning + set `AppState::db = None`; handlers that need it return 503 with a clear "persistence not configured" message

---

### Option C: Embedded Postgres (sidecar or CNPG)

**Pros**
- Real relational DB with battle-tested HA options (streaming replication, or CloudNativePG operator).
- Familiar tooling (`psql`, pg_dump, pgAdmin) — team likely already knows it.
- If usage exceeds what SQLite can handle (unlikely for deckwatch), can scale horizontally.

**Cons**
- **Another container per pod** (sidecar model) or **another workload to manage** (CNPG operator model).
- **Migrations, backups, monitoring, credentials, TLS, connection pooling** — all real ops burden the current deckwatch does not have.
- **Overkill.** The heaviest workload identified is metrics ring buffer at ~4 writes/sec — SQLite handles that in its sleep.
- **Deployment complexity.** Helm chart goes from ~10 templates to include CNPG cluster CR or a StatefulSet + PVC + Secret + Service + PodDisruptionBudget for the sidecar.
- **Startup ordering.** App needs to wait for Postgres readiness — flakier local dev.
- Contradicts the "single binary, single pod" ethos in `docs/ARCHITECTURE.md`.

**When it's the right call:** if deckwatch grows into a multi-tenant SaaS with 100s of clusters and shared centralized audit. Not now.

**Effort to adopt:** ~4-6 weeks (Helm chart rewrite, ops runbook, CI test bring-up).

---

### Option D: Hybrid (recommended)

**Model**
1. **K8s-native state stays in K8s.** Deployments, Pods, ReplicaSets, Services, Ingresses, CronJobs, Events, Secrets — never move.
2. **Lightweight app config stays in ConfigMaps.** Global settings (`deckwatch-config`), application definitions (`deckwatch-app-<name>`), app templates (roadmap). These are:
   - Small (< 100 KB each, well under the 1 MB limit)
   - Written rarely (human-initiated, not automated)
   - Restart-safe by nature
   - `kubectl`-inspectable, which matches how operators debug the tool
   - Racing on them is fine (last-write-wins via `Patch::Apply` with field manager)
3. **GitOps annotations on Deployments stay put.** They're a natural per-Deployment concern; moving them out would require a `(namespace, name) → row` mapping and cross-store consistency management. Zero benefit.
4. **Addon annotations on pod templates stay put.** Same reason.
5. **New durable state goes into SQLite** — behind a PVC, with graceful degradation.
   - Audit log
   - Metrics ring buffer (Phase 2 from `METRICS_VISUALIZATION.md`)
   - Diagnostic history (beyond the current 1h Job TTL)
   - Notification rules
   - Cached branch/registry queries (with TTL)
   - RBAC policy (once auth lands — could arguably stay in a ConfigMap; put in DB if we want audit history on policy changes, which we do)
   - Per-user preferences and session state

**Pros**
- Every concern uses the storage that fits it — no square-peg-round-hole.
- Existing code doesn't churn. Only new features touch SQLite.
- Graceful degradation: deckwatch without a PVC still does everything it does today. Auth/audit/metrics-history/notification-rules become the features that "require persistence".
- Helm chart change is additive (opt-in PVC block); existing users don't have to opt in until they need the features.
- Matches Rust ecosystem conventions — many single-binary services do exactly this (Grafana Loki's boltdb-shipper, Vector's disk buffers, Tempo's WAL).

**Cons**
- **Two storage systems to reason about.** New contributors have to learn "when does state go in a ConfigMap vs SQLite?" — mitigated by a written rubric (below).
- **HA still not achievable** for the DB-backed features without work — same as pure Option B.
- **Backup strategy needed** for the SQLite file — mitigated by using PVC snapshots or a periodic `VACUUM INTO` to object storage.

**Rubric for "which store"**
- If it's a K8s resource, use K8s.
- If it's small, changes rarely, and benefits from being `kubectl`-visible, use a ConfigMap.
- If it's per-K8s-object metadata that follows that object's lifecycle, use annotations on the object.
- If it has audit semantics, timeseries semantics, per-user semantics, or needs indexes/joins/transactions across rows, use SQLite.

---

## 4. Recommendation and Rationale

**Adopt Option D. Introduce SQLite via `rusqlite` for new concerns only, keep existing ConfigMap/annotation model unchanged.**

### Why not Option A (do nothing)?

Because at least 4 P0/P1 features on the roadmap require state that ConfigMaps cannot serve without ugly workarounds:
- P0 #4 Auth requires an **audit log** — the whole point is having a history that can't be lost or forged. A 1 MB ConfigMap holds ~4000 audit rows (assuming 250 B/row). At even 100 mutating actions/day that's a 40-day retention ceiling with no query capability.
- P0 #5 Metrics — Phase 2 in `docs/METRICS_VISUALIZATION.md` explicitly calls for a 15-minute client-side ring buffer, but Phase 3 (Prometheus range queries) is gated on "cluster has Prometheus". Many target clusters won't. A server-side SQLite ring buffer bridges that gap without the operational cost of Prometheus.
- P1 Notifications — routing rules with change history need row-level updates and audit.
- The already-listed TODO ("Add diagnostic history list endpoint") is blocked by the fact that diagnostic Jobs auto-delete after 1h.

### Why not Option B (put everything in SQLite)?

Because it takes something that works and makes it worse:
- Migrating settings out of ConfigMap loses `kubectl get configmap deckwatch-config -o yaml` as a debugging tool.
- Migrating GitOps annotations off Deployments creates a cross-store consistency problem (Deployment deleted → orphan row in DB) that owner-references solve for free with the current model.
- Application ConfigMaps *are* how the frontend queries "list apps in this namespace" — moving them into SQLite means Deckwatch must be running to answer that question, whereas today `kubectl` can.
- Adds ops burden (PVC required for restart-safe operation of features that currently need no persistence) with no compensating benefit.

### Why not Option C (Postgres)?

Because SQLite handles the identified workload with ~2 orders of magnitude of headroom, and Option C's HA benefit is moot until deckwatch is genuinely HA (which itself is a much larger architectural project — session affinity, Prometheus federation across replicas, leader-election for the git watcher, etc.). If we ever get to that point, migrating SQLite → Postgres is a straightforward `sqlx`-abstracted swap; committing to Postgres now buys nothing.

### One-line summary
Right-size each concern to its storage: K8s owns K8s state; ConfigMaps own config; annotations own per-object metadata; SQLite owns audit, history, timeseries, and per-user state — with the PVC opt-in so today's users see no change.

---

## 5. Migration Plan

**Zero migration is required for existing state.** The plan is additive.

### Phase 0 — Foundation (1 sprint)
- [ ] Add crates: `rusqlite` (bundled feature), `tokio-rusqlite` (async wrapper), `refinery` (migrations). Bundle sqlite so no libsqlite3 runtime dep.
- [ ] `src/db/mod.rs` — `DbHandle` wrapping `tokio_rusqlite::Connection`
- [ ] `src/db/migrations/` — versioned `.sql` files, run on startup
- [ ] `AppState::db: Option<DbHandle>` — `None` when PVC unavailable
- [ ] CLI/env flag: `--db-path` / `DECKWATCH_DB_PATH` (default: `/data/deckwatch.db`; env-unset = disabled)
- [ ] Health check: `/readyz` reports `db: enabled|disabled|failed`
- [ ] Helm: opt-in `persistence.enabled: false` block adds a PVC and mounts `/data`
- [ ] Error taxonomy: `AppError::PersistenceUnavailable` → 503 with `X-Deckwatch-Missing-Persistence: true` header so the frontend can render "feature requires persistence" panels

### Phase 1 — Audit log (pilot use case)
- [ ] Schema: `audit_events(id, ts, user, action, namespace, resource_kind, resource_name, diff_json)`
- [ ] Indexes: `(ts DESC)`, `(namespace, ts DESC)`, `(user, ts DESC)`
- [ ] Middleware in `routes.rs` — wraps every mutating handler, records on success
- [ ] Handler: `GET /api/audit?ns=&user=&after=&limit=`
- [ ] Retention job: nightly `DELETE FROM audit_events WHERE ts < ?` (configurable, default 90d)
- [ ] Frontend: Audit tab on ClusterOverview
- [ ] Ties into P0 #4 (Auth) — audit works standalone but really shines with OIDC identity attached

### Phase 2 — Metrics ring buffer
- [ ] Schema: `pod_metrics(pod_uid, container, ts, cpu_millicores, mem_bytes)` — `PRIMARY KEY (pod_uid, container, ts)`
- [ ] Poller: scrape metrics-server every 15s, insert
- [ ] Retention: 15-minute sliding window (`DELETE WHERE ts < now - 900s`) — or configurable
- [ ] Handler: `GET /api/namespaces/{ns}/deployments/{name}/metrics/history?window=15m`
- [ ] Wires into `docs/METRICS_VISUALIZATION.md` Phase 2

### Phase 3 — Diagnostic history persistence
- [ ] Schema: `diagnostic_runs(job_name, ns, source_pod, agent, status, started_at, completed_at, output_summary)`
- [ ] `handlers/diagnostics.rs::create_diagnostic` writes row on create
- [ ] Job monitor updates status/completed_at
- [ ] Removes dependency on Job TTL for history
- [ ] Closes TODO §AI Diagnostics: "Add diagnostic history list endpoint"

### Phase 4 — Notification rules, cached branch/registry lists, user preferences
- [ ] As each roadmap item lands, add a new table + migration
- [ ] Follow the pattern: schema → handler → frontend

### Non-goals for this migration
- No moving existing settings/apps out of ConfigMaps.
- No moving GitOps or addon annotations out of K8s objects.
- No HA / multi-writer story yet — deckwatch stays `replicaCount: 1` for the DB path.

---

## 6. Impact on Helm Chart and Deployment Model

### 6.1 Helm chart changes

**New template:** `helm/deckwatch/templates/pvc.yaml` (guarded by `persistence.enabled`)
```yaml
{{- if .Values.persistence.enabled }}
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {{ include "deckwatch.fullname" . }}-data
  labels: {{- include "deckwatch.labels" . | nindent 4 }}
spec:
  accessModes: [{{ .Values.persistence.accessMode | default "ReadWriteOnce" | quote }}]
  resources:
    requests:
      storage: {{ .Values.persistence.size | default "1Gi" | quote }}
  {{- with .Values.persistence.storageClass }}
  storageClassName: {{ . }}
  {{- end }}
{{- end }}
```

**Deployment template additions:**
- `volumeMounts` entry for `/data` when `persistence.enabled`
- `volumes` entry backed by the PVC
- `env`: `DECKWATCH_DB_PATH=/data/deckwatch.db` when enabled

**`values.yaml` addition:**
```yaml
persistence:
  # Enables the embedded SQLite store used by audit log, metrics history,
  # diagnostic history, and notification rules. Without it, those features
  # return 503 and their UI is disabled — the rest of Deckwatch works as before.
  enabled: false
  storageClass: ""
  accessMode: ReadWriteOnce
  size: 1Gi
```

### 6.2 Deployment model consequences

| Aspect | Before | After (persistence disabled) | After (persistence enabled) |
|---|---|---|---|
| Restart-safe | yes | yes | yes (PVC survives pod restarts) |
| Node-safe | yes | yes | depends on storage class (RWO + AZ-local = tied to AZ) |
| `replicaCount > 1` | untested, probably works | untested, probably works | **broken** — SQLite single-writer. Enforce with a warning in NOTES.txt |
| Backup story | not needed | not needed | operator's responsibility: PVC snapshots, or add a `CronJob` sidecar that `VACUUM INTO`s to S3 (candidate follow-up) |
| Data lost on PVC failure | n/a | n/a | audit log, metrics history, diagnostic history lost; K8s-native and CM state unaffected |
| Local dev (k3d) | works | works | works — k3d has a default local-path storage class |

### 6.3 CI test bring-up
- Existing kind-in-CI plan (`TODO.md` §Testing) needs a PVC provisioner (kind's default `standard` StorageClass covers this).
- Integration tests for DB-backed features should spin up SQLite in-memory (`":memory:"`) rather than requiring a PVC — the `DbHandle` should accept either path.

### 6.4 RBAC changes
None. SQLite is a file, not a K8s resource. The existing ClusterRole/ClusterRoleBinding is unchanged.

### 6.5 Metrics endpoint impact
Add gauges: `deckwatch_db_enabled` (0/1), `deckwatch_db_size_bytes`, `deckwatch_audit_events_total{action=...}`. Cheap; useful for capacity planning on the PVC.

---

## 7. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Migration file goes wrong and startup fails | low | high (deckwatch unavailable) | `refinery` runs migrations idempotently; on failure, fall back to `db = None` and log-loud rather than crash |
| PVC lost, audit history gone | medium | high (compliance) | Document PVC backup as operator responsibility; add optional S3-backup CronJob in a follow-up |
| Operator scales to 2+ replicas, corrupts SQLite | medium | high | Deployment strategy `Recreate` + `replicas: 1` hard-coded in the template when `persistence.enabled`; NOTES.txt warning |
| SQLite write throughput becomes a bottleneck | very low | medium | WAL mode + batched inserts; if we ever hit it, swap to Postgres via `sqlx` abstraction |
| Contributors put things in the wrong store | medium | low | Rubric in `docs/STATE_MANAGEMENT.md` (new); code review discipline |
| Backup / restore untested during actual DR | high | high | Add a runbook + rehearsal to the "before production" checklist |

---

## 8. Open Questions

1. Should the audit log be **append-only** (no UPDATE/DELETE, retention via table partitioning) for compliance posture? Recommend yes — enables "prove this record wasn't altered" audits.
2. Should we adopt `sqlx` (compile-time-checked SQL, async-native, more setup) or stick with `rusqlite` + `tokio-rusqlite` (simpler, no build-time DB required)? Recommend the latter for now; `sqlx` is a straightforward upgrade later.
3. When auth lands, do RBAC policies go in a ConfigMap (kubectl-editable) or the DB (audit-tracked)? Recommend the DB — the whole point of a policy system is knowing when it changed and by whom.
4. Do we need a formal `Container` / `Adapter` / `Repository` layer separating handlers from storage, or is `DbHandle` passed via `AppState` sufficient? Recommend the latter until a second storage backend is a real possibility.
5. Should metrics history default to 15-minute retention (matches Phase 2 doc), or something longer? Configurable, default 15 min — Prometheus is the answer for longer.

---

## Appendix A: References

- `src/handlers/settings.rs` — settings in ConfigMap
- `src/handlers/applications.rs` — applications in ConfigMaps + labels for membership
- `src/handlers/gitops.rs` — GitOps config as Deployment annotations
- `src/handlers/addons.rs` — addon tracking as pod-template annotations
- `src/handlers/diagnostics.rs` — diagnostic Job + ConfigMap for log data (1h Job TTL is the "diagnostic history disappears" issue)
- `src/watcher.rs` — poller state as Deployment annotations
- `docs/PRODUCT_ROADMAP.md` — P0/P1/P2 slate; especially §Top 5 Recommendations for auth + metrics
- `docs/METRICS_VISUALIZATION.md` — the metrics ring buffer motivation
- `docs/AUTH.md` — audit-log rationale
- `TODO.md` §Architecture Decisions — this ticket's origin
- `helm/deckwatch/values.yaml` — no persistence today; needs additive block
- `Cargo.toml` — dependency list; SQLite additions land here

## Appendix B: Suggested first-migration schema (illustrative)

```sql
-- V1__init.sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE audit_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts            INTEGER NOT NULL, -- unix millis
    actor         TEXT NOT NULL,    -- OIDC sub, or "anonymous" pre-auth
    action        TEXT NOT NULL,    -- e.g. "deployment.scale", "gitops.trigger"
    namespace     TEXT,
    resource_kind TEXT,
    resource_name TEXT,
    diff_json     TEXT              -- serde_json::Value serialized
);
CREATE INDEX idx_audit_ts       ON audit_events (ts DESC);
CREATE INDEX idx_audit_ns_ts    ON audit_events (namespace, ts DESC);
CREATE INDEX idx_audit_actor_ts ON audit_events (actor, ts DESC);

CREATE TABLE schema_notes (
    version INTEGER PRIMARY KEY,
    note    TEXT
);
INSERT INTO schema_notes (version, note) VALUES (1, 'initial audit-log-only schema');
```
