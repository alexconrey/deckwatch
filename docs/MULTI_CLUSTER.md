# Multi-Cluster Support

**Status:** Design proposal
**Date:** 2026-07-10
**Scope:** Extend deckwatch from a single-cluster dashboard to a multi-cluster
control plane, with a cluster selector at the top of the UI, per-cluster kube
clients, per-cluster RBAC, and cleanly-scoped settings.
**Related:** `docs/PRODUCT_ROADMAP.md` §Open Question 2, `docs/ARCHITECTURE_DECISION.md`
(SQLite-backed cluster registry lands in Phase 4)

---

## TL;DR

Introduce a `ClusterRegistry` behind `AppState`, keyed by a short `cluster_id`
(URL-safe slug). Every path that accepts `{ns}` today gets a
`{cluster}/{ns}` prefix; the frontend adds a cluster picker in the top bar
whose value is passed with every API request as a path segment. Cluster
credentials live in K8s Secrets in deckwatch's own namespace, referenced from a
`ClusterEntry` row in the SQLite `clusters` table. Migration is fully additive:
the old single-cluster mode is preserved as a synthetic `default` cluster with
the in-cluster ServiceAccount kubeconfig, so existing deployments keep working
without any user action.

The lift is **XL** — every handler, every route, every frontend store, and the
Helm chart all change. It only makes sense if we're committing to multi-cluster
as a first-class product surface (roadmap Open Question 2). If the answer is
"maybe someday" this design is over-scoped; a lighter alternative — one
deckwatch pod per cluster, with an external "hub" that iframes them — is
sketched in §7 for comparison.

---

## 1. Motivation

The roadmap raises multi-cluster as an open product question. Two realistic
target shapes:

- **Environment tiers.** dev / staging / prod clusters, same team, same apps.
  Users want to compare "is the same image running in staging as in prod?" and
  promote deployments across them. This is the common case for a K2-style org.
- **Fleet / edge.** N clusters, mostly independent, users pick one and drill
  in. Rarely need cross-cluster views. This is the CoreWeave / Rancher case.

Both shapes are served by the same architecture: cluster is a first-class
selector, all resource paths are cluster-scoped, and *cross-cluster* views
are opt-in aggregations layered on top.

The current model has one `kube::Client` in `AppState` (`src/state.rs:14`)
built from in-cluster config in `main.rs`. Every handler pulls
`state.kube_client.clone()` via typed helpers like `deployments_api(ns)`.
Every REST path starts at `/api/namespaces/{ns}/…`.

---

## 2. Design overview

### 2.1 URL shape

Add a `/api/clusters/{cluster}` prefix to every namespace-scoped and
cluster-scoped API. Retain the un-prefixed `/api/namespaces/…` paths as
aliases that resolve to the default cluster, for backward compat during the
transition.

```
BEFORE
GET  /api/namespaces
GET  /api/namespaces/{ns}/deployments
POST /api/namespaces/{ns}/deployments/{name}/scale

AFTER
GET  /api/clusters                                     ← new: list clusters
GET  /api/clusters/{cluster}/namespaces
GET  /api/clusters/{cluster}/namespaces/{ns}/deployments
POST /api/clusters/{cluster}/namespaces/{ns}/deployments/{name}/scale
GET  /api/clusters/{cluster}/nodes                     ← cluster-scoped resource
```

Everything cluster-scoped in K8s (Nodes, Namespaces, ClusterRoles) sits at
`/api/clusters/{cluster}/…`. Everything namespace-scoped keeps its
`/namespaces/{ns}` segment.

The `{cluster}` slug is validated against `^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$`
(K8s label rules) so it survives being used as a metric label, an audit
column, and a URL segment.

### 2.2 Data model

`ClusterEntry` in a new `clusters` SQLite table (see
`docs/ARCHITECTURE_DECISION.md` §5 for the SQLite foundation):

