use axum::extract::{Query, State};
use axum::Json;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::Set;
use sea_orm::{QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::entities::audit_log as audit_entity;
use crate::error::AppError;
use crate::metrics;
use crate::state::AppState;

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

/// Record an audit log entry. Fire-and-forget -- callers should not fail the
/// request if this returns an error; log a warning instead.
pub async fn log_action(
    db: &DatabaseConnection,
    action: &str,
    resource_type: &str,
    resource_name: &str,
    namespace: &str,
    detail: &str,
) -> Result<(), sea_orm::DbErr> {
    let model = audit_entity::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        timestamp: Set(now_utc()),
        action: Set(action.to_string()),
        resource_type: Set(resource_type.to_string()),
        resource_name: Set(resource_name.to_string()),
        namespace: Set(namespace.to_string()),
        detail: Set(detail.to_string()),
        user_identity: Set(String::new()),
    };
    audit_entity::Entity::insert(model).exec(db).await?;
    metrics::record_audit_event(action, resource_type);
    Ok(())
}

// ---------------------------------------------------------------- query API

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub resource_type: Option<String>,
    pub namespace: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub entries: Vec<AuditEntry>,
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
        }
    }
}

pub async fn list_audit_logs(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<AuditResponse>, AppError> {
    let limit = q.limit.unwrap_or(50).min(500);

    let mut query = audit_entity::Entity::find().order_by_desc(audit_entity::Column::Timestamp);

    if let Some(ref rt) = q.resource_type {
        query = query.filter(audit_entity::Column::ResourceType.eq(rt.as_str()));
    }
    if let Some(ref ns) = q.namespace {
        query = query.filter(audit_entity::Column::Namespace.eq(ns.as_str()));
    }

    let rows: Vec<audit_entity::Model> = query
        .limit(limit)
        .all(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to query audit log: {e}")))?;

    let entries = rows.into_iter().map(AuditEntry::from).collect();
    Ok(Json(AuditResponse { entries }))
}
