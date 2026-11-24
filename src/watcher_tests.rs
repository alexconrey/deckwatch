// Unit tests for the pure helpers in src/watcher.rs

use super::*;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta};
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
    let build_arches = [("linux/amd64".to_string(), "amd64".to_string())];
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
fn crane_args_use_flatten_without_docker_flags() {
    let mut crane_args = vec!["index".to_string(), "append".to_string()];
    crane_args.push("--flatten".to_string());

    assert!(crane_args.iter().any(|a| a == "--flatten"));
    assert!(!crane_args.iter().any(|a| a == "--docker-empty-base"));
    assert!(!crane_args.iter().any(|a| a == "--docker-media-types"));
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

// ── reconcile_application_plugins tests ─────────────────────────────────────

/// Verify that the application_id → plugin_names grouping logic works correctly.
/// This exercises the pure in-memory grouping step without requiring a DB or kube client.
#[test]
fn reconcile_groups_associations_by_app_id() {
    struct FakeAssoc {
        application_id: String,
        plugin_name: String,
    }

    let assocs = vec![
        FakeAssoc {
            application_id: "default/my-app".into(),
            plugin_name: "aws".into(),
        },
        FakeAssoc {
            application_id: "default/my-app".into(),
            plugin_name: "vault".into(),
        },
        FakeAssoc {
            application_id: "prod/other-app".into(),
            plugin_name: "aws".into(),
        },
    ];

    let mut by_app: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for assoc in &assocs {
        by_app
            .entry(assoc.application_id.clone())
            .or_default()
            .push(assoc.plugin_name.clone());
    }

    assert_eq!(by_app.len(), 2);
    let mut default_plugins = by_app["default/my-app"].clone();
    default_plugins.sort();
    assert_eq!(default_plugins, vec!["aws", "vault"]);
    assert_eq!(by_app["prod/other-app"], vec!["aws"]);
}

/// Verify that `application_id` values are parsed correctly as `"{ns}/{name}"`.
#[test]
fn reconcile_parses_app_id_correctly() {
    let app_id = "my-namespace/my-application";
    let result = app_id.split_once('/');
    assert!(result.is_some());
    let (ns, name) = result.unwrap();
    assert_eq!(ns, "my-namespace");
    assert_eq!(name, "my-application");
}

/// Malformed `application_id` values (no `/`) are skipped gracefully.
#[test]
fn reconcile_skips_malformed_app_id() {
    let app_id = "no-slash-here";
    assert!(app_id.split_once('/').is_none());
}

// ── is_pending tests ─────────────────────────────────────────────────────────

#[test]
fn is_pending_returns_false_for_empty_state() {
    assert!(!is_pending("{}"));
}

#[test]
fn is_pending_returns_false_for_invalid_json() {
    assert!(!is_pending("not-json"));
}

#[test]
fn is_pending_true_when_status_key_is_provisioning() {
    let state = r#"{"DB_STATUS": "provisioning", "DB_ENGINE": "postgres"}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_true_when_status_key_is_creating() {
    let state = r#"{"DB_STATUS": "creating"}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_true_when_status_key_is_modifying() {
    let state = r#"{"DB_STATUS": "modifying", "DB_HOST": "example.rds.amazonaws.com"}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_true_when_status_key_is_backing_up() {
    let state = r#"{"DB_STATUS": "backing-up", "DB_HOST": "example.rds.amazonaws.com"}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_false_when_status_is_available_and_host_present() {
    let state = r#"{"DB_STATUS": "available", "DB_ENGINE": "postgres", "DB_HOST": "bd-k2crm-db.xxx.rds.amazonaws.com"}"#;
    assert!(!is_pending(state));
}

#[test]
fn is_pending_true_when_db_engine_present_but_host_empty() {
    let state = r#"{"DB_ENGINE": "postgres", "DB_HOST": ""}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_true_when_db_engine_present_and_host_missing() {
    let state = r#"{"DB_ENGINE": "postgres", "DB_STATUS": "available"}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_false_when_db_engine_and_host_both_present() {
    let state = r#"{"DB_ENGINE": "postgres", "DB_HOST": "mydb.cluster.rds.amazonaws.com", "DB_STATUS": "available"}"#;
    assert!(!is_pending(state));
}

#[test]
fn is_pending_true_for_non_db_status_key_with_provisioning_value() {
    let state = r#"{"S3_STATUS": "provisioning", "S3_BUCKET": ""}"#;
    assert!(is_pending(state));
}

#[test]
fn is_pending_false_for_non_db_resource_with_non_pending_status() {
    let state = r#"{"S3_STATUS": "ready", "S3_BUCKET": "my-bucket"}"#;
    assert!(!is_pending(state));
}

#[test]
fn is_pending_false_for_state_with_no_status_keys_and_no_db_keys() {
    let state = r#"{"SOME_KEY": "some_value", "ANOTHER_KEY": "another_value"}"#;
    assert!(!is_pending(state));
}

// ── image_base tests ──────────────────────────────────────────────────────────

#[test]
fn image_base_strips_tag_from_ghcr_image() {
    assert_eq!(image_base("ghcr.io/org/app:v1"), "ghcr.io/org/app");
}

#[test]
fn image_base_strips_digest_from_ghcr_image() {
    assert_eq!(
        image_base("ghcr.io/org/app@sha256:abc123"),
        "ghcr.io/org/app"
    );
}

#[test]
fn image_base_plain_name_no_tag_no_registry() {
    assert_eq!(image_base("ubuntu"), "ubuntu");
}

#[test]
fn image_base_plain_name_with_tag() {
    assert_eq!(image_base("ubuntu:22.04"), "ubuntu");
}

#[test]
fn image_base_registry_with_port_strips_tag_not_port() {
    assert_eq!(
        image_base("registry:5000/org/app:tag"),
        "registry:5000/org/app"
    );
}

#[test]
fn image_base_registry_with_port_strips_digest() {
    assert_eq!(
        image_base("registry:5000/org/app@sha256:abc"),
        "registry:5000/org/app"
    );
}

#[test]
fn image_base_same_base_different_tags_are_equal() {
    let a = image_base("ghcr.io/org/app:v1");
    let b = image_base("ghcr.io/org/app:v2");
    assert_eq!(a, b);
}

#[test]
fn image_base_different_repos_are_not_equal() {
    let a = image_base("ghcr.io/org/app:v1");
    let b = image_base("ghcr.io/org/other:v1");
    assert_ne!(a, b);
}

// ── Releasing plugin resource fields ─────────────────────────────────────────

fn managed_by(manager: &str, operation: &str) -> ManagedFieldsEntry {
    ManagedFieldsEntry {
        manager: Some(manager.to_string()),
        operation: Some(operation.to_string()),
        ..Default::default()
    }
}

fn workload_meta(app: Option<&str>, managers: Vec<ManagedFieldsEntry>) -> ObjectMeta {
    ObjectMeta {
        name: Some("web".to_string()),
        namespace: Some("team".to_string()),
        labels: app.map(|a| BTreeMap::from([(APP_LABEL.to_string(), a.to_string())])),
        managed_fields: Some(managers),
        ..Default::default()
    }
}

fn app_set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn release_patch_carries_only_identity_fields() {
    let patch = release_patch("apps/v1", "Deployment", "team", "web");
    assert_eq!(
        patch,
        serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "web", "namespace": "team" },
        })
    );
}

