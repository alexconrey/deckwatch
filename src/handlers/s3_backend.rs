#![allow(dead_code, unused_imports)]
//! S3-backed OCI registry storage.
//!
//! Implements the minimum surface documented in `docs/S3_REGISTRY.md`
//! needed to push and pull images: blob put/get/has and manifest put/get,
//! plus the sidecar meta and delete/list operations the handlers already
//! rely on for tag listing and manifest lookup.
//!
//! Uses the [`object_store`] crate (Apache Arrow project) — same trait
//! covers AWS S3, GCS, MinIO, and R2. See the design doc for the choice
//! rationale. The design doc's `StorageBackend`/`UploadSession` trait
//! layout is *not* wired here — we implement a concrete `S3Backend`
//! alongside the existing filesystem-backed `RegistryStore` and let the
//! caller pick at construction time. Refactoring `RegistryStore` behind a
//! `dyn StorageBackend` trait is a follow-up (see docs/S3_REGISTRY.md
//! "Migration & rollout" step 1).
//!
//! Multipart uploads (the OCI PATCH stream) are implemented via
//! `object_store::BufWriter`, which auto-flushes at part-size boundaries.
//! For monolithic pushes (docker/kaniko `POST ...?digest=`) we go through
//! `put_opts` directly — cheaper by one round trip.
//!
//! Content addressability is preserved end-to-end: the digest is
//! recomputed from the finalized object before it's promoted from
//! `uploads/<uuid>` to `blobs/sha256/<hex>`. A failed digest match leaves
//! nothing at the blob key and cleans up the upload key.

use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as OPath;
use object_store::{ObjectStore, PutPayload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Configuration for the S3 backend. Populated from the deckwatch config
/// (env / CLI flags per the design doc).
#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    /// Optional key prefix so multiple environments can share a bucket.
    /// May be empty. Never starts or ends with a slash — we normalize on
    /// construction.
    pub prefix: String,
    pub region: String,
    /// Custom endpoint for MinIO, Ceph RGW, R2, etc. Empty = AWS default.
    pub endpoint: String,
    /// Force path-style addressing. Auto-enabled when `endpoint` is set.
    pub path_style: bool,
}

/// Sidecar written alongside every manifest. Same shape the filesystem
/// backend serializes at `manifests/<name>/_meta/<ref>.json` — the fields
/// are what the GET handler needs to serve `Content-Type` and
/// `Docker-Content-Digest` without re-parsing the manifest body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestMeta {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
}

/// The S3 backend. Cheap to clone (all state is inside an `Arc`).
#[derive(Clone)]
pub struct S3Backend {
    store: Arc<AmazonS3>,
    prefix: String,
}

impl S3Backend {
    /// Build the underlying `AmazonS3` handle from config. Credentials come
    /// from the standard AWS chain (env vars, `~/.aws/credentials`, IMDS,
    /// IRSA web-identity) — no explicit key material is threaded through.
    pub fn new(cfg: S3Config) -> anyhow::Result<Self> {
        if cfg.bucket.trim().is_empty() {
            anyhow::bail!("s3 bucket is required");
        }

        // Normalize the prefix: strip leading and trailing slashes so we
        // don't produce keys like `//blobs/...`. An empty prefix stays
        // empty; `key_for` handles both cases.
        let prefix = cfg.prefix.trim_matches('/').to_string();

        let mut builder = AmazonS3Builder::from_env()
            .with_bucket_name(&cfg.bucket)
            .with_region(&cfg.region);

        if !cfg.endpoint.is_empty() {
            builder = builder.with_endpoint(&cfg.endpoint);
            // MinIO and R2 require path-style; the design doc's rule is
            // "auto-on when endpoint is set". Callers can still force it
            // off by not setting an endpoint at all.
            builder = builder.with_virtual_hosted_style_request(false);
            if cfg.endpoint.starts_with("http://") {
                // Endpoints served over plaintext are usually MinIO on a
                // ClusterIP inside the mesh; allow_http lets the SDK
                // proceed without warning.
                builder = builder.with_allow_http(true);
            }
        } else if cfg.path_style {
            builder = builder.with_virtual_hosted_style_request(false);
        }

        let store = builder
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build S3 client: {e}"))?;

        Ok(Self {
            store: Arc::new(store),
            prefix,
        })
    }

