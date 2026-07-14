use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "applications")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub team: String,
    pub deployment_name: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::gitops_configs::Entity")]
    GitopsConfigs,
    #[sea_orm(has_many = "super::builds::Entity")]
    Builds,
}

impl Related<super::gitops_configs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::GitopsConfigs.def()
    }
}

impl Related<super::builds::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Builds.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
