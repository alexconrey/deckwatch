# Deckwatch — Distributed Tracing Integration Design

**Status:** Design proposal
**Owner:** Platform / Deckwatch
**Related design docs:** `docs/PROMETHEUS_INTEGRATION.md` (opt-in monitoring pattern), `docs/ARCHITECTURE.md`
**Prototype targets:** `src/handlers/addons.rs` (extend catalog with `otel-collector`), `helm/deckwatch/` (new tracing backend sub-chart or values-gated deployment), frontend `<TracesTab>` and `<TraceLink>` widgets (new)

---

## 1. Problem

Deckwatch's metrics story is well-defined: Phase 1 point-in-time via metrics-
server, Phase 2 client ring buffer, Phase 3 Prometheus (`PROMETHEUS_INTEGRATION.md`).
Logs are already surfaced per-pod. The missing pillar is **traces** —
distributed request context that lets operators answer "why is this request
slow?" or "which downstream call failed?" without SSH-ing into containers.

Today deckwatch users have no path to enabling tracing on a managed workload
without hand-rolling:
- A tracing collector sidecar (or DaemonSet) manifest.
- The tracing backend itself (Jaeger, Tempo, or Zipkin).
- OTLP endpoint env vars into their app.
- A UI to actually view traces (Jaeger UI, Grafana, or ad-hoc port-forwards).

This design closes that gap using the exact same opt-in pattern that worked
for Prometheus:

1. A shared **cluster tracing backend** (Jaeger or Grafana Tempo) deployed
   once by the deckwatch operator, plumbed into deckwatch settings.
2. A per-deployment **`otel-collector` addon** that injects an OpenTelemetry
   Collector sidecar and the OTLP env vars the app needs to emit spans.
3. A deckwatch UI **trace viewer / trace-link** that turns pod logs and
   deployment events into deep-links into the tracing backend.

## 2. Scope

**In scope:**

- The `otel-collector` sidecar addon (catalog entry + wiring) — this is
  the shippable core.
- Recommendation on tracing backend (Jaeger vs Tempo), rationale, and how
  deckwatch would deploy it.
- Env-var injection strategy (OTLP endpoint, service name, exporter kind).
- Sidecar-config strategy: how the collector knows where the backend lives.
- UI integration: minimal trace-link from Pod/Deployment views to the
  backend UI; a deeper embedded viewer as a Phase 2.
- Effort estimate, split into shippable slices.

**Out of scope:**

- Tail-based sampling / advanced pipeline transforms in the collector.
  Ship the head-based-sample default and revisit.
- Log-to-trace correlation via `traceparent` extraction from container
  logs. Nice-to-have, ships once the base viewer works.
- Metrics forwarding through the collector. Deckwatch already has a
  Prometheus story; adding another metrics path would just duplicate it.
- Trace-based alerting (SLO burn from spans). Not our job.

## 3. Backend recommendation: **Grafana Tempo** as default, Jaeger as escape hatch

Both are OSS, both accept OTLP, both are production-proven. Here is the tradeoff
that matters for deckwatch:

| Concern | Grafana Tempo | Jaeger |
|---|---|---|
| Storage backend | Object storage (S3, GCS, Azure blob) — cheap, no operational overhead | Cassandra / Elasticsearch / OpenSearch — needs a DB team or a hosted service |
| Deployment complexity | Single Helm chart, stateless queriers over object-store | Multiple components (collector, query, ingester), stateful DB |
| Search/query interface | TraceQL (Grafana-native), plus Jaeger-API compat shim | Rich Jaeger UI with service graph, dependencies view |
| Native visualization | None built-in — expects Grafana Explore panel | First-class UI shipped with the binary |
| Existing deckwatch alignment | Deckwatch already recommends S3 for a registry (`docs/S3_REGISTRY.md`) — S3 already available in most target clusters | Would add a DB dependency |
| OTLP ingestion | Native | Native (since 1.35) |
| Multitenancy | Native, cheap | Requires all-in on the operator |
| Retention control | TTL knobs at compactor level, per-tenant | DB-schema retention (harder, DB-specific) |

**Recommendation: default to Tempo.** Its S3-backed storage matches the
"single deployment, no DB to babysit" ethos that deckwatch already leans into
for its registry. It is also the pragmatic pick if the cluster already runs
Grafana (many do, since deckwatch's Prometheus integration produces the
metrics Grafana consumes).

**Jaeger stays as an option** for shops that already run it, or that want the
native Jaeger UI without adding Grafana. The addon side of this design is
backend-agnostic — the collector exports OTLP either way — so switching
backends is a values.yaml change, not a code change.