#[test]
fn reconciler_and_release_use_the_same_field_manager() {
    let pp = plugin_resources_patch_params();
    assert_eq!(
        pp.field_manager.as_deref(),
        Some(PLUGIN_RESOURCES_FIELD_MANAGER)
    );
    assert_eq!(PLUGIN_RESOURCES_FIELD_MANAGER, "deckwatch-plugin-resources");
    assert!(pp.force);
}

#[test]
fn owns_plugin_resource_fields_only_for_our_apply_entry() {
    let ours = workload_meta(
        Some("shop"),
        vec![
            managed_by("deckwatch", "Update"),
            managed_by(PLUGIN_RESOURCES_FIELD_MANAGER, "Apply"),
        ],
    );
    assert!(owns_plugin_resource_fields(&ours));

    let others_only = workload_meta(
        Some("shop"),
        vec![
            managed_by("deckwatch", "Update"),
            managed_by("kubectl", "Apply"),
        ],
    );
    assert!(!owns_plugin_resource_fields(&others_only));

    // Releasing with Apply only affects the Apply entry, so an Update entry
    // under the same name isn't something a sweep can or should act on.
    let update_entry = workload_meta(
        Some("shop"),
        vec![managed_by(PLUGIN_RESOURCES_FIELD_MANAGER, "Update")],
    );
    assert!(!owns_plugin_resource_fields(&update_entry));

    let mut no_managed_fields = workload_meta(Some("shop"), vec![]);
    no_managed_fields.managed_fields = None;
    assert!(!owns_plugin_resource_fields(&no_managed_fields));
}

#[test]
fn stale_plugin_workload_when_app_has_no_resources_left() {
    let meta = workload_meta(
        Some("shop"),
        vec![managed_by(PLUGIN_RESOURCES_FIELD_MANAGER, "Apply")],
    );
    assert_eq!(
        stale_plugin_workload(&meta, &app_set(&["team/blog"])),
        Some(("team", "web"))
    );
    assert_eq!(
        stale_plugin_workload(&meta, &app_set(&[])),
        Some(("team", "web"))
    );
}

#[test]
fn stale_plugin_workload_ignores_apps_that_still_have_resources() {
    let meta = workload_meta(
        Some("shop"),
        vec![managed_by(PLUGIN_RESOURCES_FIELD_MANAGER, "Apply")],
    );
    assert_eq!(stale_plugin_workload(&meta, &app_set(&["team/shop"])), None);
    // Same app name in another namespace is a different application.
    assert_eq!(
        stale_plugin_workload(&meta, &app_set(&["other/shop"])),
        Some(("team", "web"))
    );
}

#[test]
fn stale_plugin_workload_when_app_label_was_removed() {
    // Deleting an application strips the label from its workloads and
    // cascades to its resources; removing a member strips the label too.
    let meta = workload_meta(
        None,
        vec![managed_by(PLUGIN_RESOURCES_FIELD_MANAGER, "Apply")],
    );
    assert_eq!(
        stale_plugin_workload(&meta, &app_set(&["team/shop"])),
        Some(("team", "web"))
    );
}

#[test]
fn stale_plugin_workload_skips_workloads_without_plugin_fields() {
    let meta = workload_meta(None, vec![managed_by("deckwatch", "Update")]);
    assert_eq!(stale_plugin_workload(&meta, &app_set(&[])), None);
}

#[test]
fn plugin_field_sweep_runs_on_first_cycle() {
    let sweep = PluginFieldSweep::default();
    assert!(sweep.needed(&app_set(&[])));
    assert!(sweep.needed(&app_set(&["team/shop"])));
}

