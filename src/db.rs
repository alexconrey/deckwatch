use std::time::SystemTime;

use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue::Set, ConnectOptions, Database, DatabaseConnection, DbErr};
use sea_orm_migration::MigratorTrait;

use crate::entities::applications;
use crate::migrations::Migrator;

fn now_utc() -> DateTimeUtc {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

/// Connect to the database and run all pending migrations.
///
/// The URL scheme determines the backend:
///   - `sqlite://`  -> SQLite  (use `?mode=rwc` to auto-create the file)
///   - `postgres://` -> PostgreSQL
///   - `mysql://`    -> MySQL
pub async fn connect(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut opts = ConnectOptions::new(database_url);
    opts.max_connections(10)
        .min_connections(1)
        .sqlx_logging(false);

    let db = Database::connect(opts).await?;

    // Run all pending migrations automatically on startup.
    Migrator::up(&db, None).await?;

    Ok(db)
}

/// Ensure an `applications` row exists for the given namespace/name pair.
/// Uses `{ns}/{name}` as the primary key. Inserts a minimal row if missing,
/// no-ops if it already exists. Call this before inserting into any table
/// that has a FK to `applications`.
pub async fn ensure_application(
    db: &DatabaseConnection,
    ns: &str,
    name: &str,
) -> Result<String, DbErr> {
    let app_id = format!("{ns}/{name}");
    let existing = applications::Entity::find_by_id(&app_id).one(db).await?;
    if existing.is_none() {
        let now = now_utc();
        let model = applications::ActiveModel {
            id: Set(app_id.clone()),
            name: Set(name.to_string()),
            namespace: Set(ns.to_string()),
            description: Set(String::new()),
            team: Set(String::new()),
            deployment_name: Set(Some(name.to_string())),
            created_at: Set(now),
            updated_at: Set(now),
        };
        applications::Entity::insert(model).exec(db).await?;
    }
    Ok(app_id)
}

/// Return a human-readable label for the database backend detected from a URL.
pub fn backend_name(database_url: &str) -> &'static str {
    if database_url.starts_with("sqlite") {
        "sqlite"
    } else if database_url.starts_with("postgres") {
        "postgres"
    } else if database_url.starts_with("mysql") {
        "mysql"
    } else {
        "unknown"
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod db_tests;