### 3.1 Why an OpenTelemetry Collector *sidecar*, not direct app-to-backend

Three reasons the addon injects a collector instead of pointing the app
straight at the backend:

1. **Protocol translation & fan-out.** Apps often already emit spans in a
   proprietary flavor (Jaeger Thrift, Zipkin JSON, Datadog). A per-app
   collector accepts all of them and normalizes to OTLP. Switching backend
   later is zero-touch on the app.
2. **Buffering and batching.** The collector batches spans (default: every
   1s or 8192 spans) and holds a small in-memory queue across restarts of
   the backend. Direct-to-backend apps just drop spans on any backend
   hiccup.
3. **Local retry.** If the backend is temporarily unavailable, the sidecar
   retries; the app's request path is never blocked.

The alternative (a namespace-scoped or cluster-scoped collector DaemonSet)
saves a container per pod but shares failure domains — one crashy collector
takes traces down for every pod on the node, and the routing config gets
harder. Sidecar is the right default for a UI-driven "click to enable"
experience because the failure mode is scoped to that one workload.

## 4. How deckwatch deploys the tracing backend

Two options, roughly equal effort. Recommendation is **Option A** for the
initial ship because it matches how `helm/deckwatch/templates/servicemonitor.yaml`
already handles a similar shared-infrastructure concern (Prometheus operator
being present-or-absent).

### 4.1 Option A (recommended): values-gated sub-chart

Add a sub-chart under `helm/deckwatch/charts/tempo/` (or `charts/jaeger/`)
that vendors the upstream Tempo/Jaeger Helm chart. Enable via values:

```yaml
tracing:
  # Whether deckwatch should deploy a tracing backend at all. Set to
  # false if your cluster already has one you want to use.
  deployBackend: true
  # Which backend to deploy. Ignored when deployBackend=false.
  backend: tempo          # tempo | jaeger
  # Where deckwatch tells the sidecar addons to send OTLP.
  # Auto-computed from the sub-chart's service when deployBackend=true;
  # required when deployBackend=false and you point at an external
  # collector or backend (e.g. a managed Tempo, Honeycomb, Grafana Cloud).
  otlpEndpoint: ""        # e.g. http://tempo.observability:4317
  # Where the deckwatch UI deep-links users for a trace ID.
  # For Tempo, this is a Grafana Explore URL with the datasource param.
  # For Jaeger, it's the Jaeger UI base URL.
  uiUrl: ""               # e.g. https://grafana.internal/explore?left=...
  tempo:
    persistence:
      # Prefer S3 in prod; PVC ok for dev clusters.
      backend: s3         # s3 | gcs | azure | pvc
      s3:
        bucket: ""
        region: ""
        # Auth via IRSA — deckwatch already documents this pattern.
        # No secret-based access keys.
```

**Pros:** one `helm upgrade` gives you deckwatch + tracing. Consistent with
how deckwatch already ships its own Prometheus PodMonitor template
alongside the app.

**Cons:** ties tracing backend upgrades to deckwatch chart upgrades. Small
cost — Tempo minor bumps rarely require action from the deckwatch side.

### 4.2 Option B: separate deployment, deckwatch just consumes

Deckwatch ships nothing tracing-related in its chart. The cluster operator
installs Tempo/Jaeger via whatever mechanism they prefer (Helm, ArgoCD app,
Terraform module). Deckwatch reads `tracing.otlpEndpoint` and
`tracing.uiUrl` from settings and everything downstream (sidecar env vars,
UI deep-links) uses those.

**Pros:** cleaner separation of concerns. Backend upgrades independent of
deckwatch releases. Matches the current deckwatch stance on Prometheus:
"we consume it, we don't ship it."

**Cons:** more manual setup on day zero — operator has to find, install,
and configure Tempo/Jaeger *before* deckwatch tracing works. Discovery
story is worse: users may not realize the tracing feature exists until an
admin wires it up.

### 4.3 Chosen path

**Do Option A first, ship Option B as a supported alternative.** The
sub-chart is opt-in (`tracing.deployBackend: false` by default) so shops
with an existing backend can point deckwatch at theirs without spinning up
a redundant Tempo. Shops without any tracing today get a one-line values
change to bootstrap.

This mirrors what `PROMETHEUS_INTEGRATION.md` §3 concluded for the
Prometheus operator: deckwatch does not require you use its bundled thing,
but it works out of the box if you have nothing.

## 5. The `otel-collector` addon

This is the primary artifact of this design and the one thing that MUST land
for the tracing story to be usable.

### 5.1 Catalog entry

Added to `catalog()` in `src/handlers/addons.rs`, matching the shape of
existing entries (redis, memcached, nginx-proxy, fluent-bit):

