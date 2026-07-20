#![allow(dead_code, unused_imports)]
//! OCI Distribution Spec v1.1 registry endpoints.
//!
//! Implements the minimum surface a real client (docker, podman, kaniko)
//! needs to push and pull images:
//!
//! * `GET /v2/` version probe
//! * `HEAD|GET|PUT|DELETE /v2/{name}/manifests/{reference}` manifests
//! * `POST /v2/{name}/blobs/uploads/` blob upload initiate
//! * `PATCH|PUT /v2/{name}/blobs/uploads/{uuid}` chunk + finalize
//! * `HEAD|GET|DELETE /v2/{name}/blobs/{digest}` blob read + delete
//! * `GET /v2/_catalog` and `GET /v2/{name}/tags/list`
//!
//! Storage is abstracted behind [`RegistryStore`], an enum over
//! [`FsBackend`] (the historical filesystem layout) and
//! [`S3Backend`](super::s3_backend::S3Backend) (added in the S3 phase — see
//! `docs/S3_REGISTRY.md`). Every axum handler here dispatches through the
//! enum; the wire format is identical either way.
//!
//! Filesystem storage layout under [`FsBackend::root`]:
//!
//! ```text
//! blobs/sha256/<digest>          # content-addressable layer + config blobs
//! uploads/<uuid>                 # in-flight blob uploads (streamed)
//! manifests/<name>/<reference>   # tag or digest -> manifest bytes (raw)
//! manifests/<name>/_meta/<ref>   # media_type + digest sidecar JSON
//! ```
//!
//! `name` may contain `/` (nested repositories such as `myorg/api`) so we
//! validate + join it carefully — [`validate_name`] and [`repo_dir`] refuse
//! any path traversal.
//!
//! Auth: none. This registry is expected to sit on a `ClusterIP` behind the
//! cluster boundary; TLS + push auth is a deployment-layer concern.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use super::s3_backend::{ManifestMeta as S3ManifestMeta, S3Backend};

// ------------------------------------------------------------------ store enum

/// Storage dispatch for the OCI registry. The two variants share a wire
/// contract (all handlers below produce identical HTTP responses regardless
/// of backend) but keep their storage-specific state private.
///
/// The enum is `Clone` because axum's `State<T>` requires it; both variants
/// hold their heavy state behind an `Arc` internally so cloning is cheap.
#[derive(Clone)]
pub enum RegistryStore {
    Filesystem(FsBackend),
    S3(S3Backend),
}

impl RegistryStore {
    /// Historical constructor kept so existing call sites (tests, tools)
    /// that build a filesystem store don't need to change. New code should
    /// prefer [`RegistryStore::filesystem`] for symmetry with [`Self::s3`].
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::filesystem(root)
    }

    pub fn filesystem(root: impl Into<PathBuf>) -> Self {
        Self::Filesystem(FsBackend {
            root: Arc::new(root.into()),
        })
    }

    pub fn s3(backend: S3Backend) -> Self {
        Self::S3(backend)
    }

    /// One-shot setup call performed by `main.rs` after construction. On
    /// filesystem this creates the blobs/uploads/manifests dirs; on S3 it's
    /// a no-op (buckets exist out-of-band).
    pub async fn ensure_dirs(&self) -> io::Result<()> {
        match self {
            Self::Filesystem(fs) => fs.ensure_dirs().await,
            Self::S3(_) => Ok(()),
        }
    }
}

/// Filesystem-backed store. The store owns nothing but a root path; every
/// request opens its own file handles. Concurrency is left to the
/// filesystem — writes go to a per-upload temp file, then a single rename
/// into the content-addressed blob path, which is safe under POSIX rename
/// semantics.
#[derive(Clone)]
pub struct FsBackend {
    root: Arc<PathBuf>,
}

