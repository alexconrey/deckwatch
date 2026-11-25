//! SEC-22 through the real `/mcp` handler: one audit row per tool call
//! that writes, deletes or execs, or that a guardrail denied, and none for
//! reads; the client from `initialize`; cancelled calls; audit writes never
//! affecting a call; and the canary test proving no argument value outside
//! the allowlist is ever stored.

use std::collections::BTreeSet;
use std::time::Duration;

use axum::http::HeaderMap;
use mcp_k8s::telemetry::{semconv, ALLOWED_ARGUMENTS};
use sea_orm::{ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};

use super::tests::build_test_state;
use super::tool_telemetry_tests::{call, echo_client, listed_definitions, message, post, Canaries};
use super::*;
use crate::entities::audit_log;
use crate::handlers::mcp_telemetry::SESSION_HEADER;
use crate::test_tracing::capture;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A state whose Kubernetes API is `kube_client`, with a migrated SQLite
/// database in a temp dir and `allowed` as the namespace allowlist.
async fn state_with(kube_client: kube::Client, allowed: &[&str]) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = build_test_state().await;
    state.db = crate::db::connect(&format!(
        "sqlite://{}/audit.db?mode=rwc",
        dir.path().display()
    ))
    .await
    .unwrap();
    state.kube_client = kube_client;
    state.allowed_namespaces = allowed.iter().map(|s| s.to_string()).collect();
    (state, dir)
}

async fn mcp_rows(state: &AppState) -> Vec<audit_log::Model> {
    let mut rows = audit_log::Entity::find()
        .filter(audit_log::Column::Source.eq(crate::audit::SOURCE_MCP))
        .all(&state.db)
        .await
        .unwrap();
    rows.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    rows
}

/// The MCP rows once there are `n` (audit writes run in the background).
async fn wait_for_rows(state: &AppState, n: usize) -> Vec<audit_log::Model> {
    for _ in 0..500 {
        let rows = mcp_rows(state).await;
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {n} audit rows, got {:?}", mcp_rows(state).await);
}

/// Make `calls`, then a write as a sentinel, and return the rows the calls
/// wrote (writes land in order, so the sentinel's row comes after theirs).
async fn rows_of(state: &AppState, calls: Vec<Value>) -> Vec<audit_log::Model> {
    let before = mcp_rows(state).await.len();
    for c in calls {
        post(state, HeaderMap::new(), c).await;
    }
    post(
        state,
        HeaderMap::new(),
        call(
            "delete_pod",
            json!({"namespace": "sentinel", "name": "sentinel"}),
        ),
    )
    .await;
    wait_for_rows(state, before + 1).await;
    // Let any straggler land before counting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut rows = mcp_rows(state).await;
    let sentinel = rows
        .iter()
        .position(|r| r.namespace == "sentinel")
        .expect("sentinel row");
    rows.remove(sentinel);
    rows.split_off(before)
}

fn detail(row: &audit_log::Model) -> Value {
    serde_json::from_str(&row.detail).unwrap()
}

// ---------------------------------------------------------------------------
// What is audited
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writes_deletes_and_exec_are_audited_once_each() {
    let (state, _dir) = state_with(echo_client(), &[]).await;
    let manifest = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: prod\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n          env:\n            - name: TOKEN\n              value: s3cr3t-manifest\n";
    let rows = rows_of(
        &state,
        vec![
            call("apply_manifest", json!({ "manifest": manifest })),
            call(
                "exec_pod",
                json!({"namespace": "prod", "pod_name": "web-1", "container": "app", "command": "cat /run/secrets/s3cr3t-cmd"}),
            ),
            call("delete_namespace", json!({ "name": "staging" })),
            call("delete_deployment", json!({"namespace": "prod", "name": "web"})),
            call(
                "create_clusterrolebinding",
                json!({
                    "name": "ops-admin",
                    "role_ref": { "kind": "ClusterRole", "name": "cluster-admin" },
                    "subjects": [{ "kind": "User", "name": "s3cr3t-subject" }],
                }),
            ),
        ],
    )
    .await;

    let summary: Vec<(&str, &str, &str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r.tool.as_str(),
                r.action.as_str(),
                r.resource_type.as_str(),
                r.resource_name.as_str(),
                r.namespace.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("apply_manifest", "write", "deployment", "web", "prod"),
            ("exec_pod", "exec", "pod", "web-1", "prod"),
            ("delete_namespace", "delete", "namespace", "staging", ""),
            ("delete_deployment", "delete", "deployment", "web", "prod"),
            (
                "create_clusterrolebinding",
                "write",
                "clusterrolebinding",
                "ops-admin",
                ""
            ),
        ]
    );
    for row in &rows {
        // The mock API answers 404, so every call failed.
        assert_eq!(row.outcome, "error", "{row:?}");
        assert_eq!(row.source, "mcp");
        assert_eq!(row.user_identity, "");
        assert!(!format!("{row:?}").contains("s3cr3t"), "{row:?}");
    }
    let exec = detail(&rows[1]);
    assert_eq!(
        exec["argument_keys"],
        json!(["command", "container", "namespace", "pod_name"])
    );
    assert_eq!(
        exec["arguments"],
        json!({"namespace": "prod", "pod_name": "web-1", "container": "app"})
    );
    assert_eq!(detail(&rows[0])["api_version"], "apps/v1");
}

