use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "application_plugin_resources")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub application_id: String,
    pub plugin_name: String,
    pub resource_id: String,
    /// JSON blob of operator-submitted form values (for display/audit).
    #[sea_orm(column_type = "Text")]
    pub fields: String,
    /// JSON blob of plugin-returned state outputs (DB_HOST, S3_BUCKET, etc.).
    #[sea_orm(column_type = "Text")]
    pub state: String,
    /// JSON blob of deployment annotations to stamp on all application deployments.
    #[sea_orm(column_type = "Text")]
    pub annotations: String,
    /// JSON array of SidecarSpec objects to inject into all application deployments.
    #[sea_orm(column_type = "Text")]
    pub sidecars: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::applications::Entity",
        from = "Column::ApplicationId",
        to = "super::applications::Column::Id",
        on_delete = "Cascade"
    )]
    Application,
}

impl Related<super::applications::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Application.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