#[test]
fn plugin_field_sweep_runs_when_an_app_loses_all_resources() {
    let sweep = PluginFieldSweep {
        apps_with_resources: Some(app_set(&["team/shop", "team/blog"])),
        retry: false,
        ..Default::default()
    };
    assert!(sweep.needed(&app_set(&["team/blog"])));
    assert!(sweep.needed(&app_set(&[])));
}

#[test]
fn plugin_field_sweep_skipped_when_no_app_lost_resources() {
    let sweep = PluginFieldSweep {
        apps_with_resources: Some(app_set(&["team/shop"])),
        retry: false,
        ..Default::default()
    };
    assert!(!sweep.needed(&app_set(&["team/shop"])));
    assert!(!sweep.needed(&app_set(&["team/shop", "team/blog"])));
}

#[test]
fn plugin_field_sweep_retries_after_failure() {
    let sweep = PluginFieldSweep {
        apps_with_resources: Some(app_set(&["team/shop"])),
        retry: true,
        ..Default::default()
    };
    assert!(sweep.needed(&app_set(&["team/shop"])));
}

// ── reconcile_application_plugins against a fake API server ─────────────────
//
// Just enough of the Kubernetes API for the reconciler: typed and metadata
// LISTs (with label selectors) and server-side apply PATCHes for deployments
// and cronjobs. Every request is recorded. An apply records the field
// manager's `managedFields` entry the way the API server does: present
// after applying fields, gone after applying identity fields only.

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    query: String,
    body: serde_json::Value,
}

#[derive(Default)]
struct FakeClusterState {
    /// (plural, namespace, name) → object.
    objects: BTreeMap<(String, String, String), serde_json::Value>,
    requests: Vec<RecordedRequest>,
    fail_patches: bool,
}

#[derive(Clone, Default)]
struct FakeCluster(std::sync::Arc<std::sync::Mutex<FakeClusterState>>);

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(v))
}

fn matches_selector(obj: &serde_json::Value, selector: Option<&str>) -> bool {
    let Some(selector) = selector.filter(|s| !s.is_empty()) else {
        return true;
    };
    let labels = &obj["metadata"]["labels"];
    selector.split(',').all(|term| match term.split_once('=') {
        Some((k, v)) => labels[k].as_str() == Some(v),
        None => labels.get(term).is_some(),
    })
}

fn kind_for(plural: &str) -> &'static str {
    match plural {
        "deployments" => "Deployment",
        "cronjobs" => "CronJob",
        other => panic!("fake cluster does not serve {other}"),
    }
}

fn status_response(code: u16, reason: &str, message: &str) -> (u16, serde_json::Value) {
    (
        code,
        serde_json::json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {},
            "status": "Failure", "message": message, "reason": reason, "code": code,
        }),
    )
}

/// Whether an applied object carries nothing but identity fields.
fn is_identity_only(body: &serde_json::Value) -> bool {
    let Some(obj) = body.as_object() else {
        return false;
    };
    let top_ok = obj
        .keys()
        .all(|k| matches!(k.as_str(), "apiVersion" | "kind" | "metadata"));
    let meta_ok = obj
        .get("metadata")
        .and_then(|m| m.as_object())
        .is_some_and(|m| m.keys().all(|k| matches!(k.as_str(), "name" | "namespace")));
    top_ok && meta_ok
}

impl FakeCluster {
    fn with_objects(objects: Vec<serde_json::Value>) -> Self {
        let cluster = FakeCluster::default();
        {
            let mut st = cluster.0.lock().unwrap();
            for obj in objects {
                let plural = match obj["kind"].as_str() {
                    Some("Deployment") => "deployments",
                    Some("CronJob") => "cronjobs",
                    other => panic!("unsupported fixture kind {other:?}"),
                };
                let ns = obj["metadata"]["namespace"].as_str().unwrap().to_string();
                let name = obj["metadata"]["name"].as_str().unwrap().to_string();
                st.objects.insert((plural.to_string(), ns, name), obj);
            }
        }
        cluster
    }

    fn client(&self) -> kube::Client {
        let cluster = self.clone();
        let service = tower::service_fn(move |req: axum::http::Request<kube::client::Body>| {
            let cluster = cluster.clone();
            async move {
                let (parts, body) = req.into_parts();
                let bytes = body.collect_bytes().await.unwrap_or_default();
                let (status, json) = cluster.handle(&parts, &bytes);
                let response = axum::http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(kube::client::Body::from(serde_json::to_vec(&json).unwrap()))
                    .unwrap();
                Ok::<_, std::convert::Infallible>(response)
            }
        });
        kube::Client::new(service, "default")
    }

    fn handle(&self, parts: &axum::http::request::Parts, body: &[u8]) -> (u16, serde_json::Value) {
        let path = parts.uri.path().to_string();
        let query = parts.uri.query().unwrap_or("").to_string();
        let body_json: serde_json::Value = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(body).expect("request body is JSON")
        };
        let mut st = self.0.lock().unwrap();
        st.requests.push(RecordedRequest {
            method: parts.method.to_string(),
            path: path.clone(),
            query: query.clone(),
            body: body_json.clone(),
        });