impl FsBackend {
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub async fn ensure_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(self.root.join("blobs/sha256")).await?;
        fs::create_dir_all(self.root.join("uploads")).await?;
        fs::create_dir_all(self.root.join("manifests")).await?;
        Ok(())
    }

    fn blob_path(&self, digest: &str) -> Option<PathBuf> {
        let hex = digest.strip_prefix("sha256:")?;
        if !is_hex(hex) || hex.len() != 64 {
            return None;
        }
        Some(self.root.join("blobs/sha256").join(hex))
    }

    fn upload_path(&self, uuid: &str) -> Option<PathBuf> {
        if !is_uuid(uuid) {
            return None;
        }
        Some(self.root.join("uploads").join(uuid))
    }

    fn manifest_path(&self, name: &str, reference: &str) -> Option<PathBuf> {
        let repo = repo_dir(self.root.as_path(), name)?;
        if !is_valid_reference(reference) {
            return None;
        }
        Some(repo.join(reference))
    }

    fn manifest_meta_path(&self, name: &str, reference: &str) -> Option<PathBuf> {
        let repo = repo_dir(self.root.as_path(), name)?;
        if !is_valid_reference(reference) {
            return None;
        }
        Some(repo.join("_meta").join(format!("{reference}.json")))
    }
}

/// Sidecar written next to every manifest so we can serve the correct
/// `Content-Type` and `Docker-Content-Digest` header on GET without having
/// to re-parse the manifest each time.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestMeta {
    media_type: String,
    digest: String,
    size: u64,
}

impl From<S3ManifestMeta> for ManifestMeta {
    fn from(m: S3ManifestMeta) -> Self {
        Self {
            media_type: m.media_type,
            digest: m.digest,
            size: m.size,
        }
    }
}

// ------------------------------------------------------------------ helpers

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_uuid(s: &str) -> bool {
    Uuid::parse_str(s).is_ok()
}

/// Distribution spec allows lowercase alnum + `._-/`, each path component
/// non-empty. Rejecting anything else keeps traversal + shell-injection
/// grade payloads out of the filesystem layer.
fn validate_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    for part in name.split('/') {
        if part.is_empty() {
            return false;
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        {
            return false;
        }
    }
    true
}

/// A tag or a `sha256:...` digest. Constrained the same way as [`validate_name`]
/// with the addition of `:` for digest refs; we reject anything with `/` so a
/// reference cannot escape the per-repo directory.
fn is_valid_reference(reference: &str) -> bool {
    if reference.is_empty() || reference.len() > 255 || reference.contains('/') {
        return false;
    }
    reference
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
}

fn repo_dir(root: &Path, name: &str) -> Option<PathBuf> {
    if !validate_name(name) {
        return None;
    }
    Some(root.join("manifests").join(name))
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

// ------------------------------------------------------------------ errors

/// Distribution-spec error envelope. Clients (kaniko, docker) key on the
/// `code` field, so the string values match the spec verbatim.
fn oci_error(status: StatusCode, code: &str, message: &str) -> Response {
    let body = serde_json::json!({
        "errors": [{
            "code": code,
            "message": message,
            "detail": null,
        }]
    });
    (status, axum::Json(body)).into_response()
}

fn name_invalid() -> Response {
    oci_error(
        StatusCode::BAD_REQUEST,
        "NAME_INVALID",
        "invalid repository name",
    )
}

fn blob_unknown() -> Response {
    oci_error(
        StatusCode::NOT_FOUND,
        "BLOB_UNKNOWN",
        "blob unknown to registry",
    )
}

fn manifest_unknown() -> Response {
    oci_error(
        StatusCode::NOT_FOUND,
        "MANIFEST_UNKNOWN",
        "manifest unknown",
    )
}

fn digest_invalid() -> Response {
    oci_error(StatusCode::BAD_REQUEST, "DIGEST_INVALID", "invalid digest")
}

fn upload_unknown() -> Response {
    oci_error(
        StatusCode::NOT_FOUND,
        "BLOB_UPLOAD_UNKNOWN",
        "blob upload unknown to registry",
    )
}

fn internal_error(context: &str, err: impl std::fmt::Display) -> Response {
    oci_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "UNKNOWN",
        &format!("{context}: {err}"),
    )
}

// ------------------------------------------------------------------ /v2/

/// `GET /v2/` — version probe. Clients treat any 200/401 as "registry
/// speaks v2"; we return an empty JSON object like every reference impl.
pub async fn v2_root() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Docker-Distribution-API-Version",
        HeaderValue::from_static("registry/2.0"),
    );
    (StatusCode::OK, headers, "{}").into_response()
}

// ------------------------------------------------------------------ manifests

