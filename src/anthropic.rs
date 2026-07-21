//! Multi-provider Anthropic Messages API client.
//!
//! Supports three provider backends:
//!   - **Native**: direct Anthropic API (`api.anthropic.com`)
//!   - **Vertex AI**: Google Cloud `streamRawPredict` endpoint
//!   - **Bedrock**: AWS Bedrock (stubbed -- coming soon)
//!
//! Calls the streaming Messages API directly via `reqwest` and forwards
//! response text through a `tokio::sync::mpsc` channel so the caller can
//! convert the chunks into SSE events without buffering the entire response.
//!
//! Only the subset of the API surface needed by Deckwatch diagnostics and
//! AI-fix is implemented. Adding tool use, multi-turn, etc. is
//! straightforward but deliberately left out to keep the attack surface
//! small (see `docs/AI_SAFETY.md`).

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Which cloud provider hosts the Anthropic model.
#[derive(Debug, Clone)]
pub enum Provider {
    /// Direct Anthropic API with an API key.
    Native { api_key: String },
    /// Google Vertex AI via `streamRawPredict`.
    VertexAi {
        project_id: String,
        region: String,
        access_token: String,
    },
    /// AWS Bedrock (stubbed -- requires SigV4 signing).
    Bedrock { region: String, model_id: String },
}

/// A minimal Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    provider: Provider,
    model: String,
}

