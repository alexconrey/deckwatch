use sea_orm::entity::prelude::*;

/// Current state of one plugin call target. See `crate::plugin_status`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "plugin_status")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// `"{ns}/{app}"`, or `""` for plugin-wide state (fetch and load).
    pub application_id: String,
    pub plugin_name: String,
    /// The plugin resource id, or `""` for `apply()` and plugin-wide state.
    pub resource_id: String,
    /// `healthy`, `degraded` or `failing`.
    pub health: String,
    /// The export of the last recorded call.
    pub last_export: String,
    /// JSON array of `PluginEventReport`, at most `plugin_status::MAX_STORED_EVENTS`.
    #[sea_orm(column_type = "Text")]
    pub last_events: String,
    pub trace_id: Option<String>,
    /// When the current run of failures started; `None` unless failing.
    pub first_failure_at: Option<DateTimeUtc>,
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTimeUtc>,
    /// When `health` last changed.
    pub last_changed_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