pub async fn head_manifest(
    State(store): State<RegistryStore>,
    AxumPath((name, reference)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    let Some(meta) = read_manifest_meta(&store, &name, &reference).await else {
        return manifest_unknown();
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&meta.media_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(&meta.digest).unwrap_or(HeaderValue::from_static("")),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.size));
    (StatusCode::OK, headers).into_response()
}

pub async fn get_manifest(
    State(store): State<RegistryStore>,
    AxumPath((name, reference)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    let Some(meta) = read_manifest_meta(&store, &name, &reference).await else {
        return manifest_unknown();
    };
    let bytes = match get_manifest_bytes(&store, &name, &reference).await {
        Ok(Some(b)) => b,
        _ => return manifest_unknown(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&meta.media_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(&meta.digest).unwrap_or(HeaderValue::from_static("")),
    );
    (StatusCode::OK, headers, bytes).into_response()
}

pub async fn put_manifest(
    State(store): State<RegistryStore>,
    AxumPath((name, reference)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !validate_name(&name) || !is_valid_reference(&reference) {
        return name_invalid();
    }

    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();

    let digest = digest_bytes(&body);

    // If the reference is itself a digest, verify it matches the body BEFORE
    // any write so we never persist a mismatched pair.
    if let Some(want) = reference.strip_prefix("sha256:") {
        if !digest.ends_with(want) {
            return digest_invalid();
        }
    }

    if let Err(e) = write_manifest(&store, &name, &reference, &body, &media_type).await {
        return internal_error("manifest write failed", e);
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/manifests/{reference}"))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    resp_headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(&digest).unwrap_or(HeaderValue::from_static("")),
    );
    (StatusCode::CREATED, resp_headers).into_response()
}

pub async fn delete_manifest(
    State(store): State<RegistryStore>,
    AxumPath((name, reference)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) || !is_valid_reference(&reference) {
        return name_invalid();
    }
    match delete_manifest_inner(&store, &name, &reference).await {
        Ok(true) => (StatusCode::ACCEPTED, ()).into_response(),
        Ok(false) => manifest_unknown(),
        Err(e) => internal_error("manifest delete failed", e),
    }
}

// -- backend dispatch for manifests -----------------------------------------

async fn read_manifest_meta(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> Option<ManifestMeta> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let meta_path = fs.manifest_meta_path(name, reference)?;
            let bytes = tokio::fs::read(&meta_path).await.ok()?;
            serde_json::from_slice(&bytes).ok()
        }
        RegistryStore::S3(s3) => s3
            .get_manifest_meta(name, reference)
            .await
            .ok()
            .flatten()
            .map(ManifestMeta::from),
    }
}

async fn get_manifest_bytes(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> io::Result<Option<Bytes>> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.manifest_path(name, reference) else {
                return Ok(None);
            };
            match tokio::fs::read(&path).await {
                Ok(b) => Ok(Some(Bytes::from(b))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }
        RegistryStore::S3(s3) => s3.get_manifest(name, reference).await,
    }
}

async fn write_manifest(
    store: &RegistryStore,
    name: &str,
    reference: &str,
    body: &[u8],
    media_type: &str,
) -> io::Result<()> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.manifest_path(name, reference) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid name/reference",
                ));
            };
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, body).await?;

            let digest = digest_bytes(body);
            let size = body.len() as u64;
            let meta = ManifestMeta {
                media_type: media_type.to_string(),
                digest: digest.clone(),
                size,
            };
            if let Some(meta_path) = fs.manifest_meta_path(name, reference) {
                if let Some(parent) = meta_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(
                    &meta_path,
                    serde_json::to_vec_pretty(&meta)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                )
                .await?;
            }

            // Mirror by digest so a subsequent pull-by-digest resolves.
            if !reference.starts_with("sha256:") {
                if let Some(dpath) = fs.manifest_path(name, &digest) {
                    tokio::fs::write(&dpath, body).await?;
                }
                if let Some(dmeta) = fs.manifest_meta_path(name, &digest) {
                    if let Some(parent) = dmeta.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(
                        &dmeta,
                        serde_json::to_vec_pretty(&meta)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                    )
                    .await?;
                }
            }
            Ok(())
        }
        RegistryStore::S3(s3) => {
            // put_manifest on the S3 backend handles the digest mirror and
            // sidecar write internally.
            s3.put_manifest(name, reference, body, media_type).await?;
            Ok(())
        }
    }
}

