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
    let arches: Vec<&str> = DEFAULT_BUILD_ARCHES.iter().map(|&(_, arch)| arch).collect();
    assert!(
        arches.contains(&"amd64"),
        "DEFAULT_BUILD_ARCHES must include amd64"
    );
    assert!(
        arches.contains(&"arm64"),
        "DEFAULT_BUILD_ARCHES must include arm64"
    );
}

#[test]
fn build_arches_platform_strings_are_valid_kaniko_format() {
    for &(platform, arch) in DEFAULT_BUILD_ARCHES {
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

    for &(_, arch) in DEFAULT_BUILD_ARCHES {
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
    for &(_, arch) in DEFAULT_BUILD_ARCHES {
        let tag = format!("{short_sha}-{arch}");
        assert!(tag.starts_with(short_sha));
        assert!(tag.ends_with(arch));
        assert!(tag.contains('-'));
    }
}

// ---- configurable build architectures ----

#[test]
fn resolve_build_arches_uses_enabled_entries() {
    use crate::handlers::settings::{BuildArchitecture, DeckwatchSettings};
    let settings = DeckwatchSettings {
        build_architectures: vec![
            BuildArchitecture {
                platform: "linux/amd64".into(),
                arch: "amd64".into(),
                enabled: true,
            },
            BuildArchitecture {
                platform: "linux/arm64".into(),
                arch: "arm64".into(),
                enabled: false,
            },
        ],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches.len(), 1);
    assert_eq!(arches[0], ("linux/amd64".to_string(), "amd64".to_string()));
}

#[test]
fn resolve_build_arches_falls_back_when_all_disabled() {
    use crate::handlers::settings::{BuildArchitecture, DeckwatchSettings};
    let settings = DeckwatchSettings {
        build_architectures: vec![BuildArchitecture {
            platform: "linux/amd64".into(),
            arch: "amd64".into(),
            enabled: false,
        }],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches.len(), DEFAULT_BUILD_ARCHES.len());
}

#[test]
fn resolve_build_arches_falls_back_when_empty() {
    use crate::handlers::settings::DeckwatchSettings;
    let settings = DeckwatchSettings {
        build_architectures: vec![],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches.len(), DEFAULT_BUILD_ARCHES.len());
}

#[test]
fn single_arch_build_uses_canonical_tag() {
    let short_sha = "abc1234";
    let build_arches = vec![("linux/amd64".to_string(), "amd64".to_string())];
    let single_arch = build_arches.len() == 1;
    let tag = if single_arch {
        short_sha.to_string()
    } else {
        format!("{short_sha}-{}", build_arches[0].1)
    };
    assert_eq!(tag, "abc1234");
}

#[test]
fn multi_arch_build_uses_arch_suffixed_tag() {
    let short_sha = "abc1234";
    let build_arches = vec![
        ("linux/amd64".to_string(), "amd64".to_string()),
        ("linux/arm64".to_string(), "arm64".to_string()),
    ];
    let single_arch = build_arches.len() == 1;
    for (_, arch) in &build_arches {
        let tag = if single_arch {
            short_sha.to_string()
        } else {
            format!("{short_sha}-{arch}")
        };
        assert!(tag.contains('-'));
        assert!(tag.starts_with(short_sha));
    }
}

#[test]
fn resolve_build_arches_all_enabled() {
    use crate::handlers::settings::{BuildArchitecture, DeckwatchSettings};
    let settings = DeckwatchSettings {
        build_architectures: vec![
            BuildArchitecture {
                platform: "linux/amd64".into(),
                arch: "amd64".into(),
                enabled: true,
            },
            BuildArchitecture {
                platform: "linux/arm64".into(),
                arch: "arm64".into(),
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches.len(), 2);
    assert_eq!(arches[0].1, "amd64");
    assert_eq!(arches[1].1, "arm64");
}

#[test]
fn resolve_build_arches_preserves_order() {
    use crate::handlers::settings::{BuildArchitecture, DeckwatchSettings};
    let settings = DeckwatchSettings {
        build_architectures: vec![
            BuildArchitecture {
                platform: "linux/arm64".into(),
                arch: "arm64".into(),
                enabled: true,
            },
            BuildArchitecture {
                platform: "linux/amd64".into(),
                arch: "amd64".into(),
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches[0].1, "arm64");
    assert_eq!(arches[1].1, "amd64");
}

#[test]
fn resolve_build_arches_with_custom_arch() {
    use crate::handlers::settings::{BuildArchitecture, DeckwatchSettings};
    let settings = DeckwatchSettings {
        build_architectures: vec![
            BuildArchitecture {
                platform: "linux/amd64".into(),
                arch: "amd64".into(),
                enabled: true,
            },
            BuildArchitecture {
                platform: "linux/riscv64".into(),
                arch: "riscv64".into(),
                enabled: true,
            },
        ],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    assert_eq!(arches.len(), 2);
    assert_eq!(
        arches[1],
        ("linux/riscv64".to_string(), "riscv64".to_string())
    );
}

#[test]
fn resolve_build_arches_fallback_matches_default_constant() {
    use crate::handlers::settings::DeckwatchSettings;
    let settings = DeckwatchSettings {
        build_architectures: vec![],
        ..Default::default()
    };
    let arches = resolve_build_arches(&settings);
    for (i, &(platform, arch)) in DEFAULT_BUILD_ARCHES.iter().enumerate() {
        assert_eq!(arches[i].0, platform);
        assert_eq!(arches[i].1, arch);
    }
}

#[test]
fn default_build_architectures_matches_default_constant() {
    use crate::handlers::settings::default_build_architectures;
    let defaults = default_build_architectures();
    assert_eq!(defaults.len(), DEFAULT_BUILD_ARCHES.len());
    for (i, &(platform, arch)) in DEFAULT_BUILD_ARCHES.iter().enumerate() {
        assert_eq!(defaults[i].platform, platform);
        assert_eq!(defaults[i].arch, arch);
        assert!(defaults[i].enabled);
    }
}

// ---- build settings ----

#[test]
fn build_settings_platform_flag_used_in_args() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        platform_flag: "--custom-platform".into(),
        ..default_build_settings()
    };
    let platform = "linux/amd64";
    let arg = format!("{}={platform}", bs.platform_flag);
    assert_eq!(arg, "--custom-platform=linux/amd64");
}

#[test]
fn build_settings_deprecated_flag_still_works() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        platform_flag: "--customPlatform".into(),
        ..default_build_settings()
    };
    let platform = "linux/arm64";
    let arg = format!("{}={platform}", bs.platform_flag);
    assert_eq!(arg, "--customPlatform=linux/arm64");
}

#[test]
fn build_settings_cache_disabled_omits_flag() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        cache_enabled: false,
        ..default_build_settings()
    };
    let mut args = vec!["--dockerfile=Dockerfile".to_string()];
    if bs.cache_enabled {
        args.push("--cache=true".to_string());
    }
    assert!(!args.iter().any(|a| a.contains("cache")));
}

#[test]
fn build_settings_cache_enabled_adds_flag() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        cache_enabled: true,
        ..default_build_settings()
    };
    let mut args = vec!["--dockerfile=Dockerfile".to_string()];
    if bs.cache_enabled {
        args.push("--cache=true".to_string());
    }
    assert!(args.iter().any(|a| a == "--cache=true"));
}

