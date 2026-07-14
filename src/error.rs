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
}

/// Map common Kubernetes API error strings to human-friendly messages.
/// The raw message is preserved in debug logs for troubleshooting.
fn friendly_kube_message(raw: &str) -> String {
    if raw.contains("is forbidden") {
        return "You don't have permission to perform this action.".to_string();
    }
    if raw.contains("already exists") {
        return format!(
            "That name is already taken. {}",
            raw.split(':').last().unwrap_or("").trim()
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
        };

        metrics::record_error("handler", error_type);

        let body = serde_json::json!({
            "error": error_type,
            "message": message,
        });

        (status, axum::Json(body)).into_response()
    }
}