#[tokio::test]
async fn reads_are_not_audited() {
    let (state, _dir) = state_with(echo_client(), &[]).await;
    let rows = rows_of(
        &state,
        vec![
            call("list_pods", json!({"namespace": "prod"})),
            call(
                "get_deployment",
                json!({"namespace": "prod", "name": "web"}),
            ),
            call(
                "get_secret",
                json!({"namespace": "prod", "name": "db", "decode": true}),
            ),
            call(
                "get_pod_logs",
                json!({"namespace": "prod", "pod_name": "web-1"}),
            ),
            call("list_templates", json!({})),
            call("get_gitops", json!({"namespace": "prod", "name": "web"})),
            message("tools/list", json!({})),
        ],
    )
    .await;
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn guardrail_denials_are_audited_reads_too() {
    let (state, _dir) = state_with(echo_client(), &["team-a"]).await;
    let manifest =
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cfg\n  namespace: kube-system\n";
    let ((_, body), captured) = capture(post(
        &state,
        HeaderMap::new(),
        call(
            "delete_pod",
            json!({"namespace": "kube-system", "name": "etcd-0"}),
        ),
    ))
    .await;
    assert_eq!(
        body["error"]["message"],
        "Namespace 'kube-system' is not in the allowed list"
    );
    let span = &captured.spans_named(semconv::TARGET, semconv::SERVER_SPAN_NAME)[0];
    assert_eq!(span.fields[semconv::MCP_K8S_DECISION], "denied");
    assert_eq!(
        span.fields[semconv::MCP_K8S_DENIED_RULE],
        "namespace_allowlist"
    );

    rows_of(
        &state,
        vec![
            call("list_secrets", json!({"namespace": "kube-system"})),
            call("apply_manifest", json!({ "manifest": manifest })),
            // Allowed namespace: a read, not audited.
            call("list_secrets", json!({"namespace": "team-a"})),
        ],
    )
    .await;
    let rows: Vec<audit_log::Model> = mcp_rows(&state)
        .await
        .into_iter()
        .filter(|r| r.namespace != "sentinel")
        .collect();
    let summary: Vec<(&str, &str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r.tool.as_str(),
                r.action.as_str(),
                r.outcome.as_str(),
                r.namespace.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("delete_pod", "delete", "denied", "kube-system"),
            ("list_secrets", "read", "denied", "kube-system"),
            ("apply_manifest", "write", "denied", "kube-system"),
        ]
    );
    for row in &rows {
        assert_eq!(detail(row)["denied_rule"], "namespace_allowlist");
    }
}

