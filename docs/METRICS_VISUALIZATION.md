# Deckwatch — Metrics Visualization Design

**Status:** Design proposal
**Owner:** Platform / Deckwatch
**Related roadmap item:** P0 #5 — Resource Usage Metrics (`docs/PRODUCT_ROADMAP.md:44`)
**Prototype code:** `src/handlers/resource_metrics.rs` in this staging tree

---

## 1. Problem

Deckwatch currently answers "are my replicas running?" but not "is my
app healthy and sized correctly?". Users cannot see CPU or memory usage,
cannot spot OOM pressure, cannot see request rates. Adding this closes
the largest gap between deckwatch and mature dashboards (Rancher / Lens)
for the non-engineer persona.

## 2. Scope

**In scope (this design):**

- Per-pod CPU & memory usage, sparkline on `DeploymentDetailPage.vue`
  and `ApplicationDetailPage.vue`
- Per-node CPU & memory usage, sparkline on `ClusterOverviewPage.vue`
- % of limit / request context (color-coded when approaching limit)
- Graceful "install metrics-server" prompt when unavailable

**Out of scope (future):**

- Request/error rates per app (needs Prometheus + service instrumentation
  — deckwatch does not own the app's `/metrics` endpoint)
- Long-term historical charts spanning hours/days (needs a real TSDB —
  see Phase 3)
- Alert configuration / thresholds (should be its own page, uses P1
  Notifications feature)

## 3. Data source waterfall

We adopt a three-tier strategy identical to what the ROADMAP anticipates
(`docs/PRODUCT_ROADMAP.md:45`). Each tier is a separate backend endpoint
so we can ship incrementally.

### Tier 1 — metrics-server (mandatory baseline)

- **API:** `metrics.k8s.io/v1beta1` — `/apis/metrics.k8s.io/v1beta1/nodes`
  and `/apis/.../namespaces/{ns}/pods`
- **What you get:** last-sample CPU (millicores) + memory (bytes) per
  container, refreshed by kubelet every 15-60 s
- **What you don't get:** history. Metrics-server is a stateless reducer
  over kubelet's live stats endpoint; queries are always "right now".
- **Availability:** shipped by default with k3d, k3s, EKS, GKE, AKS. It
  is the lingua franca of `kubectl top`. Confirmed present on the target
  k3d dev cluster.
- **Access from Rust:** kube-rs does not include first-class typed
  bindings for the metrics API. Two viable paths:
  1. `DynamicObject` via `discovery::ApiResource`. Ergonomic for listing
     but the `usage` map arrives as `serde_json::Value` — the caller has
     to reach in and unwrap `cpu` / `memory` per item.
  2. `Client::request::<T>(req)` with a small local `serde` model.
     Preferred here: 100 lines of straightforward code, strongly typed,
     no cost. This is what the prototype does.

### Tier 2 — Frontend-accumulated ring buffer (~15 min history, zero infra)

- Backend endpoint is stateless (Tier 1). The frontend polls every 5 s
  (matches existing `usePolling(fetchDetail, 5000)` at
  `frontend/src/components/pages/DeploymentDetailPage.vue:119`) and
  pushes each sample into a per-pod ring buffer of ~180 points.
- 180 samples × 5 s = 15 min of history — enough to answer "is this pod
  climbing?" without any TSDB.
- Store the buffer in a Pinia store keyed by `${ns}/${pod}` so
  navigating away and back keeps context. Cap total series to a fixed
  bound (say 500 pods × 180 samples × 2 metrics ≈ 180 KB — trivial).
- Reset on page reload — this is UX-visible telemetry, not a source of
  truth. If the user wants durable history they configure Tier 3.

### Tier 3 — Prometheus PromQL (durable, historical, optional)

- If the operator has Prometheus in-cluster, deckwatch can hit its query
  API for real ranged data:
  `GET http://prometheus:9090/api/v1/query_range?query=<expr>&start=&end=&step=`
- Config surface: one field in `values.yaml`:
  ```yaml
  metrics:
    prometheus:
      url: ""   # e.g. http://prometheus-server.monitoring:9090
  ```
- Backend endpoint `/api/namespaces/{ns}/deployments/{name}/metrics/range?window=1h`
  builds the PromQL, proxies the response, translates timestamps.
- Canonical queries (per-container aggregation):
  - CPU seconds/s → cores: `sum(rate(container_cpu_usage_seconds_total{namespace="$ns",pod=~"$name-.*"}[1m])) by (pod)`
  - Memory working set: `sum(container_memory_working_set_bytes{namespace="$ns",pod=~"$name-.*"}) by (pod)`
- **We do NOT** run our own Prometheus or scrape workloads directly.
  That's cluster infra, not application-dashboard infra.

### Deliberate non-choice

We do **not** propose deckwatch build its own scraper (calling kubelet
`/stats/summary` per-node). That path duplicates metrics-server for no
gain and introduces node-level RBAC (`nodes/stats`) that the current
service account doesn't have.

## 4. Recommended charting library

**Recommendation: use Vuetify's built-in `VSparkline` for at-a-glance
strips, and `vue-chartjs` v5 (over Chart.js v4) for interactive detail
charts.** Add the chart library only when we start building the "detail"
view — Phase 2 — not up front.

### Reasoning

| Library | Bundle (min+gz) | Vue 3 | Vuetify look | Interactivity | License |
|---|---|---|---|---|---|
| **VSparkline** (already installed) | 0 KB extra | yes | native | none — pure SVG | MIT |
| **vue-chartjs 5 + chart.js 4** | ~60 KB | yes | tweakable | good — tooltips, zoom, legend | MIT |
| Apache ECharts + vue-echarts | ~300 KB (tree-shakable to ~150) | yes | needs theme work | excellent, heavier | Apache-2.0 |
| uPlot + vue-uplot | ~45 KB | yes (community) | manual | minimal but very fast | MIT |
| D3 (raw) | ~90 KB core | via wrappers | manual | infinite | ISC |

**Why not ECharts here:** overkill for the scope. deckwatch is not a
BI/observability tool — it's an app dashboard. ECharts is worth its
weight only if we start doing heatmaps, geo, or complex composited
views. Revisit if/when the Prometheus tier adds range charts with
brushing.

**Why not uPlot yet:** faster than chartjs, but the wrapper story is
weaker (community-maintained) and the axis / legend styling is much
more manual. Fine to swap in if we hit a perf ceiling with chart.js
(>10 concurrent panels on one page).

**Why VSparkline first:** it's already in the tree (`vuetify: ^3.8.1`
in `frontend/package.json:22`). Ships as native SVG, no JS chart engine.
Perfect for the "inline strip" use case which is 80% of what we need.