        // /apis/{group}/{version}/[namespaces/{ns}/]{plural}[/{name}]
        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (group_version, rest) = match segs.as_slice() {
            ["apis", group, version, rest @ ..] => (format!("{group}/{version}"), rest.to_vec()),
            _ => return status_response(404, "NotFound", "unknown path"),
        };
        let (namespace, plural, name) = match rest.as_slice() {
            [plural] => (None, *plural, None),
            ["namespaces", ns, plural] => (Some(*ns), *plural, None),
            ["namespaces", ns, plural, name] => (Some(*ns), *plural, Some(*name)),
            _ => return status_response(404, "NotFound", "unknown path"),
        };
        let kind = kind_for(plural);

        match (parts.method.as_str(), name) {
            ("GET", None) => {
                let selector = query_param(&query, "labelSelector");
                let items: Vec<serde_json::Value> = st
                    .objects
                    .iter()
                    .filter(|((p, ns, _), obj)| {
                        p == plural
                            && namespace.is_none_or(|want| want == ns)
                            && matches_selector(obj, selector.as_deref())
                    })
                    .map(|(_, obj)| obj.clone())
                    .collect();
                let metadata_only = parts
                    .headers
                    .get("accept")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("PartialObjectMetadataList"));
                if metadata_only {
                    let items: Vec<serde_json::Value> = items
                        .iter()
                        .map(|obj| {
                            serde_json::json!({
                                "apiVersion": "meta.k8s.io/v1",
                                "kind": "PartialObjectMetadata",
                                "metadata": obj["metadata"],
                            })
                        })
                        .collect();
                    (
                        200,
                        serde_json::json!({
                            "apiVersion": "meta.k8s.io/v1",
                            "kind": "PartialObjectMetadataList",
                            "metadata": { "resourceVersion": "1" },
                            "items": items,
                        }),
                    )
                } else {
                    (
                        200,
                        serde_json::json!({
                            "apiVersion": group_version,
                            "kind": format!("{kind}List"),
                            "metadata": { "resourceVersion": "1" },
                            "items": items,
                        }),
                    )
                }
            }
            ("PATCH", Some(name)) => {
                if st.fail_patches {
                    return status_response(500, "InternalError", "injected failure");
                }
                let manager = query_param(&query, "fieldManager").unwrap_or_default();
                let key = (
                    plural.to_string(),
                    namespace.unwrap_or_default().to_string(),
                    name.to_string(),
                );
                let Some(obj) = st.objects.get_mut(&key) else {
                    return status_response(404, "NotFound", "not found");
                };
                let mut entries: Vec<serde_json::Value> = obj["metadata"]["managedFields"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                entries
                    .retain(|e| !(e["manager"] == manager.as_str() && e["operation"] == "Apply"));
                if !is_identity_only(&body_json) {
                    entries.push(serde_json::json!({
                        "manager": manager,
                        "operation": "Apply",
                        "apiVersion": group_version,
                        "fieldsType": "FieldsV1",
                        "fieldsV1": {},
                    }));
                }
                obj["metadata"]["managedFields"] = serde_json::json!(entries);
                (200, obj.clone())
            }
            _ => status_response(405, "MethodNotAllowed", "unsupported"),
        }
    }

    fn take_requests(&self) -> Vec<RecordedRequest> {
        std::mem::take(&mut self.0.lock().unwrap().requests)
    }

    fn set_fail_patches(&self, fail: bool) {
        self.0.lock().unwrap().fail_patches = fail;
    }
}

fn patches(requests: &[RecordedRequest]) -> Vec<&RecordedRequest> {
    requests.iter().filter(|r| r.method == "PATCH").collect()
}

fn patch_to<'a>(requests: &'a [RecordedRequest], path: &str) -> Option<&'a RecordedRequest> {
    requests
        .iter()
        .find(|r| r.method == "PATCH" && r.path == path)
}

const SHOP_DEPLOYMENT_PATH: &str = "/apis/apps/v1/namespaces/team/deployments/shop-web";
const SHOP_CRONJOB_PATH: &str = "/apis/batch/v1/namespaces/team/cronjobs/shop-nightly";

fn shop_deployment(managed_fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": "shop-web",
            "namespace": "team",
            "labels": { APP_LABEL: "shop" },
            "annotations": { "team.example.com/owner": "payments" },
            "managedFields": managed_fields,
        },
        "spec": {
            "selector": { "matchLabels": { "app": "shop-web" } },
            "template": {
                "metadata": { "labels": { "app": "shop-web" } },
                "spec": { "containers": [{
                    "name": "web",
                    "image": "shop:1",
                    "env": [{ "name": "USER_VAR", "value": "1" }],
                }] },
            },
        },
    })
}

fn shop_cronjob(managed_fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "CronJob",
        "metadata": {
            "name": "shop-nightly",
            "namespace": "team",
            "labels": { APP_LABEL: "shop" },
            "managedFields": managed_fields,
        },
        "spec": {
            "schedule": "0 3 * * *",
            "jobTemplate": { "spec": { "template": { "spec": {
                "restartPolicy": "OnFailure",
                "containers": [{ "name": "job", "image": "shop:1" }],
            } } } },
        },
    })
}

fn deckwatch_update_entry() -> serde_json::Value {
    serde_json::json!([{
        "manager": "deckwatch", "operation": "Update", "apiVersion": "apps/v1",
        "fieldsType": "FieldsV1", "fieldsV1": {},
    }])
}