#[tokio::test]
async fn the_row_carries_the_clients_self_reported_name() {
    let (state, _dir) = state_with(echo_client(), &[]).await;
    let (headers, _) = post(
        &state,
        HeaderMap::new(),
        message(
            "initialize",
            json!({"clientInfo": {"name": "claude-code", "version": "2.1.0"}}),
        ),
    )
    .await;
    let session = headers.get(SESSION_HEADER).unwrap().clone();
    let mut in_session = HeaderMap::new();
    in_session.insert(SESSION_HEADER, session.clone());
    post(
        &state,
        in_session,
        call(
            "scale_deployment",
            json!({"namespace": "prod", "name": "web", "replicas": 0}),
        ),
    )
    .await;
    let rows = wait_for_rows(&state, 1).await;
    assert_eq!(rows[0].client_name, "claude-code");
    let d = detail(&rows[0]);
    assert_eq!(
        d["client"],
        json!({"name": "claude-code", "version": "2.1.0", "self_reported": true})
    );
    assert_eq!(d["session_id"], session.to_str().unwrap());
    assert_eq!(d["arguments"]["replicas"], 0);
}

// ---------------------------------------------------------------------------
// One row per call, whatever happens
// ---------------------------------------------------------------------------

/// A Kubernetes API that accepts every write: POST/PUT/PATCH echo the object
/// back, GET answers 404, DELETE succeeds.
fn accepting_client() -> kube::Client {
    let service = tower::service_fn(|req: axum::http::Request<kube::client::Body>| async move {
        let method = req.method().clone();
        let body = req.into_body().collect_bytes().await.unwrap_or_default();
        let (status, body) = match method.as_str() {
            "POST" | "PUT" | "PATCH" => {
                let mut obj: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
                if let Some(meta) = obj.get_mut("metadata").and_then(Value::as_object_mut) {
                    meta.insert("uid".into(), json!("00000000-0000-0000-0000-000000000001"));
                    meta.insert("resourceVersion".into(), json!("1"));
                }
                (201, obj)
            }
            "DELETE" => (
                200,
                json!({"kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Success"}),
            ),
            _ => (
                404,
                json!({"kind": "Status", "apiVersion": "v1", "metadata": {},
                       "status": "Failure", "reason": "NotFound", "code": 404, "message": "not found"}),
            ),
        };
        Ok::<_, std::convert::Infallible>(
            axum::http::Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
    });
    kube::Client::new(service, "default")
}

#[tokio::test]
async fn a_tool_reusing_an_audited_handler_is_still_one_row() {
    // create_ingress reuses the REST handler, which writes its own row when
    // called from the UI.
    let (state, _dir) = state_with(accepting_client(), &[]).await;
    let (_, body) = post(
        &state,
        HeaderMap::new(),
        call(
            "create_ingress",
            json!({"namespace": "prod", "name": "web", "host": "web.example.com", "service_name": "web"}),
        ),
    )
    .await;
    assert!(
        body["result"].is_object(),
        "the tool should succeed: {body}"
    );
    let _ = wait_for_rows(&state, 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let all = audit_log::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(
        (
            all[0].source.as_str(),
            all[0].tool.as_str(),
            all[0].outcome.as_str()
        ),
        ("mcp", "create_ingress", "ok")
    );
}

/// A Kubernetes API that never answers.
fn hanging_client() -> kube::Client {
    let service = tower::service_fn(|_req: axum::http::Request<kube::client::Body>| {
        futures::future::pending::<
            Result<axum::http::Response<kube::client::Body>, std::convert::Infallible>,
        >()
    });
    kube::Client::new(service, "default")
}

#[tokio::test]
async fn a_call_dropped_midway_is_audited_as_cancelled() {
    let (state, _dir) = state_with(hanging_client(), &[]).await;
    let timed_out = tokio::time::timeout(
        Duration::from_millis(200),
        post(
            &state,
            HeaderMap::new(),
            call("delete_namespace", json!({"name": "staging"})),
        ),
    )
    .await;
    assert!(timed_out.is_err(), "the call should hang");
    let rows = wait_for_rows(&state, 1).await;
    assert_eq!(
        (rows[0].tool.as_str(), rows[0].outcome.as_str()),
        ("delete_namespace", "cancelled")
    );
    assert_eq!(detail(&rows[0])["outcome"], "cancelled");
}

#[tokio::test]
async fn a_failing_audit_write_never_affects_the_call() {
    let (state, _dir) = state_with(echo_client(), &[]).await;
    state
        .db
        .execute_unprepared("DROP TABLE audit_log")
        .await
        .unwrap();
    let (_, body) = post(
        &state,
        HeaderMap::new(),
        call("delete_namespace", json!({"name": "staging"})),
    )
    .await;
    // The tool's own result (the mock's 404), not a database error.
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("not found"), "{message}");
    let (_, body) = post(&state, HeaderMap::new(), call("list_templates", json!({}))).await;
    assert!(body["result"].is_object(), "{body}");
}

// ---------------------------------------------------------------------------
// Canary test
// ---------------------------------------------------------------------------

/// Every listed tool, deckwatch's and mcp-k8s's, with a unique marker in
/// every string argument: each write, delete or exec leaves exactly one
/// row, reads none, and no marker is stored unless mcp-k8s's allowlist (or
/// the deckwatch tool's declared target) permits it.
#[tokio::test]
async fn canary_no_argument_value_is_stored_and_each_call_is_one_row() {
    let (state, _dir) = state_with(echo_client(), &[]).await;
    let defs = listed_definitions(&state).await;
    let allowed: BTreeSet<&str> = ALLOWED_ARGUMENTS.iter().map(|(k, _)| *k).collect();
    let manifest_target = [
        "manifest.kind",
        "manifest.apiVersion",
        "manifest.metadata.name",
        "manifest.metadata.namespace",
    ];

    let mut failures = Vec::new();
    let mut expected_rows = 0;
    for (name, def) in &defs {
        let own = mcp_telemetry::deckwatch_tool(name);
        let action = own.map_or_else(|| ActionClass::for_tool(name), |t| t.action);
        let mut canaries = Canaries::default();
        let args = canaries.fill(&def["inputSchema"], "");
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            post(&state, HeaderMap::new(), call(name, args)),
        )
        .await;
        if outcome.is_err() && name != "watch_build" {
            failures.push(format!("{name}: timed out"));
        }

        let audited = action != ActionClass::Read;
        expected_rows += usize::from(audited);
        let rows = if audited {
            wait_for_rows(&state, expected_rows).await
        } else {
            mcp_rows(&state).await
        };
        let mine: Vec<&audit_log::Model> = rows.iter().filter(|r| r.tool == *name).collect();
        if mine.len() != usize::from(audited) {
            failures.push(format!("{name} ({action}): {} rows", mine.len()));
        }
        let Some(row) = mine.first() else { continue };

        let stored = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            row.action, row.resource_type, row.namespace, row.detail, row.client_name, row.tool
        );
        let target_args: Vec<&str> = own.map(|t| t.target.to_vec()).unwrap_or_default();
        for (marker, path) in &canaries.by_marker {
            // A manifest's kind, apiVersion, name and namespace are its
            // target (apply_manifest, create_custom_resource).
            let may_be_stored =
                allowed.contains(path.as_str()) || manifest_target.contains(&path.as_str());
            if stored.contains(marker.as_str()) && !may_be_stored {
                failures.push(format!("{name}: value of `{path}` stored"));
            }
            // A deckwatch tool's declared target goes only into resource_name.
            if row.resource_name.contains(marker.as_str())
                && !may_be_stored
                && !target_args.contains(&path.as_str())
            {
                failures.push(format!("{name}: value of `{path}` stored as the name"));
            }
        }
    }
    let total = mcp_rows(&state).await.len();
    if total != expected_rows {
        failures.push(format!("{total} rows, expected {expected_rows}"));
    }
    assert!(
        failures.is_empty(),
        "{} problems:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