async fn delete_manifest_inner(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> io::Result<bool> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.manifest_path(name, reference) else {
                return Ok(false);
            };
            let existed = tokio::fs::remove_file(&path).await.is_ok();
            if let Some(meta) = fs.manifest_meta_path(name, reference) {
                let _ = tokio::fs::remove_file(&meta).await;
            }
            Ok(existed)
        }
        RegistryStore::S3(s3) => s3.delete_manifest(name, reference).await,
    }
}

// ------------------------------------------------------------------ blobs

pub async fn head_blob(
    State(store): State<RegistryStore>,
    AxumPath((name, digest)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    let size = match blob_size_inner(&store, &digest).await {
        Ok(Some(s)) => s,
        Ok(None) => return blob_unknown(),
        Err(_) => return blob_unknown(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(&digest).unwrap_or(HeaderValue::from_static("")),
    );
    (StatusCode::OK, headers).into_response()
}

pub async fn get_blob(
    State(store): State<RegistryStore>,
    AxumPath((name, digest)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.blob_path(&digest) else {
                return digest_invalid();
            };
            let file = match tokio::fs::File::open(&path).await {
                Ok(f) => f,
                Err(_) => return blob_unknown(),
            };
            let meta = match file.metadata().await {
                Ok(m) => m,
                Err(_) => return blob_unknown(),
            };
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = Body::from_stream(stream);

            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
            headers.insert(
                "Docker-Content-Digest",
                HeaderValue::from_str(&digest).unwrap_or(HeaderValue::from_static("")),
            );
            (StatusCode::OK, headers, body).into_response()
        }
        RegistryStore::S3(s3) => {
            // Buffers the whole blob in memory. Fine for config blobs and
            // typical kaniko layers (a few MiB compressed); streaming
            // straight from the object_store GET is a follow-up per the
            // design doc's "signed URLs for pulls" open question.
            let bytes = match s3.get_blob(&digest).await {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return blob_unknown(),
                Err(e) if e.kind() == io::ErrorKind::InvalidInput => return digest_invalid(),
                Err(_) => return blob_unknown(),
            };
            let len = bytes.len() as u64;
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            headers.insert(
                "Docker-Content-Digest",
                HeaderValue::from_str(&digest).unwrap_or(HeaderValue::from_static("")),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
    }
}

pub async fn delete_blob(
    State(store): State<RegistryStore>,
    AxumPath((name, digest)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.blob_path(&digest) else {
                return digest_invalid();
            };
            if tokio::fs::remove_file(&path).await.is_err() {
                return blob_unknown();
            }
            (StatusCode::ACCEPTED, ()).into_response()
        }
        RegistryStore::S3(s3) => match s3.delete_blob(&digest).await {
            Ok(()) => (StatusCode::ACCEPTED, ()).into_response(),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => digest_invalid(),
            Err(_) => blob_unknown(),
        },
    }
}

async fn blob_size_inner(store: &RegistryStore, digest: &str) -> io::Result<Option<u64>> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.blob_path(digest) else {
                return Ok(None);
            };
            match tokio::fs::metadata(&path).await {
                Ok(m) => Ok(Some(m.len())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }
        RegistryStore::S3(s3) => s3.has_blob(digest).await,
    }
}

// ------------------------------------------------------------------ uploads

#[derive(Debug, Deserialize)]
pub struct UploadInitQuery {
    /// Monolithic-upload shortcut: `POST ...?digest=sha256:...` with the
    /// full blob in the body. Distribution spec allows it, some clients use
    /// it for small config blobs.
    pub digest: Option<String>,
}

/// `POST /v2/{name}/blobs/uploads/` — start a new blob upload.
///
/// Returns `202 Accepted` with a `Location` pointing at the upload session
/// URL, per spec. Kaniko always uses the two-phase flow (PATCH chunks then
/// PUT with digest), but we accept the monolithic `?digest=` shortcut too.
pub async fn start_upload(
    State(store): State<RegistryStore>,
    AxumPath(name): AxumPath<String>,
    Query(q): Query<UploadInitQuery>,
    body: axum::body::Bytes,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }

    // Monolithic upload — body IS the blob, ?digest=... is required.
    if let Some(digest) = q.digest {
        return finalize_monolithic(&store, &name, &digest, &body).await;
    }

    let uuid = Uuid::new_v4().to_string();
    match &store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.upload_path(&uuid) else {
                return internal_error("upload path", "invalid uuid");
            };
            if let Err(e) = tokio::fs::File::create(&path).await {
                return internal_error("upload create failed", e);
            }
        }
        RegistryStore::S3(s3) => {
            if let Err(e) = s3.begin_upload(&uuid).await {
                return internal_error("upload create failed", e);
            }
        }
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/blobs/uploads/{uuid}"))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    headers.insert("Docker-Upload-UUID", HeaderValue::from_str(&uuid).unwrap());
    headers.insert(header::RANGE, HeaderValue::from_static("0-0"));
    (StatusCode::ACCEPTED, headers).into_response()
}

