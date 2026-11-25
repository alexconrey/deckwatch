use super::*;

/// A migrated SQLite database in a temp dir (with `sqlite::memory:`, each
/// pooled connection would get its own empty database).
async fn file_db() -> (DatabaseConnection, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/audit.db?mode=rwc", dir.path().display());
    (crate::db::connect(&url).await.unwrap(), dir)
}

fn at(secs: i64) -> DateTimeUtc {
    DateTimeUtc::from_timestamp(secs, 0).unwrap()
}

const DAY: i64 = 24 * 60 * 60;
/// "Now" in these tests.
const NOW: i64 = 1_795_000_000;

async fn insert_at(db: &DatabaseConnection, id: &str, when: DateTimeUtc) {
    audit_entity::ActiveModel {
        id: Set(id.to_string()),
        timestamp: Set(when),
        action: Set("write".to_string()),
        resource_type: Set("deployment".to_string()),
        resource_name: Set(id.to_string()),
        namespace: Set("prod".to_string()),
        detail: Set(String::new()),
        user_identity: Set(String::new()),
        source: Set(SOURCE_MCP.to_string()),
        tool: Set("delete_deployment".to_string()),
        outcome: Set("ok".to_string()),
        client_name: Set(String::new()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn ids(db: &DatabaseConnection) -> Vec<String> {
    let mut ids: Vec<String> = audit_entity::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn retention_prunes_by_age() {
    let (db, _dir) = file_db().await;
    insert_at(&db, "a-400-days", at(NOW - 400 * DAY)).await;
    insert_at(&db, "b-30-days", at(NOW - 30 * DAY)).await;
    insert_at(&db, "c-today", at(NOW - 60)).await;
    let retention = Retention {
        max_age_days: 90,
        max_rows: 0,
    };
    assert_eq!(prune(&db, retention, at(NOW)).await.unwrap(), 1);
    assert_eq!(ids(&db).await, ["b-30-days", "c-today"]);
    // Nothing more to do.
    assert_eq!(prune(&db, retention, at(NOW)).await.unwrap(), 0);
}

#[tokio::test]
async fn retention_keeps_the_newest_rows() {
    let (db, _dir) = file_db().await;
    for i in 0..5 {
        insert_at(&db, &format!("row-{i}"), at(NOW - (5 - i) * 60)).await;
    }
    // Two rows at the same second: the id breaks the tie.
    insert_at(&db, "row-4b", at(NOW - 60)).await;
    let retention = Retention {
        max_age_days: 0,
        max_rows: 3,
    };
    assert_eq!(prune(&db, retention, at(NOW)).await.unwrap(), 3);
    assert_eq!(ids(&db).await, ["row-3", "row-4", "row-4b"]);
}

#[tokio::test]
async fn retention_applies_both_limits() {
    let (db, _dir) = file_db().await;
    insert_at(&db, "ancient", at(NOW - 1000 * DAY)).await;
    for i in 0..4 {
        insert_at(&db, &format!("recent-{i}"), at(NOW - (4 - i) * 60)).await;
    }
    let retention = Retention {
        max_age_days: 365,
        max_rows: 2,
    };
    assert_eq!(prune(&db, retention, at(NOW)).await.unwrap(), 3);
    assert_eq!(ids(&db).await, ["recent-2", "recent-3"]);
}

#[tokio::test]
async fn retention_off_keeps_everything() {
    let (db, _dir) = file_db().await;
    insert_at(&db, "ancient", at(NOW - 1000 * DAY)).await;
    let off = Retention {
        max_age_days: 0,
        max_rows: 0,
    };
    assert!(off.is_off());
    assert_eq!(prune(&db, off, at(NOW)).await.unwrap(), 0);
    assert_eq!(ids(&db).await, ["ancient"]);
}

#[tokio::test]
async fn rest_handlers_inside_an_mcp_call_add_no_row() {
    let (db, _dir) = file_db().await;
    log_action(&db, "create", "ingress", "web", "prod", "from the UI", "")
        .await
        .unwrap();
    within_mcp_call(async {
        log_action(
            &db,
            "create",
            "ingress",
            "web",
            "prod",
            "via an MCP tool",
            "",
        )
        .await
        .unwrap();
    })
    .await;
    let rows = audit_entity::Entity::find().all(&db).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].detail, "from the UI");
    assert_eq!(rows[0].source, SOURCE_API);
    assert_eq!(rows[0].tool, "");
    assert_eq!(rows[0].outcome, "");
}

#[tokio::test]
async fn mcp_rows_carry_their_source_tool_outcome_and_client() {
    let (db, _dir) = file_db().await;
    let row = McpCallRow {
        action: "delete".to_string(),
        resource_type: "namespace".to_string(),
        resource_name: "staging".to_string(),
        namespace: String::new(),
        detail: r#"{"tool":"delete_namespace"}"#.to_string(),
        tool: "delete_namespace".to_string(),
        outcome: "denied".to_string(),
        client_name: "claude-code".to_string(),
    };
    insert_mcp_call(&db, &row).await.unwrap();
    let rows = audit_entity::Entity::find().all(&db).await.unwrap();
    assert_eq!(rows.len(), 1);
    let m = &rows[0];
    assert_eq!(
        (m.source.as_str(), m.tool.as_str(), m.outcome.as_str()),
        (SOURCE_MCP, "delete_namespace", "denied")
    );
    assert_eq!(m.client_name, "claude-code");
    assert_eq!(m.action, "delete");
    assert_eq!(
        m.user_identity, "",
        "no identity until MCP clients authenticate"
    );
}

#[tokio::test]
async fn the_migration_adds_the_columns_and_marks_old_mcp_rows() {
    use sea_orm::{ConnectionTrait, Database, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::migrations::Migrator;

    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/audit.db?mode=rwc", dir.path().display());
    let db = Database::connect(&url).await.unwrap();
    // Every migration before this one (later ones may exist).
    let before = Migrator::migrations()
        .iter()
        .position(|m| m.name() == "m20261124_000003_audit_log_mcp")
        .expect("the audit log migration") as u32;
    Migrator::up(&db, Some(before)).await.unwrap();
    for (id, detail) in [
        ("old-mcp", "MCP: created application 'web'"),
        ("old-ui", "settings saved"),
    ] {
        db.execute(Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO audit_log (id, timestamp, action, resource_type, resource_name, \
             namespace, detail, user_identity) VALUES (?, ?, 'create', 'application', 'web', \
             'prod', ?, '')",
            [id.into(), at(NOW).into(), detail.into()],
        ))
        .await
        .unwrap();
    }

    Migrator::up(&db, None).await.unwrap();

    let mut rows: Vec<(String, String, String)> = audit_entity::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.source, m.outcome))
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            ("old-mcp".to_string(), SOURCE_MCP.to_string(), String::new()),
            ("old-ui".to_string(), SOURCE_API.to_string(), String::new()),
        ]
    );
}