```sql
CREATE TABLE clusters (
    id                TEXT PRIMARY KEY,           -- slug, matches URL
    display_name      TEXT NOT NULL,
    api_endpoint      TEXT NOT NULL,              -- https://k8s.example:6443
    kubeconfig_secret TEXT NOT NULL,              -- Secret name in deckwatch ns
    kubeconfig_key    TEXT NOT NULL DEFAULT 'kubeconfig',
    context           TEXT,                       -- optional: which context in the file
    default_flag      INTEGER NOT NULL DEFAULT 0, -- exactly one row has this
    added_by          TEXT NOT NULL,
    added_at          INTEGER NOT NULL,
    labels_json       TEXT NOT NULL DEFAULT '{}'  -- e.g. {"env":"prod","region":"us-east-1"}
);
CREATE UNIQUE INDEX idx_clusters_default_singleton
    ON clusters (default_flag) WHERE default_flag = 1;
```

Kubeconfig material lives in a K8s `Secret` in deckwatch's own namespace, not
in the DB, so:
- rotating credentials is `kubectl edit secret` (no DB migration)
- existing K8s RBAC governs who can read them
- the audit story for "who changed the kubeconfig" is K8s events + our audit
  log at the point-of-use ("cluster X marked unhealthy")

The `Secret` may hold either:
- a full kubeconfig YAML in the `kubeconfig` key, or
- individual fields (`server`, `token`, `ca.crt`, `client-cert`, `client-key`)
  for the common ServiceAccount-token case

Loader auto-detects which form based on keys present.

### 2.3 State model change

`AppState` today holds a single `kube_client` and `allowed_namespaces`
(`src/state.rs:12-23`). It becomes:

```rust
#[derive(Clone)]
pub struct AppState {
    pub clusters: Arc<ClusterRegistry>,
    pub settings_namespace: String,
    pub settings_configmap_name: String,
    pub registry_public_url: Option<String>,
    // audit + db handle from ARCHITECTURE_DECISION land alongside
    pub db: Option<crate::db::DbHandle>,
}

pub struct ClusterRegistry {
    // Read-heavy, write-rare — RwLock is fine.
    inner: RwLock<HashMap<String, ClusterHandle>>,
    default_id: RwLock<String>,
}

pub struct ClusterHandle {
    pub id: String,
    pub display_name: String,
    pub client: kube::Client,
    pub allowed_namespaces: Vec<String>,        // per-cluster, not global
    pub labels: BTreeMap<String, String>,
    pub health: watch::Receiver<ClusterHealth>, // liveness signal
}

pub enum ClusterHealth {
    Healthy,
    Degraded { last_ok: DateTime<Utc>, reason: String },
    Unreachable { since: DateTime<Utc>, reason: String },
}
```

All the current `deployments_api(ns)`, `pods_api(ns)`, … helpers on
`AppState` take an extra leading `cluster_id: &str` and delegate to the
resolved `ClusterHandle`:

```rust
impl AppState {
    pub fn cluster(&self, cluster_id: &str) -> Result<Arc<ClusterHandle>, AppError> {
        self.clusters.get(cluster_id)
            .ok_or_else(|| AppError::ClusterNotFound(cluster_id.to_string()))
    }

    pub fn deployments_api(&self, cluster: &str, ns: &str) -> Result<Api<Deployment>, AppError> {
        let ch = self.cluster(cluster)?;
        ch.check_namespace(ns)?;
        Ok(Api::namespaced(ch.client.clone(), ns))
    }
    // …same shape for every other typed API
}
```

Each handler picks up the cluster from a new axum `Path` extractor. See §3.

### 2.4 Cluster liveness

Each `ClusterHandle` gets a lightweight liveness probe: a task that hits
`GET /version` on the API server every 30s, updates the `watch::Receiver`, and
emits `deckwatch_cluster_healthy{cluster=...}` as a gauge. This is what
powers the "grey dot" in the cluster picker when a kubeconfig has expired or
the API server is partitioned — critical because the alternative (finding out
via a request timeout) is a bad first-user experience.

Handlers do **not** block on liveness. A request to an unreachable cluster
gets the underlying `kube::Error` bubbled through as `AppError::ClusterUnreachable`
with the last-known reason, which the frontend renders as a banner rather
than a spinner-of-death.

---

## 3. Backend implementation sketch

### 3.1 Cluster extractor