async fn finalize_monolithic(
    store: &RegistryStore,
    name: &str,
    digest: &str,
    body: &[u8],
) -> Response {
    let actual = digest_bytes(body);
    if actual != digest {
        return digest_invalid();
    }
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(blob_path) = fs.blob_path(digest) else {
                return digest_invalid();
            };
            if let Err(e) = tokio::fs::write(&blob_path, body).await {
                return internal_error("blob write failed", e);
            }
        }
        RegistryStore::S3(s3) => {
            if let Err(e) = s3.monolithic_upload(digest, body).await {
                if e.kind() == io::ErrorKind::InvalidData {
                    return digest_invalid();
                }
                return internal_error("blob write failed", e);
            }
        }
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/blobs/{digest}"))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(digest).unwrap_or(HeaderValue::from_static("")),
    );
    (StatusCode::CREATED, headers).into_response()
}

pub async fn patch_upload(
    State(store): State<RegistryStore>,
    AxumPath((name, uuid)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.upload_path(&uuid) else {
                return upload_unknown();
            };
            let mut file = match tokio::fs::OpenOptions::new().append(true).open(&path).await {
                Ok(f) => f,
                Err(_) => return upload_unknown(),
            };

            let mut written: u64 = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => return internal_error("upload stream error", e),
                };
                if let Err(e) = file.write_all(&chunk).await {
                    return internal_error("upload write error", e);
                }
                written += chunk.len() as u64;
            }
            patch_response(&name, &uuid, written)
        }
        RegistryStore::S3(s3) => {
            // Confirm the session exists first so a bad UUID returns 404
            // rather than materializing an unexpected key.
            match s3.upload_length(&uuid).await {
                Ok(Some(_)) => {}
                Ok(None) => return upload_unknown(),
                Err(e) => return internal_error("upload lookup failed", e),
            }
            // Buffer the entire PATCH body — S3 has no append primitive
            // and the current backend does one read+write per PATCH. The
            // design doc's follow-up swaps this for object_store's
            // multipart BufWriter.
            let mut buf: Vec<u8> = Vec::new();
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => buf.extend_from_slice(&c),
                    Err(e) => return internal_error("upload stream error", e),
                }
            }
            let new_len = match s3.append_upload_chunk(&uuid, &buf).await {
                Ok(n) => n,
                Err(e) => return internal_error("upload write error", e),
            };
            patch_response(&name, &uuid, new_len)
        }
    }
}

fn patch_response(name: &str, uuid: &str, written: u64) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/blobs/uploads/{uuid}"))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    headers.insert("Docker-Upload-UUID", HeaderValue::from_str(uuid).unwrap());
    let end = written.saturating_sub(1);
    headers.insert(
        header::RANGE,
        HeaderValue::from_str(&format!("0-{end}")).unwrap_or(HeaderValue::from_static("0-0")),
    );
    (StatusCode::ACCEPTED, headers).into_response()
}

#[derive(Debug, Deserialize)]
pub struct UploadCompleteQuery {
    pub digest: String,
}