// ---------------------------------------------------------------------------
// GET /api/audit
// ---------------------------------------------------------------------------

/// Rows for the query tests: two UI rows and four MCP rows, a minute apart.
async fn query_state() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let rows = [
        ("a", SOURCE_API, "create", "", "", ""),
        (
            "b",
            SOURCE_MCP,
            "write",
            "apply_manifest",
            "ok",
            "claude-code",
        ),
        (
            "c",
            SOURCE_MCP,
            "delete",
            "delete_pod",
            "denied",
            "claude-code",
        ),
        ("d", SOURCE_API, "update", "", "", ""),
        ("e", SOURCE_MCP, "exec", "exec_pod", "error", "cursor"),
        ("f", SOURCE_MCP, "read", "list_secrets", "denied", "cursor"),
    ];
    for (i, (id, source, action, tool, outcome, client)) in rows.into_iter().enumerate() {
        audit_entity::ActiveModel {
            id: Set(id.to_string()),
            timestamp: Set(at(NOW + 60 * i as i64)),
            action: Set(action.to_string()),
            resource_type: Set("deployment".to_string()),
            resource_name: Set(id.to_string()),
            namespace: Set("prod".to_string()),
            detail: Set(String::new()),
            user_identity: Set(String::new()),
            source: Set(source.to_string()),
            tool: Set(tool.to_string()),
            outcome: Set(outcome.to_string()),
            client_name: Set(client.to_string()),
        }
        .insert(&state.db)
        .await
        .unwrap();
    }
    (state, dir)
}

