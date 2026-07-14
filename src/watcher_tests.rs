// Unit tests for the pure helpers in src/watcher.rs

use super::*;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::BTreeMap;

#[test]
fn test_ann_formats_key() {
    assert_eq!(ann("git-repo"), "deckwatch.io/git-repo");
    assert_eq!(ann("oci-repository"), "deckwatch.io/oci-repository");
    assert_eq!(ann("ecr-repository"), "deckwatch.io/ecr-repository");
}

#[test]
fn test_get_ann_present() {
    let mut dep = Deployment::default();
    dep.metadata.annotations = Some(BTreeMap::from([(
        "deckwatch.io/git-repo".to_string(),
        "https://github.com/org/repo".to_string(),
    )]));
    assert_eq!(
        get_ann(&dep, "git-repo"),
        Some("https://github.com/org/repo")
    );
}

#[test]
fn test_get_ann_missing() {
    let dep = Deployment::default();
    assert_eq!(get_ann(&dep, "git-repo"), None);
}

#[test]
fn test_get_ann_missing_with_other_annotations() {
    let mut dep = Deployment::default();
    dep.metadata.annotations = Some(BTreeMap::from([(
        "some-other/annotation".to_string(),
        "value".to_string(),
    )]));
    assert_eq!(get_ann(&dep, "git-repo"), None);
}

#[test]
fn test_get_oci_repository_prefers_oci() {
    let mut dep = Deployment::default();
    dep.metadata.annotations = Some(BTreeMap::from([
        (
            "deckwatch.io/oci-repository".to_string(),
            "ghcr.io/org/app".to_string(),
        ),
        (
            "deckwatch.io/ecr-repository".to_string(),
            "123456.dkr.ecr.us-east-1.amazonaws.com/app".to_string(),
        ),
    ]));
    assert_eq!(get_oci_repository(&dep), Some("ghcr.io/org/app"));
}

#[test]
fn test_get_oci_repository_falls_back_to_ecr() {
    let mut dep = Deployment::default();
    dep.metadata.annotations = Some(BTreeMap::from([(
        "deckwatch.io/ecr-repository".to_string(),
        "123456.dkr.ecr.us-east-1.amazonaws.com/app".to_string(),
    )]));
    assert_eq!(
        get_oci_repository(&dep),
        Some("123456.dkr.ecr.us-east-1.amazonaws.com/app")
    );
}

#[test]
fn test_get_oci_repository_none_when_neither_present() {
    let dep = Deployment::default();
    assert_eq!(get_oci_repository(&dep), None);
}
