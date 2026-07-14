use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "gitops_configs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub application_id: String,
    pub repo_url: String,
    pub branch: String,
    pub token_secret: String,
    pub dockerfile_path: String,
    pub docker_context: String,
    pub oci_repository: String,
    pub include_paths: String,
    pub exclude_paths: String,
    pub poll_interval_seconds: i32,
    pub webhook_enabled: bool,
    pub last_commit_sha: Option<String>,
    pub last_build_status: Option<String>,
    pub last_build_job: Option<String>,
    pub last_build_time: Option<DateTimeUtc>,
    pub last_build_error: Option<String>,
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
