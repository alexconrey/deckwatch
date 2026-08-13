mod m20260714_000001_initial;
mod m20260722_000001_app_git_config;
mod m20260722_000002_gitops_auth_user;
mod m20260730_000001_build_log;
mod m20260730_000002_gitops_encrypted_token;
mod m20260812_000001_application_plugins;
mod m20260812_000002_plugin_resources;
mod m20260812_000003_plugin_resource_annotations;
mod m20260813_000001_plugin_resource_sidecars;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260714_000001_initial::Migration),
            Box::new(m20260722_000001_app_git_config::Migration),
            Box::new(m20260722_000002_gitops_auth_user::Migration),
            Box::new(m20260730_000001_build_log::Migration),
            Box::new(m20260730_000002_gitops_encrypted_token::Migration),
            Box::new(m20260812_000001_application_plugins::Migration),
            Box::new(m20260812_000002_plugin_resources::Migration),
            Box::new(m20260812_000003_plugin_resource_annotations::Migration),
            Box::new(m20260813_000001_plugin_resource_sidecars::Migration),
        ]
    }
}
