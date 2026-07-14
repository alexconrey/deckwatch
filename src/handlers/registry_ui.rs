//! Browsable API for the embedded OCI registry.
//!
//! Lives under `/api/registry/*` and is intended for the Vue frontend, not
//! for OCI clients. Everything here reads through [`RegistryStore`] and
//! never talks to Kubernetes.
//!
//! Endpoints:
//!
//! * `GET  /api/registry/enabled`                              — capability probe
//! * `GET  /api/registry/repositories`                         — list repos + tag counts
//! * `GET  /api/registry/repositories/{name}/tags`             — list tags + sizes
//! * `GET  /api/registry/repositories/{name}/tags/{tag}`       — manifest detail
//! * `DELETE /api/registry/repositories/{name}/tags/{tag}`     — delete a tag
//!
//! `{name}` may contain `/` (multi-segment repos like `myorg/api`), so the
//! routes use `{*name}` wildcards and this module splits `name` back out
//! from `tag` manually — the wildcard swallows everything up to the last
//! segment.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::handlers::registry::{
    self, blob_size, delete_manifest_by_ref, list_repositories, list_tags_for,
    manifest_modified_at, read_manifest_json, read_meta, RegistryStore,
};

/// Capability probe — the frontend uses this to decide whether to render
/// the Registry nav button. Returns `enabled: false` when the registry
/// feature was compiled in but no store was configured at startup.
#[derive(Serialize)]
pub struct EnabledResponse {
    pub enabled: bool,
}

pub async fn enabled(State(store): State<Option<RegistryStore>>) -> Response {
    Json(EnabledResponse {
        enabled: store.is_some(),
    })
    .into_response()
}

#[derive(Serialize)]
pub struct RepositorySummary {
    pub name: String,
    pub tag_count: usize,
    /// Sum of every unique blob referenced by any tag in this repo. Blob
    /// dedup within a repo is respected; cross-repo sharing is not counted.
    pub total_size: u64,
}

#[derive(Serialize)]
pub struct RepositoryListResponse {
    pub repositories: Vec<RepositorySummary>,
}

pub async fn list_repos(State(store): State<Option<RegistryStore>>) -> Response {
    let Some(store) = store else {
        return registry_disabled();
    };
    let names = list_repositories(&store).await;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let tags = list_tags_for(&store, &name).await;
        let mut seen = std::collections::BTreeSet::new();
        let mut total: u64 = 0;
        for tag in &tags {
            let Some(manifest) = read_manifest_json(&store, &name, tag).await else {
                continue;
            };
            accumulate_blob_sizes(&manifest, &mut seen);
        }
        for digest in &seen {
            if let Some(sz) = blob_size(&store, digest).await {
                total += sz;
            }
        }
        out.push(RepositorySummary {
            name,
            tag_count: tags.len(),
            total_size: total,
        });
    }
    Json(RepositoryListResponse { repositories: out }).into_response()
}

#[derive(Serialize)]
pub struct TagSummary {
    pub tag: String,
    pub digest: String,
    pub media_type: String,
    /// Sum of every blob referenced by this specific tag's manifest
    /// (compressed layer sizes + config). Not the on-disk image size when
    /// extracted, which will always be larger.
    pub size: u64,
    /// mtime of the manifest, ISO 8601. Filesystem backend uses inode
    /// mtime; S3 backend currently returns None (see `manifest_modified_at`).
    pub created: Option<String>,
}

#[derive(Serialize)]
pub struct TagListPayload {
    pub name: String,
    pub tags: Vec<TagSummary>,
}

