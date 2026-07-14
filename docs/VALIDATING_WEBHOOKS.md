# ValidatingWebhookConfiguration Support

**Status:** Design proposal
**Date:** 2026-07-10
**Scope:** Register deckwatch as a Kubernetes admission webhook (Validating
first; Mutating as a follow-up) to enforce deckwatch-defined policy at the
kube-apiserver level rather than relying entirely on the 5s polling loop, and
to replace poll-driven "did something change?" with push-driven admission
events.
**Related:** `docs/PRODUCT_ROADMAP.md` §Auth / P0 #4,
`docs/ARCHITECTURE_DECISION.md` §Audit log (webhook events are the richest
audit source), `docs/ARCHITECTURE.md` §Polling strategy

---

## TL;DR

Add an HTTPS `/admission/validate` endpoint served by the existing axum
process (new TLS listener, port `9443`), backed by a
`ValidatingWebhookConfiguration` cluster-scoped resource that routes
Deployment, StatefulSet, Pod, Namespace, and cluster-scoped policy events to
deckwatch. The webhook does **two** jobs:

1. **Policy enforcement** — apply the guardrails the roadmap has been
   promising (memory limit ≥ request; no probes → warn; missing ingress
   class → deny; namespace-scoped resource quota drift → deny; RBAC-check
   the user against deckwatch's own policy for changes that bypass the
   deckwatch UI).
2. **Change feed** — every admitted event fans out to an internal
   subscriber (audit log write, WebSocket push to the UI, cache
   invalidation). This is what supplements — and eventually replaces —
   the 5s polling loop for state alignment.

TLS certs are managed by the built-in `cert-manager` integration if
present, else by a self-signed cert deckwatch generates at startup and
publishes into the webhook config via `caBundle`. Fallback behavior is
`failurePolicy: Ignore` (deny nothing when webhook down) for enforcement,
and "resume polling at higher frequency" for the change feed.

The lift is **L** — TLS + certificate rotation + the webhook config
lifecycle are the non-obvious hard parts; the axum handler itself is
small.

---

## 1. Motivation

Two current pain points and one strategic opportunity:

1. **Guardrails from the roadmap can't be enforced client-side.** The
   deckwatch UI can refuse to submit a Deployment with `memory.limit <
   memory.request`, but any user with `kubectl` can bypass the UI and
   POST the same YAML. If deckwatch is the "single pane of glass for
   deployments", policy has to be enforced where the API server sees
   *every* request — that's an admission webhook.
2. **Polling is slow and expensive.** `watcher.rs:35` polls every 10s;
   the frontend polls every 5s. This gives users a 5–10s window of "I
   just clicked Deploy but the UI doesn't reflect it yet", and it puts
   constant read load on kube-apiserver even when nothing is changing.
   Admission webhooks give us the change signal for free at the moment
   the API server decides to accept a change.
3. **Audit log needs the actor.** The roadmap's audit log (P0 #4) works
   well for actions taken *through* deckwatch (we know the OIDC user).
   For actions taken *outside* deckwatch (kubectl, other operators,
   CI/CD), the polling loop can only tell us "the state changed", not
   who did it. The webhook `AdmissionRequest` gives us `userInfo` — the
   actual identity that made the change — for free.

The change from "poll + reconcile" to "webhook + poll-as-fallback" is a
significant architectural shift. This document lays out the design;
adoption is deliberately incremental (see §7).

---

## 2. Which K8s events to intercept

Not everything. Being on the admission path for `pods/*` at scale is a
foot-gun (every kubelet update goes through you). Scope tightly.

### 2.1 v1: Validation-only (recommended launch scope)

