# StatefulSet Support

**Status:** Design proposal
**Date:** 2026-07-10
**Scope:** First-class management of StatefulSets in deckwatch — list, create,
edit, rollout, drill-down, and (critically) the PVC lifecycle and ordered-
rollout UX that make StatefulSets meaningfully different from Deployments.
**Related:** `docs/PRODUCT_ROADMAP.md` §Open Question 3 (StatefulSets in scope
for the target persona?), `docs/ARCHITECTURE_DECISION.md` (no new DB tables
needed — StatefulSet state lives in K8s)

---

## TL;DR

Add StatefulSet as a peer of Deployment in the UI and the API — same list/
detail/edit surfaces, plus three StatefulSet-specific concerns: **PVC
management** (create/resize/snapshot per replica), **ordered rollout** (a
canary-style "advance one pod at a time" UX using the built-in
`RollingUpdate.partition` field), and **pod-identity awareness** (pod-0 is
not interchangeable with pod-N; the UI reflects that).

The lift is **L** — the resource itself is nearly a drop-in of the Deployment
handlers (same GVK shape, mostly), but the PVC lifecycle and the ordered-
rollout UX are the meat. Roadmap Open Question 3 is a prerequisite; this
design assumes the answer is "yes, in scope" because the deckwatch persona is
increasingly running databases and stateful services on K8s.

---

## 1. Motivation

Deckwatch's persona today deploys stateless web apps and workers. But the
same persona increasingly runs:
- Postgres or Redis in-cluster (dev/staging, sometimes prod)
- Kafka/NATS/RabbitMQ (queues, event streams)
- Prometheus/Grafana/Loki (observability)
- Vector databases (Qdrant, Weaviate) for ML/RAG apps

All of these are StatefulSets. The current escape hatch is "learn kubectl or
give up" — which contradicts the roadmap's core thesis of "no kubectl needed
for the target persona".

Two orthogonal things make StatefulSets different from Deployments:
1. **Persistent identity.** Pods have stable names (`myset-0`, `myset-1`)
   and stable network identity via a headless Service. `myset-0` is *not*
   interchangeable with `myset-1`.
2. **Persistent storage.** Each pod gets its own PVC created from a
   `volumeClaimTemplate`. Those PVCs outlive the pods and even the
   StatefulSet itself (K8s does not garbage-collect them by default —
   deliberate, because deleting a database is nearly always a mistake).

Both facts are why the ordered rollout matters and why the PVC UI is a
first-class concern.

---

## 2. What StatefulSet management looks like in the UI

### 2.1 List view

New tab in the sidebar next to Deployments: **StatefulSets**. Table columns:

| Column | Notes |
|---|---|
| Name | Same shape as Deployments list |
| Replicas | `ready / desired` — matches Deployments |
| Update strategy | `RollingUpdate (partition=N)` \| `OnDelete` |
| Rollout status | `stable` \| `rolling (M/N)` \| `paused (partition=K)` \| `stuck` |
| PVC status | e.g. `3/3 bound, 300 GiB` — sum across all replica PVCs |
| Storage class | Same across replicas (nearly always) |
| Age | Same as Deployments |
| Actions | Restart · Scale · Rollout · Delete |

The **PVC status** column is the load-bearing addition — a StatefulSet with
`0/3 bound` PVCs is a very different state from `3/3 running` pods and
users need to see it at a glance.

### 2.2 Detail page

`StatefulSetDetailPage.vue` mirrors `DeploymentDetailPage.vue` with three
extra tabs:

- **Overview** (default) — pod-by-pod grid showing `myset-0`, `myset-1`, …
  each as a card with:
  - status (Running/Pending/Terminating/CrashLoopBackOff)
  - image (with revision — see rollout tab)
  - PVC binding + capacity + used %
  - restart count
  - "This is the primary" badge if the operator has marked it (see §5)
- **Storage** — the PVC lifecycle UI (see §3)
- **Rollout** — the ordered-rollout UI (see §4)
- **Config** — spec editor, same shape as Deployment's config editor
- **Logs, Events, Metrics** — same shape as Deployment

### 2.3 Create form