fn plugin_apply_entry() -> serde_json::Value {
    serde_json::json!([
        {
            "manager": "deckwatch", "operation": "Update", "apiVersion": "apps/v1",
            "fieldsType": "FieldsV1", "fieldsV1": {},
        },
        {
            "manager": PLUGIN_RESOURCES_FIELD_MANAGER, "operation": "Apply",
            "apiVersion": "apps/v1", "fieldsType": "FieldsV1", "fieldsV1": {},
        },
    ])
}

async fn reconcile_test_state(cluster: &FakeCluster) -> AppState {
    let db = crate::db::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    AppState {
        kube_client: cluster.client(),
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-settings".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        plugin_telemetry: Default::default(),
    }
}

/// Provision an SQS-like resource: an env var, a resource annotation and a
/// sidecar for every workload in `ns/app`. Returns the row id.
async fn insert_queue_resource(state: &AppState, ns: &str, app: &str) -> String {
    crate::db::ensure_application(&state.db, ns, app)
        .await
        .expect("ensure application");
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_utc();
    application_plugin_resources::ActiveModel {
        id: Set(id.clone()),
        application_id: Set(format!("{ns}/{app}")),
        plugin_name: Set("AWS".to_string()),
        resource_id: Set("sqs".to_string()),
        fields: Set(r#"{"queue_name":"orders"}"#.to_string()),
        state: Set(r#"{"SQS_QUEUE_URL":"https://sqs.example/orders"}"#.to_string()),
        annotations: Set(r#"{"sqs.deckwatch.io/orders":"orders"}"#.to_string()),
        sidecars: Set(r#"[{"name":"queue-proxy","image":"proxy:1"}]"#.to_string()),
        init_containers: Set("[]".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.db)
    .await
    .expect("insert resource row");
    id
}

async fn remove_resource(state: &AppState, id: &str) {
    application_plugin_resources::Entity::delete_by_id(id.to_string())
        .exec(&state.db)
        .await
        .expect("delete resource row");
}

/// The reconcile loop records each plugin's `apply()` health for the
/// application, once per cycle, and recovers it when the plugin is fixed.
#[tokio::test]
async fn reconcile_records_the_plugin_apply_status_of_the_application() {
    use crate::entities::plugin_status;
    use crate::plugin_fixture::{Behavior, FixturePlugin};

    let cluster = FakeCluster::with_objects(vec![shop_deployment(deckwatch_update_entry())]);
    let state = reconcile_test_state(&cluster).await;
    crate::plugin_status::spawn_worker(&state);
    *state.plugins.write().await = vec![FixturePlugin::new()
        .export("apply", Behavior::Trap)
        .loaded("vault")];
    let mut sweep = PluginFieldSweep::default();
    insert_queue_resource(&state, "team", "shop").await;

    reconcile_application_plugins(&state, &mut sweep).await;
    reconcile_application_plugins(&state, &mut sweep).await;
    state.plugin_telemetry.flush().await;
    let rows = plugin_status::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(
        (
            row.application_id.as_str(),
            row.plugin_name.as_str(),
            row.resource_id.as_str()
        ),
        ("team/shop", "vault", "")
    );
    assert_eq!(row.health, "failing");
    assert_eq!(row.consecutive_failures, 2, "one per cycle");
    assert!(row.last_events.contains("plugin.trap"));

    *state.plugins.write().await = vec![FixturePlugin::new()
        .export("apply", Behavior::output("{}"))
        .loaded("vault")];
    reconcile_application_plugins(&state, &mut sweep).await;
    state.plugin_telemetry.flush().await;
    let row = plugin_status::Entity::find()
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.health, "healthy");
    assert_eq!(row.consecutive_failures, 0);
    assert!(row.first_failure_at.is_none());
}

/// The release is reported as an `info` plugin event against the plugin
/// whose resources the application had.
#[tokio::test]
async fn releasing_plugin_fields_is_reported_as_a_plugin_event() {
    let cluster = FakeCluster::with_objects(vec![shop_deployment(deckwatch_update_entry())]);
    let state = reconcile_test_state(&cluster).await;
    let mut sweep = PluginFieldSweep::default();
    let row = insert_queue_resource(&state, "team", "shop").await;
    reconcile_application_plugins(&state, &mut sweep).await;
    cluster.take_requests();

    remove_resource(&state, &row).await;
    let (_, events) = crate::plugin_fixture::capture::capture_async(reconcile_application_plugins(
        &state, &mut sweep,
    ))
    .await;
    let released: Vec<_> = events
        .iter()
        .filter(|e| {
            e.target == "deckwatch::plugin_events"
                && e.fields.get("code").map(String::as_str)
                    == Some(crate::plugin_events::codes::FIELDS_RELEASED)
        })
        .collect();
    assert_eq!(released.len(), 1, "{events:#?}");
    assert_eq!(released[0].level, tracing::Level::INFO);
    assert_eq!(released[0].fields["plugin"], "AWS");
    assert!(
        released[0].message.contains("Deployment team/shop-web"),
        "{}",
        released[0].message
    );
}

#[tokio::test]
async fn removing_last_resource_releases_plugin_fields() {
    let cluster = FakeCluster::with_objects(vec![
        shop_deployment(deckwatch_update_entry()),
        shop_cronjob(deckwatch_update_entry()),
    ]);
    let state = reconcile_test_state(&cluster).await;
    let mut sweep = PluginFieldSweep::default();
    let row = insert_queue_resource(&state, "team", "shop").await;

    // With the resource in place, its fields are applied to both workloads.
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 2, "{requests:#?}");
    let dep_apply = patch_to(&requests, SHOP_DEPLOYMENT_PATH).expect("deployment patched");
    assert!(dep_apply
        .query
        .contains("fieldManager=deckwatch-plugin-resources"));
    assert_eq!(
        dep_apply.body["metadata"]["annotations"]["sqs.deckwatch.io/orders"],
        "orders"
    );
    let dep_containers = &dep_apply.body["spec"]["template"]["spec"]["containers"];
    assert_eq!(dep_containers[0]["env"][0]["name"], "SQS_QUEUE_URL");
    assert_eq!(dep_containers[1]["name"], "queue-proxy");
    let first_dep_body = dep_apply.body.clone();
    assert!(patch_to(&requests, SHOP_CRONJOB_PATH).is_some());

    // Remove the application's last resource: the next cycle releases
    // everything the field manager owns, on the deployment and the cronjob.
    remove_resource(&state, &row).await;
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 2, "{requests:#?}");
    let dep_release = patch_to(&requests, SHOP_DEPLOYMENT_PATH).expect("deployment released");
    assert!(dep_release
        .query
        .contains("fieldManager=deckwatch-plugin-resources"));
    assert!(dep_release.query.contains("force=true"));
    assert_eq!(
        dep_release.body,
        serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "shop-web", "namespace": "team" },
        })
    );
    let cj_release = patch_to(&requests, SHOP_CRONJOB_PATH).expect("cronjob released");
    assert!(cj_release
        .query
        .contains("fieldManager=deckwatch-plugin-resources"));
    assert_eq!(
        cj_release.body,
        serde_json::json!({
            "apiVersion": "batch/v1",
            "kind": "CronJob",
            "metadata": { "name": "shop-nightly", "namespace": "team" },
        })
    );
    {
        let fps = state.plugin_patch_fingerprints.lock().await;
        assert!(!fps.contains_key(&deployment_fingerprint_key("team", "shop-web")));
        assert!(!fps.contains_key(&cronjob_fingerprint_key("team", "shop-nightly")));
    }

    // Nothing is left to release, so later cycles make no API calls at all.
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert!(requests.is_empty(), "{requests:#?}");

    // Provisioning the same resource again re-applies its fields, even
    // though the patch is identical to the one sent before the release.
    insert_queue_resource(&state, "team", "shop").await;
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 2, "{requests:#?}");
    assert_eq!(
        patch_to(&requests, SHOP_DEPLOYMENT_PATH).unwrap().body,
        first_dep_body
    );
}

