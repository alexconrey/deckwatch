# Deckwatch Metrics

Deckwatch exposes Prometheus text-format metrics at `GET /metrics` on the
same port the app serves (`8080` by default). No authentication is
required — the endpoint is intentionally cheap so a scraper can hit it
every 15–30 seconds without adding load.

## Available metrics

### HTTP (backend)

Recorded by the Axum middleware in `src/metrics.rs`. The `path` label is
the matched route template (e.g. `/api/namespaces/{ns}/deployments`), not
the raw URI, so label cardinality stays bounded.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `deckwatch_http_requests_total` | counter | `method`, `path`, `status` | Total HTTP requests handled |
| `deckwatch_http_request_duration_seconds` | histogram | `method`, `path` | Server-side latency |

### Kubernetes API

Emitted by wrappers around kube-rs calls. Wire in via the `K8sTimer` helper
or `record_k8s_call` free function.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `deckwatch_kube_api_requests_total` | counter | `resource`, `operation`, `status` | Calls to the kube API server |
| `deckwatch_kube_api_request_duration_seconds` | histogram | `resource`, `operation` | Latency of kube API calls |

`status` is `ok` or `err`. `operation` is `list`, `get`, `create`,
`update`, `delete`, `patch`, `watch`.

### Application state

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `deckwatch_deployments_managed_total` | gauge | `namespace` | Deployments visible to deckwatch, per namespace |
| `deckwatch_ingresses_managed_total` | gauge | `namespace` | Ingresses visible to deckwatch, per namespace |
| `deckwatch_gitops_builds_total` | counter | `namespace`, `status` | GitOps build completions |
| `deckwatch_gitops_poll_duration_seconds` | histogram | `namespace` | Time spent polling git repos for changes |
| `deckwatch_audit_events_total` | counter | `action`, `resource_type` | Audit log events recorded |
| `deckwatch_errors_total` | counter | `kind` | Application errors by category |
| `deckwatch_active_sse_connections` | gauge | (none) | Currently open log-stream SSE connections |

### Frontend

Posted from the browser to `POST /api/frontend-metrics` and re-emitted
into the same recorder as backend metrics.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `frontend_page_views_total` | counter | `route` | Vue router navigations |
| `frontend_api_calls_total` | counter | `path`, `method`, `status` | Client-side API calls (status bucketed as `2xx`/`4xx`/`5xx`) |
| `frontend_api_call_duration_seconds` | histogram | `path`, `method` | Client-side round-trip time |
| `frontend_errors_total` | counter | `kind`, `route` | JS / network / API errors |
| `frontend_page_load_seconds` | histogram | `route` | Navigation Timing API load time |

`kind` is one of `network`, `api`, `js`, `unhandled_rejection`.

## Scraping

### Classic annotation-based discovery

The Helm chart adds these annotations to the Service when
`metrics.annotations` is true (the default):

```yaml
prometheus.io/scrape: "true"
prometheus.io/port: "8080"
prometheus.io/path: "/metrics"
```

Use with a Prometheus config like:

```yaml
scrape_configs:
  - job_name: kubernetes-services
    kubernetes_sd_configs:
      - role: service
    relabel_configs:
      - source_labels: [__meta_kubernetes_service_annotation_prometheus_io_scrape]
        action: keep
        regex: "true"
      - source_labels: [__meta_kubernetes_service_annotation_prometheus_io_path]
        target_label: __metrics_path__
        regex: (.+)
```

### prometheus-operator ServiceMonitor

Preferred when the cluster runs prometheus-operator. Enable in `values.yaml`:

```yaml
metrics:
  serviceMonitor:
    enabled: true
    interval: 30s
    labels:
      release: kube-prometheus-stack   # or whatever your operator selects
```

The Helm chart includes a `ServiceMonitor` template at
`helm/deckwatch/templates/servicemonitor.yaml`. When enabled, it emits a
`monitoring.coreos.com/v1` `ServiceMonitor` resource pointing at the
`http` port with path `/metrics`. The `labels` map is merged into the
`ServiceMonitor`'s metadata so the prometheus-operator's
`serviceMonitorSelector` can discover it.

### Ad-hoc scrape (dev)

```bash
kubectl port-forward svc/deckwatch 8080:80
curl http://localhost:8080/metrics
```

## Prometheus monitoring as a runtime setting

Prometheus metric collection can be toggled at runtime via the
deckwatch settings API or the Settings page in the UI. When disabled,
the `/metrics` endpoint returns `204 No Content` and the recording
middleware becomes a no-op. This is useful in environments where
Prometheus is not deployed and the per-request recording overhead is
unwanted.

## Grafana dashboard suggestions

Panels that pay for themselves early:

- **Request rate & error rate** — `sum by (status) (rate(deckwatch_http_requests_total[5m]))`
- **p50/p95/p99 latency by route** — `histogram_quantile(0.95, sum by (le, path) (rate(deckwatch_http_request_duration_seconds_bucket[5m])))`
- **Kube API dependency health** — `sum by (resource, status) (rate(deckwatch_kube_api_requests_total[5m]))`
- **SSE fan-out** — `deckwatch_active_sse_connections` as a stat panel (log tail load-shedding)
- **GitOps build success rate** — `sum by (status) (rate(deckwatch_gitops_builds_total[1h]))`
- **Audit event rate** — `sum by (action) (rate(deckwatch_audit_events_total[5m]))` — spikes indicate batch operations or unusual activity
- **Error rate by kind** — `sum by (kind) (rate(deckwatch_errors_total[5m]))` — useful for spotting kube API connectivity issues vs application bugs
- **Frontend error rate** — `sum by (kind) (rate(frontend_errors_total[5m]))` — spikes usually mean the API is down, JS bundle broke, or a new route is 404'ing
- **RUM latency vs server latency** — overlay `frontend_api_call_duration_seconds` p95 with `deckwatch_http_request_duration_seconds` p95 on the same panel; the gap is network + browser overhead

## Frontend metrics architecture

The browser aggregates in-memory and posts a JSON batch to the backend on
one of three triggers:

1. **Every 30 s** — `setInterval` in `useMetrics()`
2. **On `visibilitychange` → hidden** — via `navigator.sendBeacon()` so
   the request survives tab close
3. **On component unmount** — belt-and-suspenders for SPAs

The backend endpoint (`POST /api/frontend-metrics`) is idempotent from
the browser's perspective — it always returns `204 No Content`, even for
malformed payloads. This prevents the browser retrying a bad batch
forever if the schema drifts.

The `path` label on frontend API metrics is normalized in the browser
(e.g. `/namespaces/foo/deployments/bar` → `/namespaces/{ns}/deployments/{name}`)
to match the backend's Axum route template and keep cardinality bounded.

The `session_id` field is passed through for log correlation but is
**not** emitted as a Prometheus label — it would blow up cardinality.

## Cardinality budget

Rough upper bound with ~20 routes, 5 methods, and 10 status codes:
`20 * 5 * 10 = 1000` series per metric. Comfortably under the 10k
per-metric guideline. If you add new routes, no action needed. If you
start emitting user-supplied strings as labels, stop.
