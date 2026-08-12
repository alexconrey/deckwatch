use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260812_000002_plugin_resources"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ApplicationPluginResources::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ApplicationPluginResources::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::ApplicationId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::PluginName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::ResourceId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::Fields)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::State)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ApplicationPluginResources::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_application_plugin_resources_application_id")
                            .from(
                                ApplicationPluginResources::Table,
                                ApplicationPluginResources::ApplicationId,
                            )
                            .to(Applications::Table, Applications::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Unique index on (application_id, plugin_name, resource_id) for singletons.
        manager
            .create_index(
                Index::create()
                    .name("idx_app_plugin_resources_unique")
                    .table(ApplicationPluginResources::Table)
                    .col(ApplicationPluginResources::ApplicationId)
                    .col(ApplicationPluginResources::PluginName)
                    .col(ApplicationPluginResources::ResourceId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ApplicationPluginResources::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum ApplicationPluginResources {
    Table,
    Id,
    ApplicationId,
    PluginName,
    ResourceId,
    Fields,
    State,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Applications {
    Table,
    Id,
}
