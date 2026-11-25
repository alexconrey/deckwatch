//! The audit log: one row per change made through the REST API (the UI)
//! or MCP, kept in the database so it survives cluster rebuilds.
//!
//! - REST handlers call [`log_action`].
//! - Every MCP tool call that writes, deletes or execs, or that a guardrail
//!   denied, is recorded once at the MCP dispatch choke point
//!   (`handlers::mcp_audit`) with [`insert_mcp_call`]. REST handlers that an
//!   MCP tool reuses don't add a second row ([`within_mcp_call`]).
//! - [`spawn_retention`] prunes old rows in the background.

use std::future::Future;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::Json;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::Set;
use sea_orm::{Condition, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::entities::audit_log as audit_entity;
use crate::error::AppError;
use crate::metrics;
use crate::state::AppState;

/// `source` of rows written through the REST API.
pub const SOURCE_API: &str = "api";
/// `source` of rows for MCP tool calls.
pub const SOURCE_MCP: &str = "mcp";

/// Return the current UTC time as a `DateTimeUtc` without requiring a direct
/// `chrono` dependency. Sea-orm re-exports `DateTimeUtc` (= `chrono::DateTime<Utc>`)
/// via its entity prelude, and we construct it from a UNIX timestamp.
fn now_utc() -> DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}

tokio::task_local! {
    /// Set while an MCP tool call runs: the call gets one audit row, written
    /// at the MCP choke point, so the REST handlers it reuses (gitops,
    /// ingresses, plugin resources) don't add their own.
    static IN_MCP_CALL: ();
}

/// Run `fut` as an MCP tool call's body: [`log_action`] records nothing
/// inside it, because the call's own row covers it.
pub async fn within_mcp_call<F: Future>(fut: F) -> F::Output {
    IN_MCP_CALL.scope((), fut).await
}

/// Record an audit log entry. Fire-and-forget -- callers should not fail the
/// request if this returns an error; log a warning instead.
///
/// Inside an MCP tool call ([`within_mcp_call`]) it records nothing: the
/// call's own row covers it.
pub async fn log_action(
    db: &DatabaseConnection,
    action: &str,
    resource_type: &str,
    resource_name: &str,
    namespace: &str,
    detail: &str,
    user_identity: &str,
) -> Result<(), sea_orm::DbErr> {
    if IN_MCP_CALL.try_with(|_| ()).is_ok() {
        return Ok(());
    }
    let model = audit_entity::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        timestamp: Set(now_utc()),
        action: Set(action.to_string()),
        resource_type: Set(resource_type.to_string()),
        resource_name: Set(resource_name.to_string()),
        namespace: Set(namespace.to_string()),
        detail: Set(detail.to_string()),
        user_identity: Set(user_identity.to_string()),
        source: Set(SOURCE_API.to_string()),
        tool: Set(String::new()),
        outcome: Set(String::new()),
        client_name: Set(String::new()),
    };
    audit_entity::Entity::insert(model).exec(db).await?;
    metrics::record_audit_event(action, resource_type);
    Ok(())
}

/// The audit row of one MCP tool call. `handlers::mcp_audit` builds it from
/// the call's redacted summary, so no argument value outside mcp-k8s's
/// allowlist is ever in it.
#[derive(Clone, Debug, PartialEq)]
pub struct McpCallRow {
    /// `write`, `delete`, `exec`, or `read` (only for a denied read).
    pub action: String,
    pub resource_type: String,
    pub resource_name: String,
    pub namespace: String,
    /// JSON: see `handlers::mcp_audit::Detail`.
    pub detail: String,
    pub tool: String,
    /// `ok`, `error`, `denied` or `cancelled`.
    pub outcome: String,
    /// Self-reported by the client in `initialize`.
    pub client_name: String,
}

/// Insert an MCP tool call's row. `user_identity` stays empty until
/// deckwatch authenticates MCP clients (SEC-1/SEC-3).
pub async fn insert_mcp_call(
    db: &DatabaseConnection,
    row: &McpCallRow,
) -> Result<(), sea_orm::DbErr> {
    let model = audit_entity::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        timestamp: Set(now_utc()),
        action: Set(row.action.clone()),
        resource_type: Set(row.resource_type.clone()),
        resource_name: Set(row.resource_name.clone()),
        namespace: Set(row.namespace.clone()),
        detail: Set(row.detail.clone()),
        user_identity: Set(String::new()),
        source: Set(SOURCE_MCP.to_string()),
        tool: Set(row.tool.clone()),
        outcome: Set(row.outcome.clone()),
        client_name: Set(row.client_name.clone()),
    };
    audit_entity::Entity::insert(model).exec(db).await?;
    // The resource type can come from a manifest (client input), so it
    // isn't a label here.
    metrics::record_audit_event(&row.action, SOURCE_MCP);
    Ok(())
}

