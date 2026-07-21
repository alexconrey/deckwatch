//! Lightweight Anthropic Messages API client.
//!
//! Calls the streaming Messages API directly via `reqwest` and forwards
//! response text through a `tokio::sync::mpsc` channel so the caller can
//! convert the chunks into SSE events without buffering the entire response.
//!
//! Only the subset of the API surface needed by Deckwatch diagnostics and
//! AI-fix is implemented. Adding tool use, multi-turn, etc. is
//! straightforward but deliberately left out to keep the attack surface
//! small (see `docs/AI_SAFETY.md`).

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::Deserialize;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// A minimal Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    /// Create a new client with the given API key.
    /// Uses `claude-sonnet-4-20250514` by default.
    pub fn new(api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Send a message and return the full response text (non-streaming).
    #[allow(dead_code)]
    pub async fn message(&self, prompt: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "stream": false,
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self
            .http
            .post(ANTHROPIC_API_URL)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Anthropic API request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Anthropic API error {status}: {text}"));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse Anthropic response: {e}"))?;

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
    pub async fn message_stream(
        &self,
        prompt: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "stream": true,
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self
            .http
            .post(ANTHROPIC_API_URL)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Anthropic API request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Anthropic API error {status}: {text}"));
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

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }
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
    fn auth_headers_contain_required_fields() {
        let client = AnthropicClient::new("sk-ant-test123".to_string());
        let headers = client.auth_headers();
        assert_eq!(headers.get("x-api-key").unwrap(), "sk-ant-test123");
        assert_eq!(headers.get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }
}
