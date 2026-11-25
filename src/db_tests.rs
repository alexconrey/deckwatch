// Unit tests for src/db.rs

use super::*;

#[test]
fn test_backend_name_sqlite() {
    assert_eq!(backend_name("sqlite::memory:"), "sqlite");
    assert_eq!(
        backend_name("sqlite:///data/deckwatch.db?mode=rwc"),
        "sqlite"
    );
}

#[test]
fn test_backend_name_postgres() {
    assert_eq!(
        backend_name("postgres://user:pass@host:5432/db"),
        "postgres"
    );
}

#[test]
fn test_backend_name_mysql() {
    assert_eq!(backend_name("mysql://user:pass@host:3306/db"), "mysql");
}

#[test]
fn test_backend_name_unknown() {
    assert_eq!(backend_name("redis://localhost:6379"), "unknown");
    assert_eq!(backend_name(""), "unknown");
}

#[tokio::test]
async fn test_connect_and_migrate() {
    let db = connect("sqlite::memory:").await.expect("connect failed");
    // Verify the connection is usable by checking the backend.
    assert!(matches!(
        db.get_database_backend(),
        sea_orm::DatabaseBackend::Sqlite
    ));
}

#[tokio::test]
async fn test_ensure_application_creates_row() {
    let db = connect("sqlite::memory:").await.expect("connect failed");
    let app_id = ensure_application(&db, "prod", "api-server")
        .await
        .expect("ensure_application failed");
    assert_eq!(app_id, "prod/api-server");

    // Verify the row actually exists.
    let row = crate::entities::applications::Entity::find_by_id(&app_id)
        .one(&db)
        .await
        .expect("query failed");
    assert!(row.is_some());
    let row = row.unwrap();
    assert_eq!(row.name, "api-server");
    assert_eq!(row.namespace, "prod");
}