/// `PUT /v2/{name}/blobs/uploads/{uuid}?digest=sha256:...` — finalize.
///
/// Body may contain the final chunk (kaniko often uses PATCH+PUT). We append
/// any trailing bytes, verify sha256, then rename into the CAS blob path.
pub async fn put_upload(
    State(store): State<RegistryStore>,
    AxumPath((name, uuid)): AxumPath<(String, String)>,
    Query(q): Query<UploadCompleteQuery>,
    body: axum::body::Bytes,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }

    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(upload_path) = fs.upload_path(&uuid) else {
                return upload_unknown();
            };
            if tokio::fs::metadata(&upload_path).await.is_err() {
                return upload_unknown();
            }

            if !body.is_empty() {
                let mut file = match tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&upload_path)
                    .await
                {
                    Ok(f) => f,
                    Err(_) => return upload_unknown(),
                };
                if let Err(e) = file.write_all(&body).await {
                    return internal_error("upload write error", e);
                }
            }

            // Re-read the full upload to verify digest. Blobs are bounded by
            // the per-layer size (kaniko compresses aggressively) so a full
            // read is acceptable here without streaming hashing.
            let mut file = match tokio::fs::File::open(&upload_path).await {
                Ok(f) => f,
                Err(_) => return upload_unknown(),
            };
            let _ = file.seek(std::io::SeekFrom::Start(0)).await;
            let mut buf = Vec::new();
            if let Err(e) = tokio::io::AsyncReadExt::read_to_end(&mut file, &mut buf).await {
                return internal_error("upload read error", e);
            }
            let actual = digest_bytes(&buf);
            if actual != q.digest {
                let _ = tokio::fs::remove_file(&upload_path).await;
                return digest_invalid();
            }

            let Some(blob_path) = fs.blob_path(&q.digest) else {
                return digest_invalid();
            };
            if let Err(e) = tokio::fs::rename(&upload_path, &blob_path).await {
                // Rename can fail across filesystems even inside a single PVC
                // when subpaths use different mounts; fall back to copy+unlink.
                if let Err(copy_err) = tokio::fs::copy(&upload_path, &blob_path).await {
                    return internal_error(
                        "finalize failed",
                        format!("rename={e}, copy={copy_err}"),
                    );
                }
                let _ = tokio::fs::remove_file(&upload_path).await;
            }
        }
        RegistryStore::S3(s3) => {
            // Confirm session exists before appending / finalizing.
            match s3.upload_length(&uuid).await {
                Ok(Some(_)) => {}
                Ok(None) => return upload_unknown(),
                Err(e) => return internal_error("upload lookup failed", e),
            }
            if !body.is_empty() {
                if let Err(e) = s3.append_upload_chunk(&uuid, &body).await {
                    return internal_error("upload write error", e);
                }
            }
            if let Err(e) = s3.finalize_upload(&uuid, &q.digest).await {
                if matches!(
                    e.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
                ) {
                    return digest_invalid();
                }
                return internal_error("finalize failed", e);
            }
        }
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/blobs/{}", q.digest))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    headers.insert(
        "Docker-Content-Digest",
        HeaderValue::from_str(&q.digest).unwrap_or(HeaderValue::from_static("")),
    );
    (StatusCode::CREATED, headers).into_response()
}

pub async fn get_upload_status(
    State(store): State<RegistryStore>,
    AxumPath((name, uuid)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    let len = match &store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.upload_path(&uuid) else {
                return upload_unknown();
            };
            match tokio::fs::metadata(&path).await {
                Ok(m) => m.len(),
                Err(_) => return upload_unknown(),
            }
        }
        RegistryStore::S3(s3) => match s3.upload_length(&uuid).await {
            Ok(Some(n)) => n,
            Ok(None) => return upload_unknown(),
            Err(e) => return internal_error("upload lookup failed", e),
        },
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v2/{name}/blobs/uploads/{uuid}"))
            .unwrap_or(HeaderValue::from_static("/")),
    );
    headers.insert("Docker-Upload-UUID", HeaderValue::from_str(&uuid).unwrap());
    let end = len.saturating_sub(1);
    headers.insert(
        header::RANGE,
        HeaderValue::from_str(&format!("0-{end}")).unwrap_or(HeaderValue::from_static("0-0")),
    );
    (StatusCode::NO_CONTENT, headers).into_response()
}

pub async fn cancel_upload(
    State(store): State<RegistryStore>,
    AxumPath((name, uuid)): AxumPath<(String, String)>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    match &store {
        RegistryStore::Filesystem(fs) => {
            let Some(path) = fs.upload_path(&uuid) else {
                return upload_unknown();
            };
            let _ = tokio::fs::remove_file(&path).await;
        }
        RegistryStore::S3(s3) => {
            let _ = s3.cancel_upload(&uuid).await;
        }
    }
    (StatusCode::NO_CONTENT, ()).into_response()
}

