// Unit tests for src/error.rs
//
// To integrate, append to src/error.rs:
//
//     #[cfg(test)]
//     #[path = "error_tests.rs"]
//     mod tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        (status, v)
    }

    #[tokio::test]
    async fn namespace_not_allowed_maps_to_403() {
        let err = AppError::NamespaceNotAllowed("kube-system".to_string());
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "namespace_not_allowed");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("kube-system"));
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let err = AppError::NotFound("deploy/foo".to_string());
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not_found");
        assert!(body["message"].as_str().unwrap().contains("deploy/foo"));
    }

    #[tokio::test]
    async fn bad_request_maps_to_400() {
        let err = AppError::BadRequest("missing field".to_string());
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "bad_request");
        assert!(body["message"].as_str().unwrap().contains("missing field"));
    }

    #[tokio::test]
    async fn kube_api_error_preserves_status_code() {
        // 409 Conflict from the Kubernetes API should be forwarded verbatim.
        let api_err = kube::error::ErrorResponse {
            status: "Failure".to_string(),
            message: "already exists".to_string(),
            reason: "AlreadyExists".to_string(),
            code: 409,
        };
        let err = AppError::Kube(kube::Error::Api(api_err));
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "kube_error");
        assert_eq!(body["message"], "already exists");
    }

    #[tokio::test]
    async fn kube_api_error_invalid_code_falls_back_to_502() {
        let api_err = kube::error::ErrorResponse {
            status: "Failure".to_string(),
            message: "weird".to_string(),
            reason: "Unknown".to_string(),
            code: 9999, // invalid HTTP status
        };
        let err = AppError::Kube(kube::Error::Api(api_err));
        let (status, _body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }
}
