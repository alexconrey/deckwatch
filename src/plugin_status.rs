//! Current plugin state: one `plugin_status` row per call target.
//!
//! A target is an (application, plugin, resource id) triple:
//!
//! - a provisioned resource: (`"{ns}/{app}"`, plugin, resource id), from
//!   provision, refresh and status polling;
//! - a plugin's `apply()` for an application: (`"{ns}/{app}"`, plugin, `""`),
//!   from deployment create/update and the reconcile loop (aggregated over
//!   the application's Deployments, so one failing Deployment makes it
//!   failing without the row flapping);
//! - the plugin itself: (`""`, plugin, `""`), from fetch and load.
//!
//! Each row holds the health (`healthy`, `degraded`, `failing`), the latest
//! events (at most [`MAX_STORED_EVENTS`]), when the current run of failures
//! started, how many calls in a row failed, and the last success. It is
//! current state, not a log: rows are replaced, removed with their resource,
//! application or plugin, and never accumulate history.
//!
//! Plugin calls never touch the database themselves. Callers hand a
//! [`StatusUpdate`] to `PluginTelemetry::report`, which queues it without
//! waiting (a full queue drops the update and counts it). One worker task
//! ([`spawn_worker`]) applies updates in order, so a transition into or out
//! of `failing` is seen exactly once.

use std::collections::BTreeMap;

use sea_orm::prelude::DateTimeUtc;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tokio::sync::{mpsc, oneshot};

use crate::entities::plugin_status;
use crate::plugin_events::{Health, PluginEventReport};
use crate::state::AppState;

/// Most events kept on a row.
pub const MAX_STORED_EVENTS: usize = 10;
/// Updates waiting for the worker. Past this, updates are dropped.
pub const QUEUE_CAPACITY: usize = 1024;
/// A row whose health and events haven't changed is rewritten at most this
/// often, so a healthy plugin on the 10-second reconcile loop doesn't write
/// every cycle. Failing rows are written every time (their failure count
/// moves).
pub const UNCHANGED_REWRITE_SECS: i64 = 300;

/// Which row an update is for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusKey {
    /// `"{ns}/{app}"`, or `""` for plugin-wide state.
    pub application_id: String,
    pub plugin: String,
    /// The plugin resource id, or `""`.
    pub resource_id: String,
}

impl StatusKey {
    pub fn plugin_wide(plugin: &str) -> Self {
        Self {
            application_id: String::new(),
            plugin: plugin.to_string(),
            resource_id: String::new(),
        }
    }

    pub fn apply(application_id: &str, plugin: &str) -> Self {
        Self {
            application_id: application_id.to_string(),
            plugin: plugin.to_string(),
            resource_id: String::new(),
        }
    }

    pub fn resource(application_id: &str, plugin: &str, resource_id: &str) -> Self {
        Self {
            application_id: application_id.to_string(),
            plugin: plugin.to_string(),
            resource_id: resource_id.to_string(),
        }
    }
}

/// The outcome of one call (or one reconcile cycle) for a target.
#[derive(Debug, Clone)]
pub struct StatusUpdate {
    pub key: StatusKey,
    /// The export that produced it.
    pub export: String,
    pub health: Health,
    pub events: Vec<PluginEventReport>,
    pub trace_id: Option<String>,
    /// Add to the current row instead of replacing it: for host events that
    /// belong to a call already recorded (a Kubernetes object the call
    /// returned failed to apply afterwards). Health only gets worse.
    pub merge: bool,
}

/// A change into or out of `failing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// From healthy, degraded or no row at all, to failing.
    Failing,
    /// From failing to healthy or degraded.
    Recovered,
}

/// Messages for the worker.
pub(crate) enum SinkMsg {
    Status(StatusUpdate),
    /// The target is gone (a deprovisioned resource).
    Forget(StatusKey),
    /// The application is gone.
    ForgetApplication(String),
    /// Keep only these plugins' rows (disabled or removed plugins go).
    KeepPlugins(Vec<String>),
    /// Reply once everything queued before has been applied.
    Flush(oneshot::Sender<()>),
}

/// The row an update leads to, before it is written.
#[derive(Debug, Clone, PartialEq)]
pub struct NextRow {
    pub health: Health,
    pub last_export: String,
    pub last_events: Vec<PluginEventReport>,
    pub trace_id: Option<String>,
    pub first_failure_at: Option<DateTimeUtc>,
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTimeUtc>,
    pub last_changed_at: DateTimeUtc,
}

fn stored_events(row: &plugin_status::Model) -> Vec<PluginEventReport> {
    serde_json::from_str(&row.last_events).unwrap_or_default()
}

fn stored_health(row: &plugin_status::Model) -> Health {
    match row.health.as_str() {
        "failing" => Health::Failing,
        "degraded" => Health::Degraded,
        _ => Health::Healthy,
    }
}

