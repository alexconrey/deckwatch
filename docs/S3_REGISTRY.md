# S3-Backed OCI Registry Storage

**Status:** Design + skeleton. Filesystem path unchanged; S3 backend gated behind
`registry.storage: s3` in helm values.

## Goals

- Let deckwatch's embedded OCI registry write blobs and manifests to S3
  (or S3-compatible: MinIO, R2, Ceph RGW) instead of a PVC.
- Zero behavioral change when `registry.storage: filesystem` (today's default).
- Keep the on-the-wire OCI Distribution Spec v1.1 surface identical — only the
  storage backend swaps.
- Preserve content-addressable atomicity: an in-progress upload must not become
  visible as a finished blob until digest verification succeeds.

## Non-goals

- Cross-backend migration tooling. Operators migrate by pushing again; blobs
  are cheap and content-addressable.
- S3 multipart tuning knobs beyond sane defaults. Kaniko layers are already
  compressed; single-shot puts under a threshold, multipart above.
- Garbage collection. Same as filesystem today — none. Track separately.
- Registry-side auth. Still deferred to the deployment layer.

## Crate recommendation

**Use [`object_store`](https://docs.rs/object_store)** (Apache Arrow project,
`v0.11+`). Reasons:

1. Cloud-agnostic — same trait covers AWS S3, GCS, Azure Blob, and local FS.
   Means we can collapse `Filesystem` and `S3` behind one `dyn ObjectStore`
   and drop the enum entirely if we choose.
2. First-class `LocalFileSystem` implementation makes tests hermetic — no
   `moto`/`minio` container needed for unit tests.
3. Built-in multipart upload with a streaming `MultipartUpload` handle that
   matches OCI's chunked PATCH flow naturally.
4. Handles endpoint override, path-style vs virtual-hosted, and standard AWS
   env-var credential discovery through `aws-config`-compatible builders.
5. Actively maintained by DataFusion / delta-rs; production-hardened.

Rejected alternatives:

- `aws-sdk-s3` directly — more control but doubles the surface area (we'd
  re-implement `object_store`'s abstraction for the filesystem case too).
- `rusoto_s3` — unmaintained since 2022. Do not add.

## Cargo.toml delta

```toml
[dependencies]
# existing entries kept as-is
object_store = { version = "0.11", features = ["aws"] }
bytes = "1"
async-trait = "0.1"
```

`object_store`'s `aws` feature pulls in `aws-config` and handles standard
credential chain (env, IMDS, IRSA-style web-identity, `~/.aws/credentials`).

## Storage layout on S3

Same logical layout as filesystem, just as S3 keys under an optional prefix:

```
<prefix>/blobs/sha256/<hex>
<prefix>/uploads/<uuid>
<prefix>/manifests/<name>/<reference>
<prefix>/manifests/<name>/_meta/<reference>.json
```

Notes:

- `<prefix>` defaults to empty (bucket root). Set it to share a bucket across
  environments.
- Blob keys are already content-addressable; S3 versioning is redundant but
  harmless — recommend leaving it disabled to save cost.
- Uploads: S3 has no "append" primitive. We use `MultipartUpload` for the
  PATCH stream and complete on PUT. See "Upload flow" below.
- `_catalog` and `tags/list` become `list_with_delimiter` calls scoped to the
  `manifests/` prefix. Cost is O(repo count) LIST requests; acceptable at
  our expected scale (hundreds of repos, not millions).

## Config surface

### CLI / env (`config.rs`)

```rust
/// Storage backend: "filesystem" (default) or "s3".
#[arg(long, env = "DECKWATCH_REGISTRY_STORAGE", default_value = "filesystem")]
pub registry_storage: String,

// S3-only. Ignored when storage=filesystem.
#[arg(long, env = "DECKWATCH_REGISTRY_S3_BUCKET", default_value = "")]
pub registry_s3_bucket: String,

#[arg(long, env = "DECKWATCH_REGISTRY_S3_PREFIX", default_value = "")]
pub registry_s3_prefix: String,

#[arg(long, env = "DECKWATCH_REGISTRY_S3_REGION", default_value = "us-east-1")]
pub registry_s3_region: String,

/// Custom endpoint for MinIO / Ceph RGW / R2. Empty = AWS default.
#[arg(long, env = "DECKWATCH_REGISTRY_S3_ENDPOINT", default_value = "")]
pub registry_s3_endpoint: String,

/// Force path-style addressing (MinIO). Auto-detected when endpoint is set.
#[arg(long, env = "DECKWATCH_REGISTRY_S3_PATH_STYLE", default_value = "false")]
pub registry_s3_path_style: bool,
```

Credentials come from the standard AWS chain — no explicit CLI flags. In
Kubernetes, prefer IRSA (`serviceAccountName` bound to an IAM role) over
static keys in a Secret.

### Helm values

```yaml
registry:
  enabled: false
  storage: filesystem   # "filesystem" or "s3"

  # Only used when storage=filesystem
  storageSize: 10Gi
  storageClassName: ""
  accessMode: ReadWriteOnce

  # Only used when storage=s3
  s3:
    bucket: ""
    prefix: ""
    region: "us-east-1"
    endpoint: ""          # e.g. https://minio.default.svc:9000 for MinIO
    pathStyle: false      # force path-style, auto-on when endpoint is set

    # Credentials. Prefer IRSA:
    #   serviceAccount.annotations["eks.amazonaws.com/role-arn"] = <role>
    # Otherwise reference an existing Secret with AWS_ACCESS_KEY_ID /
    # AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN keys.
    credentialsSecret: ""

  service:
    port: 5000
  publicUrl: ""
  ingress:
    enabled: false
```

The chart's deployment template already sets `DECKWATCH_REGISTRY_ROOT`; it
adds `DECKWATCH_REGISTRY_STORAGE` and, when `storage=s3`, the S3 vars from
`.Values.registry.s3.*`, plus `envFrom.secretRef` for the credentials secret.

The PVC + volume mount are wrapped in `{{ if eq .Values.registry.storage
"filesystem" }} ... {{ end }}` so S3 mode ships nothing to mount.

## API changes

Public surface (`RegistryStore::new`, all `pub` axum handlers, `pub(crate)`
helpers used by `registry_ui`) does **not** change. Internals are swapped
for a trait.

### The trait

```rust
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync + 'static {
    // Blobs (content-addressable)
    async fn blob_exists(&self, digest: &str) -> std::io::Result<Option<u64>>;
    async fn blob_get(&self, digest: &str) -> std::io::Result<BlobRead>;
    async fn blob_put(&self, digest: &str, bytes: &[u8]) -> std::io::Result<()>;
    async fn blob_delete(&self, digest: &str) -> std::io::Result<()>;

    // Manifests (mutable, tag-or-digest keyed)
    async fn manifest_get(&self, name: &str, reference: &str)
        -> std::io::Result<Option<Bytes>>;
    async fn manifest_put(&self, name: &str, reference: &str, bytes: &[u8])
        -> std::io::Result<()>;
    async fn manifest_delete(&self, name: &str, reference: &str)
        -> std::io::Result<bool>;

    // Sidecar meta (media type, digest, size)
    async fn meta_get(&self, name: &str, reference: &str)
        -> std::io::Result<Option<ManifestMeta>>;
    async fn meta_put(&self, name: &str, reference: &str, meta: &ManifestMeta)
        -> std::io::Result<()>;
    async fn meta_delete(&self, name: &str, reference: &str) -> std::io::Result<()>;

    // Uploads (chunked writes to a UUID-keyed staging area)
    async fn upload_begin(&self, uuid: &str) -> std::io::Result<Box<dyn UploadSession>>;
    async fn upload_resume(&self, uuid: &str) -> std::io::Result<Box<dyn UploadSession>>;
    async fn upload_length(&self, uuid: &str) -> std::io::Result<Option<u64>>;
    async fn upload_finalize(&self, uuid: &str, digest: &str) -> std::io::Result<()>;
    async fn upload_cancel(&self, uuid: &str) -> std::io::Result<()>;

    // Listing
    async fn list_repositories(&self) -> std::io::Result<Vec<String>>;
    async fn list_tags(&self, name: &str) -> std::io::Result<Vec<String>>;
}

#[async_trait::async_trait]
pub trait UploadSession: Send {
    async fn append(&mut self, chunk: &[u8]) -> std::io::Result<()>;
    async fn len(&self) -> u64;
}

pub enum BlobRead {
    Bytes(Bytes),
    Stream(axum::body::Body),
}
```

`BlobRead` exists so filesystem can stream from `File` (as today) while S3
can return a body wrapping `GetObjectOutput::body`. The GET handler adapts
either.

`RegistryStore` becomes:

```rust
#[derive(Clone)]
pub struct RegistryStore {
    inner: Arc<dyn StorageBackend>,
}

impl RegistryStore {
    pub fn filesystem(root: impl Into<PathBuf>) -> Self { ... }
    pub fn s3(cfg: S3Config) -> anyhow::Result<Self> { ... }
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> { ... }
}
```

Every handler method becomes a thin adapter that calls into `self.inner.*`
and maps `io::Error` to the appropriate `oci_error()`.

## Upload flow on S3

Filesystem today: open-append PATCH chunks to a file, rename to CAS on PUT.

S3 has no append. Two viable strategies:

1. **Multipart upload per session (chosen).** On upload_begin, call
   `create_multipart_upload` at `uploads/<uuid>`. Each PATCH becomes an
   `upload_part` (S3 requires >= 5 MiB parts except the last; buffer small
   PATCHes until we cross the threshold or finalize). On finalize:
   - Complete the multipart upload -> object at `uploads/<uuid>` (temp key).
   - Server-side `copy_object` from `uploads/<uuid>` to
     `blobs/sha256/<hex>` (S3 CopyObject is atomic and free within a
     region for <= 5 GiB; use UploadPartCopy above that).
   - Verify digest by re-reading the finished object (or by streaming-hash
     during the PATCH -- see optimization).
   - Delete `uploads/<uuid>`.

2. **Buffer to local disk, single-shot PUT on finalize.** Simpler code but
   defeats the point of an S3 backend (still needs a PVC scratch dir).
   Rejected.

### Streaming-hash optimization

Today's code re-reads the finished upload to compute sha256. On S3 this
means a full download after a full upload -- 2x network. Fix by keeping a
`Sha256` state on the `UploadSession` and finalizing it in `upload_finalize`
before the copy. Filesystem gets the same win. This is a follow-up, not
blocking.

### Multipart part-size buffering

Kaniko streams layers in ~4 MiB chunks; S3 min part is 5 MiB. Wrap the
`MultipartUpload` in a buffering adapter that flushes at 8 MiB boundaries.
`object_store::BufWriter` does this out of the box.

## Listing on S3

S3 LIST is O(1000) keys per call with pagination.

- `_catalog` = `list_with_delimiter(Some(prefix + "manifests/"))` walking
  common prefixes recursively until a directory contains at least one
  non-`_meta/` leaf. Same tree walk as today, just against object_store's
  `list_with_delimiter` instead of `read_dir`.
- `tags/list` = `list_with_delimiter(Some(prefix + "manifests/<name>/"))`,
  filter out `_meta/` and `sha256:*` keys.

Both are cold-path operations (UI browse, kaniko doesn't call them). No
caching in v1; add if it shows up on flamegraphs.

## Error handling

`object_store::Error` maps to `std::io::Error` via `into()` -- already
implemented upstream. Handlers keep using `io::Error` internally and map
to OCI error envelopes at the axum boundary as they do today.

- `NotFound` -> `BLOB_UNKNOWN` / `MANIFEST_UNKNOWN` / `BLOB_UPLOAD_UNKNOWN`.
- `PreconditionFailed` (unused today, might appear for conditional writes)
  -> `DENIED`.
- Everything else -> `UNKNOWN` (500).

## Testing

- **Filesystem** backend: existing tests keep passing unchanged.
- **S3** backend: two tiers.
  1. Unit-level: use `object_store::memory::InMemory` as a drop-in for
     `dyn ObjectStore` and reuse the filesystem test suite by parametrizing
     the backend. Fast, hermetic, no containers.
  2. Integration: `testcontainers::MinIO` behind `#[cfg(feature = "s3-it")]`.
     Runs in CI on-demand, not on every commit.

## Migration & rollout

1. Ship trait + `Filesystem` impl with the enum wired but only `filesystem`
   selectable (S3 path returns `unimplemented!` behind a `todo` guard).
   No functional change, keeps the PR small.
2. Ship `S3` impl behind `registry.storage: s3`, tested against MinIO.
3. Document IRSA setup in `docs/REGISTRY.md`.
4. Enable in one dev environment for a week before recommending prod.

Rollback is `helm upgrade` back to `storage: filesystem`; blobs pushed while
S3 was active stay in S3 (or get republished by kaniko).

## Effort estimate

Assuming one senior Rust engineer familiar with `object_store` / axum:

| Phase                                          | Effort  |
|------------------------------------------------|---------|
| Trait + filesystem impl (refactor, no S3 yet)  | 1.0 day |
| S3 impl (blobs, manifests, meta)               | 1.0 day |
| S3 multipart upload session                    | 1.5 days |
| Helm chart + config wiring                     | 0.5 day |
| Test suite parametrization + MinIO IT          | 1.0 day |
| Docs + IRSA cookbook                           | 0.5 day |
| **Total**                                      | **~5.5 days** |

Add 2 days of contingency for the streaming-hash optimization and one
production-like validation pass. Budget one calendar week end-to-end.

## Open questions

1. **Object lock / retention.** Should blob keys be written with S3 Object
   Lock in compliance mode when the bucket has it enabled? Would give us
   free immutability for content-addressable data. Cheap to add; needs a
   product decision.
2. **Signed URLs for pulls.** For very large layers, could redirect kubelet
   to a presigned S3 URL instead of streaming through deckwatch. Would
   need registry spec `307` handling; kaniko supports it, kubelet does too.
   Nice-to-have, not v1.
3. **Prefix isolation per Deckwatch instance.** If two deckwatch releases
   share a bucket, do we want to auto-scope the prefix to include release
   name? Currently operator's responsibility to pick non-overlapping
   prefixes.