#[tokio::test]
async fn release_leaves_apps_that_still_have_resources_alone() {
    let blog = serde_json::json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": "blog-web",
            "namespace": "team",
            "labels": { APP_LABEL: "blog" },
            "managedFields": deckwatch_update_entry(),
        },
        "spec": {
            "selector": { "matchLabels": { "app": "blog-web" } },
            "template": {
                "metadata": { "labels": { "app": "blog-web" } },
                "spec": { "containers": [{ "name": "web", "image": "blog:1" }] },
            },
        },
    });
    let cluster = FakeCluster::with_objects(vec![shop_deployment(deckwatch_update_entry()), blog]);
    let state = reconcile_test_state(&cluster).await;
    let mut sweep = PluginFieldSweep::default();
    let shop_row = insert_queue_resource(&state, "team", "shop").await;
    insert_queue_resource(&state, "team", "blog").await;

    reconcile_application_plugins(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 2);

    remove_resource(&state, &shop_row).await;
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    // shop is released; blog's unchanged patch is skipped, not released.
    assert_eq!(patches(&requests).len(), 1, "{requests:#?}");
    assert_eq!(
        patch_to(&requests, SHOP_DEPLOYMENT_PATH).unwrap().body,
        release_patch("apps/v1", "Deployment", "team", "shop-web")
    );
}

#[tokio::test]
async fn failed_release_is_retried_next_cycle() {
    let cluster = FakeCluster::with_objects(vec![shop_deployment(deckwatch_update_entry())]);
    let state = reconcile_test_state(&cluster).await;
    let mut sweep = PluginFieldSweep::default();
    let row = insert_queue_resource(&state, "team", "shop").await;

    reconcile_application_plugins(&state, &mut sweep).await;
    cluster.take_requests();
    remove_resource(&state, &row).await;

    cluster.set_fail_patches(true);
    reconcile_application_plugins(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 1);
    assert!(sweep.retry);

    // The set of applications with resources hasn't changed since the last
    // cycle, but the failed release is retried.
    cluster.set_fail_patches(false);
    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(
        patch_to(&requests, SHOP_DEPLOYMENT_PATH).map(|r| r.body.clone()),
        Some(release_patch("apps/v1", "Deployment", "team", "shop-web"))
    );
    assert!(!sweep.retry);
}

#[tokio::test]
async fn first_cycle_releases_fields_left_behind_before_startup() {
    // The application lost its last resource (or was deleted, which strips
    // its label) while deckwatch was down, or before this fix existed.
    let mut orphan = shop_deployment(plugin_apply_entry());
    orphan["metadata"]["name"] = serde_json::json!("old-web");
    orphan["metadata"]["labels"] = serde_json::json!({});
    let cluster = FakeCluster::with_objects(vec![
        shop_deployment(plugin_apply_entry()),
        shop_cronjob(plugin_apply_entry()),
        orphan,
    ]);
    let state = reconcile_test_state(&cluster).await;
    let mut sweep = PluginFieldSweep::default();

    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 3, "{requests:#?}");
    assert!(patch_to(&requests, SHOP_DEPLOYMENT_PATH).is_some());
    assert!(patch_to(&requests, SHOP_CRONJOB_PATH).is_some());
    assert_eq!(
        patch_to(
            &requests,
            "/apis/apps/v1/namespaces/team/deployments/old-web"
        )
        .unwrap()
        .body,
        release_patch("apps/v1", "Deployment", "team", "old-web")
    );
}