/// Work out the next state of a target. Pure, so the transition rules are
/// unit tested on their own.
///
/// - `failing` counts a failure and keeps (or starts) `first_failure_at`;
///   anything else resets both and moves `last_success_at`.
/// - A transition is reported when health crosses into or out of
///   `failing`; a target seen for the first time and already failing counts
///   as a transition, one that starts healthy doesn't.
/// - A `merge` update adds its events to the row's and can only make health
///   worse; it doesn't count as another call.
pub fn next_row(
    prev: Option<&plugin_status::Model>,
    update: &StatusUpdate,
    now: DateTimeUtc,
) -> (NextRow, Option<Transition>) {
    let prev_health = prev.map(stored_health);
    let (health, mut events, export, trace_id) = match (update.merge, prev) {
        (true, Some(row)) => {
            let mut events = stored_events(row);
            for e in &update.events {
                if !events.contains(e) {
                    events.push(e.clone());
                }
            }
            (
                stored_health(row).max(update.health),
                events,
                row.last_export.clone(),
                row.trace_id.clone().or_else(|| update.trace_id.clone()),
            )
        }
        _ => (
            update.health,
            update.events.clone(),
            update.export.clone(),
            update.trace_id.clone(),
        ),
    };
    events.truncate(MAX_STORED_EVENTS);

    let failing = health == Health::Failing;
    let was_failing = prev_health == Some(Health::Failing);
    let counted_call = !(update.merge && prev.is_some());
    let consecutive_failures = match (failing, prev) {
        (false, _) => 0,
        (true, Some(row)) if was_failing => row.consecutive_failures + i32::from(counted_call),
        (true, _) => 1,
    };
    let first_failure_at = if failing {
        prev.filter(|_| was_failing)
            .and_then(|row| row.first_failure_at)
            .or(Some(now))
    } else {
        None
    };
    let last_success_at = if failing {
        prev.and_then(|row| row.last_success_at)
    } else {
        Some(now)
    };
    let last_changed_at = match prev {
        Some(row) if prev_health == Some(health) => row.last_changed_at,
        _ => now,
    };
    let transition = match (was_failing, failing) {
        (false, true) => Some(Transition::Failing),
        (true, false) => Some(Transition::Recovered),
        _ => None,
    };
    (
        NextRow {
            health,
            last_export: export,
            last_events: events,
            trace_id,
            first_failure_at,
            consecutive_failures,
            last_success_at,
            last_changed_at,
        },
        transition,
    )
}

/// Whether `next` needs writing over `prev`: always for a new row, a
/// failing row or a change; otherwise once every
/// [`UNCHANGED_REWRITE_SECS`], to keep `last_success_at` roughly current.
pub fn needs_write(prev: Option<&plugin_status::Model>, next: &NextRow, now: DateTimeUtc) -> bool {
    let Some(row) = prev else {
        return true;
    };
    next.health == Health::Failing
        || stored_health(row) != next.health
        || stored_events(row) != next.last_events
        || row.last_export != next.last_export
        || (now - row.updated_at).num_seconds() >= UNCHANGED_REWRITE_SECS
}

async fn find_row(
    db: &DatabaseConnection,
    key: &StatusKey,
) -> Result<Option<plugin_status::Model>, sea_orm::DbErr> {
    plugin_status::Entity::find()
        .filter(plugin_status::Column::ApplicationId.eq(&key.application_id))
        .filter(plugin_status::Column::PluginName.eq(&key.plugin))
        .filter(plugin_status::Column::ResourceId.eq(&key.resource_id))
        .one(db)
        .await
}

/// Apply one update to the database. Returns the transition it caused.
pub async fn apply_update(
    db: &DatabaseConnection,
    update: &StatusUpdate,
    now: DateTimeUtc,
) -> Result<Option<Transition>, sea_orm::DbErr> {
    let prev = find_row(db, &update.key).await?;
    let (next, transition) = next_row(prev.as_ref(), update, now);
    if !needs_write(prev.as_ref(), &next, now) {
        return Ok(transition);
    }
    let events_json = serde_json::to_string(&next.last_events).unwrap_or_else(|_| "[]".into());
    match prev {
        Some(row) => {
            let mut active: plugin_status::ActiveModel = row.into();
            active.health = Set(next.health.as_str().to_string());
            active.last_export = Set(next.last_export);
            active.last_events = Set(events_json);
            active.trace_id = Set(next.trace_id);
            active.first_failure_at = Set(next.first_failure_at);
            active.consecutive_failures = Set(next.consecutive_failures);
            active.last_success_at = Set(next.last_success_at);
            active.last_changed_at = Set(next.last_changed_at);
            active.updated_at = Set(now);
            active.update(db).await?;
        }
        None => {
            plugin_status::ActiveModel {
                id: Set(uuid::Uuid::new_v4().to_string()),
                application_id: Set(update.key.application_id.clone()),
                plugin_name: Set(update.key.plugin.clone()),
                resource_id: Set(update.key.resource_id.clone()),
                health: Set(next.health.as_str().to_string()),
                last_export: Set(next.last_export),
                last_events: Set(events_json),
                trace_id: Set(next.trace_id),
                first_failure_at: Set(next.first_failure_at),
                consecutive_failures: Set(next.consecutive_failures),
                last_success_at: Set(next.last_success_at),
                last_changed_at: Set(next.last_changed_at),
                updated_at: Set(now),
            }
            .insert(db)
            .await?;
        }
    }
    Ok(transition)
}

