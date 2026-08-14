//! REST API handlers for the Agent Feedback feature.
//!
//! GET  /api/agent-feedback          — list feedback (filterable by status and category)
//! PATCH /api/agent-feedback/{id}    — update feedback status

use axum::extract::{Path, Query, State};
use axum::Json;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};

use crate::entities::agent_feedback;
use crate::error::AppError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AgentFeedbackItem {
    pub id: String,
    pub created_at: String,
    pub category: String,
    pub summary: String,
    pub detail: String,
    pub suggested_tool_name: Option<String>,
    pub suggested_prompt: Option<String>,
    pub status: String,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentFeedbackListResponse {
    pub items: Vec<AgentFeedbackItem>,
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListFeedbackQuery {
    pub status: Option<String>,
    pub category: Option<String>,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpdateFeedbackStatusRequest {
    /// One of: pending | reviewed | actioned | dismissed
    pub status: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/agent-feedback`
///
/// Returns all recorded agent feedback, optionally filtered by `?status=` and/or `?category=`.
pub async fn list_agent_feedback(
    State(state): State<AppState>,
    Query(query): Query<ListFeedbackQuery>,
) -> Result<Json<AgentFeedbackListResponse>, AppError> {
    let mut select = agent_feedback::Entity::find()
        .order_by_desc(agent_feedback::Column::CreatedAt);

    if let Some(status) = &query.status {
        select = select.filter(agent_feedback::Column::Status.eq(status.as_str()));
    }
    if let Some(category) = &query.category {
        select = select.filter(agent_feedback::Column::Category.eq(category.as_str()));
    }

    let rows = select
        .all(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| AgentFeedbackItem {
            id: r.id,
            created_at: r.created_at.to_rfc3339(),
            category: r.category,
            summary: r.summary,
            detail: r.detail,
            suggested_tool_name: r.suggested_tool_name,
            suggested_prompt: r.suggested_prompt,
            status: r.status,
            reviewed_at: r.reviewed_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(Json(AgentFeedbackListResponse { items }))
}

/// `PATCH /api/agent-feedback/{id}`
///
/// Updates the `status` field of a feedback entry and sets `reviewed_at`
/// to the current timestamp when transitioning out of `pending`.
pub async fn update_agent_feedback_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateFeedbackStatusRequest>,
) -> Result<Json<AgentFeedbackItem>, AppError> {
    let valid_statuses = ["pending", "reviewed", "actioned", "dismissed"];
    if !valid_statuses.contains(&req.status.as_str()) {
        return Err(AppError::BadRequest(format!(
            "invalid status '{}'; must be one of: {}",
            req.status,
            valid_statuses.join(", ")
        )));
    }

    let row = agent_feedback::Entity::find_by_id(&id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("feedback '{id}' not found")))?;

    let now = now_utc();
    let reviewed_at = if req.status != "pending" {
        Some(now)
    } else {
        row.reviewed_at
    };

    use sea_orm::ActiveValue::Set;
    let model = agent_feedback::ActiveModel {
        id: Set(id.clone()),
        created_at: Set(row.created_at),
        category: Set(row.category.clone()),
        summary: Set(row.summary.clone()),
        detail: Set(row.detail.clone()),
        suggested_tool_name: Set(row.suggested_tool_name.clone()),
        suggested_prompt: Set(row.suggested_prompt.clone()),
        status: Set(req.status.clone()),
        reviewed_at: Set(reviewed_at),
    };

    agent_feedback::Entity::update(model)
        .exec(&state.db)
        .await
        .map_err(|e| AppError::BadRequest(format!("db error: {e}")))?;

    Ok(Json(AgentFeedbackItem {
        id,
        created_at: row.created_at.to_rfc3339(),
        category: row.category,
        summary: row.summary,
        detail: row.detail,
        suggested_tool_name: row.suggested_tool_name,
        suggested_prompt: row.suggested_prompt,
        status: req.status,
        reviewed_at: reviewed_at.map(|t| t.to_rfc3339()),
    }))
}

fn now_utc() -> sea_orm::entity::prelude::DateTimeUtc {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch");
    sea_orm::entity::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .expect("timestamp out of range")
}
