use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260730_000002_gitops_encrypted_token"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(GitopsConfigs::Table)
                    .add_column(ColumnDef::new(GitopsConfigs::EncryptedToken).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(GitopsConfigs::Table)
                    .drop_column(GitopsConfigs::EncryptedToken)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum GitopsConfigs {
    Table,
    EncryptedToken,
}
