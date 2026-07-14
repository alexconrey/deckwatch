// Unit tests for src/db.rs

use super::*;
use sea_orm::entity::prelude::*;

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