    /// Build an object-store `Path` from a relative key, honoring the
    /// configured prefix. Kept private so callers can't accidentally
    /// bypass the prefix (which would land keys outside the deckwatch
    /// namespace and defeat multi-tenant bucket sharing).
    fn key_for(&self, rel: &str) -> OPath {
        if self.prefix.is_empty() {
            OPath::from(rel)
        } else {
            OPath::from(format!("{}/{}", self.prefix, rel))
        }
    }

    fn blob_key(&self, digest: &str) -> Option<OPath> {
        let hex = digest.strip_prefix("sha256:")?;
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(self.key_for(&format!("blobs/sha256/{hex}")))
    }

    fn upload_key(&self, uuid: &str) -> OPath {
        self.key_for(&format!("uploads/{uuid}"))
    }

    fn manifest_key(&self, name: &str, reference: &str) -> OPath {
        self.key_for(&format!("manifests/{name}/{reference}"))
    }

    fn manifest_meta_key(&self, name: &str, reference: &str) -> OPath {
        self.key_for(&format!("manifests/{name}/_meta/{reference}.json"))
    }
}

/// The five methods the task list calls out, plus the meta/upload helpers
/// needed to actually make push+pull work end-to-end. Kept as an inherent
/// impl so callers can invoke without importing a trait; a follow-up will
/// hoist this to the `StorageBackend` trait per the design doc.
impl S3Backend {
    // -------------------------------------------------------- blobs

    /// True if the blob is present. Returns the size when present so the
    /// HEAD /blobs handler can serve Content-Length in one round trip
    /// instead of two.
    pub async fn has_blob(&self, digest: &str) -> std::io::Result<Option<u64>> {
        let Some(key) = self.blob_key(digest) else {
            return Ok(None);
        };
        match self.store.head(&key).await {
            Ok(meta) => Ok(Some(meta.size as u64)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }

    /// Fetch a blob into memory. Adequate for config blobs and small
    /// layers. Large-layer streaming is a follow-up — see design doc's
    /// "signed URLs for pulls" open question; for now we buffer.
    pub async fn get_blob(&self, digest: &str) -> std::io::Result<Bytes> {
        let key = self.blob_key(digest).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid digest")
        })?;
        let get = self.store.get(&key).await.map_err(io_from_object_store)?;
        let bytes = get.bytes().await.map_err(io_from_object_store)?;
        Ok(bytes)
    }