| Group/Kind | Operations | Purpose |
|---|---|---|
| `apps/v1/deployments` | CREATE, UPDATE, DELETE | Guardrails (limits vs requests, probes, image pinning); change feed to UI; audit-log actor |
| `apps/v1/statefulsets` | CREATE, UPDATE, DELETE | Same, plus PVC-template-immutability warnings before the K8s error |
| `batch/v1/cronjobs` | CREATE, UPDATE, DELETE | Same + cron schedule sanity |
| `networking/v1/ingresses` | CREATE, UPDATE | Warn on missing ingressClassName (currently a silent-fail in the roadmap's "rough edges") |
| `v1/namespaces` | CREATE, DELETE | Enforce naming policy; block deletion of protected namespaces (kube-system, monitoring, deckwatch's own) |
| `v1/configmaps` (name = `deckwatch-config` or `deckwatch-app-*`) | UPDATE, DELETE | Deckwatch-authoritative state — RBAC check the actor before allowing external tools to overwrite |
| `apps/v1/deployments` `/scale` subresource | UPDATE | Audit the actor for the Prometheus-visible replica-count metric |

Deliberately **out of scope for v1**:
- `v1/pods` (kubelet churn — cardinality nightmare)
- `v1/services`, `v1/endpoints` (mostly system-driven)
- `v1/events` (would recurse — every admit generates an Event)
- Anything CustomResource — CRDs the operator installed have their own
  policy story

### 2.2 v2: Add pod-delete interception (opt-in)

`v1/pods` on DELETE only, `failurePolicy: Ignore`, `sideEffects: None`.
Purpose: catch "user (or eviction) is about to delete a critical pod"
and emit an audit event + optionally block if the pod is annotated
`deckwatch.io/protected=true`. The subset that matches is tiny (annotated
pods only) so the cost is low.

### 2.3 v3: Mutating webhook (separate design)

A Mutating webhook could:
- Inject default resource requests/limits (auto-fill from settings)
- Inject `deckwatch.io/owner` and `deckwatch.io/last-modified-by`
  annotations
- Auto-attach a PodDisruptionBudget for StatefulSets

Mutating is a bigger commitment (every request goes through us on the
mutation path, patch semantics are subtle) so a separate design is
warranted. Not in this document.

---

## 3. How this replaces / supplements polling

Two loops today, both replaced/reduced:

### 3.1 Frontend 5s poll

The frontend polls `/api/namespaces/{ns}/deployments` every 5s. With the
webhook feeding a change stream, the frontend can subscribe to Server-Sent
Events (SSE) on `/api/events/stream?ns=…` and get updates within
~milliseconds of the API server accepting them. The 5s poll becomes a
5-minute "safety net" refresh, cutting API read load ~60×.

Wire shape:
```
kubectl edit deploy myapp
  → apiserver → webhook (ValidatingAdmissionWebhook)
      → deckwatch /admission/validate
          → allow + emit(ChangeEvent { ns, kind, name, actor, diff })
              → audit_log.insert(...)
              → sse_broadcast.send(...)
                  → frontend receives it
                      → store updates
                          → UI re-renders (< 100ms end-to-end)
```

### 3.2 Backend git-poller

`src/watcher.rs::run_poller` polls Git every 10s, then patches Deployment
annotations. Not replaced by the webhook — the webhook is about *K8s* API
changes, not git. But it *composes* with it: when the poller patches an
annotation, the webhook sees the update, and the audit event correctly
records `actor: deckwatch-git-watcher` rather than "unknown external
mutation". This closes a small but real audit gap.

### 3.3 State-alignment "reconciler"

The current design has no reconciler — every request re-fetches from
kube-apiserver. With webhook-driven change events + a small in-memory
cache invalidated on webhook admit, handlers can serve from cache when
fresh. This is the biggest performance win (roughly: constant read load
becomes zero read load between changes).

Design keeps the cache **optional and small** — a `moka::Cache` on
(cluster, gvk, ns, name) → resource, TTL 30s, invalidated on webhook
event. Handlers `cache.try_get_with(...)` and fall back to a live read.
When the webhook is down (failurePolicy=Ignore active, no push events),
the cache TTL keeps things fresh enough that the UI stays roughly
correct; the frontend polling safety net catches the rest.

---

## 4. Webhook registration and self-configuration

The `ValidatingWebhookConfiguration` is a **cluster-scoped** resource that
tells the API server where to send admission requests. Deckwatch needs to:

1. Create/update this resource on startup (or via the Helm chart).
2. Publish its CA bundle so the API server trusts the webhook's TLS cert.
3. Refresh both on cert rotation.
4. Delete or leave it cleanly on shutdown (opinionated: **leave** — see §5.5).

### 4.1 Helm-managed vs deckwatch-managed

Two viable approaches, we adopt **both** in a layered way:

**Helm-managed (default, boring):**
`helm/deckwatch/templates/validating-webhook.yaml` renders the config with
the CA bundle taken from a cert-manager `Certificate` (if installed) or
from a Helm-generated self-signed CA (via `genCA`/`genSignedCert` template
functions). This is the default because it means the webhook exists
before deckwatch's pod is even scheduled — no chicken-and-egg on first
install.

**Deckwatch-managed (opt-in, dynamic):**
When `webhook.selfManage: true`, deckwatch on startup:
- Generates a self-signed cert (via `rcgen` crate) if none exists
- Stores it in a Secret (`deckwatch-webhook-tls`) in its own namespace
- Patches the `ValidatingWebhookConfiguration.webhooks[].clientConfig.caBundle`
  with its CA
- Runs a rotation task that regenerates the cert every 60 days,
  re-publishing the CA bundle before the old cert expires

The dynamic mode is useful for deckwatch-as-operator scenarios and dev
environments where you don't want cert-manager. In prod, use cert-manager
+ Helm-managed config for the boring path.

### 4.2 The webhook config skeleton

```yaml
apiVersion: admissionregistration.k8s.io/v1
kind: ValidatingWebhookConfiguration
metadata:
  name: deckwatch
  annotations:
    # Only set when cert-manager is in play; makes cert-manager inject the CA
    cert-manager.io/inject-ca-from: {{ .Release.Namespace }}/deckwatch-webhook
webhooks:
  - name: deployments.deckwatch.io
    admissionReviewVersions: ["v1"]
    sideEffects: None                    # Well, technically we side-effect
                                         # into audit log — see §5.4 below
    timeoutSeconds: 5                    # Short — apiserver waits on us
    failurePolicy: Ignore                # Fail-open in v1; upgrade to Fail
                                         # in v2 once we trust the pipeline
    matchPolicy: Equivalent
    namespaceSelector:                   # Only namespaces deckwatch manages
      matchExpressions:
        - key: deckwatch.io/managed
          operator: In
          values: ["true"]
    rules:
      - apiGroups: ["apps"]
        apiVersions: ["v1"]
        resources: ["deployments", "deployments/scale"]
        operations: ["CREATE", "UPDATE", "DELETE"]
        scope: Namespaced
    clientConfig:
      service:
        name: deckwatch-webhook          # New Service, port 443 → pod 9443
        namespace: {{ .Release.Namespace }}
        path: /admission/validate
      caBundle: {{ .Values.webhook.caBundle | b64enc }}   # or cert-manager-injected
  # …one entry per Kind we intercept
```

Two design decisions worth flagging:

- **`namespaceSelector` gated on `deckwatch.io/managed=true`.** Deckwatch
  labels the namespaces it's allowed to touch (already the intent — the
  `allowed_namespaces` config becomes labels). This bounds blast radius:
  if the webhook misbehaves, only deckwatch's own namespaces are
  affected, not kube-system or the operator's control plane.
- **Separate Service (`deckwatch-webhook`) on port 443 → pod 9443.**
  The main deckwatch Service stays on port 80 for the UI/API. The
  webhook TLS listener is a different port + different Service because
  (a) the API server insists on HTTPS with a trusted cert, and (b)
  we don't want to accidentally expose the plaintext UI on the same
  socket that speaks TLS to the API server.

### 4.3 Multi-cluster and webhooks

Interaction with `MULTI_CLUSTER.md`: the webhook config is *per-cluster*
(each K8s cluster has its own admission chain). Deckwatch running against
N remote clusters needs a **network path** from each remote cluster's
API server back to the deckwatch webhook. That's usually the deal-breaker
for the multi-cluster + webhook combination — the API server initiates
the call, so deckwatch must be reachable from each cluster.

Three deployment shapes:

- **Deckwatch runs in one cluster, only that cluster gets the webhook.**
  Other clusters are managed by polling only. Simple, no network work,
  acceptable degradation. **Recommended default.**
- **A tiny "webhook shim" runs in each remote cluster** — same binary,
  webhook-only mode, forwards `AdmissionReview`s over an outbound
  connection to the central deckwatch. Complex; only justifies if
  multi-cluster policy enforcement is a hard requirement.
- **Deckwatch's webhook Service is public** (Ingress + TLS). All clusters'
  API servers call it. Simple network story, ugly security story
  (public admission endpoint = high-value target).

v1 ships shape 1 only.

---

## 5. TLS certificate management

The hardest part in practice. Two paths:

### 5.1 cert-manager path (recommended for production)

`helm/deckwatch/templates/webhook-certificate.yaml` (guarded by
`.Values.webhook.certManager.enabled`):

```yaml
{{- if .Values.webhook.certManager.enabled }}
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: deckwatch-webhook
spec:
  secretName: deckwatch-webhook-tls
  dnsNames:
    - deckwatch-webhook.{{ .Release.Namespace }}.svc
    - deckwatch-webhook.{{ .Release.Namespace }}.svc.cluster.local
  issuerRef:
    name: {{ .Values.webhook.certManager.issuerName | default "deckwatch-selfsigned" }}
    kind: Issuer
  duration: 8760h    # 1y
  renewBefore: 720h  # 30d
---
apiVersion: cert-manager.io/v1
kind: Issuer
metadata:
  name: deckwatch-selfsigned
spec:
  selfSigned: {}
{{- end }}
```

The `cert-manager.io/inject-ca-from` annotation on the webhook config
tells cert-manager to keep the `caBundle` synced. Rotation is entirely
outside deckwatch's control loop — cert-manager does it, kube-apiserver
picks it up.

Pod mounts the Secret at `/tls/{tls.crt,tls.key}` and axum's HTTPS
listener reloads on file change (via a filesystem-watcher +
`RustlsConfig::reload_from_pem_file`, an axum-server built-in).

### 5.2 Self-managed path (default for dev, opt-in for prod)

When cert-manager isn't installed and `webhook.selfManage: true`:

1. On startup, check for `Secret deckwatch-webhook-tls` in own namespace.
2. If absent or expired: generate CA + server cert with `rcgen`. Write
   to the Secret with SANs for the in-cluster service DNS names.
3. Patch the `ValidatingWebhookConfiguration.webhooks[].clientConfig.caBundle`
   with the CA. Requires `admissionregistration.k8s.io/validatingwebhookconfigurations`
   permission — new entry in the ClusterRole.
4. Spawn a rotation task: every 24h check cert expiry; if `< 30d`,
   regenerate + republish CA bundle. Grace: the CA bundle can contain
   two CAs during rotation (old + new) so no admission events are
   dropped during the swap.

Both paths converge on the same file layout at runtime (`/tls/tls.crt`,
`/tls/tls.key`) so the axum listener code is identical.

### 5.3 What can go wrong with certs

The classical failure: cert expires → API server can't reach webhook →
`failurePolicy: Fail` (if configured) blocks all admissions on the
targeted resources → cluster goes read-only for deckwatch-managed
namespaces. We mitigate:

- **Monitor cert expiry as a Prometheus gauge:** `deckwatch_webhook_cert_expires_seconds`.
  Alert at 30d out.
- **Prefer `failurePolicy: Ignore` in v1.** Upgrade to `Fail` only for
  specific rules where the guardrail is worth the availability cost
  (probably: deleting `deckwatch-config` ConfigMap).
- **Startup readiness gate:** don't mark the pod Ready until the TLS cert
  is loaded and the webhook config is confirmed in place with a matching
  CA bundle. Prevents rolling-restart races where the new pod publishes
  a new CA and the old pod (still serving via old cert) becomes
  unreachable.

### 5.4 sideEffects claim

We claim `sideEffects: None` — but writing to the audit log is a side
effect. This is a well-known K8s wart. The correct declaration when we do
mutate an internal store is `sideEffects: NoneOnDryRun` if we can
detect dry-run requests and skip the audit write (we can — `AdmissionRequest`
has `dryRun: true`).

Deckwatch implements: if `req.dryRun == true`, run all validation logic
but skip the audit write and skip the SSE broadcast. Every admission
review must be safe to run in dry-run mode.

### 5.5 Cleanup on uninstall

Two schools:

- **Delete the webhook config on Helm uninstall.** Clean. Risk: if the
  Helm hook order goes wrong and the API server tries to admit while the
  Service is gone, all admissions in matching namespaces fail. Only a
  problem with `failurePolicy: Fail`.
- **Leave the webhook config with an orphan finalizer.** Requires manual
  cleanup. Loud, hard to lose.

Ship default is **delete on uninstall** with a big warning in `NOTES.txt`
if `failurePolicy: Fail` is enabled anywhere. Helm hook order: delete
webhook config *first*, then delete the Service/Deployment.

---

## 6. Fallback behavior when the webhook is unavailable

Three failure modes:

### 6.1 Deckwatch pod down / crash-looping

- `failurePolicy: Ignore` → API server admits everything unimpeded. No
  guardrails, no push events. Frontend polling safety net picks up
  changes with 5s latency.
- `failurePolicy: Fail` → API server refuses matching operations. This is
  a self-inflicted outage; don't ship `Fail` until we have HA (§7 open
  question).
