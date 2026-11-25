use super::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

#[test]
fn tool_table_has_no_duplicates() {
    let mut names: Vec<&str> = DECKWATCH_TOOLS.iter().map(|t| t.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "a tool is listed twice");
}

/// Where mcp-k8s knows a tool name, or its prefix classification applies
/// (`list_`, `get_`, `create_`, `update_`, `delete_`, …), deckwatch's class
/// must agree with it. Everything else has to be classified explicitly.
#[test]
fn classes_agree_with_mcp_k8s_where_it_has_one() {
    // Names mcp-k8s can't classify by prefix (it calls them reads).
    let explicit = [
        "set_gitops",
        "trigger_build",
        "enable_monitoring",
        "disable_monitoring",
        "enable_plugin",
        "disable_plugin",
        "validate_plugin",
        "associate_plugin",
        "disassociate_plugin",
        "provision_resource",
        "deprovision_resource",
        "refresh_resource",
        "submit_agent_feedback",
    ];
    for tool in DECKWATCH_TOOLS {
        if explicit.contains(&tool.name) {
            continue;
        }
        assert_eq!(
            tool.action,
            ActionClass::for_tool(tool.name),
            "{} disagrees with mcp-k8s's classification",
            tool.name
        );
    }
    // The explicit ones are exactly those mcp-k8s would get wrong: it falls
    // back to `read` for names it doesn't know.
    for name in explicit {
        let tool = deckwatch_tool(name).expect("listed in DECKWATCH_TOOLS");
        assert_eq!(ActionClass::for_tool(name), ActionClass::Read, "{name}");
        assert_ne!(tool.action, ActionClass::Read, "{name}");
    }
}

#[test]
fn intercepted_mcp_k8s_tools_are_known_to_mcp_k8s() {
    // create_, delete_, scale_ and restart_deployment used to be intercepted
    // only to write audit rows; the MCP choke point audits every call now.
    // create_pod was intercepted to add a service account, which mcp-k8s's
    // takes itself since 0.6.
    for name in [
        "create_deployment",
        "delete_deployment",
        "scale_deployment",
        "restart_deployment",
        "create_pod",
    ] {
        assert!(deckwatch_tool(name).is_none(), "{name}");
    }
    for name in [
        "update_deployment",
        "apply_manifest",
        "exec_pod",
        "create_ingress",
        "update_ingress",
    ] {
        assert!(deckwatch_tool(name).is_some(), "{name}");
        assert!(telemetry::is_known_tool(name), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Arguments for telemetry
// ---------------------------------------------------------------------------

#[test]
fn yaml_manifest_is_parsed_for_its_target_only() {
    let args = json!({
        "manifest": "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: prod\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n          env:\n            - name: TOKEN\n              value: s3cr3t\n",
    });
    let parsed = telemetry_arguments("apply_manifest", &args);
    let summary = telemetry::summarize("apply_manifest", &parsed);
    assert_eq!(summary.target.kind.as_deref(), Some("Deployment"));
    assert_eq!(summary.target.api_version.as_deref(), Some("apps/v1"));
    assert_eq!(summary.target.name.as_deref(), Some("web"));
    assert_eq!(summary.target.namespace.as_deref(), Some("prod"));
    assert_eq!(summary.argument_keys, ["manifest"]);
    let recorded = format!("{summary:?}");
    assert!(!recorded.contains("s3cr3t"), "{recorded}");
}

#[test]
fn other_arguments_are_left_alone() {
    let args = json!({ "manifest": "not: [valid" });
    assert!(matches!(
        telemetry_arguments("apply_manifest", &args),
        Cow::Borrowed(_)
    ));
    let args = json!({ "manifest": "kind: Deployment" });
    assert!(matches!(
        telemetry_arguments("create_custom_resource", &args),
        Cow::Borrowed(_)
    ));
}

// ---------------------------------------------------------------------------
// Metric labels
// ---------------------------------------------------------------------------

#[test]
fn metric_method_is_bounded() {
    assert_eq!(metric_method("tools/call"), "tools/call");
    assert_eq!(metric_method("initialize"), "initialize");
    assert_eq!(
        metric_method("notifications/initialized"),
        "notifications/initialized"
    );
    assert_eq!(metric_method("evil/unbounded-1234"), "_OTHER");
    assert_eq!(metric_method(""), "_OTHER");
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

fn client(name: &str) -> ClientInfo {
    ClientInfo::from_initialize(&json!({ "clientInfo": { "name": name, "version": "1.0" } }))
}

#[test]
fn sessions_remember_the_client() {
    let sessions = McpSessions::new(4, Duration::from_secs(60));
    let a = sessions.start(client("claude-code"));
    let b = sessions.start(client("cursor"));
    assert_ne!(a, b);
    assert_eq!(a.len(), 32, "a random UUID: {a}");
    assert_eq!(
        sessions.client(&a).and_then(|c| c.name).as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        sessions.client(&b).and_then(|c| c.name).as_deref(),
        Some("cursor")
    );
    assert!(sessions.client("not-issued").is_none());
}

#[test]
fn sessions_are_bounded_oldest_first() {
    let sessions = McpSessions::new(2, Duration::from_secs(60));
    let first = sessions.start(client("first"));
    let second = sessions.start(client("second"));
    // Using the first session makes the second the least recently seen.
    assert!(sessions.client(&first).is_some());
    let third = sessions.start(client("third"));
    assert!(sessions.client(&second).is_none(), "evicted");
    assert!(sessions.client(&first).is_some());
    assert!(sessions.client(&third).is_some());
}

#[test]
fn idle_sessions_expire() {
    let sessions = McpSessions::new(4, Duration::ZERO);
    let id = sessions.start(client("claude-code"));
    assert!(sessions.client(&id).is_none());
}

#[test]
fn request_context_knows_only_issued_sessions() {
    let id = sessions().start(client("claude-code"));
    let mut headers = HeaderMap::new();
    headers.insert(SESSION_HEADER, id.parse().unwrap());
    headers.insert("mcp-protocol-version", "2025-11-25".parse().unwrap());
    let ctx = RequestContext::from_headers(&headers);
    let session = ctx.session.expect("known session");
    assert_eq!(session.id, id);
    assert_eq!(session.client.name.as_deref(), Some("claude-code"));
    assert_eq!(ctx.protocol_version.as_deref(), Some("2025-11-25"));

    let mut headers = HeaderMap::new();
    headers.insert(SESSION_HEADER, "made-up".parse().unwrap());
    headers.insert("mcp-protocol-version", "latest".parse().unwrap());
    let ctx = RequestContext::from_headers(&headers);
    assert!(ctx.session.is_none());
    assert!(ctx.protocol_version.is_none());
}
