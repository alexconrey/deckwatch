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
