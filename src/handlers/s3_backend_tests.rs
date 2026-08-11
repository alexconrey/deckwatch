// Unit tests for the pure helpers and types in src/handlers/s3_backend.rs.

use super::*;
use object_store::path::Path as OPath;

// ---- S3Config / S3Backend::new ----

fn test_config(bucket: &str, prefix: &str) -> S3Config {
    S3Config {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
        region: "us-east-1".to_string(),
        endpoint: "http://localhost:9000".to_string(),
        path_style: false,
    }
}

#[test]
fn s3_backend_new_fails_with_empty_bucket() {
    let cfg = test_config("", "");
    let result = S3Backend::new(cfg);
    let msg = match result {
        Ok(_) => panic!("expected error for empty bucket"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("bucket"), "error should mention bucket: {msg}");
}

#[test]
fn s3_backend_new_fails_with_whitespace_only_bucket() {
    let cfg = test_config("   ", "");
    let result = S3Backend::new(cfg);
    assert!(result.is_err());
}

#[test]
fn s3_backend_new_succeeds_with_valid_config() {
    let cfg = test_config("my-bucket", "");
    let backend = S3Backend::new(cfg);
    assert!(backend.is_ok());
}

#[test]
fn s3_backend_normalizes_prefix_slashes() {
    let cfg = test_config("my-bucket", "/leading/trailing/");
    let backend = S3Backend::new(cfg).unwrap();
    assert_eq!(backend.prefix, "leading/trailing");
}

#[test]
fn s3_backend_normalizes_prefix_multiple_slashes() {
    let cfg = test_config("my-bucket", "///middle///");
    let backend = S3Backend::new(cfg).unwrap();
    // trim_matches('/') strips all leading and trailing slashes
    assert!(!backend.prefix.starts_with('/'));
    assert!(!backend.prefix.ends_with('/'));
}

#[test]
fn s3_backend_empty_prefix_stays_empty() {
    let cfg = test_config("my-bucket", "");
    let backend = S3Backend::new(cfg).unwrap();
    assert!(backend.prefix.is_empty());
}

// ---- ManifestMeta serde roundtrip ----

#[test]
fn manifest_meta_serializes_to_json() {
    let meta = ManifestMeta {
        media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
        digest: "sha256:abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234"
            .to_string(),
        size: 1024,
    };
    let json = serde_json::to_value(&meta).unwrap();
    assert_eq!(
        json["media_type"],
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(
        json["digest"],
        "sha256:abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234"
    );
    assert_eq!(json["size"], 1024);
}

#[test]
fn manifest_meta_roundtrips_through_json() {
    let original = ManifestMeta {
        media_type: "application/vnd.docker.distribution.manifest.v2+json".to_string(),
        digest: "sha256:0000111122223333444455556666777788889999aaaabbbbccccddddeeeeffff"
            .to_string(),
        size: 42,
    };
    let json_str = serde_json::to_string(&original).unwrap();
    let deserialized: ManifestMeta = serde_json::from_str(&json_str).unwrap();
    assert_eq!(deserialized.media_type, original.media_type);
    assert_eq!(deserialized.digest, original.digest);
    assert_eq!(deserialized.size, original.size);
}

#[test]
fn manifest_meta_deserializes_zero_size() {
    let raw = r#"{"media_type":"text/plain","digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","size":0}"#;
    let meta: ManifestMeta = serde_json::from_str(raw).unwrap();
    assert_eq!(meta.size, 0);
}

#[test]
fn manifest_meta_rejects_missing_fields() {
    let raw = r#"{"media_type":"text/plain"}"#;
    assert!(serde_json::from_str::<ManifestMeta>(raw).is_err());
}

// ---- key_for ----

#[test]
fn key_for_without_prefix() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    assert_eq!(
        backend.key_for("blobs/sha256/abc").as_ref(),
        "blobs/sha256/abc"
    );
}

#[test]
fn key_for_with_prefix() {
    let backend = S3Backend::new(test_config("b", "deckwatch")).unwrap();
    assert_eq!(
        backend.key_for("blobs/sha256/abc").as_ref(),
        "deckwatch/blobs/sha256/abc"
    );
}

#[test]
fn key_for_with_nested_prefix() {
    let backend = S3Backend::new(test_config("b", "env/prod")).unwrap();
    assert_eq!(
        backend.key_for("manifests/myapp/latest").as_ref(),
        "env/prod/manifests/myapp/latest"
    );
}

// ---- blob_key ----

#[test]
fn blob_key_valid_digest() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let hex = "a".repeat(64);
    let digest = format!("sha256:{hex}");
    let key = backend.blob_key(&digest).unwrap();
    assert_eq!(key.as_ref(), format!("blobs/sha256/{hex}"));
}

#[test]
fn blob_key_with_prefix() {
    let backend = S3Backend::new(test_config("b", "registry")).unwrap();
    let hex = "b".repeat(64);
    let digest = format!("sha256:{hex}");
    let key = backend.blob_key(&digest).unwrap();
    assert_eq!(key.as_ref(), format!("registry/blobs/sha256/{hex}"));
}