// ---------------------------------------------------------------- retention

/// How long audit rows are kept. A limit of `0` is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// Rows older than this many days are deleted.
    pub max_age_days: u32,
    /// Only this many rows are kept, newest first.
    pub max_rows: u64,
}

impl Retention {
    pub fn is_off(self) -> bool {
        self.max_age_days == 0 && self.max_rows == 0
    }
}

/// How often [`spawn_retention`] prunes.
const RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Delete rows older than `retention.max_age_days` (counting back from
/// `now`), then all but the newest `retention.max_rows`. Returns how many
/// rows were deleted.
pub async fn prune(
    db: &DatabaseConnection,
    retention: Retention,
    now: DateTimeUtc,
) -> Result<u64, sea_orm::DbErr> {
    let mut deleted = 0;
    if retention.max_age_days > 0 {
        let age = i64::from(retention.max_age_days) * 24 * 60 * 60;
        let cutoff = DateTimeUtc::from_timestamp(now.timestamp() - age, 0).unwrap_or(now);
        deleted += audit_entity::Entity::delete_many()
            .filter(audit_entity::Column::Timestamp.lt(cutoff))
            .exec(db)
            .await?
            .rows_affected;
    }
    if retention.max_rows > 0 {
        // The newest row that no longer fits; it and everything older go.
        let first_dropped = audit_entity::Entity::find()
            .order_by_desc(audit_entity::Column::Timestamp)
            .order_by_desc(audit_entity::Column::Id)
            .offset(retention.max_rows)
            .limit(1)
            .one(db)
            .await?;
        if let Some(row) = first_dropped {
            deleted += audit_entity::Entity::delete_many()
                .filter(
                    Condition::any()
                        .add(audit_entity::Column::Timestamp.lt(row.timestamp))
                        .add(
                            Condition::all()
                                .add(audit_entity::Column::Timestamp.eq(row.timestamp))
                                .add(audit_entity::Column::Id.lte(row.id)),
                        ),
                )
                .exec(db)
                .await?
                .rows_affected;
        }
    }
    Ok(deleted)
}

/// Prune the audit log now and then every hour, in the background. Pruning
/// errors are logged and retried on the next round.
pub fn spawn_retention(db: DatabaseConnection, retention: Retention) {
    if retention.is_off() {
        tracing::warn!("audit log retention is off: the audit log grows without bound");
        return;
    }
    tracing::info!(
        max_age_days = retention.max_age_days,
        max_rows = retention.max_rows,
        "audit log retention (0 = no limit)"
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_INTERVAL);
        loop {
            interval.tick().await;
            match prune(&db, retention, now_utc()).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "pruned old audit log rows"),
                Err(e) => tracing::warn!(error = %e, "failed to prune the audit log"),
            }
        }
    });
}

// ---------------------------------------------------------------- query API

/// Query parameters of `GET /api/audit`. Every filter is optional and
/// matches exactly.
#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub resource_type: Option<String>,
    pub namespace: Option<String>,
    /// `api` or `mcp`.
    pub source: Option<String>,
    /// The action: `write`, `delete`, `exec` or `read` for MCP rows; `create`,
    /// `update`, `delete`, ... for API rows.
    pub action: Option<String>,
    /// MCP rows: the tool.
    pub tool: Option<String>,
    /// MCP rows: the client's self-reported name.
    pub client: Option<String>,
    /// MCP rows: `ok`, `error`, `denied` or `cancelled`.
    pub outcome: Option<String>,
    /// Only rows at or after this time (RFC 3339).
    pub since: Option<String>,
    /// `desc` (newest first, the default) or `asc`.
    pub order: Option<String>,
    pub limit: Option<u64>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub items: Vec<AuditEntry>,
    /// If present, pass as `?cursor=` to retrieve the next page.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: String,
    pub action: String,
    pub resource_type: String,
    pub resource_name: String,
    pub namespace: String,
    pub detail: String,
    pub user_identity: String,
    /// `api` or `mcp`.
    pub source: String,
    /// MCP rows: the tool called.
    pub tool: String,
    /// MCP rows: `ok`, `error`, `denied` or `cancelled`.
    pub outcome: String,
    /// MCP rows: the client's self-reported name. Not an identity.
    pub client_name: String,
}

