// Unit tests for src/handlers/addons.rs

use super::*;
use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec};

// ---- catalog ----

#[test]
fn catalog_contains_expected_addons() {
    let ids: Vec<String> = catalog().into_iter().map(|a| a.id).collect();
    assert!(ids.contains(&"redis".to_string()));
    assert!(ids.contains(&"memcached".to_string()));
    assert!(ids.contains(&"nginx-proxy".to_string()));
    assert!(ids.contains(&"fluent-bit".to_string()));
}

#[test]
fn catalog_ids_are_unique() {
    let mut ids: Vec<String> = catalog().into_iter().map(|a| a.id).collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count, "duplicate addon ids");
}

#[test]
fn catalog_entries_have_nonempty_image_and_name() {
    for a in catalog() {
        assert!(!a.name.is_empty(), "addon {} has empty name", a.id);
        assert!(!a.image.is_empty(), "addon {} has empty image", a.id);
        assert!(
            !a.description.is_empty(),
            "addon {} has empty description",
            a.id
        );
    }
}

// ---- build_resources_from_overrides ----

#[test]
fn build_resources_from_overrides_prefers_overrides_over_defaults() {
    let defaults = ResourceSpecOutput {
        cpu: Some("100m".to_string()),
        memory: Some("128Mi".to_string()),
    };
    let r = build_resources_from_overrides(
        Some(ResourceSpec {
            cpu: Some("250m".to_string()),
            memory: Some("256Mi".to_string()),
        }),
        None,
        Some(&defaults),
    )
    .unwrap();
    let req = r.requests.unwrap();
    assert_eq!(req.get("cpu").unwrap().0, "250m");
    assert_eq!(req.get("memory").unwrap().0, "256Mi");
    // Limits weren't overridden — they fall back to the addon defaults.
    let lim = r.limits.unwrap();
    assert_eq!(lim.get("cpu").unwrap().0, "100m");
    assert_eq!(lim.get("memory").unwrap().0, "128Mi");
}

#[test]
fn build_resources_from_overrides_uses_defaults_when_no_override() {
    let defaults = ResourceSpecOutput {
        cpu: Some("100m".to_string()),
        memory: Some("128Mi".to_string()),
    };
    let r = build_resources_from_overrides(None, None, Some(&defaults)).unwrap();
    // Both requests and limits fall back to the addon defaults.
    assert_eq!(r.requests.as_ref().unwrap().get("cpu").unwrap().0, "100m");
    assert_eq!(r.limits.as_ref().unwrap().get("memory").unwrap().0, "128Mi");
}

#[test]
fn build_resources_from_overrides_returns_none_when_no_defaults_or_overrides() {
    assert!(build_resources_from_overrides(None, None, None).is_none());
}

#[test]
fn build_resources_from_overrides_empty_spec_yields_none_map() {
    let r = build_resources_from_overrides(
        Some(ResourceSpec {
            cpu: None,
            memory: None,
        }),
        Some(ResourceSpec {
            cpu: None,
            memory: None,
        }),
        None,
    );
    assert!(r.is_none());
}

// ---- interpolate / resolve_default_env ----

#[test]
fn interpolate_replaces_deployment_name_placeholder() {
    let ctx = InterpolationCtx {
        deployment_name: "billing-api",
        namespace: "checkout",
    };
    assert_eq!(interpolate("{deployment_name}", &ctx), "billing-api");
}

#[test]
fn interpolate_replaces_namespace_placeholder() {
    let ctx = InterpolationCtx {
        deployment_name: "billing-api",
        namespace: "checkout",
    };
    assert_eq!(interpolate("{namespace}", &ctx), "checkout");
}

#[test]
fn interpolate_replaces_both_placeholders_in_one_string() {
    let ctx = InterpolationCtx {
        deployment_name: "api",
        namespace: "prod",
    };
    assert_eq!(
        interpolate("app={deployment_name},ns={namespace}", &ctx),
        "app=api,ns=prod"
    );
}

#[test]
fn interpolate_leaves_unrelated_content_untouched() {
    let ctx = InterpolationCtx {
        deployment_name: "api",
        namespace: "prod",
    };
    assert_eq!(
        interpolate("http://localhost:4317", &ctx),
        "http://localhost:4317"
    );
}

#[test]
fn resolve_default_env_interpolates_each_entry() {
    let ctx = InterpolationCtx {
        deployment_name: "orders",
        namespace: "shop",
    };
    let input = vec![
        EnvVarOutput {
            name: "OTEL_SERVICE_NAME".to_string(),
            value: "{deployment_name}".to_string(),
        },
        EnvVarOutput {
            name: "K8S_NAMESPACE".to_string(),
            value: "{namespace}".to_string(),
        },
        EnvVarOutput {
            name: "STATIC".to_string(),
            value: "unchanged".to_string(),
        },
    ];
    let out = resolve_default_env(&input, &ctx);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].value, "orders");
    assert_eq!(out[1].value, "shop");
    assert_eq!(out[2].value, "unchanged");
    // Names are copied verbatim.
    assert_eq!(out[0].name, "OTEL_SERVICE_NAME");
}