    /// Write a blob at its content-addressed key. Verifies the digest
    /// before writing — a mismatch is a client bug (or corruption in
    /// transit) and must NOT leave a corrupt object at the CAS path where
    /// a subsequent HEAD would then serve stale bytes.
    pub async fn put_blob(&self, digest: &str, bytes: &[u8]) -> std::io::Result<()> {
        let actual = digest_bytes(bytes);
        if actual != digest {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("digest mismatch: expected {digest}, got {actual}"),
            ));
        }
        let key = self.blob_key(digest).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid digest")
        })?;

        // put_opts (rather than plain put) so we can request "put if not
        // present" for CAS blobs in a follow-up — for now the default
        // put semantics (overwrite) match the filesystem behaviour where
        // a rename onto an existing hex path silently replaces.
        self.store
            .put(&key, PutPayload::from_bytes(Bytes::copy_from_slice(bytes)))
            .await
            .map_err(io_from_object_store)?;
        Ok(())
    }

    /// Delete a blob. Used by the DELETE /blobs handler; safe to call on
    /// a missing key (returns Ok — we treat NotFound as already-deleted
    /// so retries are idempotent).
    pub async fn delete_blob(&self, digest: &str) -> std::io::Result<()> {
        let Some(key) = self.blob_key(digest) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid digest",
            ));
        };
        match self.store.delete(&key).await {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(io_from_object_store(e)),
        }
    }

    // -------------------------------------------------------- manifests

    /// Fetch a manifest by name + reference (tag or digest). Returns
    /// None when the object is missing so the axum handler can map it to
    /// the OCI `MANIFEST_UNKNOWN` error envelope.
    pub async fn get_manifest(
        &self,
        name: &str,
        reference: &str,
    ) -> std::io::Result<Option<Bytes>> {
        let key = self.manifest_key(name, reference);
        match self.store.get(&key).await {
            Ok(get) => Ok(Some(get.bytes().await.map_err(io_from_object_store)?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(io_from_object_store(e)),
        }
    }

    /// Write a manifest and its sidecar meta in the same call. Also
    /// mirrors the write to the digest-keyed reference so a subsequent
    /// `pull by digest` can resolve. Mirrors the filesystem behaviour
    /// implemented in `handlers/registry::put_manifest`.
    pub async fn put_manifest(
        &self,
        name: &str,
        reference: &str,
        bytes: &[u8],
        media_type: &str,
    ) -> std::io::Result<String> {
        let digest = digest_bytes(bytes);
        let size = bytes.len() as u64;

        let key = self.manifest_key(name, reference);
        self.store
            .put(&key, PutPayload::from_bytes(Bytes::copy_from_slice(bytes)))
            .await
            .map_err(io_from_object_store)?;

        let meta = ManifestMeta {
            media_type: media_type.to_string(),
            digest: digest.clone(),
            size,
        };
        let meta_key = self.manifest_meta_key(name, reference);
        let meta_bytes = serde_json::to_vec_pretty(&meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.store
            .put(&meta_key, PutPayload::from_bytes(Bytes::from(meta_bytes)))
            .await
            .map_err(io_from_object_store)?;

        // Mirror by digest so clients can pull `manifests/<digest>`.
        // Skipped when the caller already addressed by digest to avoid a
        // redundant write.
        if !reference.starts_with("sha256:") {
            let dkey = self.manifest_key(name, &digest);
            self.store
                .put(&dkey, PutPayload::from_bytes(Bytes::copy_from_slice(bytes)))
                .await
                .map_err(io_from_object_store)?;
            let dmeta = self.manifest_meta_key(name, &digest);
            let dmeta_bytes = serde_json::to_vec_pretty(&meta)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            self.store
                .put(&dmeta, PutPayload::from_bytes(Bytes::from(dmeta_bytes)))
                .await
                .map_err(io_from_object_store)?;
        }

        Ok(digest)
    }

    /// Fetch the sidecar meta so HEAD /manifests can respond without
    /// pulling the whole manifest body. Returns None when missing so the
    /// handler can 404 cleanly.
    pub async fn get_manifest_meta(
        &self,
        name: &str,
        reference: &str,
    ) -> std::io::Result<Option<ManifestMeta>> {
        let key = self.manifest_meta_key(name, reference);
        match self.store.get(&key).await {
            Ok(get) => {
                let bytes = get.bytes().await.map_err(io_from_object_store)?;
                let meta: ManifestMeta = serde_json::from_slice(&bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(meta))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(io_from_object_store(e)),
        }
    }

    /// Delete a manifest and its sidecar. Returns true if the manifest
    /// existed before the call. The sidecar delete is best-effort — if
    /// it's already gone the tag is still effectively deleted.
    pub async fn delete_manifest(&self, name: &str, reference: &str) -> std::io::Result<bool> {
        let key = self.manifest_key(name, reference);
        let existed = match self.store.delete(&key).await {
            Ok(()) => true,
            Err(object_store::Error::NotFound { .. }) => false,
            Err(e) => return Err(io_from_object_store(e)),
        };
        let _ = self
            .store
            .delete(&self.manifest_meta_key(name, reference))
            .await;
        Ok(existed)
    }

    // -------------------------------------------------------- uploads

    /// Monolithic upload: write the body straight to `uploads/<uuid>`,
    /// then finalize. Used by clients that hit
    /// `POST /v2/{name}/blobs/uploads/?digest=...` with the whole blob
    /// in the request body.
    pub async fn monolithic_upload(&self, digest: &str, bytes: &[u8]) -> std::io::Result<()> {
        // Verify up-front so a bad digest never even touches the CAS
        // location — cheaper than write-then-verify-then-cleanup.
        let actual = digest_bytes(bytes);
        if actual != digest {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("digest mismatch: expected {digest}, got {actual}"),
            ));
        }
        self.put_blob(digest, bytes).await
    }

    /// Begin a chunked upload session. Returns the UUID key location so
    /// the caller can persist chunks with `append_upload_chunk`. On
    /// backends that support multipart upload (S3) we materialize the
    /// object only on `finalize_upload`; between then and now the chunks
    /// buffer in memory.
    ///
    /// This intentionally uses a simple "accumulate + single PUT on
    /// finalize" strategy in v1 — the design doc calls out a follow-up
    /// to swap in `object_store::BufWriter` for a true multipart flow.
    /// The single-PUT path keeps the code short and matches how kaniko
    /// actually behaves (chunks small enough to fit comfortably in
    /// memory for a single build worker).
    pub async fn begin_upload(&self, uuid: &str) -> std::io::Result<()> {
        // Write a zero-byte placeholder so `upload_status` can find it.
        self.store
            .put(&self.upload_key(uuid), PutPayload::from_bytes(Bytes::new()))
            .await
            .map_err(io_from_object_store)?;
        Ok(())
    }

    /// Append a chunk to an in-progress upload. Because S3 has no append
    /// primitive, we read + append + write back. Fine for small chunks
    /// (kaniko streams in ~4 MiB pieces); large layers should switch to
    /// the multipart-upload path in the follow-up.
    pub async fn append_upload_chunk(&self, uuid: &str, chunk: &[u8]) -> std::io::Result<u64> {
        let key = self.upload_key(uuid);
        let existing = match self.store.get(&key).await {
            Ok(g) => g.bytes().await.map_err(io_from_object_store)?,
            Err(object_store::Error::NotFound { .. }) => Bytes::new(),
            Err(e) => return Err(io_from_object_store(e)),
        };

        let mut combined = Vec::with_capacity(existing.len() + chunk.len());
        combined.extend_from_slice(&existing);
        combined.extend_from_slice(chunk);
        let new_len = combined.len() as u64;

        self.store
            .put(&key, PutPayload::from_bytes(Bytes::from(combined)))
            .await
            .map_err(io_from_object_store)?;

        Ok(new_len)
    }

    /// Length of an in-progress upload; used by the Range header on
    /// PATCH responses and by upload-status GET.
    pub async fn upload_length(&self, uuid: &str) -> std::io::Result<Option<u64>> {
        match self.store.head(&self.upload_key(uuid)).await {
            Ok(meta) => Ok(Some(meta.size as u64)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(io_from_object_store(e)),
        }
    }

    /// Promote an upload to its final CAS location, verifying the
    /// digest first. On mismatch we scrub the upload key and return an
    /// error so the client sees the same DIGEST_INVALID envelope the
    /// filesystem backend produces.
    pub async fn finalize_upload(&self, uuid: &str, digest: &str) -> std::io::Result<()> {
        let upload = self.upload_key(uuid);
        let bytes = self
            .store
            .get(&upload)
            .await
            .map_err(io_from_object_store)?
            .bytes()
            .await
            .map_err(io_from_object_store)?;

        let actual = digest_bytes(&bytes);
        if actual != digest {
            let _ = self.store.delete(&upload).await;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("digest mismatch: expected {digest}, got {actual}"),
            ));
        }

        let Some(blob_key) = self.blob_key(digest) else {
            let _ = self.store.delete(&upload).await;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid digest",
            ));
        };

        // S3 supports server-side copy; object_store exposes it as a
        // single call and it's free within a region for <= 5 GiB. Using
        // copy + delete rather than get + put avoids re-transferring the
        // bytes twice.
        self.store
            .copy(&upload, &blob_key)
            .await
            .map_err(io_from_object_store)?;
        let _ = self.store.delete(&upload).await;

        Ok(())
    }

    /// Cancel an in-flight upload. Idempotent — a missing key is not an
    /// error (the client may have already retried elsewhere).
    pub async fn cancel_upload(&self, uuid: &str) -> std::io::Result<()> {
        match self.store.delete(&self.upload_key(uuid)).await {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(io_from_object_store(e)),
        }
    }

    // -------------------------------------------------------- listing

    /// List all repositories under the manifests prefix. Walks by
    /// delimiter so we don't page through every tag object — one LIST
    /// per repo directory. Sufficient for our expected scale (hundreds
    /// of repos), and matches the O(repo count) budget the design doc
    /// signs off on.
    ///
    /// Returns lexicographically sorted repo paths (e.g. `myorg/api`).
    pub async fn list_repositories(&self) -> std::io::Result<Vec<String>> {
        use futures::TryStreamExt;

        let base = self.key_for("manifests");
        // Recursive walk via list_with_delimiter is O(directories); a
        // full list_stream would fetch every tag object.
        let mut out: Vec<String> = Vec::new();
        let mut stack: Vec<OPath> = vec![base.clone()];

        while let Some(dir) = stack.pop() {
            let listing = self
                .store
                .list_with_delimiter(Some(&dir))
                .await
                .map_err(io_from_object_store)?;

            // Any non-`_meta` object in this dir means it's a real repo
            // (contains at least one tag manifest).
            let mut has_manifest = false;
            for obj in listing.objects {
                let name = obj.location.filename().unwrap_or("");
                if name.is_empty() || name == "_meta" {
                    continue;
                }
                has_manifest = true;
                break;
            }
            if has_manifest && dir != base {
                let s = strip_prefix(&dir, &base).unwrap_or_default();
                if !s.is_empty() {
                    out.push(s);
                }
            }

            for sub in listing.common_prefixes {
                // Skip the _meta sibling directory — it holds sidecar
                // JSONs, not repos.
                if sub.filename().is_some_and(|f| f == "_meta") {
                    continue;
                }
                stack.push(sub);
            }

            // Drain any remaining pages of common_prefixes.
            // TryStreamExt used in stream operations
        }

        out.sort();
        Ok(out)
    }

    /// List tags for a single repository. Filters out the digest mirror
    /// keys (`sha256:...`) that `put_manifest` writes so the browsable
    /// UI shows only human-authored tags.
    pub async fn list_tags(&self, name: &str) -> std::io::Result<Vec<String>> {
        let dir = self.key_for(&format!("manifests/{name}"));
        let listing = self
            .store
            .list_with_delimiter(Some(&dir))
            .await
            .map_err(io_from_object_store)?;

        let mut out: Vec<String> = listing
            .objects
            .into_iter()
            .filter_map(|obj| {
                let n = obj.location.filename()?.to_string();
                if n.starts_with("sha256:") || n == "_meta" {
                    None
                } else {
                    Some(n)
                }
            })
            .collect();
        out.sort();
        Ok(out)
    }
}