async fn list(state: &AppState, q: AuditQuery) -> Result<AuditResponse, AppError> {
    list_audit_logs(State(state.clone()), Query(q))
        .await
        .map(|Json(r)| r)
}

fn names(r: &AuditResponse) -> Vec<&str> {
    r.items.iter().map(|e| e.resource_name.as_str()).collect()
}

#[tokio::test]
async fn audit_rows_are_newest_first_by_default() {
    let (state, _dir) = query_state().await;
    let r = list(&state, AuditQuery::default()).await.unwrap();
    assert_eq!(names(&r), ["f", "e", "d", "c", "b", "a"]);
    assert_eq!(r.next_cursor, None);
    let r = list(
        &state,
        AuditQuery {
            order: Some("asc".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(names(&r), ["a", "b", "c", "d", "e", "f"]);
}

#[tokio::test]
async fn audit_rows_filter_by_source_action_client_outcome_and_tool() {
    let (state, _dir) = query_state().await;
    let q = |f: fn(&mut AuditQuery)| {
        let mut q = AuditQuery::default();
        f(&mut q);
        q
    };
    let cases: Vec<(AuditQuery, Vec<&str>)> = vec![
        (
            q(|q| q.source = Some("mcp".into())),
            vec!["f", "e", "c", "b"],
        ),
        (q(|q| q.source = Some("api".into())), vec!["d", "a"]),
        (q(|q| q.outcome = Some("denied".into())), vec!["f", "c"]),
        (q(|q| q.client = Some("cursor".into())), vec!["f", "e"]),
        (q(|q| q.action = Some("exec".into())), vec!["e"]),
        (q(|q| q.tool = Some("apply_manifest".into())), vec!["b"]),
        (
            q(|q| {
                q.source = Some("mcp".into());
                q.outcome = Some("denied".into());
                q.client = Some("claude-code".into());
            }),
            vec!["c"],
        ),
    ];
    for (query, expected) in cases {
        let label = format!("{query:?}");
        let r = list(&state, query).await.unwrap();
        assert_eq!(names(&r), expected, "{label}");
    }
}

#[tokio::test]
async fn audit_rows_page_with_a_cursor_and_start_at_since() {
    let (state, _dir) = query_state().await;
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let r = list(
            &state,
            AuditQuery {
                limit: Some(4),
                cursor: cursor.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        seen.extend(names(&r).into_iter().map(str::to_string));
        match r.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, ["f", "e", "d", "c", "b", "a"]);

    let since = at(NOW + 180).to_rfc3339();
    let r = list(
        &state,
        AuditQuery {
            since: Some(since),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(names(&r), ["f", "e", "d"]);
}

#[tokio::test]
async fn bad_audit_queries_are_rejected() {
    let (state, _dir) = query_state().await;
    for q in [
        AuditQuery {
            order: Some("sideways".into()),
            ..Default::default()
        },
        AuditQuery {
            cursor: Some("not-a-cursor".into()),
            ..Default::default()
        },
        AuditQuery {
            since: Some("yesterday".into()),
            ..Default::default()
        },
    ] {
        let label = format!("{q:?}");
        assert!(
            matches!(list(&state, q).await, Err(AppError::BadRequest(_))),
            "{label}"
        );
    }
}
