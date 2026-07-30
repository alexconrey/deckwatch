use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260730_000001_build_log"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Builds::Table)
                    .add_column(ColumnDef::new(Builds::BuildLog).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Builds::Table)
                    .drop_column(Builds::BuildLog)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Builds {
    Table,
    BuildLog,
}
