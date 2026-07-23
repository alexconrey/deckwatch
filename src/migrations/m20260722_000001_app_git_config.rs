use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260722_000001_app_git_config"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Applications::Table)
                    .add_column(ColumnDef::new(Applications::GitConfig).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Applications::Table)
                    .drop_column(Applications::GitConfig)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Applications {
    Table,
    GitConfig,
}
