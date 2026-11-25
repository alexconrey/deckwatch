//! Schema checks that make a migration safe to run again after a crash.
//!
//! sea-orm-migration applies a migration and then records it in
//! `seaql_migrations`. On PostgreSQL every pending migration and its record
//! run in one transaction. On SQLite and MySQL nothing is wrapped, so a crash
//! between two statements of a migration, or between a migration and its
//! record, leaves schema changes behind that the next start applies again,
//! and `ADD COLUMN` or `CREATE INDEX` then fail, so deckwatch can't start.
//! Migrations that add columns or indexes through these helpers skip what is
//! already there.
//!
//! The checks are plain catalog queries, per backend: sea-orm-migration's own
//! `has_column`/`has_index` need its `sqlx-*` features, which deckwatch
//! doesn't enable.

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use sea_orm_migration::prelude::*;

/// Whether `table` has a column named `column`.
pub async fn has_column(
    manager: &SchemaManager<'_>,
    table: &str,
    column: &str,
) -> Result<bool, DbErr> {
    let sql = match manager.get_database_backend() {
        DbBackend::Sqlite => "SELECT COUNT(*) AS n FROM pragma_table_info(?) WHERE name = ?",
        DbBackend::Postgres => {
            "SELECT COUNT(*) AS n FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 AND column_name = $2"
        }
        DbBackend::MySql => {
            "SELECT COUNT(*) AS n FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = ? AND column_name = ?"
        }
    };
    exists(manager, sql, [table.into(), column.into()]).await
}

/// Whether an index named `index` exists on `table`.
pub async fn has_index(
    manager: &SchemaManager<'_>,
    table: &str,
    index: &str,
) -> Result<bool, DbErr> {
    let sql = match manager.get_database_backend() {
        DbBackend::Sqlite => {
            "SELECT COUNT(*) AS n FROM sqlite_master \
             WHERE type = 'index' AND tbl_name = ? AND name = ?"
        }
        DbBackend::Postgres => {
            "SELECT COUNT(*) AS n FROM pg_indexes \
             WHERE schemaname = current_schema() AND tablename = $1 AND indexname = $2"
        }
        DbBackend::MySql => {
            "SELECT COUNT(*) AS n FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = ? AND index_name = ?"
        }
    };
    exists(manager, sql, [table.into(), index.into()]).await
}

/// Add `column` to `table` unless the table already has it.
pub async fn add_column_if_missing(
    manager: &SchemaManager<'_>,
    table: impl Iden + 'static,
    column: &mut ColumnDef,
) -> Result<(), DbErr> {
    let column_name = column.get_column_name();
    if has_column(manager, &table.to_string(), &column_name).await? {
        return Ok(());
    }
    manager
        .alter_table(
            Table::alter()
                .table(table)
                .add_column(column.take())
                .to_owned(),
        )
        .await
}

/// Create `index` as `name` on `table` unless an index of that name exists
/// on the table.
pub async fn create_index_if_missing(
    manager: &SchemaManager<'_>,
    table: impl Iden + 'static,
    name: &str,
    mut index: IndexCreateStatement,
) -> Result<(), DbErr> {
    if has_index(manager, &table.to_string(), name).await? {
        return Ok(());
    }
    manager
        .create_index(index.name(name).table(table).to_owned())
        .await
}

async fn exists(manager: &SchemaManager<'_>, sql: &str, values: [Value; 2]) -> Result<bool, DbErr> {
    let db = manager.get_connection();
    let row = db
        .query_one(Statement::from_sql_and_values(
            manager.get_database_backend(),
            sql,
            values,
        ))
        .await?
        .ok_or_else(|| DbErr::Migration(format!("no result from schema check: {sql}")))?;
    Ok(row.try_get::<i64>("", "n")? > 0)
}