#[tokio::test]
async fn sweep_only_lists_allowed_namespaces() {
    let cluster = FakeCluster::with_objects(vec![shop_deployment(plugin_apply_entry())]);
    let mut state = reconcile_test_state(&cluster).await;
    state.allowed_namespaces = vec!["other".to_string()];
    let mut sweep = PluginFieldSweep::default();

    reconcile_application_plugins(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert!(patches(&requests).is_empty(), "{requests:#?}");
    assert!(
        requests
            .iter()
            .all(|r| r.path.contains("/namespaces/other/")),
        "{requests:#?}"
    );
}

// ── Live cluster: what releasing actually removes ───────────────────────────
//
// `#[ignore]`d like `integration_tests.rs`: needs a reachable API server
// from `KUBECONFIG` / `~/.kube/config`. Point it at a scratch cluster:
//
//   cargo test live_cluster_release -- --ignored

const LIVE_NAMESPACE: &str = "deckwatch-it-plugin-fields";

fn container<'a>(
    spec: &'a k8s_openapi::api::core::v1::PodSpec,
    name: &str,
) -> Option<&'a k8s_openapi::api::core::v1::Container> {
    spec.containers.iter().find(|c| c.name == name)
}

/// Env var names on a container, sorted (a strategic merge patch may put a
/// new entry anywhere in the list).
fn env_names(c: &k8s_openapi::api::core::v1::Container) -> Vec<String> {
    let mut names: Vec<String> = c.env.iter().flatten().map(|e| e.name.clone()).collect();
    names.sort();
    names
}

fn plugin_manager_entries(meta: &ObjectMeta) -> usize {
    meta.managed_fields
        .iter()
        .flatten()
        .filter(|e| e.manager.as_deref() == Some(PLUGIN_RESOURCES_FIELD_MANAGER))
        .count()
}