Extend the existing `DeploymentForm.vue` template picker (roadmap P0 #1) to
include StatefulSet-appropriate templates:

- **Database** — 1 replica, 1 PVC, headless Service, PDB, resource limits,
  init-container for schema migrations, readiness probe on port
- **Message Queue** — 3 replicas, PVC per replica, headless Service, PDB
  `minAvailable: 2`
- **Search Index** — 3 replicas, PVC per replica, mesh Service
- **Blank StatefulSet** — power-user escape hatch

The template fills in `serviceName` (auto-derived from set name),
`volumeClaimTemplates`, `podManagementPolicy: OrderedReady`, and
`updateStrategy.rollingUpdate.partition: <replicas>` (so a fresh
StatefulSet does NOT roll on the first spec edit — see §4).

The form's **key difference** from the Deployment form: the "Storage" panel
is required, not optional. It asks for storage class, size, access mode,
and (when supported) a snapshot class. The rest of the form (image,
resources, probes, env) is identical to the Deployment form and shares
its Vue components.

---

## 3. PVC management

The three operations that matter, in order of frequency:

### 3.1 Create

Handled automatically by K8s when the StatefulSet scales up. The UI shows
what's about to happen before the user confirms:

> Scaling from 3 → 5 will create 2 new PVCs (each 100 GiB, storage class
> `gp3-encrypted`). Total added capacity: 200 GiB. Estimated cost impact:
> ~$16/month.

The cost line is only rendered when the P2 "Cost awareness" feature lands;
until then, size in GiB is what we show.

### 3.2 Resize (grow only — K8s doesn't allow shrink)

The most common StatefulSet operation after initial deploy. Requires:
- storage class has `allowVolumeExpansion: true` (deckwatch checks the SC
  before rendering the form; if false, disable the button with a tooltip)
- edit `spec.resources.requests.storage` on each PVC directly (the
  StatefulSet's `volumeClaimTemplate` is *immutable* after creation — K8s
  won't let you edit it in place — see the "delete StatefulSet with
  --cascade=orphan then recreate" workaround in §3.4)

Two-step UX:
1. **Resize PVCs first** — `PATCH` each `PersistentVolumeClaim`'s
   `spec.resources.requests.storage`. Deckwatch watches for
   `condition: FileSystemResizePending` → `condition: Resizing` →
   completion, and shows a per-PVC progress indicator. This works
   live — no pod restart needed for most CSI drivers (online resize).
2. **Update the template** (optional but recommended) — because the
   template is immutable, this requires the recreate flow in §3.4. Users
   who don't do this end up with a StatefulSet whose newly-added replicas
   are smaller than the existing ones — a confusing footgun. The UI warns
   loudly if PVC sizes drift from the template.

### 3.3 Snapshot (per-pod or fleet)

Depends on the cluster having a `VolumeSnapshotClass` (CSI snapshotter
installed). Deckwatch discovers this via the K8s API and only shows the
snapshot button if it's available.

UX:
- **Snapshot one PVC** — right-click a pod card → "Snapshot storage".
  Creates a `VolumeSnapshot` named `{pvc}-{ISO8601}` in the same
  namespace. Progress polled via the snapshot's `.status.readyToUse`.
- **Snapshot all PVCs of a StatefulSet** — bulk button on the Storage tab.
  Fires N snapshots in parallel, waits for all, offers a single "restore"
  action later that recreates PVCs from the snapshot set (this is the
  poor-person's cluster backup).

Restore is a **new StatefulSet** creation flow — never overwrites a running
one. The form is pre-filled from the snapshot metadata + the labels
`deckwatch.io/snapshot-of` / `deckwatch.io/snapshot-set` we stamp on
snapshots at creation. Cross-references two workflows the roadmap already
wants (History + Rollback for the general case; snapshots for the
StatefulSet-specific case).

### 3.4 Delete

Two decisions the UI must force the user to make explicitly:

1. **Delete the StatefulSet, keep the PVCs (`--cascade=orphan`).** The
   default when a template edit needs the recreate dance. UI language:
   *"Recreate StatefulSet — keeps existing storage, brief pod outage while
   we tear down and rebuild the set."*
2. **Delete the StatefulSet AND the PVCs.** Nuclear. UI language: *"Delete
   StatefulSet and destroy N PVCs (total {sum} GiB) — this cannot be
   undone unless you have snapshots."* Confirmation modal requires typing
   the StatefulSet name.

These two paths use different K8s API calls (`--cascade=orphan` vs
default), and both are audit-logged with the PVC list included.

The "PVC lifecycle" state machine:

```
[template]  → K8s creates → [PVC bound] → user resizes → [PVC bound, larger]
                              ↓                              ↓
                     StatefulSet deleted             template diverges from PVCs
                              ↓                              ↓
                       [PVC orphaned]              → recreate flow re-adopts
                              ↓
                     manual delete OR retention
```

Deckwatch surfaces orphaned PVCs (PVCs matching a `deckwatch.io/set=<name>`
label but no corresponding StatefulSet) on the Storage tab with a "Reattach
to new StatefulSet" or "Delete PVC" action.

---

## 4. Ordered rollout UX

This is the piece that most deckwatch users won't have seen before, because
Deployments handle rollouts implicitly. StatefulSets don't — by design.

### 4.1 The K8s primitives

`spec.updateStrategy.rollingUpdate.partition = N` means "only pods with
ordinal ≥ N get the new template; pods 0..N-1 stay on the old template".
This is the primitive canary/blue-green mechanism K8s gives you.

Example: a 5-replica StatefulSet with a spec change and `partition = 5`.
No pods are updated. Set `partition = 4` → `myset-4` gets the new template.
Set `partition = 3` → `myset-3` gets it. Continue until `partition = 0` →
full rollout done.

`partition` is edit-in-place on the StatefulSet (unlike the
`volumeClaimTemplate`).

### 4.2 The UX

New **Rollout** tab in the detail page (also accessible via a "Rollout"
button in the actions menu when a spec drift is detected).

Layout: a vertical timeline of pods, ordinals highest to lowest (so the
"canary" — highest ordinal — is at the top, matching how K8s progresses
partition-down).

```
┌─────────────────────────────────────────┐
│  myset-4   ●  updated (rev-7)   [logs]  │  ← canary
│  myset-3   ●  updated (rev-7)   [logs]  │
│  ─────────────────────────────────────  │  ← partition line (draggable)
│  myset-2   ○  pending (rev-6)   [logs]  │
│  myset-1   ○  pending (rev-6)   [logs]  │
│  myset-0   ○  pending (rev-6)   [logs]  │  ← last to update (primary in
│                                         │     most single-writer databases)
├─────────────────────────────────────────┤
│  [Advance one pod]  [Advance all]       │
│  [Pause]  [Rollback]                    │
└─────────────────────────────────────────┘
```

Buttons:
- **Advance one pod** — `partition = current - 1`, wait for the newly-
  updated pod to become Ready, show progress inline
- **Advance all** — `partition = 0`, one-shot roll (still respects
  `podManagementPolicy: OrderedReady` at the K8s level)
- **Pause** — leave `partition` where it is; the UI clarifies "N of M
  pods are on the new revision, remainder stay on the old until you
  advance or roll back"
- **Rollback** — `partition = replicas` AND revert the template to the
  previous revision (found via ControllerRevisions — StatefulSet's
  equivalent of ReplicaSets). Newly-updated pods get replaced with old-
  template pods, oldest to newest.

The rollback path composes with the P0 #2 general Rollback story — same UI
patterns, same audit event shape, different K8s primitive underneath.

### 4.3 Safety rails

The Rollout tab enforces:
- Cannot advance past a not-Ready pod (K8s enforces this too via
  `OrderedReady`; the UI shows an inline warning rather than letting the
  user get confused about why nothing happens).
- Warns if the new template's `volumeClaimTemplate` differs — that change
  requires the recreate flow, not a rollout (see §3.4).
- Warns if the new image tag is the same as the old one (probably
  unintentional).
- Requires "type set name to confirm" on rollback if any pod is currently
  serving traffic (headless-service selector matches).

---

## 5. Pod-identity awareness

`myset-0` is not `myset-1`. Some concrete places this matters in the UI:

- **Primary badge** — For single-writer databases (Postgres primary +
  replicas, Redis master + replicas), pod-0 is conventionally the primary.
  Deckwatch reads a `deckwatch.io/primary-pod` annotation on the
  StatefulSet if present, else falls back to "pod-0 is the primary" as a
  heuristic (correct for most templates we'd ship).
- **Delete-pod confirmations** — Deleting `myset-0` on a Postgres set will
  trigger a failover. The confirm modal calls that out explicitly:
  *"myset-0 is the primary; deleting it will trigger a ~30s failover
  window during which writes may fail. Continue?"*
- **Log stitching** — When a pod dies and comes back (same name, new
  UID), deckwatch stitches its logs together in the LogViewer with a
  visible "pod restarted at HH:MM" marker rather than starting a new
  log window. Small quality-of-life win, matters a lot for debugging
  crash loops on stateful workloads.

The pod grid on the Overview tab renders these signals — primary badge in
the top-right of the card, restart-count with a warn color if > 3 in the
last hour.

---

## 6. How this differs from Deployment management

Concrete deltas the implementer will hit:

| Concern | Deployment | StatefulSet |
|---|---|---|
| Rollout | Automatic on spec change (RollingUpdate w/ maxSurge/maxUnavailable) | Manual/partition-controlled; `OnDelete` requires explicit pod deletion |
| Pod identity | Ephemeral, random-suffixed name, interchangeable | Stable ordinal name, non-interchangeable |
| Storage | Usually stateless; volumes typically ephemeral or shared PVC | Per-pod PVC from `volumeClaimTemplate`; PVC outlives the pod |
| Scale down | Terminates a random pod | Terminates highest-ordinal pod first; PVC stays behind |
| Delete | Cascade to pods, ReplicaSets, PVCs (if owner-referenced) | Cascade to pods; **PVCs stay by default** (this is the footgun) |
| Update strategy | RollingUpdate is safe default | RollingUpdate w/ partition is a canary primitive, not a strategy detail |
| Rollback | ReplicaSet revert (built-in) | ControllerRevision revert (built-in but less well-known) |
| Service | Any Service (usually ClusterIP) | Headless Service required for stable DNS |
| PodDisruptionBudget | Optional, encouraged | Effectively required for HA (quorum concerns) |
| History | ReplicaSets (visible via `kubectl rollout history`) | ControllerRevisions (same tool, different resource) |

The delta drives all the UI/API decisions in §2–5.

---

## 7. Which handlers need to change

Concrete list. Everything is additive — no existing Deployment handler
changes.

### 7.1 New handlers

`src/handlers/statefulsets.rs` — mirrors `deployments.rs`:

- `list(cluster, ns)` — list StatefulSets
- `get(cluster, ns, name)` — one StatefulSet + its pods + its PVCs
- `create(cluster, ns, body)` — POST from the create form
- `update(cluster, ns, name, body)` — PUT full spec (rare — most edits go
  through the specialized endpoints below)
- `delete(cluster, ns, name, cascade_pvcs: bool)` — the two-mode delete
- `scale(cluster, ns, name, replicas)` — same shape as Deployment
- `restart(cluster, ns, name)` — annotate with restart marker; K8s rolls
  respecting current `partition`
- `advance_rollout(cluster, ns, name, target_partition)` — patch
  `spec.updateStrategy.rollingUpdate.partition`
- `pause_rollout(cluster, ns, name)` — freeze `partition` at current value
- `rollback(cluster, ns, name, target_revision)` — revert template from
  ControllerRevision + set `partition = replicas`
- `get_yaml`, `update_yaml` — same shape as Deployment
- `list_revisions(cluster, ns, name)` — ControllerRevision list for the
  History UI

`src/handlers/pvcs.rs` — new handler set for the PVC lifecycle:

- `list(cluster, ns)` — all PVCs (filter query for `set_owner=<name>`)
- `get(cluster, ns, name)`
- `resize(cluster, ns, name, new_size)` — patch
  `spec.resources.requests.storage`; also detects the "PVC size drifted
  from template" state and offers the recreate-with-orphan flow
- `list_snapshots(cluster, ns, pvc)` — VolumeSnapshots for a given PVC
- `snapshot(cluster, ns, pvc, name?)` — create a VolumeSnapshot
- `restore(cluster, ns, snapshot, new_set_body)` — create a new
  StatefulSet from a snapshot set
- `delete(cluster, ns, name)` — explicit PVC delete for the orphan cleanup
  case

### 7.2 New API resource on `AppState`

`src/state.rs` gains `statefulsets_api(cluster, ns)`, `pvcs_api(cluster,
ns)`, `snapshots_api(cluster, ns)` (dynamic API — VolumeSnapshot is CRD),
`controller_revisions_api(cluster, ns)` — all mirroring the existing
`deployments_api` shape.

### 7.3 Extended handlers

`src/handlers/pods.rs` — pod deletion gets a lookup for "is this pod
owned by a StatefulSet where ordinal 0 is annotated primary?" to render
the failover warning. No API change; just a richer response shape.

`src/handlers/applications.rs` — the Application concept
(`docs/APPLICATIONS.md`) currently groups Deployments and CronJobs. Add
StatefulSets as a third membership kind (label
`deckwatch.io/application=<app>` already generalizes; the frontend
Applications view lists them alongside Deployments).

`src/handlers/gitops.rs` — GitOps as currently designed only knows about
Deployments (`watcher.rs`). Extending to StatefulSets is out of scope for
this design; the argument for waiting is that database images don't
usually get pushed on every commit anyway. Document it as a follow-up.

`src/handlers/exec.rs`, `logs.rs`, `portforward.rs` — no change; these
already operate on pods and don't care what owns them.

### 7.4 New frontend surfaces

Under `frontend/src/components/pages/`:

- `StatefulSetsPage.vue` — list
- `StatefulSetDetailPage.vue` — detail with Overview / Storage / Rollout /
  Config / Logs / Events / Metrics tabs
- `pvcs/StorageTab.vue` — the per-pod PVC grid + resize/snapshot actions
- `pvcs/PVCResizeModal.vue` — resize wizard (checks
  `allowVolumeExpansion`, previews the two-step template-drift warning)
- `pvcs/SnapshotModal.vue` — snapshot wizard
- `pvcs/OrphanedPVCsPanel.vue` — surfaces PVCs whose set is gone
- `rollout/RolloutTab.vue` — the ordinal timeline + partition controls

Under `frontend/src/api/`:

- `statefulsets.ts` — mirrors `deployments.ts`
- `pvcs.ts` — new
- `snapshots.ts` — new

Under `frontend/src/stores/`:

- `statefulsets.ts` — mirrors `deployments.ts`

---

## 8. RBAC delta

The deckwatch ClusterRole (`helm/deckwatch/templates/clusterrole.yaml`)
needs:

```yaml
- apiGroups: ["apps"]
  resources: ["statefulsets", "statefulsets/scale", "statefulsets/status"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
- apiGroups: ["apps"]
  resources: ["controllerrevisions"]
  verbs: ["get", "list", "watch"]
- apiGroups: [""]
  resources: ["persistentvolumeclaims"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
- apiGroups: ["snapshot.storage.k8s.io"]
  resources: ["volumesnapshots", "volumesnapshotclasses"]
  verbs: ["get", "list", "watch", "create", "delete"]
- apiGroups: ["storage.k8s.io"]
  resources: ["storageclasses"]
  verbs: ["get", "list", "watch"]  # already have; needed to check allowVolumeExpansion
```

The snapshot APIs are guarded — if the CRDs aren't installed, deckwatch
detects this at startup and hides the snapshot UI (same discovery pattern
used today for PodMonitor in `state.rs:91-103`).

---

## 9. Rollout strategy for shipping this

### Phase 0 — read-only StatefulSet view (1 sprint)
- List page, detail page (Overview + Config + Logs + Events tabs only)
- No mutations; the Deployment list gets a "StatefulSets" tab added
- Ship-worthy on its own: makes StatefulSets no-longer-invisible

### Phase 1 — scale / restart / delete (1 sprint)
- Add the mutating handlers behind the existing deployment-shaped
  actions
- Delete flow gets the two-mode confirmation UI
- Scale flow shows PVC-creation preview

### Phase 2 — PVC lifecycle (1 sprint)
- Storage tab with the per-pod PVC grid
- Resize flow (both online resize and the recreate-with-orphan flow
  documented in-app)
- Snapshot + restore (guarded on VolumeSnapshotClass presence)

### Phase 3 — ordered rollout UI (1 sprint)
- Rollout tab with partition slider + advance/pause/rollback
- ControllerRevision-backed history view
- Composes with roadmap P0 #2 (general rollback) once that ships

### Phase 4 — templates + create form (1 sprint)
- StatefulSet templates (database, queue, index) plugged into P0 #1 App
  Templates
- Create form's Storage panel

Total: ~5 sprints (~10 weeks) from zero to full StatefulSet UX.

---

## 10. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| User deletes a StatefulSet and doesn't realize PVCs stayed → surprise bill for orphan PVCs | high | medium | Orphaned PVCs panel on Storage tab; audit-log the two-mode delete choice |
| User deletes with "destroy PVCs" and loses production data | low (with confirm) | catastrophic | Type-to-confirm modal; audit-log with the full PVC list |
| VolumeSnapshotClass missing → snapshot UI absent → user thinks feature is broken | medium | low | Show a disabled button with a tooltip "install a VolumeSnapshotClass to enable" |
| Partition-driven rollout confuses users used to Deployment auto-rollouts | high | low | Rollout tab has a mode toggle: "Manual (partition)" vs "Auto (partition=0 on save)". Auto matches Deployment mental model. |
| Rollback via ControllerRevision picks a template the user didn't expect (K8s retains 10 by default) | medium | medium | History tab shows revision diffs before applying |
| Resizing PVC on a driver that doesn't support online resize → pod goes offline | medium | medium | Detect the driver's `.spec.provisioner` + capability annotations; warn if offline resize is expected |
| Editing `volumeClaimTemplate` in the create form makes the user think it's editable later → immutable-field error at update | high | low | Field is marked "immutable after creation" in the form; edit form hides it and offers the recreate-with-orphan flow instead |
| GitOps flow expected to work for StatefulSets → doesn't | medium | low | Explicit "GitOps for StatefulSets: coming later" note in the UI; keep the tab absent for StatefulSets in v1 |

---

## 11. Open questions

1. **DaemonSets in the same UI motion?** DaemonSets are conceptually
   closer to Deployments (stateless) but share the "one per node" identity
   quirk. Recommend a follow-up: same handler shape, no PVC concerns,
   different "rollout across nodes" UX. Not in scope here.
2. **Operator-managed StatefulSets** (Postgres via CNPG, Kafka via
   Strimzi) — do we surface those as first-class or ignore them? The
   operator's CR is the real API. Recommend: list them as StatefulSets
   but link out to the CR view (once we support CRs) with a "This
   StatefulSet is managed by <operator>; edit its CR instead" banner.
3. **Backup integration** — snapshot support is basic; a full backup story
   (Velero, cloud-provider-specific tools) is out of scope. Snapshots
   here are the "point-in-time restore for a single StatefulSet" tool,
   not the disaster-recovery answer.
4. **Migration from Deployment → StatefulSet** — occasionally teams
   realize a Deployment should have been a StatefulSet. Any tooling
   help? Recommend: no — it's a rare, deliberate, high-risk change
   that shouldn't have a one-click button.
5. **Multi-writer StatefulSets** (Cassandra, Elasticsearch — no single
   primary) — the "primary badge" heuristic is wrong for these.
   Explicitly document that the badge is opt-in via the
   `deckwatch.io/primary-pod` annotation, else hidden.

---

## 12. References

- `src/handlers/deployments.rs` — reference handler shape to mirror
- `src/handlers/applications.rs` — Application grouping to extend
- `src/state.rs` — new typed APIs land here
- `src/watcher.rs` — GitOps loop, explicitly *not* extended in v1
- `helm/deckwatch/templates/clusterrole.yaml` — new resources listed
- `docs/PRODUCT_ROADMAP.md` §Open Question 3 — the product question
- `docs/ARCHITECTURE_DECISION.md` — no new DB tables; StatefulSet state
  is all K8s-native (§2.1 "K8s-native state stays in K8s")
- `docs/TEMPLATES.md` (roadmap P0 #1) — StatefulSet templates plug in here
