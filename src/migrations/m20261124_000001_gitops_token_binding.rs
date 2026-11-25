//! Records which GitHub API origin a per-application git token may be sent
//! to (see `crate::outbound::binding`). Tokens stored before this migration
//! were saved while the GitHub API endpoint could not be changed, so they are
//! bound to the default, `https://api.github.com`.
//!
//! Safe to run again after a crash (see `super::schema`): the column is
//! added only if missing, and only tokens without a binding get the default.

use sea_orm_migration::prelude::*;

use super::schema;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261124_000001_gitops_token_binding"
    }
}

/// `EndpointBinding` JSON for the default GitHub API origin.
pub const DEFAULT_GITHUB_API_BINDING: &str = r#"{"github_api":"https://api.github.com"}"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        schema::add_column_if_missing(
            manager,
            GitopsConfigs::Table,
            ColumnDef::new(GitopsConfigs::TokenBinding).text().null(),
        )
        .await?;

        let bind_existing = Query::update()
            .table(GitopsConfigs::Table)
            .value(GitopsConfigs::TokenBinding, DEFAULT_GITHUB_API_BINDING)
            .and_where(Expr::col(GitopsConfigs::EncryptedToken).is_not_null())
            .and_where(Expr::col(GitopsConfigs::TokenBinding).is_null())
            .to_owned();
        manager.exec_stmt(bind_existing).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(GitopsConfigs::Table)
                    .drop_column(GitopsConfigs::TokenBinding)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum GitopsConfigs {
    Table,
    EncryptedToken,
    TokenBinding,
}