```rust
AddonDefinition {
    id: "otel-collector".to_string(),
    name: "OpenTelemetry Collector".to_string(),
    description: "Sidecar that collects and forwards traces/metrics \
                  to the cluster tracing backend".to_string(),
    image: "otel/opentelemetry-collector:latest".to_string(),
    default_port: Some(4317),   // OTLP gRPC
    default_env: vec![
        EnvVarOutput {
            name: "OTEL_EXPORTER_OTLP_ENDPOINT".to_string(),
            value: "http://localhost:4317".to_string(),
        },
        EnvVarOutput {
            name: "OTEL_SERVICE_NAME".to_string(),
            // Placeholder — resolved to deployment name at attach() time.
            // See §5.3 for how the substitution happens.
            value: "{deployment_name}".to_string(),
        },
        EnvVarOutput {
            name: "OTEL_TRACES_EXPORTER".to_string(),
            value: "otlp".to_string(),
        },
    ],
    default_resources: Some(ResourceSpecOutput {
        cpu: Some("100m".to_string()),
        memory: Some("128Mi".to_string()),
    }),
},
```

### 5.2 What attach() does, unchanged from the current pattern

The existing `attach()` handler already:

1. Creates a sidecar container with `image = otel/opentelemetry-collector`,
   `port = 4317`, and the collector's own env (empty for now — see §5.4).
2. Injects `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`,
   `OTEL_TRACES_EXPORTER` into the **primary container** so the app SDK
   discovers the sidecar.
3. Records the injected env-var names in the
   `deckwatch.addon-env/<container_name>` annotation so `detach()` can
   remove exactly those names later without clobbering user env.

Nothing new here — the addon plumbing already handles all of this. The
tracing addon is purely a catalog addition from the code's perspective.

### 5.3 `OTEL_SERVICE_NAME` placeholder substitution — a small extension

Every existing addon in the catalog uses static default env values (e.g.
`REDIS_URL=redis://localhost:6379`). Tracing has a natural per-deployment
value: the service name should default to the Deployment name so that
traces from `checkout-api` show up as `checkout-api` in the UI without the
user having to think about it.

Two implementation options:

**Option A (minimal):** don't substitute anything. Ship the addon with
`OTEL_SERVICE_NAME` **omitted from `default_env`**. Users who want a
service name pass it via the `env` override on attach (the frontend can
pre-fill it with the deployment name in the "attach addon" modal). This
requires zero backend changes beyond the catalog entry.

**Option B (nicer UX):** support a `{deployment_name}` placeholder token in
`default_env` values that `attach()` interpolates against the Deployment's
`metadata.name`. This is a small addition to
`inject_addon_env_into_primary()` and the sidecar env construction: after
resolving `def.default_env`, run each value through a substitute-known-tokens
step.

**Recommendation: Option B**, because it generalizes cleanly (future
addons can use `{namespace}`, `{cluster}`, etc.) and is ~5 LOC of
substitution logic. Full stub:

```rust
fn interpolate(v: &str, ctx: &InterpolationCtx) -> String {
    v.replace("{deployment_name}", &ctx.deployment_name)
     .replace("{namespace}", &ctx.namespace)
}
```

Applied both when building the sidecar's env and when injecting into the
primary. If the addon catalog grows past a handful of these tokens,
promote to a real templating pass; for now, `String::replace` is fine.

### 5.4 Collector config: how does the sidecar know where to send?

The `otel-collector` binary needs a config file to know its receivers,
processors, and exporters. Two ways to get one in:

1. **Baked default via env vars only.** The collector supports a "no config
   file" mode when env vars are enough. Requires the collector be
   configured to pick up `OTEL_EXPORTER_OTLP_ENDPOINT` for its own
   downstream export. This is not the collector's default — it reads the
   file at `/etc/otelcol/config.yaml` and ignores those envs unless the
   config references them via `${OTEL_EXPORTER_OTLP_ENDPOINT}`.

2. **ConfigMap-mounted config, rendered by deckwatch.** deckwatch creates
   a per-deployment ConfigMap named `{deployment}-otel-collector-config`
   at addon-attach time, containing a templated `config.yaml`. Mount it
   into the sidecar at `/etc/otelcol/config.yaml`.

**Recommendation: option 2** for the initial ship. Template:

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318
processors:
  batch: {}
  memory_limiter:
    check_interval: 1s
    limit_percentage: 80
    spike_limit_percentage: 25
exporters:
  otlp:
    endpoint: {tracing.otlpEndpoint}         # from deckwatch settings
    tls:
      insecure: {tracing.otlpInsecure}       # default false
