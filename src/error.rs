use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

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

    /// Per-namespace AI-job quota is exhausted. `retry_after_secs` is the
    /// number of seconds until the oldest job in the sliding window ages
    /// out — surfaced to the client both in the JSON body (so the UI can
    /// render a countdown) and in the standard `Retry-After` header.
    #[error("AI job quota exceeded for namespace '{namespace}': {used}/{limit} used, retry in {retry_after_secs}s")]
    RateLimited {
        namespace: String,
        limit: u32,
        used: u32,
        retry_after_secs: u64,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Rate-limited responses need a body + header we can't build via
        // the shared `(status, json).into_response()` shortcut below, so
        // handle them first.
        if let AppError::RateLimited {
            namespace,
            limit,
            used,
            retry_after_secs,
        } = &self
        {
            let body = serde_json::json!({
                "error": "rate_limited",
                "message": self.to_string(),
                "namespace": namespace,
                "limit": limit,
                "used": used,
                "retry_after_secs": retry_after_secs,
            });
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
            if let Ok(hv) = retry_after_secs.to_string().parse() {
                resp.headers_mut().insert("Retry-After", hv);
            }
            return resp;
        }

        let (status, error_type, message) = match &self {
            AppError::Kube(kube::Error::Api(err)) => {
                let status = StatusCode::from_u16(err.code).unwrap_or(StatusCode::BAD_GATEWAY);
                (status, "kube_error", err.message.clone())
            }
            AppError::Kube(e) => (StatusCode::BAD_GATEWAY, "kube_error", e.to_string()),
            AppError::NamespaceNotAllowed(_) => {
                (StatusCode::FORBIDDEN, "namespace_not_allowed", self.to_string())
            }
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found", self.to_string()),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            AppError::RateLimited { .. } => unreachable!("handled above"),
        };

        let body = serde_json::json!({
            "error": error_type,
            "message": message,
        });

        (status, axum::Json(body)).into_response()
    }
}