// ---- inject_addon_env_into_primary ----

fn pod_with_primary(env: Vec<EnvVar>) -> PodSpec {
    PodSpec {
        containers: vec![Container {
            name: "app".to_string(),
            env: if env.is_empty() { None } else { Some(env) },
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn inject_addon_env_adds_missing_vars_and_returns_their_names() {
    let mut pod = pod_with_primary(vec![]);
    let addon = vec![
        EnvVarOutput {
            name: "REDIS_URL".to_string(),
            value: "redis://localhost:6379".to_string(),
        },
        EnvVarOutput {
            name: "CACHE_TTL".to_string(),
            value: "60".to_string(),
        },
    ];
    let injected = inject_addon_env_into_primary(&mut pod, &addon);
    assert_eq!(injected, vec!["REDIS_URL", "CACHE_TTL"]);
    let env = pod.containers[0].env.as_ref().unwrap();
    assert_eq!(env.len(), 2);
    assert_eq!(env[0].name, "REDIS_URL");
    assert_eq!(env[0].value.as_deref(), Some("redis://localhost:6379"));
    assert_eq!(env[1].name, "CACHE_TTL");
}

#[test]
fn inject_addon_env_never_overwrites_user_supplied_var() {
    let mut pod = pod_with_primary(vec![EnvVar {
        name: "REDIS_URL".to_string(),
        value: Some("redis://user-override:6379".to_string()),
        ..Default::default()
    }]);
    let addon = vec![EnvVarOutput {
        name: "REDIS_URL".to_string(),
        value: "redis://localhost:6379".to_string(),
    }];
    let injected = inject_addon_env_into_primary(&mut pod, &addon);
    assert!(injected.is_empty());
    let env = pod.containers[0].env.as_ref().unwrap();
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].value.as_deref(), Some("redis://user-override:6379"));
}

#[test]
fn inject_addon_env_is_a_noop_when_addon_env_is_empty() {
    let mut pod = pod_with_primary(vec![]);
    let injected = inject_addon_env_into_primary(&mut pod, &[]);
    assert!(injected.is_empty());
    assert!(pod.containers[0].env.is_none());
}

#[test]
fn inject_addon_env_is_a_noop_when_pod_has_no_containers() {
    let mut pod = PodSpec {
        containers: vec![],
        ..Default::default()
    };
    let addon = vec![EnvVarOutput {
        name: "X".to_string(),
        value: "y".to_string(),
    }];
    let injected = inject_addon_env_into_primary(&mut pod, &addon);
    assert!(injected.is_empty());
}

// ---- remove_injected_env_from_primary ----

#[test]
fn remove_injected_env_deletes_only_named_vars() {
    let mut pod = pod_with_primary(vec![
        EnvVar {
            name: "USER_VAR".to_string(),
            value: Some("keep".to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "REDIS_URL".to_string(),
            value: Some("redis://localhost:6379".to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "CACHE_TTL".to_string(),
            value: Some("60".to_string()),
            ..Default::default()
        },
    ]);
    remove_injected_env_from_primary(
        &mut pod,
        &["REDIS_URL".to_string(), "CACHE_TTL".to_string()],
    );
    let env = pod.containers[0].env.as_ref().unwrap();
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].name, "USER_VAR");
}

#[test]
fn remove_injected_env_clears_env_to_none_when_last_var_removed() {
    let mut pod = pod_with_primary(vec![EnvVar {
        name: "REDIS_URL".to_string(),
        value: Some("redis://localhost:6379".to_string()),
        ..Default::default()
    }]);
    remove_injected_env_from_primary(&mut pod, &["REDIS_URL".to_string()]);
    assert!(pod.containers[0].env.is_none());
}

#[test]
fn remove_injected_env_is_a_noop_when_names_list_empty() {
    let mut pod = pod_with_primary(vec![EnvVar {
        name: "USER_VAR".to_string(),
        value: Some("v".to_string()),
        ..Default::default()
    }]);
    remove_injected_env_from_primary(&mut pod, &[]);
    assert_eq!(pod.containers[0].env.as_ref().unwrap().len(), 1);
}

// Round-trip: injecting and then removing by the returned name list
// should leave the primary container's env in its original state.
#[test]
fn inject_then_remove_is_a_round_trip() {
    let mut pod = pod_with_primary(vec![EnvVar {
        name: "USER_VAR".to_string(),
        value: Some("keep".to_string()),
        ..Default::default()
    }]);
    let addon = vec![
        EnvVarOutput {
            name: "REDIS_URL".to_string(),
            value: "redis://localhost:6379".to_string(),
        },
        EnvVarOutput {
            name: "USER_VAR".to_string(),
            value: "should-not-overwrite".to_string(),
        },
    ];
    let injected = inject_addon_env_into_primary(&mut pod, &addon);
    assert_eq!(injected, vec!["REDIS_URL"]);
    remove_injected_env_from_primary(&mut pod, &injected);
    let env = pod.containers[0].env.as_ref().unwrap();
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].name, "USER_VAR");
    assert_eq!(env[0].value.as_deref(), Some("keep"));
}