- Prometheus alert: `deckwatch_webhook_reachability_probe{cluster=…} == 0`.
  Alerts wire in via existing notification rules.

### 6.2 Cert expired / CA mismatch

- API server returns TLS handshake failure to caller.
  `failurePolicy` determines what happens next (same as pod down).
- Deckwatch's own view: it never sees the failed calls, so we monitor
  from the *outside* — a startup-time probe that sends a synthetic
  `AdmissionReview` to `deckwatch-webhook.<ns>.svc` and verifies a
  200 response.

### 6.3 Handler returns a 500 or times out

The API server treats this per `failurePolicy`. Handler-side we:

- Structured-log every admission with request ID and outcome.
- Emit `deckwatch_admission_reviews_total{outcome=…}` counter.
- Circuit-breaker: if the handler is failing at > 50% for 30s, we
  deliberately return a canned `AdmissionResponse{allowed=true}` and log
  loudly. Better a degraded audit trail than an outage.

### 6.4 Explicit fallback: promote polling frequency

When the webhook is `unhealthy` (self-diagnosed via the outside-in probe
in §6.2), the poller in `watcher.rs` bumps its interval from 10s to 2s
so that we catch changes faster in the absence of push events. The
frontend does the same. Automatic degrade / re-promote based on the
health signal — same signal the multi-cluster picker uses for the
cluster-health dot.