impl AnthropicClient {
    /// Create a new client using the native Anthropic API.
    /// Uses `claude-sonnet-4-20250514` by default.
    pub fn native(api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            provider: Provider::Native { api_key },
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Create a new client targeting Google Vertex AI.
    pub fn vertex(project_id: String, region: String, access_token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            provider: Provider::VertexAi {
                project_id,
                region,
                access_token,
            },
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Create a new client targeting AWS Bedrock (stubbed).
    pub fn bedrock(region: String, model_id: String) -> Self {
        let model_id = if model_id.is_empty() {
            "anthropic.claude-sonnet-4-20250514-v1:0".to_string()
        } else {
            model_id
        };
        Self {
            http: reqwest::Client::new(),
            provider: Provider::Bedrock {
                region,
                model_id: model_id.clone(),
            },
            model: model_id,
        }
    }

    /// Backward-compatible constructor. Equivalent to `AnthropicClient::native`.
    #[allow(dead_code)]
    pub fn new(api_key: String) -> Self {
        Self::native(api_key)
    }

    /// Build the endpoint URL for the configured provider.
    fn endpoint_url(&self) -> String {
        match &self.provider {
            Provider::Native { .. } => ANTHROPIC_API_URL.to_string(),
            Provider::VertexAi {
                project_id, region, ..
            } => {
                format!(
                    "https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}\
                     /locations/{region}/publishers/anthropic/models/{model}:streamRawPredict",
                    model = self.model,
                )
            }
            Provider::Bedrock { region, model_id } => {
                format!(
                    "https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/invoke-with-response-stream"
                )
            }
        }
    }

    /// Build the request body. Vertex AI omits the `model` field (it's in
    /// the URL path). Native and Bedrock include it.
    fn build_body(&self, prompt: &str, stream: bool) -> serde_json::Value {
        match &self.provider {
            Provider::VertexAi { .. } => {
                serde_json::json!({
                    "anthropic_version": ANTHROPIC_VERSION,
                    "max_tokens": DEFAULT_MAX_TOKENS,
                    "stream": stream,
                    "messages": [{"role": "user", "content": prompt}]
                })
            }
            _ => {
                serde_json::json!({
                    "model": self.model,
                    "max_tokens": DEFAULT_MAX_TOKENS,
                    "stream": stream,
                    "messages": [{"role": "user", "content": prompt}]
                })
            }
        }
    }

    /// Build provider-specific auth headers.
    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        match &self.provider {
            Provider::Native { api_key } => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
                );
                headers.insert(
                    "anthropic-version",
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
            }
            Provider::VertexAi { access_token, .. } => {
                let bearer = format!("Bearer {access_token}");
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&bearer).unwrap_or_else(|_| HeaderValue::from_static("")),
                );
                // Vertex AI also requires the anthropic-version header.
                headers.insert(
                    "anthropic-version",
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
            }
            Provider::Bedrock { .. } => {
                // TODO: AWS SigV4 signing is required for Bedrock.
                // For now, headers are empty; the send will fail with a
                // clear error from the stub check in message_stream / message.
            }
        }
        headers
    }

    /// Return a human-readable provider name for error messages.
    fn provider_name(&self) -> &'static str {
        match &self.provider {
            Provider::Native { .. } => "Anthropic",
            Provider::VertexAi { .. } => "Vertex AI",
            Provider::Bedrock { .. } => "Bedrock",
        }
    }

    /// Send a message and return the full response text (non-streaming).
    #[allow(dead_code)]
    pub async fn message(&self, prompt: &str) -> Result<String, String> {
        if matches!(&self.provider, Provider::Bedrock { .. }) {
            return Err("AWS Bedrock provider is not yet implemented. \
                 SigV4 request signing is required. Coming soon."
                .to_string());
        }

        let body = self.build_body(prompt, false);
        let url = self.endpoint_url();

        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{} API request failed: {e}", self.provider_name()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "{} API error {status}: {text}",
                self.provider_name()
            ));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse {} response: {e}", self.provider_name()))?;

        // Extract text from content blocks.
        let text = json["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        Ok(text)
    }

    /// Send a message and stream response text chunks through the channel.
    ///
    /// The Anthropic streaming API returns SSE events. We parse each line,
    /// extract `content_block_delta` text deltas, and forward them. The
    /// channel is closed (dropped) when the stream ends or on error.
    ///
    /// Vertex AI's `streamRawPredict` returns the same SSE format as the
    /// native API, so the streaming parser is shared.
    ///
    /// Bedrock uses a different event-stream format (AWS event stream) and
    /// is not yet implemented.
    pub async fn message_stream(
        &self,
        prompt: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), String> {
        if matches!(&self.provider, Provider::Bedrock { .. }) {
            return Err("AWS Bedrock provider is not yet implemented. \
                 SigV4 request signing is required. Coming soon."
                .to_string());
        }

        let body = self.build_body(prompt, true);
        let url = self.endpoint_url();

        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{} API request failed: {e}", self.provider_name()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "{} API error {status}: {text}",
                self.provider_name()
            ));
        }

        // The response body is an SSE stream: each event is prefixed with
        // `event: <type>\n` followed by `data: <json>\n\n`.
        //
        // We read line by line, tracking the current event type so we can
        // identify `content_block_delta` events and extract their text.
        let bytes_stream = resp.bytes_stream();
        use futures::StreamExt;
        let mut byte_buf = Vec::new();
        let mut current_event_type = String::new();

        let mut stream = bytes_stream;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| format!("stream read error: {e}"))?;
            byte_buf.extend_from_slice(&chunk);

            // Process complete lines (delimited by \n).
            while let Some(newline_pos) = byte_buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = byte_buf.drain(..=newline_pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim();

                if line.is_empty() {
                    // Empty line = end of SSE event. Reset event type.
                    current_event_type.clear();
                    continue;
                }

                if let Some(event_type) = line.strip_prefix("event: ") {
                    current_event_type = event_type.trim().to_string();
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if current_event_type == "content_block_delta" {
                        if let Some(text) = extract_delta_text(data) {
                            if tx.send(text).await.is_err() {
                                // Receiver dropped (client disconnected).
                                return Ok(());
                            }
                        }
                    } else if current_event_type == "message_stop" {
                        // Stream is complete.
                        return Ok(());
                    } else if current_event_type == "error" {
                        // The API sent an error event.
                        let msg = extract_error_message(data)
                            .unwrap_or_else(|| "unknown streaming error".to_string());
                        return Err(msg);
                    }
                }
            }
        }

        Ok(())
    }
}

