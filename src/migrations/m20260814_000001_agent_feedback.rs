use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260814_000001_agent_feedback"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AgentFeedback::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentFeedback::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AgentFeedback::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    // missing_tool | mcp_tuning | workflow | documentation | other
                    .col(ColumnDef::new(AgentFeedback::Category).string().not_null())
                    .col(ColumnDef::new(AgentFeedback::Summary).string().not_null())
                    .col(ColumnDef::new(AgentFeedback::Detail).text().not_null())
                    .col(
                        ColumnDef::new(AgentFeedback::SuggestedToolName)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(AgentFeedback::SuggestedPrompt).text().null())
                    // pending | reviewed | actioned | dismissed
                    .col(
                        ColumnDef::new(AgentFeedback::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(AgentFeedback::ReviewedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_agent_feedback_status")
                    .table(AgentFeedback::Table)
                    .col(AgentFeedback::Status)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AgentFeedback::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum AgentFeedback {
    Table,
    Id,
    CreatedAt,
    Category,
    Summary,
    Detail,
    SuggestedToolName,
    SuggestedPrompt,
    Status,
    ReviewedAt,
}