---

## 7. Rollout plan

### Phase 0 — foundation (1 sprint)
- New crate deps: `rcgen`, `rustls`, `axum-server` (already have axum;
  need the TLS listener variant), `moka` (for the cache)
- New listener on port 9443 with TLS, serving `/admission/validate`
- New Service `deckwatch-webhook` + Helm template
- ClusterRole gains permissions for
  `admissionregistration.k8s.io/validatingwebhookconfigurations`
  (get/create/update/patch, for the self-managed path)
- Handler is a stub: return `allowed: true` for everything, log request
  metadata

### Phase 1 — audit ingest + change feed (1 sprint)
- Handler starts writing `AdmissionRequest.userInfo` + object diff to
  the audit log (composes with ARCHITECTURE_DECISION §5 Phase 1)
- SSE endpoint `/api/events/stream` for the frontend
- Cache invalidation on admit
- Metrics: reviews_total, reviews_duration_seconds, cert_expires_seconds

### Phase 2 — guardrails (1 sprint)
- Enforce the roadmap "no memory.limit < request", "no missing probe on
  Deployment > 1 replica", "no missing ingressClassName" checks
- Each check has a config toggle (default warn, opt-in deny)
- Deny outcomes include a structured `AdmissionResponse.warnings[]` and
  `.status.message` explaining the rule violation

