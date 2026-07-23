use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260722_000002_gitops_auth_user"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(GitopsConfigs::Table)
                    .add_column(
                        ColumnDef::new(GitopsConfigs::GitAuthUser)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(GitopsConfigs::Table)
                    .drop_column(GitopsConfigs::GitAuthUser)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum GitopsConfigs {
    Table,
    GitAuthUser,
}
