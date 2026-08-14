use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260814_000002_perf_indices"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // UNIQUE index on builds.job_name — watcher looks up builds by job name
        // to update status; a unique index makes this O(1) instead of a full
        // table scan.
        manager
            .create_index(
                Index::create()
                    .name("idx_builds_job_name")
                    .table(Alias::new("builds"))
                    .col(Alias::new("job_name"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Non-unique index on builds.application_id — used when listing build
        // history for a specific application.
        manager
            .create_index(
                Index::create()
                    .name("idx_builds_application_id")
                    .table(Alias::new("builds"))
                    .col(Alias::new("application_id"))
                    .to_owned(),
            )
            .await?;

        // Non-unique index on gitops_configs.last_build_status — monitor_builds
        // filters on this column every poll cycle to find active builds.
        manager
            .create_index(
                Index::create()
                    .name("idx_gitops_configs_last_build_status")
                    .table(Alias::new("gitops_configs"))
                    .col(Alias::new("last_build_status"))
                    .to_owned(),
            )
            .await?;

        // Non-unique index on gitops_configs.application_id — watcher and
        // handlers filter on this column to look up a config by application.
        manager
            .create_index(
                Index::create()
                    .name("idx_gitops_configs_application_id")
                    .table(Alias::new("gitops_configs"))
                    .col(Alias::new("application_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_gitops_configs_application_id")
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_gitops_configs_last_build_status")
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_builds_application_id")
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(Index::drop().name("idx_builds_job_name").to_owned())
            .await?;

        Ok(())
    }
}
