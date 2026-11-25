use super::*;
use crate::handlers::mcp_telemetry::{deckwatch_tool, Session};
use mcp_k8s::telemetry::summarize;
use serde_json::json;

fn ctx_with_client() -> RequestContext {
    RequestContext {
        session: Some(Session {
            id: "0123456789abcdef0123456789abcdef".to_string(),
            client: ClientInfo::from_initialize(&json!({
                "clientInfo": { "name": "claude-code", "version": "2.1.0" }
            })),
        }),
        trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string()),
        ..Default::default()
    }
}

fn record(tool: &str, args: &Value, ctx: &RequestContext) -> CallRecord {
    let own = deckwatch_tool(tool);
    let action = own.map_or_else(|| ActionClass::for_tool(tool), |t| t.action);
    let args_for_summary = crate::handlers::mcp_telemetry::telemetry_arguments(tool, args);
    CallRecord::new(
        tool,
        own,
        action,
        &summarize(tool, &args_for_summary),
        args,
        ctx,
    )
}

fn detail(row: &McpCallRow) -> Value {
    serde_json::from_str(&row.detail).unwrap()
}

#[test]
fn reads_are_audited_only_when_denied() {
    assert!(!audited(ActionClass::Read, false));
    assert!(audited(ActionClass::Read, true));
    for action in [ActionClass::Write, ActionClass::Delete, ActionClass::Exec] {
        assert!(audited(action, false), "{action}");
    }
}

#[test]
fn a_row_has_the_target_keys_allowlisted_values_and_client() {
    let args = json!({
        "namespace": "prod",
        "name": "web",
        "image": "nginx:1.27",
        "env": { "TOKEN": "s3cr3t" },
        "command": ["sh", "-c", "echo s3cr3t"],
    });
    let row = record("create_deployment", &args, &ctx_with_client())
        .row(Outcome::Ok, Duration::from_millis(12));
    assert_eq!(row.action, "write");
    assert_eq!(row.resource_type, "deployment");
    assert_eq!(row.resource_name, "web");
    assert_eq!(row.namespace, "prod");
    assert_eq!(row.tool, "create_deployment");
    assert_eq!(row.outcome, "ok");
    assert_eq!(row.client_name, "claude-code");
    assert!(!format!("{row:?}").contains("s3cr3t"), "{row:?}");
    assert_eq!(
        detail(&row),
        json!({
            "tool": "create_deployment",
            "outcome": "ok",
            // mcp-k8s's create_deployment declares no `command`: an
            // undeclared key is only counted, as `?`.
            "argument_keys": ["env", "image", "name", "namespace", "?"],
            "arguments": { "namespace": "prod", "name": "web", "image": "nginx:1.27" },
            "client": { "name": "claude-code", "version": "2.1.0", "self_reported": true },
            "session_id": "0123456789abcdef0123456789abcdef",
            "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
            "duration_ms": 12,
        })
    );
}

#[test]
fn a_denied_row_names_the_rule() {
    let mut r = record(
        "delete_pod",
        &json!({ "namespace": "kube-system", "name": "etcd-0" }),
        &RequestContext::default(),
    );
    r.deny("namespace_allowlist");
    let row = r.row(Outcome::Denied, Duration::ZERO);
    assert_eq!(
        (row.action.as_str(), row.outcome.as_str()),
        ("delete", "denied")
    );
    let d = detail(&row);
    assert_eq!(d["denied_rule"], "namespace_allowlist");
    assert!(d.get("client").is_none(), "no session, no client: {d}");
    assert!(d.get("session_id").is_none());
    assert!(d.get("trace_id").is_none());
    assert_eq!(row.client_name, "");
}

