//! MCP calls in the audit log (SEC-22): where a row came from (`source`,
//! `api` or `mcp`), and for MCP rows the tool, how the call ended and the
//! client's self-reported name, so the audit API can filter on them. Plus an
//! index on `timestamp` for newest-first listing and retention pruning.
//!
//! Rows written before this migration by the old per-tool MCP audit calls
//! (their detail starts with `MCP:`) are marked `mcp`.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261124_000003_audit_log_mcp"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One column per ALTER: SQLite can't add several at once.
        for (column, default) in [
            (AuditLog::Source, "api"),
            (AuditLog::Tool, ""),
            (AuditLog::Outcome, ""),
            (AuditLog::ClientName, ""),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(AuditLog::Table)
                        .add_column(ColumnDef::new(column).string().not_null().default(default))
                        .to_owned(),
                )
                .await?;
        }

        let mark_mcp = Query::update()
            .table(AuditLog::Table)
            .value(AuditLog::Source, "mcp")
            .and_where(Expr::col(AuditLog::Detail).like("MCP:%"))
            .to_owned();
        manager.exec_stmt(mark_mcp).await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_audit_log_timestamp")
                    .table(AuditLog::Table)
                    .col(AuditLog::Timestamp)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_audit_log_source")
                    .table(AuditLog::Table)
                    .col(AuditLog::Source)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for index in ["idx_audit_log_source", "idx_audit_log_timestamp"] {
            manager
                .drop_index(Index::drop().name(index).table(AuditLog::Table).to_owned())
                .await?;
        }
        for column in [
            AuditLog::ClientName,
            AuditLog::Outcome,
            AuditLog::Tool,
            AuditLog::Source,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(AuditLog::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(Iden, Clone, Copy)]
enum AuditLog {
    Table,
    Timestamp,
    Detail,
    Source,
    Tool,
    Outcome,
    ClientName,
}
