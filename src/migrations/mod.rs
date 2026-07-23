mod m20260714_000001_initial;
mod m20260722_000001_app_git_config;
mod m20260722_000002_gitops_auth_user;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260714_000001_initial::Migration),
            Box::new(m20260722_000001_app_git_config::Migration),
            Box::new(m20260722_000002_gitops_auth_user::Migration),
        ]
    }
}
