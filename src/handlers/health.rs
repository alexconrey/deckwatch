use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn features(State(state): State<AppState>) -> Json<Value> {
    let settings = crate::handlers::settings::load_settings_from_db(&state).await;
    Json(json!({
        "prometheus": settings.prometheus_enabled,
        "registry": settings.registry_enabled,
    }))
}

pub async fn readyz(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match state.kube_client.apiserver_version().await {
        Ok(_) => Ok(Json(json!({ "status": "ok" }))),
        Err(e) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "error",
                "message": format!("Cannot reach Kubernetes API: {e}")
            })),
        )),
    }
}