#[test]
fn blob_key_rejects_missing_sha256_prefix() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let hex = "a".repeat(64);
    assert!(backend.blob_key(&hex).is_none());
}

#[test]
fn blob_key_rejects_short_hex() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    assert!(backend.blob_key("sha256:abcd").is_none());
}

#[test]
fn blob_key_rejects_non_hex_characters() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let bad = format!("sha256:{}zzzz", "a".repeat(60));
    assert!(backend.blob_key(&bad).is_none());
}

#[test]
fn blob_key_rejects_too_long_hex() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let hex = "a".repeat(65);
    assert!(backend.blob_key(&format!("sha256:{hex}")).is_none());
}

// ---- upload_key ----

#[test]
fn upload_key_without_prefix() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let key = backend.upload_key("550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(key.as_ref(), "uploads/550e8400-e29b-41d4-a716-446655440000");
}

#[test]
fn upload_key_with_prefix() {
    let backend = S3Backend::new(test_config("b", "pfx")).unwrap();
    let key = backend.upload_key("my-uuid");
    assert_eq!(key.as_ref(), "pfx/uploads/my-uuid");
}

// ---- manifest_key ----

#[test]
fn manifest_key_without_prefix() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let key = backend.manifest_key("myorg/myapp", "latest");
    assert_eq!(key.as_ref(), "manifests/myorg/myapp/latest");
}

#[test]
fn manifest_key_with_prefix() {
    let backend = S3Backend::new(test_config("b", "reg")).unwrap();
    let key = backend.manifest_key("myapp", "v1.0.0");
    assert_eq!(key.as_ref(), "reg/manifests/myapp/v1.0.0");
}

#[test]
fn manifest_key_digest_reference() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let hex = "a".repeat(64);
    let reference = format!("sha256:{hex}");
    let key = backend.manifest_key("app", &reference);
    assert_eq!(key.as_ref(), format!("manifests/app/sha256:{hex}"));
}

// ---- manifest_meta_key ----

#[test]
fn manifest_meta_key_structure() {
    let backend = S3Backend::new(test_config("b", "")).unwrap();
    let key = backend.manifest_meta_key("myapp", "latest");
    assert_eq!(key.as_ref(), "manifests/myapp/_meta/latest.json");
}

#[test]
fn manifest_meta_key_with_prefix() {
    let backend = S3Backend::new(test_config("b", "pfx")).unwrap();
    let key = backend.manifest_meta_key("myapp", "v2");
    assert_eq!(key.as_ref(), "pfx/manifests/myapp/_meta/v2.json");
}

// ---- digest_bytes ----

#[test]
fn digest_bytes_empty_input() {
    // SHA-256 of empty input is the well-known constant.
    let d = digest_bytes(b"");
    assert_eq!(
        d,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn digest_bytes_known_value() {
    // SHA-256("hello") is well-known.
    let d = digest_bytes(b"hello");
    assert_eq!(
        d,
        "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn digest_bytes_deterministic() {
    let a = digest_bytes(b"test data");
    let b = digest_bytes(b"test data");
    assert_eq!(a, b);
}

// ---- strip_prefix ----

#[test]
fn strip_prefix_basic() {
    let base = OPath::from("manifests");
    let child = OPath::from("manifests/myapp/latest");
    assert_eq!(strip_prefix(&child, &base).unwrap(), "myapp/latest");
}

#[test]
fn strip_prefix_empty_base() {
    let base = OPath::from("");
    let child = OPath::from("manifests/myapp");
    assert_eq!(strip_prefix(&child, &base).unwrap(), "manifests/myapp");
}

#[test]
fn strip_prefix_no_match() {
    let base = OPath::from("blobs");
    let child = OPath::from("manifests/myapp");
    assert!(strip_prefix(&child, &base).is_none());
}

#[test]
fn strip_prefix_exact_match() {
    let base = OPath::from("manifests/myapp");
    let child = OPath::from("manifests/myapp");
    // Child == base means the stripped suffix is empty.
    let result = strip_prefix(&child, &base);
    // The function should either return None or Some("") — both are
    // acceptable since the caller only uses non-empty results.
    match result {
        None => {} // fine
        Some(s) => assert!(s.is_empty(), "should be empty for exact match"),
    }
}

// ---- io_from_object_store ----

#[test]
fn io_from_object_store_maps_not_found() {
    let err = object_store::Error::NotFound {
        path: "missing".to_string(),
        source: Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")),
    };
    let io_err = io_from_object_store(err);
    assert_eq!(io_err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn io_from_object_store_maps_already_exists() {
    let err = object_store::Error::AlreadyExists {
        path: "dup".to_string(),
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "exists",
        )),
    };
    let io_err = io_from_object_store(err);
    assert_eq!(io_err.kind(), std::io::ErrorKind::AlreadyExists);
}