### Where each is used

- **`VSparkline`** — inline in tables and cards. Examples:
  - Pod row in `DeploymentDetailPage` pod table: 60 px × 20 px CPU strip
  - Application card on `ApplicationsPage`: two tiny strips (CPU/mem)
  - Node row on `ClusterOverviewPage`: cluster CPU/mem strip
- **`vue-chartjs`** — full-panel time-series in a "Metrics" tab:
  - Stacked area chart, one series per container, CPU/mem side-by-side
  - Overlay lines for `resource_requests` and `resource_limits`
  - Tooltip with exact value + timestamp on hover

## 5. Backend API design

Two new routes wired in `src/routes.rs`:

```
GET /api/namespaces/{ns}/pods/metrics
    ?label_selector=app=<deployment-name>    # optional, scopes to one app
GET /api/nodes/metrics
```

Both handlers live in `src/handlers/resource_metrics.rs` (prototype
included in this staging tree). Both:

- Use `Client::request` with a small local serde model of `PodMetrics`
  and `NodeMetrics` (metrics.k8s.io/v1beta1 wire format).
- Emit `k8s_api_requests_total` / `k8s_api_request_duration_seconds` via
  the existing `K8sTimer` helper in `src/metrics.rs`, tagged with
  `resource=podmetrics` / `nodemetrics`.
- Normalize K8s quantity strings on the server side:
  - CPU → `cpu_millicores: u64` (unit-safe integer)
  - Memory → `memory_bytes: u64`
  Keeps the frontend free of `parseQuantity` code, which is a
  historical source of off-by-1024 bugs.
- On `404` from the API server, return an empty `pods: []` plus
  `unavailable_reason: "metrics-server does not appear to be installed…"`.
  The frontend renders this as a call-out card instead of an error state
  — this matches the existing "GitOps not configured" pattern.