/// Exchange a GCP service account key JSON for an OAuth2 access token.
///
/// Steps:
///   1. Parse the SA key JSON to extract `client_email` and `private_key`.
///   2. Create a JWT signed with RS256 (using `jsonwebtoken`).
///   3. POST to `https://oauth2.googleapis.com/token` with the JWT assertion.
///   4. Return the `access_token` from the response.
pub async fn exchange_sa_key_for_token(sa_key_json: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct SaKey {
        client_email: String,
        private_key: String,
    }

    let sa_key: SaKey = serde_json::from_str(sa_key_json)
        .map_err(|e| format!("failed to parse GCP SA key JSON: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock error: {e}"))?
        .as_secs();

    let claims = serde_json::json!({
        "iss": sa_key.client_email,
        "scope": "https://www.googleapis.com/auth/cloud-platform",
        "aud": "https://oauth2.googleapis.com/token",
        "iat": now,
        "exp": now + 3600,
    });

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(sa_key.private_key.as_bytes())
        .map_err(|e| format!("failed to parse GCP SA private key: {e}"))?;
    let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
        .map_err(|e| format!("failed to sign GCP JWT: {e}"))?;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let http = reqwest::Client::new();
    let resp = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await
        .map_err(|e| format!("GCP token exchange request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("GCP token exchange error {status}: {text}"));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse GCP token response: {e}"))?;

    Ok(token_resp.access_token)
}

/// SSE data payload for `content_block_delta` events.
#[derive(Deserialize)]
struct ContentBlockDelta {
    delta: Option<DeltaPayload>,
}

#[derive(Deserialize)]
struct DeltaPayload {
    text: Option<String>,
}

/// Extract the text field from a `content_block_delta` SSE data line.
fn extract_delta_text(data: &str) -> Option<String> {
    let parsed: ContentBlockDelta = serde_json::from_str(data).ok()?;
    parsed.delta?.text
}