fn now_utc() -> DateTimeUtc {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos()).unwrap_or_default()
}

/// Start the worker that applies status updates, and connect it to
/// `state.plugin_telemetry`. Call once, after `AppState` is built. Until
/// then (and in tests that don't call it), updates are discarded.
pub fn spawn_worker(state: &AppState) {
    let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
    if !state.plugin_telemetry.connect(tx) {
        tracing::warn!("plugin status worker already running");
        return;
    }
    tokio::spawn(run_worker(state.clone(), rx));
}

async fn run_worker(state: AppState, mut rx: mpsc::Receiver<SinkMsg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            SinkMsg::Status(update) => match apply_update(&state.db, &update, now_utc()).await {
                Ok(Some(transition)) => {
                    tracing::info!(
                        plugin = %update.key.plugin,
                        application = %update.key.application_id,
                        resource_id = %update.key.resource_id,
                        transition = ?transition,
                        "plugin status changed"
                    );
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    plugin = %update.key.plugin,
                    error = %e,
                    "failed to record plugin status"
                ),
            },
            SinkMsg::Forget(key) => {
                let result = plugin_status::Entity::delete_many()
                    .filter(plugin_status::Column::ApplicationId.eq(&key.application_id))
                    .filter(plugin_status::Column::PluginName.eq(&key.plugin))
                    .filter(plugin_status::Column::ResourceId.eq(&key.resource_id))
                    .exec(&state.db)
                    .await;
                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to remove plugin status");
                }
            }
            SinkMsg::ForgetApplication(application_id) => {
                let result = plugin_status::Entity::delete_many()
                    .filter(plugin_status::Column::ApplicationId.eq(&application_id))
                    .exec(&state.db)
                    .await;
                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to remove application plugin status");
                }
            }
            SinkMsg::KeepPlugins(plugins) => {
                let result = plugin_status::Entity::delete_many()
                    .filter(plugin_status::Column::PluginName.is_not_in(plugins))
                    .exec(&state.db)
                    .await;
                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to prune plugin status");
                }
            }
            SinkMsg::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Combine one reconcile cycle's `apply()` outcomes for an application into
/// one update per plugin: the worst health, every event once, and the first
/// trace id. Keeps the `apply` row steady when only some Deployments fail.
#[derive(Debug, Default)]
pub struct ApplyAggregate {
    by_plugin: BTreeMap<String, StatusUpdate>,
}

impl ApplyAggregate {
    pub fn add(
        &mut self,
        application_id: &str,
        plugin: &str,
        health: Health,
        events: Vec<PluginEventReport>,
        trace_id: Option<String>,
    ) {
        let entry = self
            .by_plugin
            .entry(plugin.to_string())
            .or_insert_with(|| StatusUpdate {
                key: StatusKey::apply(application_id, plugin),
                export: "apply".to_string(),
                health: Health::Healthy,
                events: Vec::new(),
                trace_id: None,
                merge: false,
            });
        entry.health = entry.health.max(health);
        for e in events {
            if entry.events.len() < MAX_STORED_EVENTS && !entry.events.contains(&e) {
                entry.events.push(e);
            }
        }
        if entry.trace_id.is_none() {
            entry.trace_id = trace_id;
        }
    }

    pub fn into_updates(self) -> impl Iterator<Item = StatusUpdate> {
        self.by_plugin.into_values()
    }
}

/// Turn Kubernetes apply failures reported after a call into one `merge`
/// update per plugin for that plugin's `apply` row in `application_id`.
pub fn failures_by_plugin(
    application_id: &str,
    failures: Vec<PluginEventReport>,
) -> Vec<StatusUpdate> {
    let mut by_plugin: BTreeMap<String, Vec<PluginEventReport>> = BTreeMap::new();
    for failure in failures {
        by_plugin
            .entry(failure.plugin.clone())
            .or_default()
            .push(failure);
    }
    by_plugin
        .into_iter()
        .map(|(plugin, events)| StatusUpdate {
            key: StatusKey::apply(application_id, &plugin),
            export: "apply".to_string(),
            health: Health::Failing,
            trace_id: events.first().and_then(|e| e.trace_id.clone()),
            events,
            merge: true,
        })
        .collect()
}

#[cfg(test)]
#[path = "plugin_status_tests.rs"]
mod tests;