#[tokio::test]
#[ignore]
async fn live_cluster_release_removes_only_plugin_owned_fields() {
    use k8s_openapi::api::core::v1::Namespace;
    use kube::api::{DeleteParams, PostParams};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = kube::Client::try_default()
        .await
        .expect("kube client -- is KUBECONFIG pointing at a scratch cluster?");

    let ns_api: Api<Namespace> = Api::all(client.clone());
    if ns_api.get_opt(LIVE_NAMESPACE).await.unwrap().is_none() {
        let ns: Namespace =
            serde_json::from_value(serde_json::json!({ "metadata": { "name": LIVE_NAMESPACE } }))
                .unwrap();
        ns_api.create(&PostParams::default(), &ns).await.unwrap();
    }

    // Workloads as a user (or deckwatch's own create path) makes them: an
    // Update operation under another field manager, with the user's own
    // annotation and env var. Zero replicas / suspended so nothing runs.
    let user = PostParams {
        field_manager: Some("deckwatch-it-user".to_string()),
        ..Default::default()
    };
    let deps: Api<Deployment> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let cjs: Api<CronJob> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let _ = deps.delete("shop-web", &DeleteParams::default()).await;
    let _ = cjs.delete("shop-nightly", &DeleteParams::default()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let dep: Deployment = serde_json::from_value(serde_json::json!({
        "metadata": {
            "name": "shop-web",
            "labels": { APP_LABEL: "shop" },
            "annotations": { "team.example.com/owner": "payments" },
        },
        "spec": {
            "replicas": 0,
            "selector": { "matchLabels": { "app": "shop-web" } },
            "template": {
                "metadata": { "labels": { "app": "shop-web" } },
                "spec": { "containers": [{
                    "name": "web",
                    "image": "registry.invalid/shop:1",
                    "env": [{ "name": "USER_VAR", "value": "1" }],
                }] },
            },
        },
    }))
    .unwrap();
    deps.create(&user, &dep).await.unwrap();

    let cj: CronJob = serde_json::from_value(serde_json::json!({
        "metadata": { "name": "shop-nightly", "labels": { APP_LABEL: "shop" } },
        "spec": {
            "schedule": "0 3 * * *",
            "suspend": true,
            "jobTemplate": { "spec": { "template": { "spec": {
                "restartPolicy": "OnFailure",
                "containers": [{
                    "name": "job",
                    "image": "registry.invalid/shop:1",
                    "env": [{ "name": "USER_VAR", "value": "1" }],
                }],
            } } } },
        },
    }))
    .unwrap();
    cjs.create(&user, &cj).await.unwrap();

    // A resource with every kind of field the reconciler applies.
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    let state = AppState {
        kube_client: client.clone(),
        allowed_namespaces: vec![LIVE_NAMESPACE.to_string()],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-settings".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        plugin_telemetry: Default::default(),
    };
    crate::db::ensure_application(&state.db, LIVE_NAMESPACE, "shop")
        .await
        .unwrap();
    let row_id = uuid::Uuid::new_v4().to_string();
    let now = now_utc();
    application_plugin_resources::ActiveModel {
        id: Set(row_id.clone()),
        application_id: Set(format!("{LIVE_NAMESPACE}/shop")),
        plugin_name: Set("AWS".to_string()),
        resource_id: Set("sqs".to_string()),
        fields: Set("{}".to_string()),
        state: Set(r#"{"SQS_QUEUE_URL":"https://sqs.example/orders"}"#.to_string()),
        annotations: Set(r#"{"sqs.deckwatch.io/orders":"orders"}"#.to_string()),
        sidecars: Set(serde_json::json!([{
            "name": "queue-proxy",
            "image": "registry.invalid/proxy:1",
            "volumes": [{ "name": "plugin-scratch", "volume_type": "emptyDir" }],
            "volume_mounts": [{ "name": "plugin-scratch", "mount_path": "/scratch" }],
            "primary_volume_mounts": [{ "name": "plugin-scratch", "mount_path": "/plugin" }],
        }])
        .to_string()),
        init_containers: Set(
            r#"[{"name":"plugin-init","image":"registry.invalid/init:1"}]"#.to_string(),
        ),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.db)
    .await
    .unwrap();

    let mut sweep = PluginFieldSweep::default();
    reconcile_application_plugins(&state, &mut sweep).await;

    // The user changes the workloads after the plugin fields were applied.
    deps.patch(
        "shop-web",
        &PatchParams {
            field_manager: Some("deckwatch-it-user".to_string()),
            ..Default::default()
        },
        &Patch::Strategic(serde_json::json!({
            "metadata": { "annotations": { "team.example.com/later": "yes" } },
            "spec": { "template": { "spec": { "containers": [{
                "name": "web",
                "env": [{ "name": "LATER_VAR", "value": "2" }],
            }] } } },
        })),
    )
    .await
    .unwrap();

    let applied = deps.get("shop-web").await.unwrap();
    let annotations = applied.metadata.annotations.clone().unwrap_or_default();
    assert_eq!(
        annotations
            .get("sqs.deckwatch.io/orders")
            .map(String::as_str),
        Some("orders")
    );
    let pod = applied
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap();
    assert!(env_names(container(pod, "web").unwrap()).contains(&"SQS_QUEUE_URL".to_string()));
    assert!(container(pod, "queue-proxy").is_some());
    assert_eq!(pod.init_containers.as_ref().map(|c| c.len()), Some(1));
    assert_eq!(plugin_manager_entries(&applied.metadata), 1);
    let applied_cj = cjs.get("shop-nightly").await.unwrap();
    let cj_pod = applied_cj
        .spec
        .as_ref()
        .unwrap()
        .job_template
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .clone();
    assert!(env_names(container(&cj_pod, "job").unwrap()).contains(&"SQS_QUEUE_URL".to_string()));
    assert!(container(&cj_pod, "queue-proxy").is_some());

    // Remove the last resource and reconcile once.
    application_plugin_resources::Entity::delete_by_id(row_id)
        .exec(&state.db)
        .await
        .unwrap();
    reconcile_application_plugins(&state, &mut sweep).await;
    assert!(!sweep.retry, "release failed; see warnings above");

    let released = deps.get("shop-web").await.unwrap();
    let annotations = released.metadata.annotations.clone().unwrap_or_default();
    assert!(
        !annotations.contains_key("sqs.deckwatch.io/orders"),
        "{annotations:?}"
    );
    assert_eq!(
        annotations
            .get("team.example.com/owner")
            .map(String::as_str),
        Some("payments")
    );
    assert_eq!(
        annotations
            .get("team.example.com/later")
            .map(String::as_str),
        Some("yes")
    );
    assert_eq!(
        released
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(APP_LABEL))
            .map(String::as_str),
        Some("shop")
    );
    let pod = released
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap();
    let web = container(pod, "web").expect("primary container kept");
    assert_eq!(web.image.as_deref(), Some("registry.invalid/shop:1"));
    assert_eq!(
        env_names(web),
        vec!["LATER_VAR".to_string(), "USER_VAR".to_string()]
    );
    assert!(web
        .volume_mounts
        .iter()
        .flatten()
        .all(|m| m.mount_path != "/plugin"));
    assert_eq!(
        pod.containers.len(),
        1,
        "sidecar removed: {:?}",
        pod.containers
    );
    assert!(pod.init_containers.as_ref().is_none_or(|c| c.is_empty()));
    assert!(pod
        .volumes
        .iter()
        .flatten()
        .all(|v| v.name != "plugin-scratch"));
    assert_eq!(plugin_manager_entries(&released.metadata), 0);

    let released_cj = cjs.get("shop-nightly").await.unwrap();
    let cj_pod = released_cj
        .spec
        .as_ref()
        .unwrap()
        .job_template
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(
        env_names(container(&cj_pod, "job").unwrap()),
        vec!["USER_VAR".to_string()]
    );
    assert_eq!(
        cj_pod.containers.len(),
        1,
        "sidecar removed: {:?}",
        cj_pod.containers
    );
    assert!(cj_pod.init_containers.as_ref().is_none_or(|c| c.is_empty()));
    assert_eq!(plugin_manager_entries(&released_cj.metadata), 0);

    // A second cycle has nothing left to do.
    reconcile_application_plugins(&state, &mut sweep).await;
    assert_eq!(
        deps.get("shop-web")
            .await
            .unwrap()
            .metadata
            .resource_version,
        released.metadata.resource_version
    );

    let _ = deps.delete("shop-web", &DeleteParams::default()).await;
    let _ = cjs.delete("shop-nightly", &DeleteParams::default()).await;
}