#[test]
fn an_apply_manifest_row_takes_its_target_from_the_yaml() {
    let args = json!({
        "manifest": "apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRoleBinding\nmetadata:\n  name: ops-admin\nsubjects:\n  - kind: User\n    name: mallory\nroleRef:\n  kind: ClusterRole\n  name: cluster-admin\n",
    });
    let row = record("apply_manifest", &args, &RequestContext::default())
        .row(Outcome::Error, Duration::ZERO);
    assert_eq!(row.resource_type, "clusterrolebinding");
    assert_eq!(row.resource_name, "ops-admin");
    assert_eq!(row.namespace, "");
    let d = detail(&row);
    assert_eq!(d["api_version"], "rbac.authorization.k8s.io/v1");
    assert_eq!(d["argument_keys"], json!(["manifest"]));
    assert_eq!(d["arguments"], json!({}));
    assert!(!row.detail.contains("mallory"), "{}", row.detail);
}

#[test]
fn deckwatch_tools_name_their_target_from_their_own_arguments() {
    let ctx = RequestContext::default();
    let row = record(
        "set_gitops",
        &json!({
            "namespace": "prod",
            "deployment_name": "web",
            "repo_url": "https://github.com/org/repo",
            "token": "ghp_secret",
        }),
        &ctx,
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert_eq!(row.resource_type, "gitops");
    assert_eq!(row.resource_name, "web");
    assert_eq!(
        detail(&row)["argument_keys"],
        json!(["deployment_name", "namespace", "repo_url", "token"])
    );
    assert!(!row.detail.contains("ghp_secret"));
    assert!(!row.detail.contains("github.com"));

    let row = record(
        "deprovision_resource",
        &json!({
            "namespace": "prod",
            "application_name": "shop",
            "plugin_name": "aws",
            "resource_id": "rds-postgres",
        }),
        &ctx,
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert_eq!(
        (row.action.as_str(), row.resource_type.as_str()),
        ("delete", "plugin-resource")
    );
    assert_eq!(row.resource_name, "shop/aws/rds-postgres");
}

#[test]
fn intercepted_tools_use_deckwatchs_schema_for_argument_keys() {
    let ctx = RequestContext::default();
    // mcp-k8s's exec_pod has `name`, deckwatch's has `pod_name`.
    let row = record(
        "exec_pod",
        &json!({ "namespace": "prod", "pod_name": "web-1", "command": "cat /etc/shadow" }),
        &ctx,
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert_eq!(row.action, "exec");
    assert_eq!(row.resource_type, "pod");
    assert_eq!(row.resource_name, "web-1");
    assert_eq!(
        detail(&row)["argument_keys"],
        json!(["command", "namespace", "pod_name"])
    );
    assert!(!row.detail.contains("shadow"));

    // update_deployment's service_account is deckwatch's.
    let row = record(
        "update_deployment",
        &json!({ "namespace": "prod", "name": "web", "service_account": "admin", "x-extra": 1 }),
        &ctx,
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert_eq!(
        detail(&row)["argument_keys"],
        json!(["name", "namespace", "service_account", "?"])
    );
    assert!(!row.detail.contains("admin"));
}

#[test]
fn mcp_k8s_tools_name_their_resource_from_the_tool() {
    for (tool, resource) in [
        ("create_clusterrolebinding", "clusterrolebinding"),
        ("delete_namespace", "namespace"),
        ("drain_node", "node"),
        ("evict_pod", "pod"),
        ("approve_csr", "csr"),
        ("port_forward_check", "port_forward_check"),
    ] {
        assert_eq!(resource_from_tool(tool), resource);
    }
    // An explicit kind wins (custom resources).
    let row = record(
        "delete_custom_resource",
        &json!({ "group": "cert-manager.io", "version": "v1", "kind": "Certificate", "namespace": "prod", "name": "tls" }),
        &RequestContext::default(),
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert_eq!(row.resource_type, "certificate");
    assert_eq!(detail(&row)["api_version"], "cert-manager.io/v1");
}

#[test]
fn target_values_are_clipped_and_cleaned() {
    let long = "a".repeat(1000);
    let row = record(
        "set_gitops",
        &json!({ "namespace": "prod", "deployment_name": format!("web\n{long}") }),
        &RequestContext::default(),
    )
    .row(Outcome::Ok, Duration::ZERO);
    assert!(!row.resource_name.contains('\n'));
    assert_eq!(row.resource_name.len(), MAX_VALUE_LEN);
}
