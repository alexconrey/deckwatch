use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_feedback")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub created_at: DateTimeUtc,
    /// One of: missing_tool | mcp_tuning | workflow | documentation | other
    pub category: String,
    pub summary: String,
    pub detail: String,
    pub suggested_tool_name: Option<String>,
    pub suggested_prompt: Option<String>,
    /// One of: pending | reviewed | actioned | dismissed
    pub status: String,
    pub reviewed_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