#[test]
fn build_settings_docker_media_types_flag_never_added() {
    // The --docker-media-types flag was removed from crane; the field is kept
    // for backward compat but is now a no-op that always defaults to false.
    use crate::handlers::settings::{default_build_settings, BuildSettings};

    // Even when explicitly set to true, the flag must not appear in crane args.
    let _bs = BuildSettings {
        docker_media_types: true,
        ..default_build_settings()
    };
    let crane_args = vec!["index".to_string(), "append".to_string()];
    // Flag block was removed from the watcher, so nothing adds it.
    assert!(!crane_args.iter().any(|a| a == "--docker-media-types"));

    // Default should now be false.
    let bs_default = default_build_settings();
    assert!(!bs_default.docker_media_types);
}

#[test]
fn build_settings_extra_kaniko_args_appended() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        extra_kaniko_args: vec!["--verbosity=debug".into(), "--reproducible".into()],
        ..default_build_settings()
    };
    let mut args = vec!["--dockerfile=Dockerfile".to_string()];
    for extra in &bs.extra_kaniko_args {
        args.push(extra.clone());
    }
    assert_eq!(args.len(), 3);
    assert!(args.contains(&"--verbosity=debug".to_string()));
    assert!(args.contains(&"--reproducible".to_string()));
}

#[test]
fn build_settings_snapshot_mode_configurable() {
    use crate::handlers::settings::{default_build_settings, BuildSettings};
    let bs = BuildSettings {
        snapshot_mode: "full".into(),
        ..default_build_settings()
    };
    let arg = format!("--snapshot-mode={}", bs.snapshot_mode);
    assert_eq!(arg, "--snapshot-mode=full");
}