/// Extract a human-readable error message from an `error` SSE event.
fn extract_error_message(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    v["error"]["message"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| v["message"].as_str().map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_delta_text_parses_valid_payload() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(extract_delta_text(data), Some("Hello".to_string()));
    }

    #[test]
    fn extract_delta_text_returns_none_for_missing_text() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta"}}"#;
        assert_eq!(extract_delta_text(data), None);
    }

    #[test]
    fn extract_delta_text_returns_none_for_invalid_json() {
        assert_eq!(extract_delta_text("not json"), None);
    }

    #[test]
    fn extract_error_message_parses_api_error() {
        let data =
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}"#;
        assert_eq!(
            extract_error_message(data),
            Some("Rate limited".to_string())
        );
    }

    #[test]
    fn extract_error_message_returns_none_for_missing_message() {
        let data = r#"{"type":"error","error":{"type":"unknown"}}"#;
        assert_eq!(extract_error_message(data), None);
    }

    #[test]
    fn client_new_uses_default_model() {
        let client = AnthropicClient::new("sk-test-key".to_string());
        assert_eq!(client.model, DEFAULT_MODEL);
    }

    #[test]
    fn native_auth_headers_contain_required_fields() {
        let client = AnthropicClient::native("sk-ant-test123".to_string());
        let headers = client.auth_headers();
        assert_eq!(headers.get("x-api-key").unwrap(), "sk-ant-test123");
        assert_eq!(headers.get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn vertex_auth_headers_contain_bearer_token() {
        let client = AnthropicClient::vertex(
            "my-project".to_string(),
            "us-central1".to_string(),
            "ya29.test-token".to_string(),
        );
        let headers = client.auth_headers();
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer ya29.test-token"
        );
        assert_eq!(headers.get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        // Native x-api-key should NOT be present.
        assert!(headers.get("x-api-key").is_none());
    }

    #[test]
    fn bedrock_auth_headers_have_content_type_only() {
        let client = AnthropicClient::bedrock(
            "us-east-1".to_string(),
            "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
        );
        let headers = client.auth_headers();
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        // No API key or bearer token -- SigV4 is not yet implemented.
        assert!(headers.get("x-api-key").is_none());
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn native_endpoint_url() {
        let client = AnthropicClient::native("key".to_string());
        assert_eq!(client.endpoint_url(), ANTHROPIC_API_URL);
    }

    #[test]
    fn vertex_endpoint_url() {
        let client = AnthropicClient::vertex(
            "my-project".to_string(),
            "us-central1".to_string(),
            "token".to_string(),
        );
        let url = client.endpoint_url();
        assert!(url.starts_with("https://us-central1-aiplatform.googleapis.com/"));
        assert!(url.contains("/projects/my-project/"));
        assert!(url.contains("/locations/us-central1/"));
        assert!(url.contains(&format!("/models/{}:streamRawPredict", DEFAULT_MODEL)));
    }

    #[test]
    fn bedrock_endpoint_url() {
        let client = AnthropicClient::bedrock(
            "us-east-1".to_string(),
            "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
        );
        let url = client.endpoint_url();
        assert!(url.starts_with("https://bedrock-runtime.us-east-1.amazonaws.com/"));
        assert!(url.contains("/model/anthropic.claude-sonnet-4-20250514-v1:0/"));
    }

    #[test]
    fn native_body_includes_model() {
        let client = AnthropicClient::native("key".to_string());
        let body = client.build_body("hello", true);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn vertex_body_omits_model() {
        let client =
            AnthropicClient::vertex("proj".to_string(), "region".to_string(), "tok".to_string());
        let body = client.build_body("test prompt", false);
        assert!(
            body.get("model").is_none(),
            "Vertex AI body should not contain 'model'"
        );
        assert_eq!(body["anthropic_version"], ANTHROPIC_VERSION);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["content"], "test prompt");
    }

    #[test]
    fn bedrock_body_includes_model() {
        let client = AnthropicClient::bedrock(
            "us-west-2".to_string(),
            "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
        );
        let body = client.build_body("analyze this", true);
        assert_eq!(body["model"], "anthropic.claude-sonnet-4-20250514-v1:0");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn bedrock_default_model_id() {
        let client = AnthropicClient::bedrock("us-east-1".to_string(), String::new());
        assert_eq!(client.model, "anthropic.claude-sonnet-4-20250514-v1:0");
    }

    #[test]
    fn test_message_request_body_shape() {
        // Construct the request body the same way the client does for
        // message_stream and verify its JSON structure.
        let client = AnthropicClient::native("key".to_string());
        let body = client.build_body("Analyze this log output.", true);

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["stream"], true);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Analyze this log output.");
    }

    #[test]
    fn test_parse_error_event() {
        // An SSE `event: error` payload should be parsed by extract_error_message.
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(extract_error_message(data), Some("Overloaded".to_string()));

        // Fallback to top-level "message" field when "error.message" is absent.
        let data_fallback = r#"{"type":"error","message":"Something went wrong"}"#;
        assert_eq!(
            extract_error_message(data_fallback),
            Some("Something went wrong".to_string())
        );

        // Completely missing message fields should return None.
        let data_no_msg = r#"{"type":"error"}"#;
        assert_eq!(extract_error_message(data_no_msg), None);
    }

    #[test]
    fn test_parse_message_stop() {
        // A `message_stop` event carries a data payload but no text delta.
        // extract_delta_text should return None (it's not a content_block_delta).
        let data = r#"{"type":"message_stop"}"#;
        assert_eq!(extract_delta_text(data), None);
    }

    #[test]
    fn test_empty_delta_text() {
        // A content_block_delta with an empty text string should return
        // Some("") -- the empty string is valid, not None.
        let data =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}"#;
        assert_eq!(extract_delta_text(data), Some("".to_string()));
    }

    #[test]
    fn test_model_default() {
        assert_eq!(DEFAULT_MODEL, "claude-sonnet-4-20250514");
        // Also verify the client stores this default.
        let client = AnthropicClient::new("sk-test".to_string());
        assert_eq!(client.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn provider_name_native() {
        let client = AnthropicClient::native("key".to_string());
        assert_eq!(client.provider_name(), "Anthropic");
    }

    #[test]
    fn provider_name_vertex() {
        let client =
            AnthropicClient::vertex("proj".to_string(), "region".to_string(), "tok".to_string());
        assert_eq!(client.provider_name(), "Vertex AI");
    }

    #[test]
    fn provider_name_bedrock() {
        let client = AnthropicClient::bedrock("us-east-1".to_string(), "model-id".to_string());
        assert_eq!(client.provider_name(), "Bedrock");
    }

    #[test]
    fn backward_compat_new_is_native() {
        let client = AnthropicClient::new("sk-ant-key".to_string());
        assert!(matches!(client.provider, Provider::Native { .. }));
        let headers = client.auth_headers();
        assert_eq!(headers.get("x-api-key").unwrap(), "sk-ant-key");
    }
}
