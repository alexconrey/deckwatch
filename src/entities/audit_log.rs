use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub timestamp: DateTimeUtc,
    pub action: String,
    pub resource_type: String,
    pub resource_name: String,
    pub namespace: String,
    pub detail: String,
    pub user_identity: String,
    /// Where the row came from: `api` (the REST API, so the UI) or `mcp`.
    pub source: String,
    /// MCP rows: the tool called.
    pub tool: String,
    /// MCP rows: `ok`, `error`, `denied` or `cancelled`.
    pub outcome: String,
    /// MCP rows: the client's name from `initialize`. Self-reported by the
    /// client, so a label, never an identity.
    pub client_name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