### Phase 3 — frontend integration (1 sprint)
- Frontend subscribes to SSE, updates stores on push
- 5s poll drops to 5m safety-net poll
- "Live" indicator dot in the UI header (green when SSE connected,
  yellow when polling-only)

### Phase 4 — pod-delete interception + protected-pod annotation (0.5 sprint)
- Adds v2 scope from §2.2
- Powers a "cannot delete this pod, it's marked protected" UX

### Phase 5 — production hardening (1 sprint)
- Cert-manager path battle-tested
- `failurePolicy: Fail` for well-established rules (e.g., protect
  `deckwatch-config` ConfigMap from external tools)
- HA story (see open questions)

Total: ~5 sprints to a fully-integrated webhook, incrementally shippable
after each phase.

---

## 8. Interaction with other roadmap items

- **P0 #4 Auth + Audit log** — the webhook is the *complete* audit
  source (captures kubectl and other-tool actions too). The audit log
  design in ARCHITECTURE_DECISION §5 needs one extra column,
  `source: 'deckwatch-ui' | 'webhook'`, and a Phase 0.5 ingest path from
  the webhook.
- **P0 #2 Rollback** — webhook admission events power the
  "deploy failed within 60s → auto-rollback" logic. When the webhook sees
  a Deployment go from Ready → NotReady after a spec change, we can
  trigger the rollback within seconds rather than waiting for the next
  poll cycle.