New axum extractor that resolves `{cluster}` → `Arc<ClusterHandle>` in one
place, so handlers get a typed handle instead of a string:

```rust
// src/handlers/cluster_extract.rs
pub struct ClusterCtx(pub Arc<ClusterHandle>);

#[async_trait]
impl FromRequestParts<AppState> for ClusterCtx {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let cluster_id: Path<String> = Path::from_request_parts(parts, state).await
            .map_err(|_| AppError::BadRequest("missing cluster path segment".into()))?;
        let handle = state.cluster(&cluster_id.0)?;
        Ok(Self(handle))
    }
}
```

Handlers get shorter, not longer:

```rust
// BEFORE
pub async fn list(State(state): State<AppState>, Path(ns): Path<String>) -> Result<Json<Vec<…>>, AppError> {
    let api = state.deployments_api(&ns)?;
    …
}

// AFTER
pub async fn list(
    ClusterCtx(cluster): ClusterCtx,
    Path((_cluster_id, ns)): Path<(String, String)>,
) -> Result<Json<Vec<…>>, AppError> {
    let api: Api<Deployment> = Api::namespaced(cluster.client.clone(), &ns);
    cluster.check_namespace(&ns)?;
    …
}
```

(The double-extraction of `_cluster_id` is unavoidable because axum's `Path`
extractor is greedy; the `ClusterCtx` handles resolution + errors, the `Path`
gives us the `ns` piece.)

### 3.2 Route restructuring

`routes.rs` currently mounts everything at `/api/namespaces/…`. Restructure
via a nested router:

```rust
let cluster_scoped = Router::new()
    .route("/namespaces", get(namespaces::list).post(namespaces::create))
    .route("/namespaces/{ns}/deployments", …)
    // …all existing routes, minus their /api/namespaces prefix
    .route("/nodes", get(nodes::list))
    // cluster-scoped resources
    ;

let private_api = Router::new()
    .route("/api/clusters", get(clusters::list).post(clusters::add))
    .route("/api/clusters/{cluster}", get(clusters::get).delete(clusters::remove))
    .route("/api/clusters/{cluster}/health", get(clusters::health))
    .nest("/api/clusters/{cluster}", cluster_scoped.clone())
    // Back-compat alias: unprefixed paths hit the default cluster
    .nest("/api/namespaces", cluster_scoped_default_alias(&state))
    ;
```

The back-compat alias is a very thin router that rewrites the request into
the default cluster's context. It stays until Q3 of the migration then
gets deleted with a deprecation-header warning issued for the interim.

### 3.3 Watcher and background tasks

`watcher::run_poller` (`src/watcher.rs:35`) iterates over
`state.allowed_namespaces` and polls each. It becomes iteration over
`(cluster, namespace)` pairs:

```rust
pub async fn run_poller(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    loop {
        interval.tick().await;
        for cluster in state.clusters.all() {
            if !cluster.health.borrow().is_healthy() { continue; }
            for ns in cluster.namespaces_to_poll() {
                if let Err(e) = poll_cycle(&cluster, &ns, &http).await {
                    tracing::error!(cluster=%cluster.id, ns=%ns, error=%e, "poll failed");
                }
            }
        }
    }
}
```

Metrics get a `cluster` label added everywhere (Prometheus cardinality: N
clusters × existing dimensions — usually fine, since N is in the single
digits for the environment-tier shape, and never above ~50 for fleet).

### 3.4 Adding, editing, removing clusters

New handlers in `src/handlers/clusters.rs`:

- `GET /api/clusters` — list, includes `health` and `is_default`
- `POST /api/clusters` — body: `{id, display_name, kubeconfig, labels?}`.
  Writes the Secret, writes the row, spins up a `ClusterHandle`, runs a
  test `GET /version` before returning 201.
