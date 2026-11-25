//! The application's Observability section (#398): a nullable JSON column,
//! `applications.observability`, holding its OpenTelemetry injection
//! settings (`crate::otel_inject::ApplicationObservability`). NULL means
//! the defaults: injection off.
//!
//! Safe to run again after a crash (see `super::schema`): the column is
//! added only if missing.

use sea_orm_migration::prelude::*;

use super::schema;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261125_000001_application_observability"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        schema::add_column_if_missing(
            manager,
            Applications::Table,
            ColumnDef::new(Applications::Observability).text().null(),
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Applications::Table)
                    .drop_column(Applications::Observability)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden, Clone, Copy)]
enum Applications {
    Table,
    Observability,
}
