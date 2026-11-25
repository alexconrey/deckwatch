# Distributed Tracing

Deckwatch does not store traces itself. Tracing involves three parts, each configured
separately:

| Part | What it does | Where it is configured |
|---|---|---|
| **Tracing backend** | Stores spans (Grafana Tempo or Jaeger) | Your own installation, or the bundled Helm option ([§4](#4-deploying-the-backend)) |
| **Workload export** | Sends the application's spans to the backend | The application's OpenTelemetry SDK, through the standard `OTEL_*` variables ([§5](#5-exporting-from-your-workloads)) |
| **Trace queries** | Lets deckwatch list recent traces and link into the backend's UI | The `tracing` block of deckwatch's settings ([§6](#6-trace-queries-and-deep-links)) |

These parts are about your workloads' traces. deckwatch can also export its **own** traces, log
lines and metrics over OTLP. That export is separate and off by default; see
[§10](#10-deckwatchs-own-telemetry).

```
 app container (OTEL_* env) ────────────OTLP─────────────▶ Tempo / Jaeger
                                                               ▲      ▲
          deckwatch ── query_url: GET /api/traces?service= ────┘      │  (Jaeger)
          browser ──── ui_url deep link (Grafana Explore / Jaeger UI) ┘
```

## 1. What you get

- **Backend-agnostic export.** Applications export OTLP, so moving from Tempo to Jaeger or to
  a managed backend only means changing the endpoint they export to.
- **Trace queries and deep links.** deckwatch can list recent traces and link into the
  backend's UI ([§6](#6-trace-queries-and-deep-links)).

## 2. Quick start

1. Have a backend that accepts OTLP ([§4](#4-deploying-the-backend)).
2. Point your workloads at the backend's OTLP endpoint with the standard `OTEL_*` variables.
   deckwatch doesn't set them for you yet ([§5](#5-exporting-from-your-workloads)).
3. Point deckwatch's `tracing` settings at the backend's query API and UI
   ([§6](#6-trace-queries-and-deep-links)).
4. Instrument the application ([§7](#7-instrumenting-applications)).

## 3. Choosing a backend

Both backends accept OTLP. They differ in what deckwatch itself can query.

| Concern | Grafana Tempo | Jaeger |
|---|---|---|
| Storage | Object storage (S3, GCS, Azure Blob) or local disk | Cassandra, Elasticsearch/OpenSearch, or in-memory |
| Operations | A single binary, or stateless components over object storage | Several components plus a database for production |
| UI | None built in; uses Grafana Explore | Ships its own UI |
| deckwatch trace queries ([§6.1](#61-trace-query-proxy)) | Not supported: Tempo has no `/api/traces?service=` endpoint | `/api/traces` on port 16686 |
| deckwatch deep links ([§6.2](#62-deep-links)) | Grafana Explore | Jaeger UI |

**Tempo is the default.** Deckwatch treats a blank or unknown `backend_kind` as `tempo`. Its
object-storage model avoids running a database, and it fits clusters that already run Grafana.
Choose **Jaeger** if you already operate it, want its UI without Grafana, or want deckwatch's
trace query API to work.

## 4. Deploying the backend

### 4.1 Bring your own (recommended for production)

Install Tempo or Jaeger however you normally install infrastructure (the upstream Helm charts,
Argo CD, Terraform), or use a managed backend such as Grafana Cloud or Honeycomb. Then:

- point your workloads' OTLP export at the backend ([§5](#5-exporting-from-your-workloads));
- set deckwatch's `tracing.query_url` and `tracing.ui_url` ([§6](#6-trace-queries-and-deep-links)).

### 4.2 Bundled backend (Helm)

The deckwatch chart can run a small, single-replica backend for development and evaluation.
It is not meant for production: Tempo writes to an `emptyDir` and Jaeger keeps everything in
memory, so traces are lost when the pod restarts.

`values.yaml` ships only `tracing.enabled: false` and **no image defaults**, so set the image
yourself when you enable it:

```yaml
tracing:
  enabled: true
  backend: tempo            # tempo | jaeger
  tempo:
    image:
      repository: grafana/tempo
      tag: "<version>"      # pin a release you have tested with the bundled config
      pullPolicy: IfNotPresent
    retention: 24h          # compactor block_retention
    resources:
      requests: { cpu: 100m, memory: 256Mi }
      limits: { memory: 1Gi }
  # jaeger:                 # used when backend: jaeger
  #   image:
  #     repository: jaegertracing/all-in-one
  #     tag: "<version>"
  #     pullPolicy: IfNotPresent
  #   resources: {}
```

| Value | Effect |
|---|---|
| `tracing.enabled` | Renders the backend Deployment, Service and (for Tempo) ConfigMap. |
| `tracing.backend` | `tempo` (default) or `jaeger`. |
| `tracing.tempo.image.*`, `tracing.tempo.resources`, `tracing.tempo.retention` | The Tempo container and how long blocks are kept (default `24h`). |
| `tracing.jaeger.image.*`, `tracing.jaeger.resources` | The Jaeger all-in-one container (OTLP enabled through `COLLECTOR_OTLP_ENABLED`). |

The chart exposes the backend as a ClusterIP Service named after the release, for example
`deckwatch-tempo` for a release named `deckwatch`:

| Backend | Service | Ports |
|---|---|---|
| Tempo | `<release>-tempo` | 4317 OTLP gRPC, 4318 OTLP HTTP, 3200 HTTP API (Tempo search, readiness) |
| Jaeger | `<release>-jaeger` | 4317 OTLP gRPC, 4318 OTLP HTTP, 16686 UI and query API |

The OTLP gRPC endpoint is then `<release>-tempo.<namespace>.svc:4317` (or
`<release>-jaeger…`), plaintext. Only the Jaeger Service also serves deckwatch's trace
queries, as `http://<release>-jaeger.<namespace>.svc:16686` ([§6.1](#61-trace-query-proxy)).

## 5. Exporting from your workloads

The OpenTelemetry Collector plugin (`deckwatch-plugin-otel`) is retired. Built-in support is
planned to replace it: OTLP endpoints that the operator sets in deckwatch's settings, and
per-application injection of the `OTEL_*` variables. Until then, set the `OTEL_*` variables on
your workloads yourself ([§7](#7-instrumenting-applications)).

deckwatch's own telemetry export is not affected. It is configured through the standard `OTEL_*`
environment variables and works as described in [§10](#10-deckwatchs-own-telemetry).

## 6. Trace queries and deep links

Deckwatch reads tracing configuration from a `tracing` block in its settings.

### 6.1 Trace query proxy

`GET /api/namespaces/{ns}/deployments/{name}/traces?service=<service>&limit=<n>` returns
recent trace summaries: trace ID, root span name, duration, span count and start time. It
proxies to `{query_url}/api/traces?service=…&limit=…`, which is Jaeger's query API. The
handler:

- returns at most 20 traces and times out after 10 seconds;
- only answers for namespaces the caller is allowed to see;
- accepts a restricted set of characters in `service`;
- returns HTTP 200 with an `unavailable_reason` when `query_url` is unset, the backend cannot
  be reached, or the backend returns an error.

Only Jaeger serves this API, on port 16686. Tempo does not: Tempo 2.9 and 3.0 answer
`/api/traces?service=` on their HTTP port (3200) with **404**, and search through
`/api/search` instead. With Tempo, deckwatch's trace queries therefore report
`unavailable_reason: tracing backend returned HTTP 404`, unless `query_url` points at a
Jaeger-compatible query service in front of Tempo. Deep links into Grafana
([§6.2](#62-deep-links)) work either way.

deckwatch's trace components look traces up with `service` set to the **deployment name**,
so set `OTEL_SERVICE_NAME` to the deployment name. The deployment page does not show a trace
list yet; for now, use the API or the backend's UI.

### 6.2 Deep links

Each response echoes `ui_url` and `backend_kind`, and deckwatch builds trace links from them:

| `backend_kind` | Link |
|---|---|
| `jaeger` | `{ui_url}/trace/{trace_id}` |
| `tempo` (default) | `{ui_url}/explore?left=…`: a Grafana Explore query for the trace ID against a Tempo datasource with the UID `tempo` |

An empty `ui_url` hides the link.

### 6.3 Settings and values.yaml

| Setting | Used by | Meaning |
|---|---|---|
| `backend_kind` | Deep links | `tempo` or `jaeger`. Blank or unknown means `tempo`. |
| `query_url` | Query proxy | HTTP base URL of a Jaeger query API, such as `http://<jaeger>:16686`. Empty disables queries. Tempo does not serve this API ([§6.1](#61-trace-query-proxy)). |
| `ui_url` | Deep links | Public base URL of Grafana (for Tempo) or the Jaeger UI. |
| `otlp_endpoint` | Operators | Where the backend receives OTLP. deckwatch itself does not use it. deckwatch's own export is configured separately ([§10](#10-deckwatchs-own-telemetry)). |
| `otlp_insecure` | Operators | `true` when that endpoint is plaintext. |

Set these through Helm under `settings.defaults.tracing`. The chart renders them into the
settings ConfigMap (`settings.configMapName`, by default `deckwatch-config`), and the query
proxy reads the `tracing` block **from that ConfigMap** on every request. The Settings page
has no tracing section. Values saved through `PUT /api/settings` go to the settings database,
which the query proxy does not read.

With the bundled Jaeger backend, both queries and links work:

```yaml
settings:
  defaults:
    tracing:
      backend_kind: jaeger
      otlp_endpoint: deckwatch-jaeger.deckwatch.svc:4317
      otlp_insecure: true
      query_url: http://deckwatch-jaeger.deckwatch.svc:16686
      ui_url: https://jaeger.example.com   # port 16686 through your ingress
```

With Tempo, only the links work. Leave `query_url` empty, or point it at a Jaeger-compatible
query service:

```yaml
settings:
  defaults:
    tracing:
      backend_kind: tempo
      otlp_endpoint: deckwatch-tempo.deckwatch.svc:4317
      otlp_insecure: true
      query_url: ""
      ui_url: https://grafana.example.com
```

## 7. Instrumenting applications

The application needs an OpenTelemetry SDK, or auto-instrumentation such as the Java agent,
`opentelemetry-instrument` for Python, or `--require @opentelemetry/auto-instrumentations-node`
for Node. Most SDKs configure themselves from the standard variables
`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_SERVICE_NAME`,
`OTEL_RESOURCE_ATTRIBUTES` and `OTEL_<SIGNAL>_EXPORTER`; set them in the workload's environment
([§5](#5-exporting-from-your-workloads)). Some SDKs default to a different OTLP protocol than
the endpoint expects, so set `OTEL_EXPORTER_OTLP_PROTOCOL` to match its port: `grpc` for 4317,
`http/protobuf` for 4318.

## 8. Troubleshooting

| Symptom | Check |
|---|---|
| Trace queries return `unavailable_reason` | `query_url` is unset in the settings ConfigMap, or deckwatch cannot reach it. `HTTP 404` means `query_url` points at Tempo, which does not serve the Jaeger query API ([§6.1](#61-trace-query-proxy)). |
| Queries find no traces | The application's `service.name` is not the deployment name, or the application is not instrumented. |

## 9. Security

- **Pinned images.** Pin the bundled backend images ([§4.2](#42-bundled-backend-helm)).
- **Trace queries** go only to the configured `query_url`, with a strict allowlist on the
  service name and a capped result size.

## 10. deckwatch's own telemetry

deckwatch can export what it records about itself over OTLP: its own traces, its log lines and
metrics. This includes the embedded mcp-k8s library, which implements most MCP tools, and plugin
log lines. It is separate from your workloads' telemetry ([§5](#5-exporting-from-your-workloads)).
Export is **off** unless an OTLP endpoint is configured, and with no endpoint deckwatch behaves
exactly as before.

### 10.1 What is exported

| Signal | What | Notes |
|---|---|---|
| Traces | One span per HTTP request (`http`, with `method`, `path` and `version` attributes), plus any span deckwatch or a library opens. Each MCP request also gets a SERVER span from mcp-k8s, named `tools/call <tool>` (or the method), with the OpenTelemetry MCP attributes (see [§10.5](#105-mcp-requests)) | Requests include MCP calls (`POST /mcp`), health probes and `/metrics` scrapes. Drop the latter in your collector if you don't want them. |
| Logs | Every log line that passes the log filter, as an OTLP log record carrying the trace and span id of the span it was logged in | Includes mcp-k8s's `tool call finished` / `request finished` line for every MCP request (never argument values outside the allowlist), other mcp-k8s library lines and plugin lines (target `extism::pdk`, attribute `plugin` = the extism instance id). Lines from background tasks (GitOps poller, plugin reconciler) have no span, so no trace id. |
| Metrics | Log events with fields named `monotonic_counter.*`, `counter.*` or `histogram.*` | mcp-k8s's `mcp.server.operation.duration` histogram (seconds), one observation per MCP request. Prometheus `/metrics` is unchanged and separate. |

- **One filter for both outputs.** The log filter (`RUST_LOG`, Helm `logFilter`) decides what is
  recorded, for stdout and OTLP alike.
- **No self-export.** Events from the HTTP and TLS stack used for the export (`hyper`, `h2`,
  `reqwest`, `rustls`) and from the OpenTelemetry SDK are never exported, even with
  `RUST_LOG=trace`. They still go to stdout.
- **Traces-only backends.** When logs are not exported, log lines inside a span are attached to it
  as span events instead, so a backend that accepts only traces still shows them. Error lines also
  mark their span as failed.
- **Resource:** `service.name=deckwatch` (`OTEL_SERVICE_NAME` overrides it) and `service.version`
  (the deckwatch version). The chart adds `k8s.namespace.name` and `k8s.pod.name`.
- **Telemetry carries no sensitive values.** Keep it that way: never put tool arguments, Secret
  data, environment values or manifest bodies in log or span fields.

### 10.2 Configuration

deckwatch reads the standard OpenTelemetry environment variables once, at startup.

| Variable | Effect |
|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP/HTTP base URL; deckwatch appends `/v1/traces`, `/v1/logs` and `/v1/metrics`. **Setting it (or a per-signal endpoint) turns export on.** |
| `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT` | Full URL for one signal. On its own, it turns on only that signal. |
| `OTEL_{TRACES,METRICS,LOGS}_EXPORTER` | `otlp` (default) or `none`. |
| `OTEL_EXPORTER_OTLP_PROTOCOL` (and per signal) | Must be unset or `http/protobuf`. `grpc` and `http/json` are refused with an error in the log, and that signal is not exported. |
| `OTEL_EXPORTER_OTLP_HEADERS` (and per signal) | Headers such as credentials, `name=value,...`. |
| `OTEL_EXPORTER_OTLP_TIMEOUT`, `OTEL_EXPORTER_OTLP_COMPRESSION` | Export deadline (default 10 s) and `gzip`. |
| `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` | Resource attributes. |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | Sampling. The default, `parentbased_always_on`, keeps every trace. |
| `OTEL_BSP_*`, `OTEL_BLRP_*`, `OTEL_METRIC_EXPORT_INTERVAL` | Batching (5 s for spans and logs) and the metric interval (60 s). |
| `OTEL_SDK_DISABLED=true` | Turns export off. |

**TLS and proxies.** Export goes through deckwatch's shared outbound HTTP client
([docs/TLS.md](TLS.md)). It trusts the system roots plus `DECKWATCH_EXTRA_CA_FILE` (Helm
`outbound.extraCaBundle`) and honours `HTTPS_PROXY`/`NO_PROXY`, with a 5 s connect and 10 s total
timeout. `OTEL_EXPORTER_OTLP_CERTIFICATE`, `..._CLIENT_CERTIFICATE` and `..._CLIENT_KEY` are
ignored with a warning; mutual TLS is not supported.

In the chart, set `telemetry.otlp`:

```yaml
telemetry:
  otlp:
    enabled: true
    endpoint: http://otel-collector.observability:4318   # OTLP/HTTP, not the gRPC port
    headers:                       # optional: OTEL_EXPORTER_OTLP_HEADERS from a Secret
      secretName: otlp-credentials
      key: headers
    traces: true
    metrics: true
    logs: true
    serviceName: deckwatch
    resourceAttributes:
      deployment.environment: production
    sampler: ""                    # e.g. parentbased_traceidratio
    samplerArg: ""                 # e.g. "0.25"
```

| Value | Default | Sets |
|---|---|---|
| `telemetry.otlp.enabled` | `false` | Renders the variables below. Rendering fails if neither an endpoint nor `tracing.enabled` is set. |
| `telemetry.otlp.endpoint` | `""` | `OTEL_EXPORTER_OTLP_ENDPOINT`. Empty with `tracing.enabled` means the bundled backend ([§10.3](#103-with-your-workloads-and-the-bundled-backend)). |
| `telemetry.otlp.headers.secretName`, `.key` | `""`, `headers` | `OTEL_EXPORTER_OTLP_HEADERS` from that Secret key. |
| `telemetry.otlp.traces`, `.metrics`, `.logs` | `true` | `OTEL_{TRACES,METRICS,LOGS}_EXPORTER` (`otlp` or `none`). |
| `telemetry.otlp.serviceName` | `deckwatch` | `OTEL_SERVICE_NAME`. |
| `telemetry.otlp.resourceAttributes` | `{}` | Added to `OTEL_RESOURCE_ATTRIBUTES` after `k8s.namespace.name` and `k8s.pod.name`. Values must not contain commas or `=`. |
| `telemetry.otlp.sampler`, `.samplerArg` | `""` | `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG`. |

**Why not `tracing.otlp_endpoint`?** deckwatch's export does not fall back to the
`tracing.otlp_endpoint` setting ([§6.3](#63-settings-and-valuesyaml)), for four reasons:

- **Different format.** That setting is documented as a gRPC `host:port` for collectors.
  deckwatch exports OTLP/HTTP, which needs a URL and usually a different port (4318, not 4317).
- **Exfiltration risk.** Settings can be changed at runtime through `PUT /api/settings`, and that
  API is not authenticated yet. An export endpoint read from settings would let anyone who can
  reach deckwatch redirect its telemetry, and any credential headers, to a host of their choosing.
  The endpoint is therefore deployment configuration (environment variables), like
  `DECKWATCH_ALLOW_INSECURE_ENDPOINTS` ([docs/TLS.md](TLS.md)).
- **Startup only.** The pipeline is built at startup, before the database is read; a settings
  change would not take effect without a restart.
- **Settings are already split.** The query proxy reads the `tracing` block from the ConfigMap,
  not the database. Coupling export to that block would inherit the confusion.

Point both at the same backend when that is what you want ([§10.3](#103-with-your-workloads-and-the-bundled-backend)).

### 10.3 With your workloads and the bundled backend

- **Your workloads.** To see deckwatch next to your applications, export both to the same
  backend or central collector. Remember that deckwatch needs that backend's **OTLP/HTTP** port
  (4318), while applications are often configured for gRPC (4317).
- **The bundled Tempo or Jaeger** ([§4.2](#42-bundled-backend-helm)) accepts traces only. Enable
  `telemetry.otlp` with an empty `endpoint` and `tracing.enabled: true`. The chart then points
  deckwatch at `http://<release>-<backend>:4318` and turns metrics and logs off, so deckwatch's log
  lines arrive as span events. In Jaeger, pick the `deckwatch` service; in Grafana, query Tempo
  for `{resource.service.name="deckwatch"}`.
- **For logs and metrics,** send to an OpenTelemetry Collector that forwards traces to Tempo or
  Jaeger, logs to Loki or similar, and metrics to Prometheus or Mimir. Log records carry trace ids,
  so a log line links to its trace.

### 10.4 Troubleshooting and limitations

| Symptom or limitation | Explanation |
|---|---|
| Startup logs `exporting deckwatch's telemetry over OTLP (http/protobuf)` | Export is on; the line lists the signals and the endpoint (without credentials). No such line means export is off. |
| `OTLP export request failed` with a reason (unknown issuer, connection refused, `HTTP 401`, `HTTP 404`) | Logged once per signal each time the failure changes, and `OTLP export requests succeed again` once it recovers. Unknown issuer: add the CA with `outbound.extraCaBundle`. `HTTP 404` on logs or metrics: the backend accepts traces only, so turn those signals off. |
| `BatchSpanProcessor.ExportError` / `Export.Failed.Exhausted` (target `opentelemetry_sdk`, `opentelemetry-otlp`) | The SDK's own report for each failed batch, which is then dropped. Telemetry never blocks requests: when the backend is down, data is lost, not queued without bound. |
| `OTEL_EXPORTER_OTLP_PROTOCOL=grpc is not supported` | deckwatch exports `http/protobuf` only. Use the backend's OTLP/HTTP port. |
| HTTP spans are named `http`, with kind `internal` | MCP requests have semantic-convention SERVER spans ([§10.5](#105-mcp-requests)); HTTP ones come with the HTTP instrumentation planned next. An MCP request with a remote parent has its SERVER span in the caller's trace, apart from the `http` span. |
| The last few seconds of telemetry are lost on shutdown | deckwatch has no graceful shutdown yet, so the final batch (up to 5 s) is not flushed. |
| The `plugin` attribute is an instance id, not the plugin name | That is what extism records. The plugin's message usually names it. |

### 10.5 MCP requests

Every `POST /mcp` request is one SERVER span, one `mcp.server.operation.duration` observation and
one log line, whether the tool is deckwatch's own or one of mcp-k8s's. The names follow the
OpenTelemetry GenAI MCP conventions (status *Development*), implemented in
`mcp_k8s::telemetry::semconv`.

- **Span:** `tools/call <tool>`, or just the method for other requests. A tool name deckwatch
  doesn't serve never appears in a span name. Attributes include `mcp.method.name`,
  `gen_ai.tool.name`, `jsonrpc.request.id`, `mcp.session.id`, `mcp_k8s.action` (`read`, `write`,
  `delete` or `exec`), `mcp_k8s.decision`, `mcp_k8s.outcome`, `error.type`, the target
  (`k8s.namespace.name`, `mcp_k8s.resource.kind`, `mcp_k8s.resource.name`), the argument keys
  and the allowlisted argument values (`mcp_k8s.tool.arguments`).
- **Client:** in a session started by `initialize`, `mcp_k8s.client.name` and
  `mcp_k8s.client.version`. The client reports these itself: a label, never an identity.
- **Trace context:** the caller's `params._meta.traceparent` (and `tracestate`) is the span's
  parent, as the MCP conventions specify; the HTTP `traceparent` header is the fallback, or a link
  when both are present. The span's trace id is also recorded as `trace_id`, so the JSON log lines
  of the request carry it.
- **Metric:** `mcp.server.operation.duration`, in seconds, with the conventions' buckets and only
  `mcp.method.name`, `gen_ai.tool.name`, `gen_ai.operation.name`, `error.type`, `mcp_k8s.action` and
  `mcp_k8s.decision` as attributes. The namespace, resource, client and session are never metric
  attributes.
- mcp-k8s reports the metric as log events on the target `mcp_k8s::telemetry::metrics`. deckwatch
  keeps them out of stdout and OTLP logs; only the metrics pipeline reads them.

## 11. Known limitations (deckwatch 0.6.x)

| Limitation | Effect | Workaround |
|---|---|---|
| The query proxy reads tracing settings from the ConfigMap, not the database | `PUT /api/settings` does not change trace queries | Set `settings.defaults.tracing` in Helm values |
| The query proxy speaks only Jaeger's query API | No trace queries against Tempo | Use Jaeger for queries, or Grafana links with Tempo |
| The deployment page has no trace list | Traces are not shown in the deckwatch UI | Use the API or the backend's UI |