- `GET /api/clusters/{id}` — details + health
- `PUT /api/clusters/{id}` — update display name / labels / allowed namespaces
  (kubeconfig rotation is `kubectl edit secret …` — surface a "rotate
  credentials" button in the UI that opens a Secret-edit modal)
- `DELETE /api/clusters/{id}` — refuse if it's the default; drop the row,
  drop the Secret, drop the handle
- `POST /api/clusters/{id}/set-default` — flips `default_flag`

All of these are audit-logged; adding/removing a cluster is a high-impact
event.

### 3.5 Handling the "no DB" case

The SQLite-based design assumes the persistence layer from
`ARCHITECTURE_DECISION.md`. If persistence is disabled:

- exactly one cluster is available (the in-cluster ServiceAccount, id
  `default`), matching today's behavior
- `POST /api/clusters` returns `503 X-Deckwatch-Missing-Persistence: true`
- the frontend hides the "add cluster" button and shows the picker as a
  static single-item dropdown

This is the graceful-degradation stance from the SQLite doc: multi-cluster
becomes one of the features that "requires persistence".

---

## 4. Frontend implementation sketch

### 4.1 Cluster selector

A new component in the top bar (currently the app title + user menu).
Layout: `[Cluster ▾]` positioned before the existing namespace picker,
because switching cluster resets the namespace picker (a namespace in
cluster A does not exist in cluster B by default). Skeleton:

- **Store:** new `frontend/src/stores/cluster.ts` with `currentCluster: Ref<string>`,
  `clusters: Ref<ClusterSummary[]>`, `refresh()`, `switchTo(id)`. Persists
  `currentCluster` to `localStorage` so a refresh doesn't reset.
- **Component:** `frontend/src/components/common/ClusterPicker.vue` — dropdown
  with health indicator dot next to each name (green/yellow/red).
- **Router guard:** on navigation, if the URL contains `/clusters/{id}/…`
  make sure the store matches; if not, rewrite the URL and cluster store
  atomically. This is the "shareable link to a specific cluster" case.

### 4.2 API client

`frontend/src/api/client.ts` today builds URLs like `/api/namespaces/{ns}/…`.
Wrap it with a `withCluster()` helper that reads from the cluster store and
prepends `/api/clusters/{id}` transparently. This keeps every existing
`api/*.ts` module's public surface unchanged — the callers don't have to
thread cluster IDs through every function.

Where a page genuinely needs cross-cluster (rare — see §5), it opts out with
`api.forCluster(id).deployments.list(ns)` explicitly.

### 4.3 Namespace store scoping

The namespace store currently caches `namespaces: Namespace[]`. It becomes
keyed by cluster: `namespacesByCluster: Record<string, Namespace[]>`. When
the cluster switches, the current namespace validity is checked; if the
current namespace doesn't exist in the new cluster, the picker falls back
to the first allowed one and the URL updates.

### 4.4 Deployments store scoping

Same shape — `deploymentsByCluster: Record<string, Deployment[]>`. Also
holds a per-cluster last-updated timestamp so the 5s polling can be
suspended for background clusters (only poll the currently-viewed cluster,
poll others lazily on switch). This keeps the API load roughly constant
regardless of how many clusters are registered.

### 4.5 Cluster management UI

New `SettingsClustersPage.vue` under Settings:

- table: id, display name, endpoint, health, labels, default flag, actions
- "Add cluster" modal: paste kubeconfig, name, labels, allowed namespaces
- "Test connection" button that hits `POST /api/clusters/{id}/test` (a new
  endpoint) which reruns the `GET /version` liveness probe on demand
- "Rotate credentials" opens the underlying Secret in the deckwatch
  namespace via the existing Secrets UI

---

## 5. Settings scoping — per-cluster vs global

The current settings shape (`helm/deckwatch/values.yaml` §settings and
`docs/SETTINGS.md`) treats everything as global to the single cluster. That
splits into three tiers when multi-cluster lands:

| Setting | Scope | Where it lives | Why |
|---|---|---|---|
| Cluster registry | **Global** | SQLite `clusters` + K8s Secrets in deckwatch ns | It's how you get anywhere at all |
| Auth (OIDC issuer, tenant, client id) | **Global** | ConfigMap `deckwatch-config` | Users log into deckwatch, not into a cluster; one identity, N cluster memberships |
| Audit log | **Global** | SQLite `audit_events` with `cluster` column | One pane of glass for "who did what where" |
| Notifications routing | **Per-cluster** | SQLite `notification_rules(cluster, …)` | An OOM in prod pages the on-call; the same OOM in a dev cluster goes to a low-priority channel |
| Allowed namespaces | **Per-cluster** | `clusters.allowed_namespaces` (JSON column, or separate table) | Same team may have `team-a` in every cluster but only manage prod `team-a` |
| RBAC (once auth lands) | **Both** | Roles are global (viewer/editor/admin), bindings are `(user, cluster, namespace, role)` | A viewer in dev may be an editor in prod, or vice versa |
| Default resource limits | **Per-cluster** | ConfigMap `deckwatch-config-{cluster}` OR JSON column on `clusters` row | Prod may allow larger limits than dev |
| Git repositories, OCI registries, git-token secrets | **Global** | ConfigMap `deckwatch-config` | These are org-wide managed lists (roadmap TODO §GitOps) |
| App templates | **Global** | ConfigMap `deckwatch-templates` | Same reason — one authoring surface for the whole org |
| Applications (groups of deployments) | **Per-cluster** | ConfigMap `deckwatch-app-<name>` **in the managed cluster's namespace** (as today) | An app in dev is a different instance than the app in prod, even if they share a name |

Rule of thumb: **user-facing routing/policy is per-cluster; identity and
platform vocabulary is global.**

---

## 6. RBAC — per-cluster

There are two RBACs to reason about:

- **K8s RBAC (deckwatch → each cluster).** The kubeconfig for each cluster
  determines what deckwatch itself can do there. Deckwatch does not attempt
  to impersonate the end-user against the API server (that's a much larger
  design and hits kube-apiserver `--enable-impersonation` requirements).
  Per-cluster kubeconfigs can have narrower ClusterRoles — e.g. dev
  cluster's SA gets full read/write, prod cluster's SA gets read + rollback
  only. This is the operator's control point, not the app's.
- **Deckwatch RBAC (end-user → deckwatch → cluster).** Once auth (roadmap
  P0 #4) lands, the policy row shape becomes `(subject_group, cluster,
  namespace, role)` — one extra column vs the single-cluster case. All
  authorization checks in handlers go `authorize(user, cluster.id, ns,
  Action::Scale)` instead of `authorize(user, ns, Action::Scale)`.

For clusters where you want operator-side hard-guarantees (not just "the app
promises to check"), give deckwatch a **narrower kubeconfig** for that
cluster. Belt-and-suspenders.

---

## 7. Migration path

Fully additive; existing single-cluster deployments keep working with zero
config change.

### Phase 0 — foundation (1 sprint)
- Add `clusters` table + migration (SQLite from ARCHITECTURE_DECISION §5)
- Introduce `ClusterRegistry` and `ClusterHandle`; keep `AppState` API
  surface but internally route to `default` cluster
- Startup path: seed the `default` row from in-cluster ServiceAccount if
  the table is empty; store the derived kubeconfig in a Secret named
  `deckwatch-cluster-default`
- No route changes yet — everything still works via `/api/namespaces/…`

### Phase 1 — clusters API + selector (2 sprints)
- New `/api/clusters/…` routes; nest existing routes under
  `/api/clusters/{cluster}` and add the un-prefixed alias for back-compat
- Cluster picker in the frontend top bar; single-cluster deployments see it
  as a static one-item dropdown (still shows health)
- Add-cluster UI + backend handlers behind a feature flag
  (`clusters.enabled: false` in `values.yaml`; default false)

### Phase 2 — per-cluster settings + watcher (2 sprints)
- Watcher loops over `(cluster, ns)`
- Notification rules migrated to per-cluster
- Metrics get `cluster` label
- Frontend stores keyed by cluster

### Phase 3 — cross-cluster views (opt-in, 1 sprint)
- "Fleet overview" page: N cluster health cards
- "Compare deployments" page: side-by-side same-named deployment across
  selected clusters (useful for env-tier shape)
- These are additive; if a user never opens them they cost nothing

### Phase 4 — remove back-compat alias
- Deprecation-header warning issued on `/api/namespaces/…` starting
  Phase 1; deletion in Phase 4 (~2 quarters out)

### Migration mechanics for existing installs
- Upgrade to a `clusters.enabled: false` build: no visible change. Under
  the hood the `default` cluster row exists in SQLite; everything runs
  against it exactly like before.
- Flip `clusters.enabled: true`: picker appears; users see one cluster
  (their existing one) and can add more.
- No data migration required; no user-triggered migration step. The
  bad-day rollback is `clusters.enabled: false` again.

---

## 8. Alternative considered: "hub of pods"

Instead of teaching deckwatch multi-cluster, run one deckwatch pod per
cluster (that's already what you'd do today) and build a thin **hub** — a
static site with an iframe per cluster or a URL-router that redirects to
`deckwatch-{env}.example.com`. Pros: no code change in deckwatch, each
cluster's blast radius is contained. Cons: no cross-cluster audit log, no
"compare deployments" view, no unified identity (users log in N times), no
shared managed-repo/template lists, per-cluster upgrades happen at N
different times.

Recommended answer: **hub of pods is fine as a stopgap** for organizations
that just want dev/staging/prod isolation. First-class multi-cluster is
worth doing only if the roadmap commits to fleet-scale (N > 3) or cross-
cluster views.

---

## 9. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| kubeconfig expires silently, users see spinner-of-death | high | medium | Liveness watcher + red-dot in picker + clear error banner |
| Metric cardinality explodes at fleet scale | medium (fleet shape only) | low | Cap at 50 clusters by config; per-cluster subset in ServiceMonitor rules |
| Cluster registry grows without bound (users forget to delete stale clusters) | medium | low | UI shows "unhealthy > 7 days" with a delete prompt |
| Cross-cluster views become a slow N+1 API call | medium | medium | Parallelize with a bounded semaphore (max 8 concurrent), cache summaries in the store for 15s |
| A cluster kubeconfig is compromised → blast radius is N-cluster wide | low | high | Per-cluster kubeconfigs (narrow ClusterRoles); audit-log all cluster-registry mutations; document rotation procedure |
| Users get confused about which cluster they're on | medium | high (foot-gun for destructive actions) | Cluster label always visible in top bar; destructive-action modals repeat the cluster name in the confirm text; header color-code (env-tier labels drive it) |
| Back-compat alias becomes forever tech debt | medium | low | Deprecation-header warning + explicit removal date in the changelog |

---

## 10. Open questions

1. Do we want **cluster impersonation** (deckwatch forwards the end-user's
   OIDC identity to the K8s API server as `Impersonate-User`) or the
   app-level RBAC-only model? Impersonation gives cluster admins one place
   to enforce policy but requires `--enable-impersonation` on the API
   server, which many hosted control planes disable. Recommend app-level
   RBAC as the primary model, impersonation as an opt-in for organizations
   that want it.
2. Should **applications** span clusters (i.e. "my-app in dev + my-app in
   prod = one Application")? Recommend no — keep the current
   ConfigMap-in-cluster model. Cross-cluster "compare" is a view, not a
   model.
3. **Audit log location** — write per-cluster events to the global audit
   table (with a `cluster` column) or shard by cluster? Recommend global
   table; the query pattern is "show me everything user X did in the last
   hour", not "show me everything in cluster Y".
4. Do we need a **cluster-selector-less** deep-link (e.g. `?cluster=prod`
   query string) for external tools that don't want to hardcode
   `/api/clusters/prod/…`? Cheap to add if a use case emerges; skip until
   asked.

---

## 11. References

- `src/state.rs` — `AppState`, the touch-point for the client change
- `src/config.rs` — `namespaces` becomes per-cluster; CLI flag semantics
  change
- `src/routes.rs` — router restructuring
- `src/watcher.rs:35` — poller loop becomes cluster-aware
- `docs/ARCHITECTURE_DECISION.md` §2.2 — cluster registry named as a
  candidate DB-backed table
- `docs/PRODUCT_ROADMAP.md` §Open Question 2 — the product question this
  document answers
- `docs/AUTH.md` — RBAC policy shape gets the `cluster` column
- `helm/deckwatch/values.yaml` — `clusters.enabled` and the seeded
  ClusterRole/RoleBinding for the in-cluster default remain