service:
  pipelines:
    traces:
      receivers: [otlp]
      processors: [memory_limiter, batch]
      exporters: [otlp]
```

This adds a small amount to attach() (create a ConfigMap alongside the
sidecar) and detach() (delete the ConfigMap). Cleanest owner: use
`ownerReferences` back to the Deployment, same pattern the Prometheus
design uses for materialized CRDs (`PROMETHEUS_INTEGRATION.md` §5.1).

**Note on scope:** if the config becomes a first-class thing users want to
edit, promote it to a settings tab (Phase 2). For now it is invisible
plumbing — an implementation detail of the addon.

### 5.5 What the "attached" state looks like in the API

`DeploymentDetail` already surfaces addon state via deployment-level
annotations. After attaching `otel-collector`:

```yaml
metadata:
  annotations:
    deckwatch.addon/addon-otel-collector: "otel-collector"
    deckwatch.addon-env/addon-otel-collector: "OTEL_EXPORTER_OTLP_ENDPOINT,OTEL_SERVICE_NAME,OTEL_TRACES_EXPORTER"
```

The AddonsCard in the frontend already knows how to render this. No new
API shape.

## 6. UI integration

### 6.1 Trace-link deep-link (Phase 1, ships with the addon)

Anywhere deckwatch already renders a **pod**, **deployment**, or **log
line with a trace ID**, add a "View traces" link that opens the
configured `tracing.uiUrl` with the appropriate query pre-filled.

Two entry points:

1. **Deployment detail page.** Right next to the AddonsCard row for the
   attached `otel-collector`, render:
   > *View traces for `checkout-api` →*

   URL construction (backend-agnostic, driven by `tracing.uiUrl`):
   - **Tempo (via Grafana Explore):**
     `{uiUrl}?left=[..."queries":[{"query":"{service.name=\"checkout-api\"}"}]]`
   - **Jaeger:** `{uiUrl}/search?service=checkout-api`

   The link renderer picks the format from the backend kind in settings.

2. **Log viewer.** If a log line contains a `traceparent`, `trace_id`, or
   `traceId` field (whether OTLP-flavored or Jaeger-flavored), render the
   trace ID as a clickable badge that opens the backend UI focused on
   that specific trace. Requires a small extraction pass in the log-panel
   component; the parsing itself is well-covered by open-source regex.

### 6.2 Embedded trace viewer (Phase 2, deferred)

Long-term the right move is an in-deckwatch trace viewer so users never
leave the app. That is a full component build — waterfall rendering,
span attribute inspection, service-graph fetch — and materially larger
than everything else in this design.

Deferring is safe because Phase 1 gets users to a working trace view via
Grafana/Jaeger, and we learn what queries they actually run before
committing UI to reproduce that surface area.

### 6.3 Settings surface for the URLs

Add to the deckwatch settings ConfigMap (mirrors §4.1 values):

```yaml
settings:
  defaults:
    tracing:
      # Where sidecar collectors export to.
      otlp_endpoint: ""
      # Whether the OTLP connection is plaintext or TLS.
      otlp_insecure: false
      # Where the deckwatch UI deep-links for a trace / service view.
      ui_url: ""
      # tempo | jaeger — controls the URL construction template above.
      backend_kind: "tempo"