#[tokio::test]
async fn test_ensure_application_idempotent() {
    let db = connect("sqlite::memory:").await.expect("connect failed");
    let id1 = ensure_application(&db, "prod", "api-server")
        .await
        .expect("first call failed");
    let id2 = ensure_application(&db, "prod", "api-server")
        .await
        .expect("second call failed");
    assert_eq!(id1, id2);

    // Verify only one row exists.
    let count = crate::entities::applications::Entity::find()
        .count(&db)
        .await
        .expect("count failed");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn gitops_token_binding_migration_binds_existing_tokens_to_the_default() {
    use sea_orm::{ConnectionTrait, Database, Statement};
    use sea_orm_migration::MigratorTrait;

    let db = Database::connect("sqlite::memory:").await.expect("connect");
    // Every migration before the token binding one (later ones may exist).
    let before = Migrator::migrations()
        .iter()
        .position(|m| m.name() == "m20261124_000001_gitops_token_binding")
        .expect("token binding migration");
    Migrator::up(&db, Some(before as u32))
        .await
        .expect("migrate to the previous version");

    let backend = db.get_database_backend();
    db.execute(Statement::from_string(
        backend,
        "INSERT INTO applications (id, name, namespace, description, team, created_at, updated_at) \
         VALUES ('prod/api', 'api', 'prod', '', '', '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
    ))
    .await
    .expect("insert application");
    for (id, token) in [("with-token", "'ciphertext'"), ("without-token", "NULL")] {
        db.execute(Statement::from_string(
            backend,
            format!(
                "INSERT INTO gitops_configs (id, application_id, repo_url, branch, token_secret, \
                 encrypted_token, git_auth_user, dockerfile_path, docker_context, oci_repository, \
                 include_paths, exclude_paths, poll_interval_seconds, webhook_enabled, created_at, \
                 updated_at) VALUES ('{id}', 'prod/api', 'https://github.com/o/r', 'main', '', \
                 {token}, '', 'Dockerfile', '.', 'r', '', '', 60, false, '2026-01-01 00:00:00', \
                 '2026-01-01 00:00:00')"
            ),
        ))
        .await
        .expect("insert gitops config");
    }

    Migrator::up(&db, None).await.expect("migrate");

    let rows = crate::entities::gitops_configs::Entity::find()
        .all(&db)
        .await
        .expect("query");
    let binding = |id: &str| {
        rows.iter()
            .find(|r| r.id == id)
            .expect("row")
            .token_binding
            .clone()
    };
    let bound = binding("with-token").expect("existing token is bound");
    let parsed: crate::outbound::EndpointBinding =
        serde_json::from_str(&bound).expect("binding JSON");
    assert_eq!(
        parsed,
        crate::outbound::EndpointBinding::defaults(&[crate::outbound::EndpointId::GithubApi])
    );
    assert_eq!(binding("without-token"), None);
}

#[tokio::test]
async fn plugin_status_migration_creates_the_table_and_rolls_back() {
    use sea_orm::{ConnectionTrait, Database, Statement};
    use sea_orm_migration::MigratorTrait;

    let db = Database::connect("sqlite::memory:").await.expect("connect");
    Migrator::up(&db, None).await.expect("migrate");
    let backend = db.get_database_backend();
    let count = |db: &sea_orm::DatabaseConnection| {
        let db = db.clone();
        async move {
            db.query_one(Statement::from_string(
                backend,
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table' AND name = 'plugin_status'",
            ))
            .await
            .expect("query")
            .expect("row")
            .try_get::<i64>("", "n")
            .expect("n")
        }
    };
    assert_eq!(count(&db).await, 1);
    assert!(crate::entities::plugin_status::Entity::find()
        .all(&db)
        .await
        .expect("entity matches the table")
        .is_empty());

    // Rolling back the plugin status migration drops only its table.
    let after = Migrator::migrations()
        .iter()
        .position(|m| m.name() == "m20261124_000002_plugin_status")
        .expect("plugin status migration");
    let later = Migrator::migrations().len() - after;
    Migrator::down(&db, Some(later as u32))
        .await
        .expect("roll back");
    assert_eq!(count(&db).await, 0);
    Migrator::up(&db, None).await.expect("migrate again");
    assert_eq!(count(&db).await, 1);
}

// ---------------------------------------------------------------------------
// The 0.7.0 migrations survive a crash (src/migrations/schema.rs)
// ---------------------------------------------------------------------------

/// The migrations added in 0.7.0.
const MIGRATIONS_0_7_0: [&str; 3] = [
    "m20261124_000001_gitops_token_binding",
    "m20261124_000002_plugin_status",
    "m20261124_000003_audit_log_mcp",
];

/// A fresh SQLite database in a file under `dir` (with `sqlite::memory:`,
/// each pooled connection would get a database of its own).
async fn file_db(dir: &std::path::Path) -> sea_orm::DatabaseConnection {
    let url = format!("sqlite://{}?mode=rwc", dir.join("crash.db").display());
    sea_orm::Database::connect(url).await.expect("connect")
}

/// Apply every migration before the 0.7.0 ones.
async fn migrate_to_0_6(db: &sea_orm::DatabaseConnection) {
    use sea_orm_migration::MigratorTrait;
    let before = Migrator::migrations()
        .iter()
        .position(|m| m.name() == MIGRATIONS_0_7_0[0])
        .expect("first 0.7.0 migration");
    Migrator::up(db, Some(before as u32))
        .await
        .expect("migrate to 0.6");
}

async fn recorded_0_7_0(db: &sea_orm::DatabaseConnection) -> usize {
    use sea_orm::{ConnectionTrait, Statement};
    let sql = format!(
        "SELECT COUNT(*) AS n FROM seaql_migrations WHERE version IN ('{}')",
        MIGRATIONS_0_7_0.join("', '")
    );
    db.query_one(Statement::from_string(db.get_database_backend(), sql))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n") as usize
}

/// Every column and index the 0.7.0 migrations add exists, and the entities
/// read their tables.
async fn assert_0_7_0_schema(db: &sea_orm::DatabaseConnection) {
    let manager = sea_orm_migration::SchemaManager::new(db);
    for (table, column) in [
        ("gitops_configs", "token_binding"),
        ("audit_log", "source"),
        ("audit_log", "tool"),
        ("audit_log", "outcome"),
        ("audit_log", "client_name"),
    ] {
        assert!(
            crate::migrations::schema::has_column(&manager, table, column)
                .await
                .expect("column check"),
            "{table}.{column}"
        );
    }
    for (table, index) in [
        ("plugin_status", "idx_plugin_status_target"),
        ("plugin_status", "idx_plugin_status_health"),
        ("audit_log", "idx_audit_log_timestamp"),
        ("audit_log", "idx_audit_log_source"),
    ] {
        assert!(
            crate::migrations::schema::has_index(&manager, table, index)
                .await
                .expect("index check"),
            "{index} on {table}"
        );
    }
    crate::entities::gitops_configs::Entity::find()
        .all(db)
        .await
        .expect("gitops_configs");
    crate::entities::plugin_status::Entity::find()
        .all(db)
        .await
        .expect("plugin_status");
    crate::entities::audit_log::Entity::find()
        .all(db)
        .await
        .expect("audit_log");
}

#[tokio::test]
async fn schema_checks_see_columns_and_indexes() {
    use sea_orm_migration::MigratorTrait;
    let dir = tempfile::tempdir().unwrap();
    let db = file_db(dir.path()).await;
    Migrator::up(&db, None).await.expect("migrate");
    let manager = sea_orm_migration::SchemaManager::new(&db);
    use crate::migrations::schema::{has_column, has_index};
    assert!(has_column(&manager, "audit_log", "outcome").await.unwrap());
    assert!(!has_column(&manager, "audit_log", "nope").await.unwrap());
    assert!(!has_column(&manager, "no_table", "outcome").await.unwrap());
    assert!(has_index(&manager, "audit_log", "idx_audit_log_source")
        .await
        .unwrap());
    assert!(
        !has_index(&manager, "audit_log", "idx_plugin_status_health")
            .await
            .unwrap()
    );
}

/// A crash after a migration ran but before `seaql_migrations` recorded it
/// (SQLite and MySQL don't wrap the two in a transaction) leaves its schema
/// changes in place; the next start runs it again. A rollback that deletes
/// only those records leaves the same state.
#[tokio::test]
async fn the_0_7_0_migrations_run_again_when_their_records_are_lost() {
    let dir = tempfile::tempdir().unwrap();
    records_lost(&file_db(dir.path()).await).await;
}

/// The scenario of the test above, on an empty database `db`.
async fn records_lost(db: &sea_orm::DatabaseConnection) {
    use sea_orm::{ConnectionTrait, Statement};
    use sea_orm_migration::MigratorTrait;

    Migrator::up(db, None).await.expect("migrate");
    let backend = db.get_database_backend();

    // A token saved under 0.7.0 with its own binding, and one without.
    crate::db::ensure_application(db, "prod", "api")
        .await
        .unwrap();
    for (id, token, binding) in [
        (
            "bound",
            "'ciphertext'",
            r#"'{"github_api":"https://ghe.example"}'"#,
        ),
        ("unbound", "'ciphertext'", "NULL"),
    ] {
        db.execute(Statement::from_string(
            backend,
            format!(
                "INSERT INTO gitops_configs (id, application_id, repo_url, branch, token_secret, \
                 encrypted_token, token_binding, git_auth_user, dockerfile_path, docker_context, \
                 oci_repository, include_paths, exclude_paths, poll_interval_seconds, \
                 webhook_enabled, created_at, updated_at) VALUES ('{id}', 'prod/api', \
                 'https://github.com/o/r', 'main', '', {token}, {binding}, '', 'Dockerfile', '.', \
                 'r', '', '', 60, false, '2026-01-01 00:00:00', '2026-01-01 00:00:00')"
            ),
        ))
        .await
        .expect("insert gitops config");
    }

    db.execute(Statement::from_string(
        backend,
        format!(
            "DELETE FROM seaql_migrations WHERE version IN ('{}')",
            MIGRATIONS_0_7_0.join("', '")
        ),
    ))
    .await
    .expect("forget the 0.7.0 records");
    assert_eq!(recorded_0_7_0(db).await, 0);

    Migrator::up(db, None)
        .await
        .expect("the 0.7.0 migrations run again");
    assert_eq!(recorded_0_7_0(db).await, 3);
    assert_0_7_0_schema(db).await;

    let rows = crate::entities::gitops_configs::Entity::find()
        .all(db)
        .await
        .unwrap();
    let binding = |id: &str| {
        rows.iter()
            .find(|r| r.id == id)
            .unwrap()
            .token_binding
            .clone()
    };
    // An existing binding is kept; a token without one gets the default.
    assert_eq!(
        binding("bound").as_deref(),
        Some(r#"{"github_api":"https://ghe.example"}"#)
    );
    assert_eq!(
        binding("unbound").as_deref(),
        Some(r#"{"github_api":"https://api.github.com"}"#)
    );
}

/// A crash part-way through: some of each migration's statements ran, none
/// was recorded.
#[tokio::test]
async fn the_0_7_0_migrations_finish_after_a_crash_halfway_through() {
    let dir = tempfile::tempdir().unwrap();
    crash_halfway(&file_db(dir.path()).await).await;
}

/// The scenario of the test above, on an empty database `db`.
async fn crash_halfway(db: &sea_orm::DatabaseConnection) {
    use sea_orm_migration::prelude::*;
    use sea_orm_migration::MigratorTrait;

    migrate_to_0_6(db).await;
    let manager = SchemaManager::new(db);

    // m20261124_000001: the column, but not the bindings.
    manager
        .alter_table(
            Table::alter()
                .table(Alias::new("gitops_configs"))
                .add_column(ColumnDef::new(Alias::new("token_binding")).text().null())
                .to_owned(),
        )
        .await
        .unwrap();
    // m20261124_000002: the table and its first index, then the crash. The
    // migration really ran, so take its table from the migrator and drop the
    // second index again.
    Migrator::up(db, Some(2)).await.expect("000001 and 000002");
    manager
        .drop_index(
            Index::drop()
                .name("idx_plugin_status_health")
                .table(Alias::new("plugin_status"))
                .to_owned(),
        )
        .await
        .unwrap();
    // m20261124_000003: two of the four columns and one index.
    for (column, default) in [("source", "api"), ("tool", "")] {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("audit_log"))
                    .add_column(
                        ColumnDef::new(Alias::new(column))
                            .string()
                            .not_null()
                            .default(default),
                    )
                    .to_owned(),
            )
            .await
            .unwrap();
    }
    manager
        .create_index(
            Index::create()
                .name("idx_audit_log_timestamp")
                .table(Alias::new("audit_log"))
                .col(Alias::new("timestamp"))
                .to_owned(),
        )
        .await
        .unwrap();
    // None of it recorded.
    {
        use sea_orm::{ConnectionTrait, Statement};
        db.execute(Statement::from_string(
            db.get_database_backend(),
            format!(
                "DELETE FROM seaql_migrations WHERE version IN ('{}')",
                MIGRATIONS_0_7_0.join("', '")
            ),
        ))
        .await
        .unwrap();
    }

    Migrator::up(db, None)
        .await
        .expect("the migrations finish what the crash left");
    assert_eq!(recorded_0_7_0(db).await, 3);
    assert_0_7_0_schema(db).await;
}

/// `applications.observability` (#398): added as a nullable column, NULL
/// reads as injection off, the migration runs again after a crash (its
/// record lost, the column there), and rolling it back drops the column.
#[tokio::test]
async fn application_observability_migration_adds_a_nullable_column() {
    use sea_orm::{ConnectionTrait, Statement};
    use sea_orm_migration::MigratorTrait;

    const NAME: &str = "m20261125_000001_application_observability";
    let dir = tempfile::tempdir().unwrap();
    let db = file_db(dir.path()).await;
    Migrator::up(&db, None).await.expect("migrate");
    let has = |db: &sea_orm::DatabaseConnection| {
        let db = db.clone();
        async move {
            let manager = sea_orm_migration::SchemaManager::new(&db);
            crate::migrations::schema::has_column(&manager, "applications", "observability")
                .await
                .expect("column check")
        }
    };
    assert!(has(&db).await);
    ensure_application(&db, "team", "shop").await.unwrap();
    let row = applications::Entity::find_by_id("team/shop".to_string())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.observability, None);
    assert_eq!(
        crate::otel_inject::ApplicationObservability::from_column(row.observability.as_deref())
            .unwrap(),
        crate::otel_inject::ApplicationObservability::default()
    );

    // A crash between the ALTER and its record: the next start runs it again.
    db.execute(Statement::from_string(
        db.get_database_backend(),
        format!("DELETE FROM seaql_migrations WHERE version = '{NAME}'"),
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.expect("migrate again");
    assert!(has(&db).await);

    let position = Migrator::migrations()
        .iter()
        .position(|m| m.name() == NAME)
        .expect("the migration is registered");
    let later = Migrator::migrations().len() - position;
    Migrator::down(&db, Some(later as u32))
        .await
        .expect("roll back");
    assert!(!has(&db).await);
    Migrator::up(&db, None)
        .await
        .expect("migrate after rollback");
    assert!(has(&db).await);
}
