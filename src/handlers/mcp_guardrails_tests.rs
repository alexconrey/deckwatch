use super::*;
use mcp_k8s::policy::{check_pod_spec, check_service, into_result};
use serde_json::json;

fn guardrails(value: serde_json::Value) -> McpGuardrails {
    serde_json::from_value(value).expect("guardrail settings")
}

/// The text mcp-k8s returns when `policy` rejects `service`.
fn service_rejection(policy: &WorkloadPolicy, service: serde_json::Value) -> Option<String> {
    let mut violations = Vec::new();
    check_service(policy, &service, &mut violations);
    into_result(violations).err().map(|e| e.to_string())
}

fn load_balancer() -> serde_json::Value {
    json!({ "spec": { "type": "LoadBalancer" } })
}

fn privileged_pod_spec() -> serde_json::Value {
    json!({
        "containers": [{
            "name": "app",
            "image": "busybox",
            "securityContext": { "privileged": true }
        }]
    })
}

fn pod_rejected(policy: &WorkloadPolicy, spec: &serde_json::Value) -> bool {
    let mut violations = Vec::new();
    check_pod_spec(policy, spec, None, "spec", &mut violations);
    !violations.is_empty()
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

#[test]
fn the_defaults_are_secure_but_keep_apply_manifest() {
    let g = McpGuardrails::default();
    assert_eq!(g.workload_policy, WorkloadPolicyMode::Restrictive);
    assert!(g.exceptions.is_empty());
    assert!(g.allowed_custom_resource_groups.is_empty());
    assert!(!g.allow_privileged_csr);
    assert!(g.apply_manifest_enabled);
    assert!(!g.secret_decode_enabled);
    assert!(g.secure_defaults);

    let perms = g.permissions();
    // mcp-k8s's restrictive policy, unchanged.
    assert_eq!(perms.workload_policy(), &WorkloadPolicy::default());
    assert!(perms.apply_manifest_enabled);
    assert!(perms.is_tool_allowed("apply_manifest"));
    assert!(!perms.secret_decode_enabled);
    for tool in ["create_deployment", "delete_pod", "exec_pod", "approve_csr"] {
        assert!(perms.is_tool_allowed(tool), "{tool}");
    }
}

#[test]
fn missing_fields_take_the_defaults() {
    // A stored settings document from before this setting existed.
    assert_eq!(guardrails(json!({})), McpGuardrails::default());
    let tuning: crate::handlers::settings::McpTuning =
        serde_json::from_value(json!({ "global": "Be careful." })).unwrap();
    assert_eq!(tuning.guardrails, None);
    assert_eq!(tuning.effective_guardrails(), McpGuardrails::default());
    // One field set: the others keep their defaults, including the two that
    // default to on.
    let g = guardrails(json!({ "secret_decode_enabled": true }));
    assert!(g.secret_decode_enabled && g.apply_manifest_enabled && g.secure_defaults);
}

#[test]
fn the_settings_serialize_with_the_library_field_names() {
    assert_eq!(
        serde_json::to_value(McpGuardrails::default()).unwrap(),
        json!({
            "workload_policy": "restrictive",
            "exceptions": [],
            "allowed_custom_resource_groups": [],
            "allow_privileged_csr": false,
            "apply_manifest_enabled": true,
            "secret_decode_enabled": false,
            "secure_defaults": true,
        })
    );
    assert!(
        serde_json::from_value::<McpGuardrails>(json!({ "workload_policy": "lenient" })).is_err()
    );
}

// ---------------------------------------------------------------------------
// Each setting changes the permissions
// ---------------------------------------------------------------------------

#[test]
fn apply_manifest_follows_its_setting() {
    let off = guardrails(json!({ "apply_manifest_enabled": false })).permissions();
    assert!(!off.apply_manifest_enabled);
    assert!(!off.is_tool_allowed("apply_manifest"));
    assert_eq!(
        off.denied_by("apply_manifest").as_deref(),
        Some("apply_manifest:disabled")
    );
    // Nothing else changes.
    assert_eq!(off.workload_policy(), &WorkloadPolicy::default());
    assert!(off.is_tool_allowed("create_deployment"));
}

#[test]
fn secret_decoding_follows_its_setting() {
    assert!(!McpGuardrails::default().permissions().secret_decode_enabled);
    let on = guardrails(json!({ "secret_decode_enabled": true })).permissions();
    assert!(on.secret_decode_enabled);
    assert_eq!(on.workload_policy(), &WorkloadPolicy::default());
}

#[test]
fn the_permissive_mode_turns_the_workload_guardrails_off() {
    let g = guardrails(json!({ "workload_policy": "permissive" }));
    let policy = g.workload_policy();
    assert!(service_rejection(&policy, load_balancer()).is_none());
    assert!(!pod_rejected(&policy, &privileged_pod_spec()));
    assert!(policy.custom_resource_group_allowed("anything.example.com"));
    // The CSR check and the secure defaults keep their own switches.
    let mut expected = WorkloadPolicy::permissive();
    expected.allow_privileged_csr = false;
    expected.secure_defaults = true;
    assert_eq!(policy, expected);
    assert_eq!(g.permissions().workload_policy(), &expected);
}

#[test]
fn the_restrictive_mode_rejects_until_an_exception_allows_the_rule() {
    let strict = McpGuardrails::default().workload_policy();
    let rejection = service_rejection(&strict, load_balancer()).expect("rejected");
    assert!(
        rejection.contains("WorkloadPolicy.allow_load_balancer"),
        "{rejection}"
    );
    assert!(pod_rejected(&strict, &privileged_pod_spec()));

    let relaxed = guardrails(json!({ "exceptions": ["allow_load_balancer"] })).workload_policy();
    assert!(service_rejection(&relaxed, load_balancer()).is_none());
    // Only that rule.
    assert!(pod_rejected(&relaxed, &privileged_pod_spec()));
    assert!(service_rejection(&relaxed, json!({ "spec": { "type": "NodePort" } })).is_some());
}

#[test]
fn every_exception_relaxes_its_own_rule_and_nothing_else() {
    let all: Vec<&str> = EXCEPTIONS.iter().map(|e| e.flag.field).collect();
    let relaxed = guardrails(json!({ "exceptions": all })).workload_policy();
    // Every boolean rule but the CSR check is off; the lists and the secure
    // defaults are the restrictive ones.
    let mut expected = WorkloadPolicy::permissive();
    expected.allowed_capabilities = Vec::new();
    expected.allowed_volume_types = Vec::new();
    expected.allowed_manifest_kinds = Vec::new();
    expected.allowed_custom_resource_groups = Vec::new();
    expected.allow_privileged_csr = false;
    expected.secure_defaults = true;
    assert_eq!(relaxed, expected);

    // One by one: each changes the policy, and differently.
    let mut seen = Vec::new();
    for e in EXCEPTIONS {
        let policy = guardrails(json!({ "exceptions": [e.flag.field] })).workload_policy();
        assert_ne!(policy, WorkloadPolicy::default(), "{}", e.flag.field);
        assert!(
            !seen.contains(&policy),
            "{} repeats another rule",
            e.flag.field
        );
        seen.push(policy);
    }
}

#[test]
fn custom_resource_groups_are_allowed_one_by_one() {
    let strict = McpGuardrails::default().workload_policy();
    assert!(!strict.custom_resource_group_allowed("cert-manager.io"));

    let policy = guardrails(json!({ "allowed_custom_resource_groups": ["cert-manager.io"] }))
        .workload_policy();
    assert!(policy.custom_resource_group_allowed("cert-manager.io"));
    assert!(!policy.custom_resource_group_allowed("argoproj.io"));

    let any = guardrails(json!({ "allowed_custom_resource_groups": ["*"] })).workload_policy();
    assert!(any.custom_resource_group_allowed("argoproj.io"));
}

#[test]
fn privileged_csr_approval_follows_its_setting_in_either_mode() {
    for mode in ["restrictive", "permissive"] {
        let off = guardrails(json!({ "workload_policy": mode })).workload_policy();
        assert!(!off.allow_privileged_csr, "{mode}");
        let on = guardrails(json!({ "workload_policy": mode, "allow_privileged_csr": true }))
            .workload_policy();
        assert!(on.allow_privileged_csr, "{mode}");
    }
}

#[test]
fn secure_defaults_follow_their_setting_in_either_mode() {
    for mode in ["restrictive", "permissive"] {
        assert!(
            guardrails(json!({ "workload_policy": mode }))
                .workload_policy()
                .secure_defaults
        );
        let off = guardrails(json!({ "workload_policy": mode, "secure_defaults": false }));
        assert!(!off.workload_policy().secure_defaults, "{mode}");
        assert!(
            !off.permissions().workload_policy().secure_defaults,
            "{mode}"
        );
    }
}

/// mcp-k8s `WorkloadPolicy` settings deckwatch doesn't expose (yet): the
/// lists of extra capabilities, volume types, registries, ingress annotations
/// and manifest kinds. A rejection that names one of them can only be lifted
/// with the permissive mode, and the error hint says so.
const NOT_EXPOSED: &[&PolicyFlag] = &[
    &flags::ALLOW_CAPABILITIES,
    &flags::ALLOW_VOLUME_TYPES,
    &flags::ALLOWED_REGISTRIES,
    &flags::INGRESS_ANNOTATION_ALLOWLIST,
    &flags::INGRESS_ANNOTATION_DENYLIST,
    &flags::ALLOW_MANIFEST_KINDS,
];

/// mcp-k8s grows a rule: decide whether deckwatch exposes it.
#[test]
fn every_library_policy_setting_is_exposed_or_listed_as_not_exposed() {
    let own_settings = [
        flags::ALLOW_PRIVILEGED_CSR.field,
        flags::ALLOW_CUSTOM_RESOURCE_GROUPS.field,
        flags::NO_SECURE_DEFAULTS.field,
    ];
    for flag in flags::ALL {
        let exposed = exception(flag.field).is_some()
            || own_settings.contains(&flag.field)
            || NOT_EXPOSED.iter().any(|f| f.field == flag.field);
        assert!(exposed, "mcp-k8s setting {} is unaccounted for", flag.field);
    }
    for e in EXCEPTIONS {
        assert!(e.flag.field.starts_with("allow_"), "{}", e.flag.field);
    }
    for flag in NOT_EXPOSED {
        let hint = remedy(flag.field);
        assert!(hint.contains("only the permissive policy"), "{hint}");
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn validation_normalizes_exceptions_and_groups() {
    let g = guardrails(json!({
        "exceptions": ["allow_node_port", " allow_privileged ", "allow_node_port"],
        "allowed_custom_resource_groups": [" Cert-Manager.io ", "", "cert-manager.io", "*"],
    }))
    .validated()
    .expect("valid");
    // Once each, in the documented order.
    assert_eq!(g.exceptions, ["allow_privileged", "allow_node_port"]);
    assert_eq!(g.allowed_custom_resource_groups, ["cert-manager.io", "*"]);
}

#[test]
fn validation_rejects_unknown_rules() {
    let err = guardrails(json!({ "exceptions": ["allow_everything"] }))
        .validated()
        .unwrap_err();
    assert!(err.contains("allow_everything"), "{err}");
    assert!(
        err.contains("allow_load_balancer"),
        "lists the rules: {err}"
    );

    let err = guardrails(json!({ "exceptions": ["allow_privileged_csr"] }))
        .validated()
        .unwrap_err();
    assert!(err.contains("a setting of its own"), "{err}");
}

#[test]
fn validation_rejects_bad_and_built_in_api_groups() {
    for bad in ["cert manager", "-bad.example.com", "UPPER_case", "a..b"] {
        let err = guardrails(json!({ "allowed_custom_resource_groups": [bad] }))
            .validated()
            .unwrap_err();
        assert!(
            err.contains("allowed_custom_resource_groups"),
            "{bad}: {err}"
        );
    }
    let err = guardrails(json!({ "allowed_custom_resource_groups": ["apps"] }))
        .validated()
        .unwrap_err();
    assert!(err.contains("built-in"), "{err}");
}

#[test]
fn changes_name_each_setting_with_its_old_and_new_value() {
    let before = McpGuardrails::default();
    let after = guardrails(json!({
        "apply_manifest_enabled": false,
        "secret_decode_enabled": true,
        "exceptions": ["allow_load_balancer"],
    }));
    let changes = McpGuardrails::changes(&before, &after);
    let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
    assert_eq!(
        fields,
        [
            "exceptions",
            "apply_manifest_enabled",
            "secret_decode_enabled"
        ]
    );
    assert_eq!(changes[0].from, "none");
    assert_eq!(changes[0].to, "allow_load_balancer");
    assert_eq!(
        changes[1].describe(),
        "mcp_tuning.guardrails.apply_manifest_enabled changed from true to false; MCP tool \
         calls use it from the next request"
    );
    assert!(McpGuardrails::changes(&after, &after).is_empty());
}

// ---------------------------------------------------------------------------
// Unserved tools and refusal messages
// ---------------------------------------------------------------------------

#[test]
fn the_multi_cluster_tools_are_not_served() {
    for tool in ["list_clusters", "switch_cluster", "get_active_cluster"] {
        assert!(is_unserved(tool), "{tool}");
    }
    for tool in ["list_pods", "apply_manifest", "create_application"] {
        assert!(!is_unserved(tool), "{tool}");
    }
}

#[test]
fn a_policy_rejection_gets_the_deckwatch_setting() {
    let rejection =
        service_rejection(&WorkloadPolicy::default(), load_balancer()).expect("rejected");
    let message = with_setting_hint(rejection.clone());
    assert!(message.starts_with(&rejection), "keeps mcp-k8s's message");
    assert!(message.contains("--allow-loadbalancer"), "{message}");
    assert!(message.contains(SETTINGS_PATH), "{message}");
    assert!(
        message.contains(
            "allow_load_balancer is an exception to add (mcp_tuning.guardrails.exceptions)"
        ),
        "{message}"
    );
}

#[test]
fn each_policy_setting_gets_its_own_remedy() {
    let hint = |field: &str| {
        setting_hint(&format!(
            "rejected by the mcp-k8s workload policy (1 violation):\n- x: y; an operator can \
             allow this with --flag (env FLAG; library: WorkloadPolicy.{field})\nDon't work \
             around it."
        ))
        .expect("a hint")
    };
    assert!(hint("allowed_custom_resource_groups")
        .contains("mcp_tuning.guardrails.allowed_custom_resource_groups"));
    assert!(hint("allow_privileged_csr").contains("mcp_tuning.guardrails.allow_privileged_csr"));
    let capabilities = hint("allowed_capabilities");
    assert!(
        capabilities.contains("only the permissive policy"),
        "{capabilities}"
    );
    assert!(capabilities.contains("mcp_tuning.guardrails.workload_policy"));
}

#[test]
fn a_secret_decode_refusal_gets_the_deckwatch_setting() {
    let refusal = "secret decoding is disabled on this server, so get_secret can't return \
                   values. Call it without decode to see the keys. The operator can enable \
                   values with --enable-secret-decode.";
    let message = with_setting_hint(refusal.to_string());
    assert!(
        message.contains("mcp_tuning.guardrails.secret_decode_enabled"),
        "{message}"
    );
}

#[test]
fn other_errors_are_left_alone() {
    for error in ["namespace is required", "not found: pods \"web\""] {
        assert_eq!(with_setting_hint(error.to_string()), error);
    }
}

#[test]
fn the_apply_manifest_refusal_names_the_setting() {
    let message = apply_manifest_disabled();
    assert!(
        message.contains("mcp_tuning.guardrails.apply_manifest_enabled"),
        "{message}"
    );
    assert!(message.contains(SETTINGS_PATH), "{message}");
}
