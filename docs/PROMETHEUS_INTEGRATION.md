# Deckwatch — Prometheus Integration Design

**Status:** Design proposal
**Owner:** Platform / Deckwatch
**Related design docs:** `docs/METRICS_VISUALIZATION.md` (Phase 3), `docs/ARCHITECTURE_DECISION.md` (Option D hybrid storage)
**Prototype targets:** `src/handlers/resource_metrics.rs` (extend), `helm/deckwatch/templates/podmonitor.yaml` (new)

---

## 1. Problem

Deckwatch has a solid point-in-time metrics story via metrics-server (Phase 1 of
`METRICS_VISUALIZATION.md`) and a plan for a client-accumulated ring buffer
(Phase 2). Phase 3 was described as "call Prometheus if configured" but only in
the outbound direction — deckwatch reading Prometheus.

The missing half is inbound: **how do the applications deckwatch manages get
scraped in the first place?** Today an operator has to hand-roll a ServiceMonitor
or PodMonitor per deployment, or fall back to the classic
`prometheus.io/scrape` annotation SD job. That is a big papercut for a dashboard
whose value prop is "make Kubernetes usable for non-experts".

This design adds:

1. A togglable per-deployment (or per-application) setting that materializes
   the correct prometheus-operator CRD alongside the workload.
2. A backend query proxy that turns "show me CPU for this deployment for the
   last hour" into the right PromQL and returns time-series data the frontend
   can chart.
3. A curated PromQL query catalog for the four charts users actually want:
   CPU, memory, request rate, error rate.

## 2. Scope

**In scope:**

- PodMonitor vs ServiceMonitor recommendation (spoiler: PodMonitor as default).
- Helm-time schema for a togglable `monitor` block in the deckwatch-managed
  Deployment annotations.
- Backend endpoints: create/update/delete a monitor CRD for a given deployment,
  proxy PromQL range queries.
- Frontend integration with `vue-chartjs` for the Phase 2 detail panel.
- RBAC deltas required on the deckwatch ClusterRole.
- Effort estimate broken into shippable chunks.

**Out of scope:**

- Standing up Prometheus itself. Deckwatch is a *consumer* of the operator.
- Alertmanager rules or PrometheusRule CRs. That's a separate ticket (Phase 4).
- Recording rules. Nice-to-have, revisit once the query proxy is stable.
- Long-term storage / Thanos / Mimir federation. Operator concern.

## 3. PodMonitor vs ServiceMonitor — recommendation

**Recommendation: PodMonitor is the default. ServiceMonitor is an escape
hatch, exposed as `monitor.kind: ServiceMonitor` in the settings block.**

### Why PodMonitor by default

Both CRDs ultimately drop config into Prometheus's `kubernetes_sd_configs`
generator. The relevant tradeoffs for deckwatch's model:

