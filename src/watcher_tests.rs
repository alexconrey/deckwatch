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

// ---- resolve_git_auth_user ----

#[test]
fn git_auth_user_explicit_value_takes_precedence() {
    assert_eq!(
        resolve_git_auth_user("my-user", "https://gitlab.example.com/org/repo"),
        "my-user"
    );
}

#[test]
fn git_auth_user_gitlab_auto_detected() {
    assert_eq!(
        resolve_git_auth_user("", "https://gitlab.com/org/repo.git"),
        "oauth2"
    );
    assert_eq!(
        resolve_git_auth_user("", "https://gitlab.gravitas.sh/kore/examples/nginx.git"),
        "oauth2"
    );
}

#[test]
fn git_auth_user_github_auto_detected() {
    assert_eq!(
        resolve_git_auth_user("", "https://github.com/org/repo.git"),
        "x-access-token"
    );
}

#[test]
fn git_auth_user_bitbucket_auto_detected() {
    assert_eq!(
        resolve_git_auth_user("", "https://bitbucket.org/org/repo.git"),
        "x-token-auth"
    );
}

#[test]
fn git_auth_user_unknown_host_defaults_to_oauth2() {
    assert_eq!(
        resolve_git_auth_user("", "https://gitea.internal.example.com/org/repo"),
        "oauth2"
    );
}

// ---- multi-arch build helpers ----

#[test]
fn build_arches_contains_both_platforms() {
    let arches: Vec<&str> = BUILD_ARCHES.iter().map(|&(_, arch)| arch).collect();
    assert!(arches.contains(&"amd64"), "BUILD_ARCHES must include amd64");
    assert!(arches.contains(&"arm64"), "BUILD_ARCHES must include arm64");
}

#[test]
fn build_arches_platform_strings_are_valid_kaniko_format() {
    for &(platform, arch) in BUILD_ARCHES {
        assert!(
            platform.starts_with("linux/"),
            "platform {platform} must start with linux/"
        );
        assert!(
            platform.ends_with(arch),
            "platform {platform} must end with arch {arch}"
        );
    }
}

#[test]
fn arch_node_affinity_sets_correct_label() {
    let affinity = arch_node_affinity("arm64");
    let na = affinity.node_affinity.expect("node_affinity should be set");
    let required = na
        .required_during_scheduling_ignored_during_execution
        .expect("required scheduling should be set");
    assert_eq!(required.node_selector_terms.len(), 1);
    let term = &required.node_selector_terms[0];
    let exprs = term
        .match_expressions
        .as_ref()
        .expect("match_expressions should be set");
    assert_eq!(exprs.len(), 1);
    assert_eq!(exprs[0].key, "kubernetes.io/arch");
    assert_eq!(exprs[0].operator, "In");
    assert_eq!(
        exprs[0].values.as_ref().unwrap(),
        &vec!["arm64".to_string()]
    );
}

#[test]
fn arch_node_affinity_amd64() {
    let affinity = arch_node_affinity("amd64");
    let na = affinity.node_affinity.unwrap();
    let terms = na
        .required_during_scheduling_ignored_during_execution
        .unwrap()
        .node_selector_terms;
    let values = terms[0].match_expressions.as_ref().unwrap()[0]
        .values
        .as_ref()
        .unwrap();
    assert_eq!(values, &vec!["amd64".to_string()]);
}

// ---- rewrite_registry_url ----

#[test]
fn rewrite_registry_url_rewrites_matching_public() {
    let result = rewrite_registry_url(
        Some("registry.default.svc.cluster.local:5000"),
        Some("registry.zeus.gc.aws.notskunk.works"),
        "registry.zeus.gc.aws.notskunk.works/myapp",
    );
    assert_eq!(result, "registry.default.svc.cluster.local:5000/myapp");
}

#[test]
fn rewrite_registry_url_no_rewrite_different_registry() {
    let result = rewrite_registry_url(
        Some("registry.default.svc.cluster.local:5000"),
        Some("registry.zeus.gc.aws.notskunk.works"),
        "ghcr.io/org/myapp",
    );
    assert_eq!(result, "ghcr.io/org/myapp");
}

#[test]
fn rewrite_registry_url_no_rewrite_when_no_internal() {
    let result = rewrite_registry_url(
        None,
        Some("registry.zeus.gc.aws.notskunk.works"),
        "registry.zeus.gc.aws.notskunk.works/myapp",
    );
    assert_eq!(result, "registry.zeus.gc.aws.notskunk.works/myapp");
}

#[test]
fn rewrite_registry_url_no_rewrite_when_no_public() {
    let result = rewrite_registry_url(
        Some("registry.default.svc.cluster.local:5000"),
        None,
        "registry.zeus.gc.aws.notskunk.works/myapp",
    );
    assert_eq!(result, "registry.zeus.gc.aws.notskunk.works/myapp");
}

#[test]
fn rewrite_registry_url_no_rewrite_when_both_none() {
    let result = rewrite_registry_url(None, None, "ghcr.io/org/myapp");
    assert_eq!(result, "ghcr.io/org/myapp");
}

// ---- check_internal_registry ----

#[test]
fn check_internal_registry_matches_public_url() {
    assert!(check_internal_registry(
        Some("registry.zeus.gc.aws.notskunk.works"),
        Some("registry.default.svc.cluster.local:5000"),
        "registry.zeus.gc.aws.notskunk.works/myapp:latest",
    ));
}

#[test]
fn check_internal_registry_falls_back_to_internal_url() {
    assert!(check_internal_registry(
        None,
        Some("registry.default.svc.cluster.local:5000"),
        "registry.default.svc.cluster.local:5000/myapp:latest",
    ));
}

#[test]
fn check_internal_registry_false_for_external() {
    assert!(!check_internal_registry(
        Some("registry.zeus.gc.aws.notskunk.works"),
        Some("registry.default.svc.cluster.local:5000"),
        "ghcr.io/org/myapp:latest",
    ));
}

#[test]
fn check_internal_registry_false_when_both_none() {
    assert!(!check_internal_registry(
        None,
        None,
        "ghcr.io/org/myapp:latest"
    ));
}

// ---- job naming conventions ----

#[test]
fn build_job_naming_conventions() {
    let dep_name = "myapp";
    let short_sha = "abc1234";
    let job_name = format!("{dep_name}-build-{short_sha}");

    for &(_, arch) in BUILD_ARCHES {
        let arch_job = format!("{job_name}-{arch}");
        assert!(arch_job.ends_with(arch));
        assert!(arch_job.contains(&job_name));
    }

    let manifest_job = format!("{job_name}-manifest");
    assert!(manifest_job.ends_with("-manifest"));
    assert!(manifest_job.contains(&job_name));
}

#[test]
fn arch_tag_format() {
    let short_sha = "abc1234";
    for &(_, arch) in BUILD_ARCHES {
        let tag = format!("{short_sha}-{arch}");
        assert!(tag.starts_with(short_sha));
        assert!(tag.ends_with(arch));
        assert!(tag.contains('-'));
    }
}
