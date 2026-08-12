use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260812_000001_application_plugins"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ApplicationPlugins::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ApplicationPlugins::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // References applications.id (format: "{namespace}/{name}")
                    .col(
                        ColumnDef::new(ApplicationPlugins::ApplicationId)
                            .string()
                            .not_null(),
                    )
                    // Plugin name — must match LoadedPlugin.name / PluginConfig.name
                    .col(
                        ColumnDef::new(ApplicationPlugins::PluginName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ApplicationPlugins::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_application_plugins_app_plugin")
                    .table(ApplicationPlugins::Table)
                    .col(ApplicationPlugins::ApplicationId)
                    .col(ApplicationPlugins::PluginName)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ApplicationPlugins::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum ApplicationPlugins {
    Table,
    Id,
    ApplicationId,
    PluginName,
    CreatedAt,
}
