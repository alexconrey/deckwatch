use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::debug;

use crate::metrics;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("Namespace '{0}' is not in the allowed list")]
    NamespaceNotAllowed(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    /// A plugin call the request depended on failed. `summary` is the first
    /// error as `code: message`; `events` is everything the call reported,
    /// already sanitized. Returned as 502 with the events in the body.
    #[error("Plugin call failed: {summary}")]
    PluginFailed {
        summary: String,
        events: Vec<crate::plugin_events::PluginEventReport>,
    },
}

/// Map common Kubernetes API error strings to human-friendly messages.
/// The raw message is preserved in debug logs for troubleshooting.
pub(crate) fn friendly_kube_message(raw: &str) -> String {
    if raw.contains("is forbidden") {
        return "You don't have permission to perform this action.".to_string();
    }
    if raw.contains("already exists") {
        return format!(
            "That name is already taken. {}",
            raw.split(':').next_back().unwrap_or("").trim()
        );
    }
    if raw.contains("not found") && raw.contains("namespaces") {
        return "The namespace doesn't exist.".to_string();
    }
    if raw.contains("ImagePullBackOff") || raw.contains("ErrImagePull") {
        return "The container image couldn't be pulled. Check the image name and tag.".to_string();
    }
    if raw.contains("Unauthorized") || raw.contains("unauthorized") {
        return "Authentication failed. Check your credentials.".to_string();
    }
    if raw.contains("exceeded quota") {
        return "Resource quota exceeded. Ask your cluster admin to increase limits.".to_string();
    }
    // Fall through to original
    raw.to_string()
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_type, message) = match &self {
            AppError::Kube(kube::Error::Api(err)) => {
                let status = StatusCode::from_u16(err.code).unwrap_or(StatusCode::BAD_GATEWAY);
                debug!(raw_kube_error = %err.message, "Kubernetes API error (raw)");
                let friendly = friendly_kube_message(&err.message);
                (status, "kube_error", friendly)
            }
            AppError::Kube(e) => (StatusCode::BAD_GATEWAY, "kube_error", e.to_string()),
            AppError::NamespaceNotAllowed(_) => (
                StatusCode::FORBIDDEN,
                "namespace_not_allowed",
                self.to_string(),
            ),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found", self.to_string()),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            AppError::PluginFailed { .. } => {
                (StatusCode::BAD_GATEWAY, "plugin_failed", self.to_string())
            }
        };

        metrics::record_error("handler", error_type);

        let mut body = serde_json::json!({
            "error": error_type,
            "message": message,
        });
        if let AppError::PluginFailed { events, .. } = &self {
            body["plugin_events"] = serde_json::to_value(events).unwrap_or_default();
        }

        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
