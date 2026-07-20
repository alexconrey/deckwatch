#![allow(dead_code, unused_imports)]
use axum::extract::{Path, Query, State};
use axum::Json;
use kube::api::ListParams;
use serde::Deserialize;

use crate::error::AppError;
use crate::kube_ext::{event_summary, EventSummary};
use crate::metrics::K8sTimer;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub involved_object: Option<String>,
    pub field_selector: Option<String>,
}

#[derive(serde::Serialize)]
pub struct EventListResponse {
    pub events: Vec<EventSummary>,
}

// Sort helper: newest event first. Prefers `last_timestamp`, falls back to
// `first_timestamp`, then to the empty string so events without any timestamp
// sink to the bottom of the list rather than randomizing their relative order.
fn sort_key(e: &EventSummary) -> String {
    e.last_timestamp
        .clone()
        .or_else(|| e.first_timestamp.clone())
        .unwrap_or_default()
}

fn merge_field_selectors(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(x), Some(y)) if !x.is_empty() && !y.is_empty() => Some(format!("{x},{y}")),
        (Some(x), _) if !x.is_empty() => Some(x),
        (_, Some(y)) if !y.is_empty() => Some(y),
        _ => None,
    }
}

pub async fn list_namespaced(
    State(state): State<AppState>,
    Path(ns): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<EventListResponse>, AppError> {
    let api = state.events_api(&ns)?;
    let selector = merge_field_selectors(
        query
            .involved_object
            .map(|name| format!("involvedObject.name={name}")),
        query.field_selector,
    );
    let mut lp = ListParams::default();
    if let Some(sel) = selector {
        lp = lp.fields(&sel);
    }
    let t = K8sTimer::new("events", "list");
    let list = api.list(&lp).await;
    t.finish(list.is_ok());
    let list = list?;
    let mut events: Vec<EventSummary> = list.iter().map(event_summary).collect();
    events.sort_by_key(|e| std::cmp::Reverse(sort_key(e)));
    Ok(Json(EventListResponse { events }))
}

pub async fn list_cluster(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<EventListResponse>, AppError> {
    let api = state.events_api_all();
    let selector = merge_field_selectors(
        query
            .involved_object
            .map(|name| format!("involvedObject.name={name}")),
        query.field_selector,
    );
    let mut lp = ListParams::default();
    if let Some(sel) = selector {
        lp = lp.fields(&sel);
    }
    let t = K8sTimer::new("events", "list");
    let list = api.list(&lp).await;
    t.finish(list.is_ok());
    let list = list?;
    let allow_all = state.allowed_namespaces.is_empty();
    let mut events: Vec<EventSummary> = list
        .iter()
        .filter(|e| {
            if allow_all {
                return true;
            }
            match e.metadata.namespace.as_deref() {
                Some(ns) => state.is_namespace_allowed(ns),
                None => true,
            }
        })
        .map(event_summary)
        .collect();
    events.sort_by_key(|e| std::cmp::Reverse(sort_key(e)));
    Ok(Json(EventListResponse { events }))
}
