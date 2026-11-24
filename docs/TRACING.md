# Distributed Tracing

Deckwatch does not store traces itself. Tracing involves three parts, each configured
separately:

| Part | What it does | Where it is configured |
|---|---|---|
| **Tracing backend** | Stores spans (Grafana Tempo or Jaeger) | Your own installation, or the bundled Helm option ([§4](#4-deploying-the-backend)) |
| **Collector sidecar** | Receives spans from the application and forwards them to the backend | The [`deckwatch-plugin-otel`](https://github.com/alexconrey/deckwatch-plugin-otel) plugin ([§5](#5-the-opentelemetry-collector-plugin)) |
| **Trace queries** | Lets deckwatch list recent traces and link into the backend's UI | The `tracing` block of deckwatch's settings ([§6](#6-trace-queries-and-deep-links)) |

These parts are about your workloads' traces. deckwatch can also export its **own** traces, log
lines and metrics over OTLP. That export is separate and off by default; see
[§10](#10-deckwatchs-own-telemetry).

```
 pod ──────────────────────────────────────────┐
 │ app container ──OTLP──▶ otel-collector       │──OTLP──▶ Tempo / Jaeger
 │ (OTEL_* env)  localhost (127.0.0.1 only)     │              ▲      ▲
 └──────────────────────────────────────────────┘              │      │
          deckwatch ── query_url: GET /api/traces?service= ────┘      │  (Jaeger)
          browser ──── ui_url deep link (Grafana Explore / Jaeger UI) ┘
```

## 1. What you get

- **One switch per workload.** Enabling the plugin for a deployment, or for a whole
  application, adds an OpenTelemetry Collector sidecar. It also points the application's
  OpenTelemetry SDK at that sidecar through the standard `OTEL_*` environment variables.
- **No application changes beyond instrumentation.** The application has to emit
  OpenTelemetry spans ([§7](#7-instrumenting-applications)). The plugin handles where those
  spans go.
- **Backend-agnostic export.** The collector exports OTLP, so moving from Tempo to Jaeger or
  to a managed backend only means changing the plugin's exporter settings.

## 2. Quick start

1. Have a backend that accepts OTLP ([§4](#4-deploying-the-backend)).
2. Install `deckwatch-plugin-otel` and set its **Exporter endpoint** to the backend's OTLP
   endpoint ([§5.1](#51-install-and-configure)).
3. Enable it for a deployment (annotation `deckwatch.io/otel: enabled`,
   [§5.2](#52-enable-for-a-deployment)) or for an application (the **OpenTelemetry
   Collector** infrastructure resource, [§5.3](#53-enable-for-an-application)).
4. Point deckwatch's `tracing` settings at the backend's query API and UI
   ([§6](#6-trace-queries-and-deep-links)).
5. Instrument the application ([§7](#7-instrumenting-applications)).

## 3. Choosing a backend

Both backends accept OTLP from the collector. They differ in what deckwatch itself can query.

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

- set the plugin's exporter to the backend's OTLP endpoint ([§5.1](#51-install-and-configure));
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

The OTLP endpoint for the plugin is then `<release>-tempo.<namespace>.svc:4317` (or
`<release>-jaeger…`), plaintext. Only the Jaeger Service also serves deckwatch's trace
queries, as `http://<release>-jaeger.<namespace>.svc:16686` ([§6.1](#61-trace-query-proxy)).

## 5. The OpenTelemetry Collector plugin

[`deckwatch-plugin-otel`](https://github.com/alexconrey/deckwatch-plugin-otel) adds the
collector sidecar. Its README is the full reference. This section covers what an operator
needs to know.

### 5.1 Install and configure

Requires deckwatch **0.6.4 or later**. Add the plugin under **Settings → Plugins**. Name it
`otel`, pin the release checksum (each release publishes `plugin.wasm.sha256`), and leave
`allowed_hosts` empty, because the plugin makes no outbound requests:

```json
{
  "name": "otel",
  "enabled": true,
  "source": {
    "type": "github",
    "repo": "alexconrey/deckwatch-plugin-otel",
    "ref": "v0.2.0",
    "path": "plugin.wasm",
    "use_release": true
  },
  "sha256": "<plugin.wasm.sha256 from the release>",
  "allowed_hosts": []
}
```

Then configure it under **Settings → Plugins → otel**. The main settings:

| Key | Default | Notes |
|---|---|---|
| `OTLP_ENDPOINT` | **required** | The backend's OTLP endpoint. Use the value you set as `tracing.otlp_endpoint`, because plugins cannot read deckwatch's settings. gRPC: `host:port`. HTTP: a base URL. |
| `OTLP_PROTOCOL` | `grpc` | `grpc` or `http/protobuf`, for the collector-to-backend connection. |
| `OTLP_INSECURE` | `false` | Plaintext export, like `tracing.otlp_insecure`. It must agree with an `http://` or `https://` scheme. |
| `OTLP_HEADERS` | — | A Secret field holding exporter headers, usually credentials, in `name=value,...` form. |
| `SIGNALS` | `traces` | Which of `traces`, `metrics` and `logs` the collector forwards. |
| `APP_OTLP_PROTOCOL` | `grpc` | How the application reaches the sidecar (`http://localhost:4317` or `:4318`). |
| `COLLECTOR_IMAGE` | `otel/opentelemetry-collector-contrib:0.165.0@sha256:…` | Must be pinned. `:latest` is rejected. |
| `COLLECTOR_*_REQUEST` / `_LIMIT` | `100m`/`500m` CPU, `128Mi`/`256Mi` memory | Sidecar resources. |
| `BATCH_*`, `MEMORY_*` | collector defaults | Tuning for the `batch` and `memory_limiter` processors. |

**Exporter headers.** The values go into a Kubernetes Secret in each workload's namespace.
That Secret is mounted read-only into the collector only, and its values never appear in the
pod spec or in environment variables. deckwatch 0.6.x passes `Secret` fields saved from the
plugin settings form to plugins still encrypted. The plugin refuses that ciphertext, so
supply `OTLP_HEADERS` through `inherit_env_keys` (an environment variable on the deckwatch
pod) or the `update_plugin_config` MCP tool instead.

### 5.2 Enable for a deployment

Opt the deployment in with an annotation, or with a pod template label when you create it:

```yaml
metadata:
  annotations:
    deckwatch.io/otel: enabled
```

Then save the deployment through deckwatch's deployment API: the deployment edit form in the
UI, or `POST`/`PUT /api/namespaces/{ns}/deployments[/{name}]`. These are the only paths that
run plugins. The YAML editor, the MCP `create_deployment`/`update_deployment` tools, `kubectl`
and GitOps do not.

The deployment gets:

- an `otel-collector` sidecar. Its OTLP gRPC (4317) and HTTP (4318) receivers listen on
  127.0.0.1 only, so other pods cannot send through it. Data passes through `memory_limiter`
  and then `batch` before it is exported to the backend;
- on the application container: `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317`,
  `OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_SERVICE_NAME=<deployment name>`,
  `OTEL_RESOURCE_ATTRIBUTES` (`k8s.namespace.name`, `k8s.deployment.name`) and
  `OTEL_<SIGNAL>_EXPORTER`. Variables you set yourself are kept;
- the ConfigMap `<deployment>-otelcol` (the collector configuration) and, if headers are
  configured, the Secret `<deployment>-otelcol-headers`, both in the deployment's namespace.

Saving again changes nothing. To **disable**, set `deckwatch.io/otel: disabled` and save.
deckwatch 0.6.x does not remove what a plugin added earlier, so also delete these by hand
from the deployment YAML:

- the `otel-collector` container;
- the `otelcol-config` and `otelcol-headers` volumes;
- the env vars listed in the `deckwatch.plugin-env/otel` annotation.

Then delete the ConfigMap and Secret.

### 5.3 Enable for an application

Provision the **OpenTelemetry Collector** resource (id `tracing`) on the application:

- **UI:** the **Infrastructure** tab;
- **MCP:** `provision_resource` with `plugin_name: otel` and `resource_id: tracing`;
- **REST:** `POST /api/namespaces/{ns}/applications/{app}/resources/otel/tracing`.

deckwatch's reconciler then adds the sidecar and the variables to every deployment and
cronjob in the application. On deckwatch 0.6.x the values are the same for every workload,
so there is no per-deployment `OTEL_SERVICE_NAME`. Variables with the same names that you set
yourself are replaced. Do not use application-level enablement for applications with
cronjobs: their Jobs never complete while the sidecar keeps running.

After you change the plugin settings, click **Refresh** on the resource; the pods roll with
the new configuration. To **disable**, deprovision the resource. `associate_plugin` has no
effect on this plugin.

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
which is why the plugin defaults `OTEL_SERVICE_NAME` to the deployment name. The deployment
page does not show a trace list yet; for now, use the API or the backend's UI.

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
| `otlp_endpoint` | Operators | Where the backend receives OTLP. deckwatch itself does not use it, and plugins cannot read it, so copy it into the plugin's `OTLP_ENDPOINT`. deckwatch's own export is configured separately ([§10](#10-deckwatchs-own-telemetry)). |
| `otlp_insecure` | Operators | `true` when that endpoint is plaintext. Copy it into the plugin's `OTLP_INSECURE`. |

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

The plugin does not instrument code. The application needs an OpenTelemetry SDK, or
auto-instrumentation such as the Java agent, `opentelemetry-instrument` for Python, or
`--require @opentelemetry/auto-instrumentations-node` for Node. Most SDKs configure
themselves from the variables the plugin sets: `OTEL_EXPORTER_OTLP_ENDPOINT`,
`OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` and
`OTEL_<SIGNAL>_EXPORTER`. Some SDKs default to a different OTLP protocol than the sidecar
expects; the plugin sets `OTEL_EXPORTER_OTLP_PROTOCOL` so that the protocol and the port
match.

## 8. Troubleshooting

| Symptom | Check |
|---|---|
| No sidecar after enabling at deployment level | The deployment was saved through a path that does not run plugins ([§5.2](#52-enable-for-a-deployment)). The plugin is not loaded (`list_plugins`). deckwatch logs `otel-collector not injected into <deployment>` with the configuration error. deckwatch is older than 0.6.4. |
| Provisioning the application resource fails | The error names the plugin setting to fix, for example a missing `OTLP_ENDPOINT`. |
| Pod stays in `ContainerCreating` | The ConfigMap `<name>-otelcol` (or the Secret) does not exist yet. deckwatch creates it right after the deployment, and the kubelet retries the mount. |
| Collector logs `Exporting failed` | `OTLP_ENDPOINT`, `OTLP_PROTOCOL` and `OTLP_INSECURE` do not match the backend, or the backend rejects the headers. Check the collector logs (container `otel-collector`). |
| The application cannot connect to the sidecar | The SDK's protocol differs from `APP_OTLP_PROTOCOL`, or you overrode `OTEL_EXPORTER_OTLP_ENDPOINT` yourself. |
| Trace queries return `unavailable_reason` | `query_url` is unset in the settings ConfigMap, or deckwatch cannot reach it. `HTTP 404` means `query_url` points at Tempo, which does not serve the Jaeger query API ([§6.1](#61-trace-query-proxy)). |
| Queries find no traces | The application's `service.name` is not the deployment name, or the application is not instrumented. |
| CronJob Jobs never finish | The collector was enabled at application level ([§5.3](#53-enable-for-an-application)). |

## 9. Security

- **Collector receivers listen on 127.0.0.1 only**, and the sidecar declares no ports. Only
  containers in the same pod can send telemetry through it.
- **Exporter credentials live in a namespaced Secret** that is mounted read-only into the
  collector and referenced from its configuration by file path. They are never in the pod
  spec, in environment variables or in the ConfigMap.
- **Namespaced objects only.** The plugin creates a ConfigMap and, with headers configured,
  a Secret in the workload's namespace. It creates no RBAC and no cluster-scoped objects.
- **No plugin egress.** Keep the plugin's `allowed_hosts` empty.
- **Pinned images.** The collector image is pinned by digest, and `:latest` is rejected. Pin
  the bundled backend images as well.
- **Container hardening.** deckwatch 0.6.x does not let plugins set a sidecar
  `securityContext`. The default collector image runs as a non-root user (UID 10001) from a
  `scratch` base. It also works with a read-only root filesystem and all capabilities
  dropped.
- **Trace queries** go only to the configured `query_url`, with a strict allowlist on the
  service name and a capped result size.

## 10. deckwatch's own telemetry

deckwatch can export what it records about itself over OTLP: its own traces, its log lines and
metrics. This includes the embedded mcp-k8s library, which implements most MCP tools, and plugin
log lines. It is unrelated to the collector sidecar in [§5](#5-the-opentelemetry-collector-plugin),
which handles your workloads' telemetry. Export is **off** unless an OTLP endpoint is configured,
and with no endpoint deckwatch behaves exactly as before.

### 10.1 What is exported

| Signal | What | Notes |
|---|---|---|
| Traces | One span per HTTP request (`http`, with `method`, `path` and `version` attributes), plus any span deckwatch or a library opens | Requests include MCP calls (`POST /mcp`), health probes and `/metrics` scrapes. Drop the latter in your collector if you don't want them. |
| Logs | Every log line that passes the log filter, as an OTLP log record carrying the trace and span id of the span it was logged in | Includes the `mcp tool call` lines (tool, status, duration; never arguments), mcp-k8s library lines and plugin lines (target `extism::pdk`, attribute `plugin` = the extism instance id). Lines from background tasks (GitOps poller, plugin reconciler) have no span, so no trace id. |
| Metrics | Log events with fields named `monotonic_counter.*`, `counter.*` or `histogram.*` | None today; this is how libraries that only use `tracing` (mcp-k8s) will report metrics. Prometheus `/metrics` is unchanged and separate. |

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
| `telemetry.otlp.endpoint` | `""` | `OTEL_EXPORTER_OTLP_ENDPOINT`. Empty with `tracing.enabled` means the bundled backend ([§10.3](#103-with-the-otel-plugin-and-the-bundled-backend)). |
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

Point both at the same backend when that is what you want ([§10.3](#103-with-the-otel-plugin-and-the-bundled-backend)).

### 10.3 With the otel plugin and the bundled backend

- **The otel plugin** (§5) adds a collector sidecar to *your workloads*. deckwatch doesn't use it.
  To see deckwatch next to your applications, export both to the same backend or central
  collector. Remember that deckwatch needs that backend's **OTLP/HTTP** port (4318), while the
  plugin is often configured for gRPC (4317).
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
| Spans are named `http`, with kind `internal` | Semantic-convention names and kinds come with the MCP and HTTP instrumentation planned next. |
| The last few seconds of telemetry are lost on shutdown | deckwatch has no graceful shutdown yet, so the final batch (up to 5 s) is not flushed. |
| The `plugin` attribute is an instance id, not the plugin name | That is what extism records. The plugin's message usually names it. |

## 11. Known limitations (deckwatch 0.6.x)

| Limitation | Effect | Workaround |
|---|---|---|
| Disabling a plugin does not remove what it injected at deployment level | The sidecar and env vars stay | Remove them from the YAML ([§5.2](#52-enable-for-a-deployment)) |
| Application-level injection is static and applied with force | No per-deployment service name; same-named user variables are replaced | Use deployment-level enablement |
| Application sidecars are also added to cronjobs | Their Jobs never complete | Do not enable at application level for applications with cronjobs |
| Plugins cannot set `securityContext`, `args` or probes on sidecars | Hardening relies on the image defaults | — |
| `Secret` plugin fields saved in the UI reach plugins encrypted | `OTLP_HEADERS` set that way is refused | Use `inherit_env_keys` or `update_plugin_config` |
| Plugins cannot read `tracing.otlp_endpoint` | The endpoint must be set twice | Copy it into `OTLP_ENDPOINT` |
| The query proxy reads tracing settings from the ConfigMap, not the database | `PUT /api/settings` does not change trace queries | Set `settings.defaults.tracing` in Helm values |
| The query proxy speaks only Jaeger's query API | No trace queries against Tempo | Use Jaeger for queries, or Grafana links with Tempo |
| The deployment page has no trace list | Traces are not shown in the deckwatch UI | Use the API or the backend's UI |