- **P0 #5 Metrics** — no direct interaction, but the SSE change feed
  can multiplex metric updates too (currently a separate pull).
- **STATEFULSET.md** — the immutable-`volumeClaimTemplate` warning is
  best surfaced as a webhook warning (`AdmissionResponse.warnings[]`)
  rather than a UI-only check, so kubectl users see the same message.
- **MULTI_CLUSTER.md** — see §4.3 above. Webhook is single-cluster in
  v1; the multi-cluster shim is a separate design.
- **P1 Deployment validation + dry-run** (roadmap) — the webhook is
  *literally* server-side validation. Roadmap §P1 mentions
  `Patch::Apply` with `dryRun=All` which composes: the frontend can
  submit a dry-run request, apiserver forwards it to our webhook, we
  return the same warnings/denials the user would get on a real submit.

---

## 9. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Cert expires, `failurePolicy: Fail`, cluster read-only for managed ns | low if monitored | catastrophic | Ship `Ignore` in v1; cert-expiry Prometheus alert; readiness gate on cert validity |
| Handler bug denies legitimate admissions | medium | high | Warn-first mode (`sideEffects: None`, always allow, log denials); require an explicit config-flip per rule to go Deny |
| Latency added to every admission (5s timeout is the API server's; we should be <100ms) | medium | medium | Cache resource lookups; profile p99 in staging; alert on `reviews_duration_seconds > 200ms` |
| Deckwatch deleted, webhook config orphaned, cluster admissions all fail | low | high | Helm uninstall deletes webhook config before Service; `failurePolicy: Ignore` default |
| Webhook + deckwatch in the same pod → deckwatch upgrade rolling restart drops admissions | high | medium | `PodDisruptionBudget minAvailable: 1`; startup readiness gate; consider running webhook as a separate Deployment in Phase 5 |
| Circular dependency: webhook validates the ConfigMap that configures the webhook | low | medium | Excluded ConfigMaps via `objectSelector` matchExpressions; unit-tested |
| Chatty audit log because kubelet/controllers generate spam updates | medium | low | Tight `rules` — no /pods, no /events; filter Deployment status-only diffs from the audit write |
| Multi-cluster + webhook: remote cluster can't reach central deckwatch | high | medium | Documented as unsupported in v1; polling-only fallback for remote clusters |
| dry-run false negative — check that read from the DB decides allow/deny in dry-run mode, then the state changes before the real submit | low | low | For dry-run, don't rely on DB state that could change (rare in practice — most guardrails are pure-function checks on the incoming object) |

---

## 10. Open questions

1. **HA for the webhook.** Single-pod deckwatch means every rolling
   restart is a ~10s window of `failurePolicy: Ignore` admissions.
   Options: (a) run webhook as a separate 2-replica Deployment sharing
   the audit DB via connection (only viable if DB moves to Postgres —
   ARCHITECTURE_DECISION §Option C), (b) run webhook in the same pod
   but ship a `PodDisruptionBudget minAvailable: 1` + rolling with a
   surge (requires replicaCount ≥ 2 which today is broken by SQLite —
   ARCHITECTURE_DECISION §6.2), (c) accept the outage window and
   default `failurePolicy: Ignore`. **Recommend (c) for v1**;
   revisit if the guardrails become load-bearing.
2. **Mutating webhook — separate design or add to this one?**
   Recommend separate; the semantics are different enough that
   conflating them muddies the API surface.
3. **Should webhook events be a public API** (e.g. a separate
   `/api/admission/stream` for external consumers to subscribe to)?
   Probably yes eventually — it's a natural extension point for
   integrations. Not v1.
4. **`objectSelector` vs `namespaceSelector`** — namespace is coarser
   but simpler. Object selector could let us match only
   `deckwatch.io/managed=true` at the *object* level (per-Deployment
   opt-in). Recommend namespace-level v1, object-level as a follow-up
   for advanced cases.
5. **Should we register as a `MutatingAdmissionPolicy`** (K8s 1.30+
   built-in CEL-based policy engine) instead of a webhook, for the
   pure-function guardrails? Cheaper (no webhook to run), but less
   flexible (CEL only, no audit-log side effect). Recommend: webhook
   for the audit path, CEL policies as a delivery channel for pure
   guardrails once K8s baseline is 1.30+. Two-track.
6. **Rate limit?** A misbehaving controller could hammer the webhook.
   Axum `tower::limit` middleware bounds concurrent admission handling
   at 100; excess returns 503 and `failurePolicy: Ignore` kicks in.
   Sane default.

---

## 11. References

- `src/routes.rs` — new listener + `/admission/validate` route
- `src/handlers/admission.rs` — new handler (this design's core)
- `src/watcher.rs:35` — poller interval becomes dynamic based on webhook
  health
- `src/state.rs` — gains an `event_bus` (broadcast channel) for the SSE
  fanout and cache invalidation
- `helm/deckwatch/templates/validating-webhook.yaml` — new
- `helm/deckwatch/templates/webhook-service.yaml` — new
- `helm/deckwatch/templates/webhook-certificate.yaml` — new (guarded)
- `helm/deckwatch/templates/clusterrole.yaml` — adds admissionregistration
  permissions when self-manage is enabled
- `docs/PRODUCT_ROADMAP.md` — the guardrails motivation (§Rough edges)
  and the auth/audit intersection (§Top 5 #4)
- `docs/ARCHITECTURE_DECISION.md` — audit log ingest path integrates
  with §5 Phase 1
- `docs/ARCHITECTURE.md` — polling → push architectural shift
