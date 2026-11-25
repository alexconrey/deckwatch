//! Current state of every plugin call target: one row per (application,
//! plugin, resource id). Holds current state only (the latest events, not a
//! history), so it stays bounded. See `crate::plugin_status`.
//!
//! Safe to run again after a crash (see `super::schema`): the table and each
//! index are created only if missing.

use sea_orm_migration::prelude::*;

use super::schema;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261124_000002_plugin_status"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PluginStatus::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PluginStatus::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // "{ns}/{app}", or "" for plugin-wide state (fetch, load).
                    .col(
                        ColumnDef::new(PluginStatus::ApplicationId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PluginStatus::PluginName).string().not_null())
                    // The plugin resource id, or "" for apply() and plugin-wide.
                    .col(ColumnDef::new(PluginStatus::ResourceId).string().not_null())
                    // healthy | degraded | failing
                    .col(ColumnDef::new(PluginStatus::Health).string().not_null())
                    .col(ColumnDef::new(PluginStatus::LastExport).string().not_null())
                    // JSON array of PluginEventReport, at most 10.
                    .col(ColumnDef::new(PluginStatus::LastEvents).text().not_null())
                    .col(ColumnDef::new(PluginStatus::TraceId).string().null())
                    .col(
                        ColumnDef::new(PluginStatus::FirstFailureAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PluginStatus::ConsecutiveFailures)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(PluginStatus::LastSuccessAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PluginStatus::LastChangedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PluginStatus::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        schema::create_index_if_missing(
            manager,
            PluginStatus::Table,
            "idx_plugin_status_target",
            Index::create()
                .col(PluginStatus::ApplicationId)
                .col(PluginStatus::PluginName)
                .col(PluginStatus::ResourceId)
                .unique()
                .to_owned(),
        )
        .await?;

        schema::create_index_if_missing(
            manager,
            PluginStatus::Table,
            "idx_plugin_status_health",
            Index::create().col(PluginStatus::Health).to_owned(),
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PluginStatus::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum PluginStatus {
    Table,
    Id,
    ApplicationId,
    PluginName,
    ResourceId,
    Health,
    LastExport,
    LastEvents,
    TraceId,
    FirstFailureAt,
    ConsecutiveFailures,
    LastSuccessAt,
    LastChangedAt,
    UpdatedAt,
}