- On `503`, same treatment with a different reason string ("metrics-server
  is installed but not ready — give it 60s").

### Response shape (per pod)

```jsonc
{
  "pods": [
    {
      "name": "web-abc-123",
      "namespace": "prod",
      "timestamp": "2026-07-10T14:32:00Z",
      "window": "30s",
      "containers": [
        { "name": "app",     "cpu": "150m",  "cpu_millicores": 150,
                             "memory": "128Mi", "memory_bytes": 134217728 },
        { "name": "sidecar", "cpu": "5m",    "cpu_millicores": 5,
                             "memory": "8Mi",   "memory_bytes": 8388608 }
      ],
      "total_cpu_millicores": 155,
      "total_memory_bytes": 142606336
    }
  ]
}
```

`unavailable_reason` field is emitted only when metrics-server is
missing — otherwise it's absent (via `#[serde(skip_serializing_if)]`).

### Phase 3 — Prometheus range endpoint (later)

```
GET /api/namespaces/{ns}/deployments/{name}/metrics/range?window=1h&step=1m
```

Returns `{ series: [{ pod, cpu: [[ts, value], …], memory: [[ts, value], …] }] }`.
Config-gated on `metrics.prometheus.url` being set. Falls back to
`unavailable_reason: "prometheus not configured"` when unset.

## 6. Frontend component design

### 6.1 New composable — `useResourceMetrics.ts`

Single source of truth for the polling + ring buffer:

```ts
// composables/useResourceMetrics.ts
export interface Sample { t: number; cpuMillicores: number; memBytes: number; }
export interface PodSeries { pod: string; samples: Sample[]; }

export function useResourceMetrics(ns: Ref<string>, labelSelector?: Ref<string>) {
  const series = ref<Map<string, PodSeries>>(new Map());
  const unavailable = ref<string | null>(null);

  usePolling(async () => {
    const { pods, unavailable_reason } = await metricsApi.listPods(ns.value, labelSelector?.value);
    unavailable.value = unavailable_reason ?? null;
    const now = Date.now();
    for (const p of pods) {
      const key = p.name;
      const buf = series.value.get(key) ?? { pod: p.name, samples: [] };
      buf.samples.push({ t: now, cpuMillicores: p.total_cpu_millicores, memBytes: p.total_memory_bytes });
      if (buf.samples.length > 180) buf.samples.shift();  // ~15 min at 5 s
      series.value.set(key, buf);
    }
    // Evict pods that vanished (deployment restart, rollout)
    const currentNames = new Set(pods.map(p => p.name));
    for (const key of series.value.keys()) if (!currentNames.has(key)) series.value.delete(key);
  }, 5000);

  return { series, unavailable };
}
```

Keeps the ring buffer in a `Map` scoped to the composable, so unmounting
the page reclaims memory. If we want cross-page persistence later, hoist
the `Map` into a Pinia store.

### 6.2 New atoms — `<MetricSparkline>` and `<MetricPanel>`

`<MetricSparkline>` — wrapper around `VSparkline` that:
- Accepts `Sample[]` and a `metric: 'cpu' | 'memory'` prop
- Normalizes for VSparkline's `value: number[]` prop
- Applies threshold color: green < 60% of limit, amber 60-85%, red > 85%
  (or grey if no limit configured)
- Renders "—" placeholder when `samples.length === 0`

`<MetricPanel>` (Phase 2) — full time-series in a card:
- Uses `vue-chartjs` `<Line>` with stacked areas per container
- Shows overlay dashed lines for request / limit
- Time axis: relative ("−5 min ago") for < 1 h windows

### 6.3 Placement

**`DeploymentDetailPage.vue`** (adds ~40 lines):

- New section between "Replicas" card and "Pods" table:
  ```
  ┌─────────────────────────────────────┐
  │ Resource Usage                      │
  │ ┌───────────────┬───────────────┐   │
  │ │ CPU  ▂▃▅▂▃▁   │ Mem ▂▂▃▃▃▃▄   │   │
  │ │ 155m/200m 77% │ 142M/512M 27% │   │
  │ └───────────────┴───────────────┘   │
  └─────────────────────────────────────┘
  ```
- New CPU / Mem columns in the existing pod table, each rendering a
  20-px-tall sparkline.

**`ApplicationDetailPage.vue`** — same treatment.

**`ClusterOverviewPage.vue`** — node table gains two sparkline columns
sourced from `/api/nodes/metrics`.

**`ApplicationsPage.vue`** (list) — one CPU strip per app card,
compressed. Only if perf allows (Phase 2 decision).

### 6.4 "metrics-server not installed" UX

When `unavailable_reason` comes back:

- Replace the resource-usage card with a Vuetify `<v-alert type="info">`:
  > **CPU / memory metrics unavailable.** Install
  > [metrics-server](https://github.com/kubernetes-sigs/metrics-server)
  > in your cluster to see usage data.
- No red-error styling — this is a degraded-mode state, not an error.

## 7. Effort estimate

Broken down by phase; sizes match the T-shirt in `PRODUCT_ROADMAP.md`.

| Phase | Scope | Effort |
|---|---|---|
| **1 — Point-in-time** | Backend `resource_metrics.rs` (this prototype hardened + wired), frontend `useResourceMetrics`, `<MetricSparkline>`, "unavailable" UX, sparklines on DeploymentDetail / ApplicationDetail / ClusterOverview | **~2 days** |
| **2 — Client history + detail panel** | Ring-buffer accumulation across polls, `<MetricPanel>` w/ vue-chartjs, request/limit overlay lines, threshold coloring | **~3 days** |
| **3 — Prometheus range** | Config field, PromQL builder, `/metrics/range` endpoint, connect `<MetricPanel>` to it when configured, still fall through to client ring buffer otherwise | **~4 days** |

Total for all three phases: ~9 days. Phase 1 alone delivers 80% of the
user-visible value and is safe to ship on its own.

## 8. Recommended sequencing

1. **Merge Phase 1 first, as a standalone PR.** metrics-server is on
   the k3d dev cluster today; this ships value immediately with no new
   external dependency.
2. Ship the `useResourceMetrics` composable + `<MetricSparkline>` in the
   same PR — they have no consumers without each other.
3. Watch how users engage. If they routinely hover looking for values,
   move to Phase 2 (chart.js detail panel). If they only glance at
   sparklines and move on, Phase 2 is a lower priority.
4. Phase 3 only when at least one production cluster has Prometheus and
   users ask for "yesterday's memory graph". Do not build ahead of that
   demand.

## 9. Risks & open questions

- **metrics-server API stability**: `v1beta1` has been in use for years;
  `v1` has been proposed but not shipped. Low risk. If `v1` lands, the
  fix is a one-line URI change.
- **RBAC**: The deckwatch service account must have `list` on
  `pods.metrics.k8s.io` and `nodes.metrics.k8s.io`. The Helm chart's
  ClusterRole needs to add these — an easy oversight, add a Helm chart
  test that lists metrics from a k3d pod to catch this in CI.
- **Cardinality**: The frontend caps at 180 samples × N pods. If
  someone opens a namespace with 500 pods, that's ~180 KB in memory,
  fine. If they leave the tab open overnight it's still ~180 KB — the
  ring buffer doesn't grow. Safe.
- **Nanocore precision**: metrics-server frequently returns CPU as
  nanocores (`"123456789n"` = 123 mcpu). Our parser rounds down; a pod
  using 0.5 mcpu shows as 0. Acceptable for a dashboard — mention this
  in tooltip copy.
- **Namespace allow-list**: The pod-metrics endpoint respects
  `AppState::is_namespace_allowed`, matching every other namespaced
  handler (prototype enforces this). No new authorization surface.

## 10. Prototype summary

`src/handlers/resource_metrics.rs` in this staging tree contains a
compilable prototype:

- Two handlers (`list_pod_metrics`, `list_node_metrics`) using
  `Client::request` and typed serde models
- Robust K8s quantity parsing (CPU: `m`, `u`, `n`, bare cores; Memory:
  binary + decimal suffixes) with unit tests
- Graceful `unavailable_reason` on 404/503, error passthrough otherwise
- Integrates with the existing `K8sTimer` metrics helper
- Reuses `AppError`, `AppState`, and the namespace allow-list — no new
  cross-cutting infra

To wire in (post-review):

1. `pub mod resource_metrics;` in `src/handlers/mod.rs`
2. Two routes in `src/routes.rs`:
   ```rust
   .route("/api/namespaces/{ns}/pods/metrics", get(resource_metrics::list_pod_metrics))
   .route("/api/nodes/metrics",                get(resource_metrics::list_node_metrics))
   ```
3. Add `list`, `get` verbs on `pods.metrics.k8s.io` and
   `nodes.metrics.k8s.io` to the Helm ClusterRole.
4. Frontend `metricsApi` in `frontend/src/api/metrics.ts` mirroring the
   response types.