impl From<audit_entity::Model> for AuditEntry {
    fn from(m: audit_entity::Model) -> Self {
        Self {
            id: m.id,
            timestamp: m.timestamp.to_rfc3339(),
            action: m.action,
            resource_type: m.resource_type,
            resource_name: m.resource_name,
            namespace: m.namespace,
            detail: m.detail,
            user_identity: m.user_identity,
            source: m.source,
            tool: m.tool,
            outcome: m.outcome,
            client_name: m.client_name,
        }
    }
}

/// A page position: the timestamp (nanoseconds) and id of the last row of
/// the previous page. Rows are ordered by timestamp, then id.
fn encode_cursor(m: &audit_entity::Model) -> String {
    format!(
        "{}~{}",
        m.timestamp.timestamp_nanos_opt().unwrap_or_default(),
        m.id
    )
}

fn decode_cursor(cursor: &str) -> Option<(DateTimeUtc, String)> {
    let (nanos, id) = cursor.split_once('~')?;
    let nanos: i64 = nanos.parse().ok()?;
    let when = DateTimeUtc::from_timestamp(
        nanos.div_euclid(1_000_000_000),
        u32::try_from(nanos.rem_euclid(1_000_000_000)).ok()?,
    )?;
    Some((when, id.to_string()))
}

/// `GET /api/audit`: audit rows, newest first unless `order=asc`, filtered
/// by the query, in pages of `limit` (at most 200) linked by `next_cursor`.
pub async fn list_audit_logs(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<AuditResponse>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let descending = match q.order.as_deref() {
        None | Some("desc") => true,
        Some("asc") => false,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "order must be asc or desc, not {other}"
            )))
        }
    };

    let mut query = audit_entity::Entity::find();
    query = if descending {
        query
            .order_by_desc(audit_entity::Column::Timestamp)
            .order_by_desc(audit_entity::Column::Id)
    } else {
        query
            .order_by_asc(audit_entity::Column::Timestamp)
            .order_by_asc(audit_entity::Column::Id)
    };

    if let Some(ref cursor) = q.cursor {
        let (when, id) = decode_cursor(cursor)
            .ok_or_else(|| AppError::BadRequest("invalid cursor".to_string()))?;
        let (ts_past, id_past) = if descending {
            (
                audit_entity::Column::Timestamp.lt(when),
                audit_entity::Column::Id.lt(id),
            )
        } else {
            (
                audit_entity::Column::Timestamp.gt(when),
                audit_entity::Column::Id.gt(id),
            )
        };
        query = query.filter(
            Condition::any().add(ts_past).add(
                Condition::all()
                    .add(audit_entity::Column::Timestamp.eq(when))
                    .add(id_past),
            ),
        );
    }
    if let Some(ref since) = q.since {
        let since: DateTimeUtc = since.parse().map_err(|_| {
            AppError::BadRequest(format!("since must be an RFC 3339 time, not {since}"))
        })?;
        query = query.filter(audit_entity::Column::Timestamp.gte(since));
    }
    for (column, value) in [
        (audit_entity::Column::ResourceType, &q.resource_type),
        (audit_entity::Column::Namespace, &q.namespace),
        (audit_entity::Column::Source, &q.source),
        (audit_entity::Column::Action, &q.action),
        (audit_entity::Column::Tool, &q.tool),
        (audit_entity::Column::ClientName, &q.client),
        (audit_entity::Column::Outcome, &q.outcome),
    ] {
        if let Some(value) = value {
            query = query.filter(column.eq(value.as_str()));
        }
    }

    // Fetch one extra row to detect whether there is a next page.
    let mut rows: Vec<audit_entity::Model> = query
        .limit(limit + 1)
        .all(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to query audit log: {e}")))?;

    let has_more = rows.len() as u64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| rows.last().map(encode_cursor)).flatten();
    let items = rows.into_iter().map(AuditEntry::from).collect();
    Ok(Json(AuditResponse { items, next_cursor }))
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