| Concern | PodMonitor | ServiceMonitor |
|---|---|---|
| Requires a Service pointing at the pods | no | **yes** |
| Selector ergonomics for deckwatch-managed Deployments | matches on pod labels — deckwatch already sets `deckwatch.io/application`, `app` label from the pod template | matches on Service labels — deckwatch would have to author or reuse a Service |
| Coverage of "Deployments without a Service" (workers, jobs, one-off tools) | works | **does not work** — the workload has no Service |
| Scrape target cardinality | one target per pod (matches what you'd want anyway) | one target per Endpoint slice entry (same in practice, but you're one level of indirection away from pod identity) |
| Pod-level labels available in metrics (`pod`, `container`) | yes, natively | yes, via `honorLabels` + `endpointslice`-based SD |
| Playing nicely with an existing `Service` you already manage | no reason to plumb one through | natural fit |
| Config drift risk (Service selector doesn't match pods) | none | real — a stale Service selector silently drops all scrapes |

Deckwatch already owns Deployments, so it always has pod labels to select on.
It does *not* always own a Service — plenty of deckwatch-managed workloads are
worker deployments with no ingress. Making the default require a Service would
force deckwatch to also author Services just to enable monitoring, which is
scope creep and a source of drift.

**ServiceMonitor is the right pick when:**

- The user already has a curated Service they own (e.g., a `type: ClusterIP`
  service in front of their API) and wants the scrape config expressed at
  that level for consistency with their existing observability config.
- Their prometheus-operator install has `serviceMonitorSelector` set restrictively
  and their `podMonitorSelector` isn't wired up.

Support both, default to the one that always works.

### RBAC posture

Prometheus operator CRDs live in `monitoring.coreos.com/v1`. Deckwatch needs
to be able to create/update/delete these resources in each managed namespace.

Add to `helm/deckwatch/templates/clusterrole.yaml`:

```yaml
- apiGroups: ["monitoring.coreos.com"]
  resources: ["podmonitors", "servicemonitors"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
```

We don't need `prometheuses`, `alertmanagers`, or `prometheusrules` for the
scope of this design. If we later add alerting UX, we scope up then.

**Discovery:** on startup, probe once with `discovery::pinned_kind` for both
kinds. If the CRDs are absent, log at `INFO` and set
`AppState::monitor_support = MonitorSupport::Absent`. All monitor endpoints
short-circuit to 503 `{ "unavailable_reason": "prometheus-operator CRDs not
installed" }` — same pattern as the metrics-server graceful degrade.

## 4. Data model — how deckwatch tracks "monitoring on/off"

State lives on the Deployment itself, mirroring the existing GitOps
annotation pattern (`docs/ARCHITECTURE_DECISION.md` §1.3). No SQLite needed —
the toggle is per-Deployment metadata that follows the Deployment lifecycle.

### Annotations (prefix `deckwatch.io/`)

- `deckwatch.io/monitor` — `enabled` | `disabled` (missing = disabled).
- `deckwatch.io/monitor-kind` — `PodMonitor` | `ServiceMonitor`. Default `PodMonitor`.
- `deckwatch.io/monitor-port` — string, must match a container port `name` (preferred)
  or a numeric port on the pod template. Default `metrics`.
- `deckwatch.io/monitor-path` — default `/metrics`.
- `deckwatch.io/monitor-interval` — default `30s`.
- `deckwatch.io/monitor-scheme` — `http` | `https`. Default `http`.

These are namespaced on the Deployment. When toggled on, the backend
materializes a CRD in the same namespace named
`{deployment-name}-deckwatch` with `ownerReferences` back to the Deployment
so it's garbage-collected on Deployment delete.

### Where the toggle lives in the UI

**Per-deployment**, on `DeploymentDetailPage.vue` — same "Settings" tab that
today hosts GitOps config. This matches the existing pattern (GitOps is per-
Deployment, not per-Application) and keeps the mental model consistent.

Per-application toggling would need special handling: an application is a set
of Deployments and CronJobs — do you turn on scraping for every member, and
what happens when a new member is added? Better to keep per-deployment as the
source of truth and add a "bulk enable for this application" button in Phase
2 if usage justifies it.

## 5. CRD templates

The backend renders these via `k8s_openapi` on-demand — no static Helm
templates in this design. (Helm creates one PodMonitor for deckwatch's own
`/metrics` endpoint, which is separate and already exists as
`helm/deckwatch/templates/servicemonitor.yaml`.)

### 5.1 PodMonitor (default)

```yaml
apiVersion: monitoring.coreos.com/v1
kind: PodMonitor
metadata:
  name: {deployment-name}-deckwatch
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: deckwatch
    deckwatch.io/deployment: {deployment-name}
  ownerReferences:
    - apiVersion: apps/v1
      kind: Deployment
      name: {deployment-name}
      uid: {deployment-uid}
      controller: false
      blockOwnerDeletion: false
spec:
  selector:
    matchLabels:
      # copy the Deployment's spec.selector.matchLabels — this is what the pods
      # already carry, and it's the K8s idiomatic way to reach them
      {matchLabels-from-deployment}
  podMetricsEndpoints:
    - port: {monitor-port}          # named port from container spec
      path: {monitor-path}
      interval: {monitor-interval}
      scheme: {monitor-scheme}
      # honor pod-level labels so `pod` and `container` land in metrics
      honorLabels: true
```

**Selector rule:** copy the target Deployment's `spec.selector.matchLabels`
verbatim. Do NOT invent a new label. If we invented one we'd need to also
patch the pod template to add it, which is a mutation of user-owned config we
don't want to do quietly.

### 5.2 ServiceMonitor (escape hatch)

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: {deployment-name}-deckwatch
  namespace: {ns}
  ownerReferences: [... same as above ...]
spec:
  selector:
    matchLabels:
      # match Services that carry the same labels as the Deployment;
      # if the user's convention differs, they'll set monitor.selector
      # explicitly (Phase 2)
      {matchLabels-from-deployment}
  endpoints:
    - port: {monitor-port}          # this is the Service port name
      path: {monitor-path}
      interval: {monitor-interval}
      scheme: {monitor-scheme}
      honorLabels: true
```

If the user picks ServiceMonitor and no Service matches, the CRD is created
but scrape targets stay empty. The backend's "monitor status" endpoint
(§6.3) surfaces this as a warning so the UI can prompt them.

## 6. Backend API

Three new routes in `src/routes.rs`, one new handler module
`src/handlers/monitors.rs`:

### 6.1 Enable / update a monitor

```
PUT /api/namespaces/{ns}/deployments/{name}/monitor
Body:
  {
    "kind": "PodMonitor" | "ServiceMonitor",
    "port": "metrics",
    "path": "/metrics",
    "interval": "30s",
    "scheme": "http"
  }
```

Behavior:
1. Load the Deployment. 404 if missing.
2. Server-side-apply the annotations under `deckwatch.io/monitor*`.
3. Materialize (create-or-update) the PodMonitor/ServiceMonitor via
   `kube::Api::patch(Apply)` with field manager `deckwatch-monitor`.
4. Set `ownerReferences` pointing at the Deployment.
5. Return the new spec plus a `discovered_targets_hint` field indicating
   how many pods currently match the selector (from a live pod list).

### 6.2 Disable a monitor

```
DELETE /api/namespaces/{ns}/deployments/{name}/monitor
```

Behavior:
1. Remove the `deckwatch.io/monitor*` annotations from the Deployment.
2. Delete `{deployment-name}-deckwatch` PodMonitor/ServiceMonitor if it exists.
3. Idempotent: 204 on repeat calls.

### 6.3 Read monitor status

```
GET /api/namespaces/{ns}/deployments/{name}/monitor
```

Response:
```json
{
  "enabled": true,
  "kind": "PodMonitor",
  "port": "metrics",
  "path": "/metrics",
  "interval": "30s",
  "scheme": "http",
  "matching_pods": 3,
  "unavailable_reason": null,
  "warnings": []
}
```

`warnings` populated with things like:
- `"deployment has no container port named 'metrics'"`
- `"ServiceMonitor selector matches no Service in namespace"`
- `"CRD present but no Prometheus instance is configured to pick it up (no matching podMonitorSelector)"`

That last one requires optional read access to `prometheuses.monitoring.coreos.com`
to inspect selectors; scope it out of Phase 1 to keep RBAC small.

### 6.4 Prometheus range query proxy

```
GET /api/namespaces/{ns}/deployments/{name}/metrics/range
    ?window=1h           # 5m | 15m | 1h | 6h | 24h
    &step=1m             # optional; sane defaults per window
    &metric=cpu|memory|requests|errors
```

Response:
```jsonc
{
  "series": [
    { "pod": "web-abc-123", "points": [[1720626000, 0.42], ...] },
    { "pod": "web-abc-456", "points": [[1720626000, 0.51], ...] }
  ],
  "unit": "cores",
  "unavailable_reason": null
}
```

Behavior:
1. Gated on `settings.metrics.prometheus.url` being set in the deckwatch
   config ConfigMap. If empty, return `{ series: [], unavailable_reason:
   "prometheus not configured" }`.
2. Build the PromQL from the query catalog in §7 with the deployment's
   `metadata.name` and `metadata.namespace` interpolated. **Never accept
   user-supplied PromQL** — that's a query-injection footgun. Users pick
   from the enumerated `metric` values.
3. Timeout: 10s (configurable). Prometheus is a shared resource; we
   don't want deckwatch requests to pile up.
4. Emit `k8s_api_requests_total{resource="prom_query"}` for observability
   parity with the existing K8s handlers.

### 6.5 Settings surface

Extend `helm/deckwatch/values.yaml` and `deckwatch-config` ConfigMap:

```yaml
settings:
  defaults:
    metrics:
      prometheus:
        url: ""                                    # e.g. http://prometheus-server.monitoring:9090
        # Optional bearer token or basic-auth secret ref for query API
        auth_secret_ref: ""
      monitor:
        # Default kind used when a user enables monitoring in the UI
        # without picking one explicitly.
        default_kind: "PodMonitor"
        default_port: "metrics"
        default_path: "/metrics"
        default_interval: "30s"
```

## 7. PromQL query catalog

Curated set of parameterized queries — the *only* PromQL deckwatch will
execute. Each entry has a unique `metric` key that the frontend picks from a
dropdown.

Placeholders: `{ns}` = namespace, `{name}` = deployment name. Pod-name
matching uses `pod=~"{name}-.*"` which relies on the standard
Deployment→ReplicaSet→Pod naming convention. If a workload has been renamed
manually, the pattern degrades to no-match — surface as a warning.

### 7.1 CPU (cores per pod)

```promql
sum by (pod) (
  rate(container_cpu_usage_seconds_total{namespace="{ns}",pod=~"{name}-.*",container!="",container!="POD"}[2m])
)
```

- Unit: `cores` (fractional). Frontend renders as `mCPU` when < 1.
- Excludes the pause container (`POD`) and the empty container sum row that
  cAdvisor emits.
- 2-minute lookback gives smoother lines than the more common 1m and is
  fine for a dashboard's aggregation.

### 7.2 Memory (working set bytes per pod)

```promql
sum by (pod) (
  container_memory_working_set_bytes{namespace="{ns}",pod=~"{name}-.*",container!="",container!="POD"}
)
```

- Unit: `bytes`.
- Working set > RSS: this is what OOMKill decisions are made against, so
  it's the number that actually matters for capacity planning.

### 7.3 Request rate (per pod, per second)

Assumes the app exposes an HTTP `handler` metric family. Support two of the
most common instrumentation conventions:

```promql
# Preferred (prometheus/client_golang, Rust `metrics` crate, OpenMetrics)
sum by (pod) (
  rate(http_requests_total{namespace="{ns}",pod=~"{name}-.*"}[2m])
)

# Fallback query, tried if the first returns empty:
sum by (pod) (
  rate(http_server_requests_total{namespace="{ns}",pod=~"{name}-.*"}[2m])
)
```

- Unit: `req/s`.
- Fallback logic runs in the backend (small "query with fallback" helper).
  Return `warnings: ["fell back to http_server_requests_total"]` so users
  know which metric name is being used.

### 7.4 Error rate (5xx per pod, per second)

```promql
sum by (pod) (
  rate(http_requests_total{namespace="{ns}",pod=~"{name}-.*",status=~"5.."}[2m])
)
```

- Unit: `req/s`.
- Same fallback logic as request rate.
- Alternative "as a ratio" query is Phase 2 — showing a percentage is more
  useful than raw error count but requires two range queries (or a
  recording rule) and per-window bucketing.

### 7.5 Node-level context (Phase 2)

Not per-deployment, per-node. Skip for the initial ship; add when we build
the ClusterOverview detail panel:

```promql
# per-node CPU used cores
sum by (node) (rate(node_cpu_seconds_total{mode!="idle"}[2m]))

# per-node memory used bytes
node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes
```

## 8. Frontend integration (Phase 2 metrics viz)

### 8.1 New composable — `useMonitorSettings.ts`

Handles the toggle CRUD (§6.1–6.3):

```ts
export function useMonitorSettings(ns: Ref<string>, deployment: Ref<string>) {
  const settings = ref<MonitorSettings | null>(null);
  const loading = ref(false);
  const warnings = ref<string[]>([]);

  async function load() { /* GET /api/.../monitor */ }
  async function enable(spec: MonitorSpec) { /* PUT */ }
  async function disable() { /* DELETE */ }

  return { settings, loading, warnings, load, enable, disable };
}
```

### 8.2 New composable — `useMetricsRange.ts`

Wraps the range-query proxy. Reused by `<MetricPanel>` for both metrics-server
history (Phase 2 ring buffer if the SQLite hybrid lands) and Prometheus
(Phase 3):

```ts
export interface RangePoint { t: number; v: number; }
export interface PodSeries { pod: string; points: RangePoint[]; }

export function useMetricsRange(
  ns: Ref<string>,
  deployment: Ref<string>,
  metric: Ref<'cpu' | 'memory' | 'requests' | 'errors'>,
  window: Ref<'5m' | '15m' | '1h' | '6h' | '24h'>,
) {
  const series = ref<PodSeries[]>([]);
  const unit = ref('');
  const unavailable = ref<string | null>(null);

  usePolling(async () => {
    const r = await metricsApi.range(ns.value, deployment.value, {
      metric: metric.value,
      window: window.value,
    });
    series.value = r.series;
    unit.value = r.unit;
    unavailable.value = r.unavailable_reason ?? null;
  }, () => pollIntervalFor(window.value));

  return { series, unit, unavailable };
}
```

Poll interval scales with window: 5s for `5m`/`15m`, 30s for `1h`, 5min for
`6h`/`24h`. No point refreshing a 24-hour chart every 5 seconds.

### 8.3 New component — `<MetricPanel>`

Full-panel Chart.js chart. Props: `metric`, `window`, `namespace`,
`deployment`. Renders:

- One line per pod (bounded to top 10 by peak — long-tail pods aggregated
  into an "others" line to keep the legend usable).
- Y-axis auto-formatted from `unit` (cores → "0.5" or "500m"; bytes →
  "128 MiB"; req/s → "12.3/s").
- Overlay dashed line for `resources.limits.cpu` / `resources.limits.memory`
  (fetched from the Deployment spec, not Prometheus). Skipped for
  request/error charts.
- Empty-state variants: "no data (Prometheus not configured)",
  "no data (no scrape targets — enable monitoring above)",
  "no data (metric not exposed by this workload)".

Chart library: `vue-chartjs` 5 + `chart.js` 4, matching the
`METRICS_VISUALIZATION.md` recommendation. Add as *lazy imports* in the
metrics tab so pages without the tab don't pay the bundle cost.

### 8.4 New card — `<MonitorSettingsCard>`

Sits in the Deployment Detail "Settings" tab, next to the GitOps card.
Renders:

- Enable/disable switch.
- CRD kind selector (PodMonitor / ServiceMonitor) — disabled when the CRDs
  aren't installed (backend returns `unavailable_reason`).
- Port, path, interval, scheme fields with sensible defaults.
- Live "matching pods: 3" indicator.
- Warnings list from the status endpoint.

### 8.5 Placement in the tab layout

`DeploymentDetailPage.vue`:

- **Overview tab** (default): sparklines from metrics-server (Phase 1 —
  already designed).
- **Metrics tab** (new): `<MetricPanel>` × 4 (CPU, memory, requests,
  errors) in a 2×2 grid. Window selector at the top.
- **Settings tab**: existing GitOps card + new `<MonitorSettingsCard>`.

## 9. RBAC requirements

**Additions to `helm/deckwatch/templates/clusterrole.yaml`:**

```yaml
- apiGroups: ["monitoring.coreos.com"]
  resources: ["podmonitors", "servicemonitors"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
```

**Already have** (relevant to this feature):

- `apps/deployments` — needed to annotate deployments with monitor state.
- `""/services` — needed to detect matching Services for the ServiceMonitor
  "warnings" check.
- `metrics.k8s.io/pods,nodes` — used by the existing metrics-server handlers,
  not this feature.

**Explicitly NOT adding:**

- `prometheuses`, `alertmanagers`, `prometheusrules` — out of scope.
- `nodes/proxy`, `services/proxy` — deckwatch does not proxy scrapes.
- No changes needed to the Prometheus operator's own RBAC.

**Prometheus operator selector caveat:**

The operator only picks up PodMonitors/ServiceMonitors that match its
configured `podMonitorSelector` / `serviceMonitorSelector` (and the same
for namespace selectors). Default installs match all — but many production
installs restrict to a label like `release=prometheus`. Document this
loudly in the UI: when the user enables monitoring, we display

> "Created PodMonitor `foo-deckwatch`. If your Prometheus instance uses a
> `podMonitorSelector`, ensure your admin has whitelisted PodMonitors
> labeled `app.kubernetes.io/managed-by: deckwatch`, or add the required
> label under Settings → Metrics → Extra CRD labels."

That "extra labels" field is a Phase 2 addition — for the initial ship, the
label `app.kubernetes.io/managed-by: deckwatch` is enough and operators can
add it to their selector.

## 10. Effort estimate

Broken into shippable slices. Each slice is standalone-mergeable.

| Slice | Scope | Effort |
|---|---|---|
| **A — Monitor CRD lifecycle** | `handlers/monitors.rs` (create/update/delete/status), CRD probe on startup, RBAC delta, unit tests for annotation → CRD spec mapping | **~2 days** |
| **B — MonitorSettingsCard UI** | `useMonitorSettings` composable, `<MonitorSettingsCard>` component, wire into DeploymentDetail Settings tab, "CRDs not installed" empty state | **~1.5 days** |
| **C — Prometheus proxy + query catalog** | Config surface in settings + values, `handlers/prom_query.rs` with the four catalog queries and fallback logic, timeout / metrics, unit tests for PromQL template rendering | **~2 days** |
| **D — MetricPanel + Metrics tab** | `useMetricsRange` composable, `<MetricPanel>` with vue-chartjs (lazy-loaded), 2×2 grid layout, window selector, empty states, limit-line overlay | **~3 days** |
| **E — Warnings / discovery polish** | "matching pods" live count, ServiceMonitor→Service match check, fallback-metric warning surfacing, notes doc | **~1 day** |

Total: **~9.5 days** for the complete feature. Slices A + B alone (**~3.5
days**) deliver the "one-click monitoring" UX with no PromQL/chart work
required — a defensible standalone ship.

## 11. Recommended sequencing

1. **Ship Slice A + B together first.** Users can enable monitoring on a
   deployment and see it working via their existing Prometheus tooling
   (Grafana, kubectl). No new charting code, minimal risk.
2. **Then Slice C alone.** Backend-only PR. Adds the query proxy without a
   UI to expose it — verifiable via `curl` in dev. Keeps the frontend PR
   scope small.
3. **Then Slice D.** The chart panel. Now Prometheus data has a home in
   the UI. This is the largest slice and touches the most files, so
   isolating it makes review easier.
4. **Slice E last**, or defer indefinitely if user feedback shows the base
   feature is enough.

## 12. Risks and open questions

- **CRD version drift:** `monitoring.coreos.com/v1` is stable since 2020.
  If `v1beta2` or `v2` lands, it's a one-line change in `handlers/monitors.rs`.
  Low risk.
- **Prometheus operator not present but stock Prometheus is:** the monitor
  toggle would be misleading — CRDs don't exist, so we correctly disable it.
  Users who have a bare Prometheus can still use the query proxy (Slice C)
  because that hits `/api/v1/query_range` directly; the monitor UI just
  doesn't apply to them.
- **Query proxy as a DoS vector:** deckwatch is trusted infra and PromQL is
  restricted to the catalog, but a range query with `step=1s` over `24h` is
  86,400 points. Cap `step` server-side to keep every window returning ≤ 2000
  points.
- **Pod name pattern breaks on manual renames:** `pod=~"{name}-.*"` misses
  workloads where someone `kubectl edit`ed the pod name. Rare. Warning
  message in the empty-state.
- **`http_requests_total` isn't universal:** two of the four charts (requests,
  errors) assume this metric. If neither the primary nor fallback matches,
  return `unavailable_reason: "workload does not expose an HTTP request
  counter"` and let the UI render a helpful message. Do *not* try to guess
  further conventions — the failure mode of guessing wrong is worse than
  saying nothing.
- **Multi-cluster Prometheus:** out of scope. If deckwatch grows to
  multi-cluster (P2), each cluster's query URL is a per-cluster setting.
- **Auth to Prometheus:** many prod deployments front Prometheus with
  basic-auth or an OIDC proxy. The `auth_secret_ref` field in settings
  reads a K8s Secret with either `bearer-token` or `basic-auth-user` /
  `basic-auth-password` keys. TLS verification defaults on; add a
  `metrics.prometheus.insecure_skip_verify` escape hatch only if a real
  operator asks for it.

## 13. Appendix — comparison with the existing `servicemonitor.yaml` template

Deckwatch already ships `helm/deckwatch/templates/servicemonitor.yaml` for
scraping **deckwatch itself** at `/metrics`. That template is unchanged by
this design — it's for the deckwatch pod's own instrumentation, not for
user workloads. The new PodMonitors created by the backend live in the
namespace of the target Deployment and have distinct owner references and
names (`{deployment}-deckwatch`).

We could offer an equivalent `podmonitor.yaml` Helm template for deckwatch
itself as a follow-up, but that's a nit — the existing ServiceMonitor
works fine.

## 14. Appendix — glossary of the settings block

Final proposed `values.yaml` shape after this design lands, folded into
the existing metrics block:

```yaml
metrics:
  enabled: true
  annotations: true

  serviceMonitor:               # unchanged — for deckwatch itself
    enabled: false
    namespace: ""
    interval: 30s
    scrapeTimeout: 10s
    labels: {}
    metricRelabelings: []
    relabelings: []

  # NEW — configures deckwatch's ability to create PodMonitors/ServiceMonitors
  # for user workloads and query Prometheus for chart data.
  prometheus:
    # URL of the Prometheus query API. Empty = query proxy disabled and the
    # UI hides Metrics tabs that require it.
    url: ""
    # Optional K8s Secret with a `bearer-token` or basic-auth pair.
    authSecretRef: ""
    # Timeout for any single range/instant query.
    queryTimeout: 10s
    # Defaults used when a user enables monitoring in the UI without
    # picking specifics.
    monitor:
      defaultKind: PodMonitor      # PodMonitor | ServiceMonitor
      defaultPort: metrics
      defaultPath: /metrics
      defaultInterval: 30s
      defaultScheme: http
      # Extra labels applied to every created PodMonitor/ServiceMonitor —
      # use this to satisfy your Prometheus's podMonitorSelector.
      extraLabels: {}
```