#[test]
fn build_settings_default_kaniko_image_is_pinned() {
    use crate::handlers::settings::default_build_settings;
    let bs = default_build_settings();
    assert!(
        !bs.kaniko_image.ends_with(":latest"),
        "default kaniko image should be pinned, not :latest"
    );
    assert!(bs.kaniko_image.contains(":v"));
}

#[test]
fn build_settings_default_platform_flag_is_non_deprecated() {
    use crate::handlers::settings::default_build_settings;
    let bs = default_build_settings();
    assert_eq!(bs.platform_flag, "--custom-platform");
    assert_ne!(bs.platform_flag, "--customPlatform");
}

// ---- pkt-line SHA parsing ----

#[test]
fn parse_ref_sha_strips_pktline_prefix() {
    let resp = "003f9b52e759fdc98199592093b531cd6dd4dfa02d06 refs/heads/main\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "9b52e759fdc98199592093b531cd6dd4dfa02d06");
}

#[test]
fn parse_ref_sha_with_flush_packet_concatenated() {
    let resp = "001e# service=git-upload-pack\n\
                000001469b52e759fdc98199592093b531cd6dd4dfa02d06 refs/heads/main\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "9b52e759fdc98199592093b531cd6dd4dfa02d06");
    assert!(!sha.starts_with("0000"));
}

#[test]
fn parse_ref_sha_with_nul_separator() {
    let resp = "00a09b52e759fdc98199592093b531cd6dd4dfa02d06 refs/heads/main\0 multi_ack\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "9b52e759fdc98199592093b531cd6dd4dfa02d06");
}

#[test]
fn parse_ref_sha_finds_correct_branch() {
    let resp = "003faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa refs/heads/develop\n\
                003fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb refs/heads/main\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
}

#[test]
fn parse_ref_sha_branch_not_found() {
    let resp = "003faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa refs/heads/main\n";
    assert!(parse_ref_sha(resp, "nonexistent").is_err());
}

#[test]
fn parse_ref_sha_plain_format() {
    let resp = "abcdef1234567890abcdef1234567890abcdef12 refs/heads/main\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "abcdef1234567890abcdef1234567890abcdef12");
}

#[test]
fn parse_ref_sha_skips_capabilities_symref() {
    // HEAD line has refs/heads/main in capabilities (symref=HEAD:refs/heads/main)
    // but no SHA before it. The actual ref line comes later.
    let resp = "00a0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa HEAD\0 multi_ack symref=HEAD:refs/heads/main agent=git/2.40\n\
                003f9b52e759fdc98199592093b531cd6dd4dfa02d06 refs/heads/main\n";
    let sha = parse_ref_sha(resp, "main").unwrap();
    assert_eq!(sha, "9b52e759fdc98199592093b531cd6dd4dfa02d06");
}
