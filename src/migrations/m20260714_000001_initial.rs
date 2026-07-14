use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260714_000001_initial"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- settings ---
        manager
            .create_table(
                Table::create()
                    .table(Settings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Settings::Key)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Settings::Value).string().not_null())
                    .col(
                        ColumnDef::new(Settings::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // --- applications ---
        manager
            .create_table(
                Table::create()
                    .table(Applications::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Applications::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Applications::Name).string().not_null())
                    .col(ColumnDef::new(Applications::Namespace).string().not_null())
                    .col(
                        ColumnDef::new(Applications::Description)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(Applications::Team)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Applications::DeploymentName).string().null())
                    .col(
                        ColumnDef::new(Applications::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Applications::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // UNIQUE(namespace, name) on applications
        manager
            .create_index(
                Index::create()
                    .name("idx_applications_namespace_name")
                    .table(Applications::Table)
                    .col(Applications::Namespace)
                    .col(Applications::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // --- gitops_configs ---
        manager
            .create_table(
                Table::create()
                    .table(GitopsConfigs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GitopsConfigs::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::ApplicationId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(GitopsConfigs::RepoUrl).string().not_null())
                    .col(
                        ColumnDef::new(GitopsConfigs::Branch)
                            .string()
                            .not_null()
                            .default("main"),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::TokenSecret)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::DockerfilePath)
                            .string()
                            .not_null()
                            .default("Dockerfile"),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::DockerContext)
                            .string()
                            .not_null()
                            .default("."),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::OciRepository)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::IncludePaths)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::ExcludePaths)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::PollIntervalSeconds)
                            .integer()
                            .not_null()
                            .default(60),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::WebhookEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(GitopsConfigs::LastCommitSha).string().null())
                    .col(
                        ColumnDef::new(GitopsConfigs::LastBuildStatus)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(GitopsConfigs::LastBuildJob).string().null())
                    .col(
                        ColumnDef::new(GitopsConfigs::LastBuildTime)
                            .timestamp()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::LastBuildError)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(GitopsConfigs::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_gitops_configs_application_id")
                            .from(GitopsConfigs::Table, GitopsConfigs::ApplicationId)
                            .to(Applications::Table, Applications::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // --- builds ---
        manager
            .create_table(
                Table::create()
                    .table(Builds::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Builds::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Builds::ApplicationId).string().not_null())
                    .col(ColumnDef::new(Builds::JobName).string().not_null())
                    .col(ColumnDef::new(Builds::CommitSha).string().not_null())
                    .col(ColumnDef::new(Builds::ImageTag).string().not_null())
                    .col(
                        ColumnDef::new(Builds::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(Builds::StartedAt).timestamp().null())
                    .col(ColumnDef::new(Builds::CompletedAt).timestamp().null())
                    .col(ColumnDef::new(Builds::ErrorMessage).string().null())
                    .col(
                        ColumnDef::new(Builds::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_builds_application_id")
                            .from(Builds::Table, Builds::ApplicationId)
                            .to(Applications::Table, Applications::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // --- audit_log ---
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditLog::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Timestamp)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(AuditLog::Action).string().not_null())
                    .col(ColumnDef::new(AuditLog::ResourceType).string().not_null())
                    .col(ColumnDef::new(AuditLog::ResourceName).string().not_null())
                    .col(
                        ColumnDef::new(AuditLog::Namespace)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Detail)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AuditLog::UserIdentity)
                            .string()
                            .not_null()
                            .default("anonymous"),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AuditLog::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Builds::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(GitopsConfigs::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Applications::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Settings::Table).to_owned())
            .await?;
        Ok(())
    }
}

// ---- Iden enums for type-safe table/column references ----

#[derive(Iden)]
enum Settings {
    Table,
    Key,
    Value,
    UpdatedAt,
}

#[derive(Iden)]
enum Applications {
    Table,
    Id,
    Name,
    Namespace,
    Description,
    Team,
    DeploymentName,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum GitopsConfigs {
    Table,
    Id,
    ApplicationId,
    RepoUrl,
    Branch,
    TokenSecret,
    DockerfilePath,
    DockerContext,
    OciRepository,
    IncludePaths,
    ExcludePaths,
    PollIntervalSeconds,
    WebhookEnabled,
    LastCommitSha,
    LastBuildStatus,
    LastBuildJob,
    LastBuildTime,
    LastBuildError,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Builds {
    Table,
    Id,
    ApplicationId,
    JobName,
    CommitSha,
    ImageTag,
    Status,
    StartedAt,
    CompletedAt,
    ErrorMessage,
    CreatedAt,
}

#[derive(Iden)]
enum AuditLog {
    Table,
    Id,
    Timestamp,
    Action,
    ResourceType,
    ResourceName,
    Namespace,
    Detail,
    UserIdentity,
}