// ------------------------------------------------------------------ catalog

#[derive(Debug, Serialize)]
struct CatalogResponse {
    repositories: Vec<String>,
}

pub async fn list_catalog(State(store): State<RegistryStore>) -> Response {
    let repos = list_repositories(&store).await;
    (
        StatusCode::OK,
        axum::Json(CatalogResponse {
            repositories: repos,
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct TagListResponse {
    name: String,
    tags: Vec<String>,
}

pub async fn list_tags(
    State(store): State<RegistryStore>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if !validate_name(&name) {
        return name_invalid();
    }
    let tags = list_tags_for(&store, &name).await;
    (StatusCode::OK, axum::Json(TagListResponse { name, tags })).into_response()
}

/// Walks the manifest tree collecting every directory (S3: prefix) that
/// contains at least one tag file. Used by both `_catalog` and the
/// browsable UI API.
pub(crate) async fn list_repositories(store: &RegistryStore) -> Vec<String> {
    match store {
        RegistryStore::Filesystem(fs) => list_repositories_fs(fs.root()).await,
        RegistryStore::S3(s3) => s3.list_repositories().await.unwrap_or_default(),
    }
}

async fn list_repositories_fs(root: &Path) -> Vec<String> {
    let root = root.join("manifests");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut has_manifest = false;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let ft = match entry.file_type().await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str == "_meta" {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                has_manifest = true;
            }
        }
        if has_manifest {
            if let Ok(rel) = dir.strip_prefix(&root) {
                let s = rel.to_string_lossy().to_string();
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    out.sort();
    out
}

/// List tags for a single repository, filtering out digest mirror entries
/// that [`put_manifest`] writes so `pull by digest` works.
pub(crate) async fn list_tags_for(store: &RegistryStore, name: &str) -> Vec<String> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let Some(dir) = repo_dir(fs.root(), name) else {
                return Vec::new();
            };
            list_tags_in_fs(&dir).await
        }
        RegistryStore::S3(s3) => s3.list_tags(name).await.unwrap_or_default(),
    }
}

async fn list_tags_in_fs(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return out,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let ft = match entry.file_type().await {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip anything that looks like a digest — those are the mirror
        // copies we write in put_manifest so a pull-by-digest works.
        if name.starts_with("sha256:") {
            continue;
        }
        out.push(name);
    }
    out.sort();
    out
}

/// Read the sidecar (media type, digest, size) for a given manifest ref,
/// returning None if either the manifest or its meta is missing. Used by
/// the UI handler so it doesn't have to reparse the manifest itself.
pub(crate) async fn read_meta(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> Option<(String, String, u64)> {
    let meta = read_manifest_meta(store, name, reference).await?;
    Some((meta.media_type, meta.digest, meta.size))
}

/// Read + parse a manifest JSON, returning the raw bytes so the UI handler
/// can walk `.layers[]` and `.config` without owning the schema.
pub(crate) async fn read_manifest_json(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> Option<serde_json::Value> {
    let bytes = get_manifest_bytes(store, name, reference).await.ok()??;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) async fn blob_size(store: &RegistryStore, digest: &str) -> Option<u64> {
    blob_size_inner(store, digest).await.ok().flatten()
}

/// Deletes a manifest file + its sidecar. Called from the UI DELETE handler.
pub(crate) async fn delete_manifest_by_ref(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> bool {
    delete_manifest_inner(store, name, reference)
        .await
        .unwrap_or(false)
}

/// Best-effort "when was this tag pushed" mtime, used by the UI to show a
/// timestamp in the tag list. Filesystem reads inode mtime; on S3 we don't
/// yet plumb `LastModified` through, so it returns None and the UI shows
/// "unknown". Populating S3 mtimes is a small follow-up.
pub(crate) async fn manifest_modified_at(
    store: &RegistryStore,
    name: &str,
    reference: &str,
) -> Option<String> {
    match store {
        RegistryStore::Filesystem(fs) => {
            let path = fs.manifest_path(name, reference)?;
            let meta = tokio::fs::metadata(&path).await.ok()?;
            let ts = meta.modified().ok()?;
            let jiff_ts = jiff::Timestamp::try_from(ts).ok()?;
            Some(jiff_ts.to_string())
        }
        RegistryStore::S3(_) => None,
    }
}