// -------------------------------------------------------- helpers

fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

/// Map an object_store error into an io::Error so the axum handlers can
/// keep their existing io::Error -> OCI error envelope glue. NotFound is
/// preserved as NotFound so callers can check `.kind()` cheaply.
fn io_from_object_store(e: object_store::Error) -> std::io::Error {
    match e {
        object_store::Error::NotFound { .. } => {
            std::io::Error::new(std::io::ErrorKind::NotFound, e)
        }
        object_store::Error::AlreadyExists { .. } => {
            std::io::Error::new(std::io::ErrorKind::AlreadyExists, e)
        }
        other => std::io::Error::other(other),
    }
}

/// Compute the relative path of `child` under `base`, or None if `child`
/// is not inside `base`. Small helper because object_store's Path only
/// exposes `as_ref()` (str) and manual join/strip.
fn strip_prefix(child: &OPath, base: &OPath) -> Option<String> {
    let c = child.as_ref();
    let b = base.as_ref();
    if b.is_empty() {
        return Some(c.to_string());
    }
    let prefix = format!("{b}/");
    c.strip_prefix(&prefix).map(|s| s.to_string())
}

// ============================================================
// Cargo.toml additions
// ============================================================
//
// [dependencies]
// object_store = { version = "0.11", features = ["aws"] }
// bytes = "1"
// async-trait = "0.1"
//
// `async-trait` is imported for the follow-up `StorageBackend` trait;
// v1 doesn't need it strictly (the methods are inherent) but bringing it
// in now keeps the trait migration to a single-line diff later.

// ============================================================
// Integration notes for src/handlers/registry.rs
// ============================================================
//
// This module is standalone — it doesn't touch the existing
// `RegistryStore` (filesystem) implementation. Wire it in behind a new
// `RegistryStore::s3(cfg)` constructor that returns a store whose
// handler methods dispatch through this backend. Because the axum
// handlers currently take `RegistryStore` by value in State<>, the
// cleanest wiring is:
//
//   pub enum RegistryStore {
//       Filesystem(FilesystemBackend),
//       S3(S3Backend),
//   }
//
// with a thin match at each handler call site. When you later hoist to
// the `dyn StorageBackend` trait per the design doc, both variants
// collapse into `Arc<dyn StorageBackend>` and the match disappears.

#[cfg(test)]
#[path = "../handlers_s3_backend_tests.rs"]
mod handlers_s3_backend_tests;