pub async fn list_tags(
    State(store): State<Option<RegistryStore>>,
    Path(name): Path<String>,
) -> Response {
    let Some(store) = store else {
        return registry_disabled();
    };
    let tag_names = list_tags_for(&store, &name).await;
    let mut out = Vec::with_capacity(tag_names.len());
    for tag in tag_names {
        let Some((media_type, digest, _manifest_size)) = read_meta(&store, &name, &tag).await
        else {
            continue;
        };
        let Some(manifest) = read_manifest_json(&store, &name, &tag).await else {
            continue;
        };
        let mut seen = std::collections::BTreeSet::new();
        accumulate_blob_sizes(&manifest, &mut seen);
        let mut total: u64 = 0;
        for d in &seen {
            if let Some(sz) = blob_size(&store, d).await {
                total += sz;
            }
        }
        let created = manifest_modified_at(&store, &name, &tag).await;

        out.push(TagSummary {
            tag,
            digest,
            media_type,
            size: total,
            created,
        });
    }
    Json(TagListPayload { name, tags: out }).into_response()
}

#[derive(Serialize)]
pub struct LayerSummary {
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct ManifestDetailResponse {
    pub name: String,
    pub tag: String,
    pub digest: String,
    pub media_type: String,
    pub config: Option<LayerSummary>,
    pub layers: Vec<LayerSummary>,
    pub total_size: u64,
    /// Raw manifest JSON as pushed, so the UI can render "view raw" without
    /// a second round-trip.
    pub manifest: serde_json::Value,
}

pub async fn get_manifest_detail(
    State(store): State<Option<RegistryStore>>,
    Path((name, tag)): Path<(String, String)>,
) -> Response {
    let Some(store) = store else {
        return registry_disabled();
    };
    let Some((media_type, digest, _)) = read_meta(&store, &name, &tag).await else {
        return not_found("tag not found");
    };
    let Some(manifest) = read_manifest_json(&store, &name, &tag).await else {
        return not_found("manifest not readable");
    };

    let config = manifest.get("config").and_then(|c| layer_from_json(c));

    let layers: Vec<LayerSummary> = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .map(|arr| arr.iter().filter_map(layer_from_json).collect())
        .unwrap_or_default();

    let total_size =
        config.as_ref().map(|c| c.size).unwrap_or(0) + layers.iter().map(|l| l.size).sum::<u64>();

    Json(ManifestDetailResponse {
        name,
        tag,
        digest,
        media_type,
        config,
        layers,
        total_size,
        manifest,
    })
    .into_response()
}

pub async fn delete_tag(
    State(store): State<Option<RegistryStore>>,
    Path((name, tag)): Path<(String, String)>,
) -> Response {
    let Some(store) = store else {
        return registry_disabled();
    };
    if !delete_manifest_by_ref(&store, &name, &tag).await {
        return not_found("tag not found");
    }
    // Note: we intentionally don't garbage-collect blobs here — that's a
    // background sweep concern, and deleting shared layers is a footgun.
    (StatusCode::NO_CONTENT, ()).into_response()
}

// ------------------------------------------------------------------ helpers

fn layer_from_json(v: &serde_json::Value) -> Option<LayerSummary> {
    Some(LayerSummary {
        digest: v.get("digest")?.as_str()?.to_string(),
        media_type: v
            .get("mediaType")
            .and_then(|m| m.as_str())
            .unwrap_or("application/octet-stream")
            .to_string(),
        size: v.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
    })
}

fn accumulate_blob_sizes(
    manifest: &serde_json::Value,
    out: &mut std::collections::BTreeSet<String>,
) {
    if let Some(config) = manifest
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
    {
        out.insert(config.to_string());
    }
    if let Some(layers) = manifest.get("layers").and_then(|l| l.as_array()) {
        for l in layers {
            if let Some(d) = l.get("digest").and_then(|d| d.as_str()) {
                out.insert(d.to_string());
            }
        }
    }
}

fn registry_disabled() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "registry_disabled",
            "message": "the deckwatch registry is not enabled on this deployment",
        })),
    )
        .into_response()
}

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "not_found",
            "message": msg,
        })),
    )
        .into_response()
}

// Silences dead-code warnings if `registry` module is only used via UI.
#[allow(dead_code)]
fn _touch() {
    let _ = registry::v2_root;
}
