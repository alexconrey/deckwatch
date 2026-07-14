# Deployment History & Rollback

Every Deployment mutation Kubernetes performs (image bump, env change,
resource tweak, probe change) creates a new ReplicaSet with a monotonically
increasing revision annotation. Deckwatch exposes these revisions as first-
class UI so a non-engineer can see "what shipped when" and undo a bad
rollout without touching `kubectl rollout undo`.

## Endpoints

### `GET /api/namespaces/{ns}/deployments/{name}/history`

Lists all ReplicaSets owned by the deployment, newest revision first.

Response:

```json
{
  "revisions": [
    {
      "revision": 7,
      "replica_set_name": "myapp-7f8b9c",
      "image": "myapp:v1.2.3",
      "replicas": 3,
      "ready_replicas": 3,
      "created_at": "2026-07-10T14:22:03Z",
      "change_cause": null,
      "is_current": true
    },
    {
      "revision": 6,
      "replica_set_name": "myapp-6a2d1e",
      "image": "myapp:v1.2.2",
      "replicas": 0,
      "ready_replicas": 0,
      "created_at": "2026-07-10T13:11:44Z",
      "change_cause": "rollback to revision 5 via deckwatch",
      "is_current": false
    }
  ]
}
```

Fields worth calling out:

- **`is_current`** — the frontend uses this to hide "Roll back to this"
  on the active revision.
- **`change_cause`** — free-form annotation
  (`kubernetes.io/change-cause`) written by `kubectl` when you pass
  `--record`, and by our own rollback endpoint. Nothing else populates
  it today.
- **`replicas` vs `ready_replicas`** — for non-current revisions both
  are usually `0` (Kubernetes scales the old RS down after a
  rollout). We include them anyway so the operator can spot a stuck
  rollout mid-flight (e.g. `ready_replicas: 2, replicas: 3` on the
  current revision + non-zero on the previous one).

### `POST /api/namespaces/{ns}/deployments/{name}/rollback`

Body:

```json
{ "revision": 5 }
```

Applies a strategic-merge patch that copies the target ReplicaSet's
`spec.template.spec` back onto the Deployment. The response is a
`DeploymentDetailResponse` with `pods` and `ingresses` empty — the
caller is expected to poll `GET .../{name}` for the reconciled state.

## Design decisions

### Why match ReplicaSets by selector, not ownerReference?

`kubectl rollout history` does the same — it lets us find revisions
even after an offline restore that dropped ownerReferences. It also
keeps the query cheap: one labeled list instead of an owner walk.

### Why preserve `spec.replicas` during rollback?

A rollback should undo the container spec, not resurrect the replica
count in effect at the time of the target revision. If the user
manually scaled to 5 replicas last week and now rolls back to
yesterday's image, they still want 5 replicas — not the 3 that were
running when the target revision was created.

### Why write a `change-cause` annotation?

Without it, the new revision that our rollback creates would show up
in history with no context — indistinguishable from a manual edit.
Stamping the annotation makes it clear to the next operator that this
row was a rollback, not a forward change.

## Frontend flow

`HistoryCard.vue` mounts on `DeploymentDetailPage` and polls once on
mount. Every "Roll back" click opens a `ConfirmDialog`, and on
confirmation calls `POST .../rollback` then refetches history plus the
deployment detail. If the rollback fails, the error banner sticks
inside the card (not the page-level alert) so the operator can compare
it against the row that triggered it.

The card also exposes a `refresh()` method via `defineExpose`, so the
parent detail page can force a re-fetch after an edit/scale/restart
without waiting for the next 5-second poll.

## What's intentionally *not* done here

- **No auto-rollback on failed deploys.** The product spec mentions it
  as a follow-up; it needs a health-signal source (metrics-server or
  Prometheus) to be useful, and that lands with the resource-metrics
  feature. Manual rollback works well as a stopgap.
- **No revision diff.** Users can compare images by eyeballing the
  table today. A future revision-diff view would want to render the
  full pod spec diff — probably a v-dialog with a monaco `diff-editor`
  wired to the YAML endpoint.
- **`revision-history-limit` unchanged.** Deployments cap history at
  10 by default; Deckwatch respects that limit. Bumping it is a
  per-deployment decision (impacts etcd size) and belongs in the YAML
  editor, not this feature.
