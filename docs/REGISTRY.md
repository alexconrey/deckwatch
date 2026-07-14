# Embedded OCI Registry

Deckwatch ships with an optional [OCI Distribution Spec v1.1] registry
built into the same axum binary. When turned on, deckwatch:

- Serves `/v2/*` on its main container port, so it acts as a real OCI
  registry that `docker push`, `podman push`, and `kaniko` all speak to
  natively.
- Publishes a browsable UI at `/registry` that lists repositories, tags,
  manifests, and layer sizes.
- Auto-populates a **Deckwatch Registry (local)** entry in the GitOps
  dialog's OCI Registries dropdown so operators don't have to type the
  in-cluster URL by hand.
- Wires kaniko builds with `--insecure --skip-tls-verify` when the
  destination points at the embedded registry.

The goal is to eliminate the need for an external registry (ECR, Docker
Hub, GHCR) for local / dev / airgap deployments. Everything runs inside
your cluster on the same PVC that already survives pod restarts.

## Enabling the registry

Set `registry.enabled: true` in your Helm values and `helm upgrade`:

```yaml
registry:
  enabled: true

  # Optional overrides — defaults shown.
  storageSize: 10Gi
  storageClassName: ""      # empty = cluster default
  accessMode: ReadWriteOnce # ReadWriteMany if replicaCount > 1
  service:
    port: 5000              # kaniko's default port

  # Optional — leave empty to derive `<fullname>-registry.<ns>.svc.cluster.local:5000`
  publicUrl: ""

  # Only enable behind TLS + an auth proxy. No auth in the registry itself.
  ingress:
    enabled: false
```

This provisions:

- A `PersistentVolumeClaim` (`<release>-registry`) mounted at
  `/data/registry` on the deckwatch pod.
- A separate `Service` (`<release>-registry`) on port 5000 pointing at
  the same pod, so kaniko can push to a stable `registry:5000` URL.
- Environment variables (`DECKWATCH_REGISTRY_ENABLED`,
  `DECKWATCH_REGISTRY_ROOT`, `DECKWATCH_REGISTRY_PUBLIC_URL`) that turn
  on the `/v2/*` endpoints inside the app.

## Pushing with kaniko (via deckwatch GitOps)

Once the registry is enabled, the GitOps dialog on any deployment shows
**Deckwatch Registry (local)** as the first entry in the OCI Registries
dropdown. Pick it, save, and the very next build:

1. Kaniko is launched with:
   ```
   --destination=<release>-registry.<ns>.svc.cluster.local:5000/<app>:<sha>
   --insecure --skip-tls-verify
   ```
2. Kaniko streams layers to the registry over HTTP.
3. The manifest lands in the registry, `read_meta` writes a sidecar with
   the media type + digest, and the tag becomes visible in the Registry
   UI within seconds.
4. Deckwatch patches the target Deployment's container image to the new
   ref, and Kubernetes pulls it from the same registry.

## Pushing manually with docker or podman

Port-forward the registry Service:

```bash
kubectl -n deckwatch port-forward svc/<release>-registry 5000:5000
```

Then push against `localhost:5000`. `docker` insists on TLS by default,
so add `localhost:5000` to `daemon.json`'s `insecure-registries` list.
Podman is happier with `--tls-verify=false`:

```bash
podman push --tls-verify=false my-image:latest localhost:5000/my-image:latest
```

## Browsing images

Visit `/registry` in the deckwatch UI. The page shows:

- **Left pane**: every repository with tag count and total (compressed)
  size, plus a filter box.
- **Right pane**: tag table for the selected repo with digest, size,
  and push time.
- **Manifest button**: opens a dialog showing the raw manifest, the
  config blob, and every layer's digest/size/media type.
- **Delete button**: removes a tag from the registry. Blobs are kept
  (they may be shared with other tags); there's no garbage collector.

## Storage layout

Everything lives under `DECKWATCH_REGISTRY_ROOT` (default
`/data/registry`):

```
blobs/sha256/<hex>          content-addressable layer + config blobs
uploads/<uuid>              in-flight blob uploads (auto-cleared on finalize)
manifests/<repo>/<ref>      manifest bytes, indexed by tag AND by digest
manifests/<repo>/_meta/     sidecar JSON with media_type + digest + size
```

The store is safe under concurrent writes because every finished upload
is `rename(2)`'d into the CAS blob path — POSIX atomicity gives us
tearing-free reads even mid-push.

## Sizing the PVC

Kaniko emits one compressed layer per Dockerfile `RUN` plus one config
blob per image. Rough guidance:

| Use case              | PVC size |
|-----------------------|----------|
| Single dev deployment | 5Gi      |
| ~20 service images    | 10Gi     |
| CI mirror / airgap    | 50Gi+    |

Bump `registry.storageSize` and re-apply; expanding a PVC works on any
StorageClass with `allowVolumeExpansion: true`. Downsizing is not
supported by Kubernetes — you must delete + recreate.

## Migrating to an external registry

The embedded registry is intentionally low-ceiling: no auth, no
replication, no garbage collection, no signing. When you outgrow it:

1. In Settings → OCI Registries, add your external registry (ECR, GHCR,
   Harbor, etc.).
2. On each GitOps-enabled deployment, switch the OCI Registry dropdown
   to the new entry.
3. Trigger a build. Kaniko will now push to the external registry and
   the deployment will pull from there on the next reconcile.
4. Once the new registry is serving production, set
   `registry.enabled: false` and `helm upgrade`. The PVC is retained by
   default (per Kubernetes conventions) — delete it manually if you're
   sure you don't want the old images anymore.

You can run both registries simultaneously while you migrate: the
"Deckwatch Registry (local)" entry just moves down in the dropdown once
you add another.

## Security posture

The registry has **no authentication** on the `/v2/*` endpoints. This is
deliberate — auth is a deployment concern, not a registry concern.
Options in order of preference:

1. **Keep it on ClusterIP only** (default). Kaniko and kubelets in the
   cluster reach it; nothing outside does. This is fine for most
   dev/local setups.
2. **Add an auth proxy in front of the Ingress** if you enable
   `registry.ingress`. Use OAuth2-proxy, mTLS at the ingress
   controller, or a bearer-token sidecar.
3. **Use NetworkPolicies** to restrict which pods can hit the registry
   Service — usually just kaniko builder jobs and the deckwatch pod
   itself.

Do **not** enable the registry Ingress without one of (2) or (3) — an
unauthenticated public OCI registry is a very effective mining
platform for other people.

[OCI Distribution Spec v1.1]: https://github.com/opencontainers/distribution-spec/blob/main/spec.md