```

Frontend reads these via the existing `/api/settings` endpoint. Empty
`otlp_endpoint` disables the addon toggle (or the UI shows it with a
"tracing backend not configured — talk to your admin" hint).

## 7. Effort estimate

Broken into shippable slices, matching the `PROMETHEUS_INTEGRATION.md`
convention.

| Slice | Scope | Effort |
|---|---|---|
| **A — Addon catalog entry** | Add `otel-collector` to `catalog()` in `handlers/addons.rs`; support `{deployment_name}` / `{namespace}` placeholder substitution; ConfigMap create/delete alongside sidecar attach/detach; unit tests for the substitution + configmap lifecycle | **~1.5 days** |
| **B — Backend Helm sub-chart (Tempo)** | Vendor tempo Helm chart under `helm/deckwatch/charts/tempo/`, values.yaml gate (`tracing.deployBackend`, S3 config), auto-wire `tracing.otlpEndpoint` from the sub-chart's Service, docs entry | **~2 days** |
| **C — Trace-link UI** | Read `tracing.ui_url` + `backend_kind` in the frontend, render "View traces" link on DeploymentDetail next to AddonsCard, add trace-id badge extraction in the log panel, empty-state ("tracing not configured") | **~1.5 days** |
| **D — Settings surface + docs** | ConfigMap + values.yaml plumbing for `tracing.*`, admin docs (`docs/TRACING.md`) covering backend choice, addon usage, per-app opt-in, troubleshooting | **~1 day** |
| **E — Embedded viewer (deferred)** | Full trace waterfall UI: fetch traces via the backend's query API, render spans, per-span attributes, service graph | **~5–8 days** |

**Total for shippable core (Slices A–D): ~6 days.** Slice A alone
(**~1.5 days**) delivers the "one-click sidecar" experience for shops
that already run Tempo/Jaeger and just need the injection wired up — a
defensible first ship.

## 8. Recommended sequencing

1. **Slice A first.** Backend-only PR. Adds the catalog entry, the
   placeholder substitution, and the ConfigMap plumbing. Users with an
   existing tracing backend and a manually-set `tracing.otlp_endpoint`
   can click "Attach OpenTelemetry Collector" and immediately get spans
   flowing. Low risk, low review surface, immediately useful.
2. **Slice D next.** Documents the settings and hooks up the values so
   subsequent slices have a home.
3. **Slice B in parallel with C.** Backend infra (sub-chart) and frontend
   (trace-link) don't touch each other — safe to ship independently.
4. **Slice E deferred.** Revisit only if users ask; Grafana/Jaeger UIs
   are good enough for a while.

## 9. Risks and open questions

- **Sidecar cost per pod.** ~30–50 MiB memory per collector, ~100m CPU
  under load. For workloads with 20+ replicas this adds up. If it becomes
  a problem, offer a "shared collector" mode (namespace-scoped Deployment
  that the app sends to directly) as an alternative addon variant — but
  measure first, don't preemptively add the complexity.
- **Backend selection drift.** If we default to Tempo and a shop already
  runs Jaeger, they can just set `tracing.backend: jaeger` and
  `tracing.deployBackend: false`. Fine. If they *don't* set the URL
  correctly, the sidecar buffers until it OOMs. Add a startup probe on the
  sidecar that tests the OTLP endpoint and surfaces the failure via pod
  events — deckwatch already renders pod events in the detail view.
- **OTLP TLS/auth.** Tempo/Jaeger in a shared cluster is often not
  TLS-fronted for internal OTLP; managed backends (Grafana Cloud,
  Honeycomb) require TLS + a bearer token. Support both via
  `tracing.otlp_insecure` (bool) and `tracing.otlp_headers_secret_ref`
  (Secret with `x-headers` key of the form `Authorization=Bearer ...`).
  Phase 2 — the initial ship targets in-cluster deployments.
- **Auto-instrumentation.** OpenTelemetry has language-specific
  auto-instrumentation (Java agent, Python `opentelemetry-instrument`,
  Node `--require @opentelemetry/auto-instrumentations-node`, Rust manual
  via the `tracing-opentelemetry` crate). deckwatch cannot magically
  instrument arbitrary apps — the addon assumes the app is already
  OTel-aware or the user has enabled auto-instrumentation via an init
  container. Document this loudly in the addon's description hover
  (out of the current `description` field's length budget — Phase 2 add a
  `long_description` markdown field to `AddonDefinition`).
- **CollectorConfig ConfigMap churn on settings changes.** When
  `tracing.otlp_endpoint` changes in deckwatch settings, existing
  attached collectors keep their old endpoint until the addon is
  re-attached. Fix in Phase 2 with a reconciler that patches all
  addon ConfigMaps on settings change; for the initial ship, document
  that a settings change requires re-attach.
- **`otel/opentelemetry-collector:latest` is a floating tag.** Aerospace-
  grade practice (per `CLAUDE.md`) is pinned images. Ship pinned to the
  current stable minor (e.g. `otel/opentelemetry-collector:0.109.0`) and
  let the operator override via `image` at attach time. Update the pin
  quarterly. (The team-lead spec says `:latest`; call this out as a
  small deviation for review.)

## 10. Alignment with existing deckwatch patterns

- **Opt-in per-Deployment** via addon annotations — same shape as the
  monitor CRDs in `PROMETHEUS_INTEGRATION.md`. Users always know what is
  or isn't attached by inspecting the Deployment.
- **Owner references for garbage collection** — ConfigMap owned by the
  Deployment, deleted when the Deployment is deleted. Same pattern the
  Prometheus design uses for PodMonitors.
- **Backend feature-flagged by presence** — no `tracing.otlp_endpoint`,
  no attach path is offered. Same as the Prometheus operator's CRDs.
- **Sensible defaults, escape hatches for the escape hatches** — Tempo
  is default, Jaeger is one values flag away, external collector is one
  more. Same layering as PodMonitor → ServiceMonitor → user-owned Service.
