//! Multi-provider Anthropic Messages API client.
//!
//! Supports three provider backends:
//!   - **Native**: direct Anthropic API (`api.anthropic.com` by default)
//!   - **Vertex AI**: Google Cloud `streamRawPredict` endpoint
//!   - **Bedrock**: AWS Bedrock `InvokeModel`, signed with SigV4 using the
//!     standard AWS credential chain (IRSA, pod identity, environment).
//!
//! Calls the streaming Messages API directly via `reqwest` and forwards
//! response text through a `tokio::sync::mpsc` channel so the caller can
//! convert the chunks into SSE events without buffering the entire response.
//! Bedrock uses the non-streaming `InvokeModel` call (its streaming variant
//! uses the binary AWS event-stream framing) and forwards the reply as one
//! chunk.
//!
//! Endpoints come from Settings (`crate::outbound::endpoints`). Every
//! constructor checks the credential's binding first
//! (`crate::outbound::binding`): a key saved for one host is never sent to
//! another, and the error is returned before any request is built.
//!
//! Only the subset of the API surface needed by Deckwatch diagnostics and
//! AI-fix is implemented. Adding tool use, multi-turn, etc. is
//! straightforward but deliberately left out to keep the attack surface
//! small (see `docs/AI_SAFETY.md`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use object_store::aws::{AwsAuthorizer, AwsCredential, AwsCredentialProvider};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

use crate::outbound::{
    authorize, authorize_aws, error_chain, BindingError, CredentialSource, Endpoint, EndpointError,
};

const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_BEDROCK_MODEL: &str = "anthropic.claude-sonnet-4-20250514-v1:0";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Audience of the service-account JWT assertion. Google documents it as
/// fixed, including when the token request goes through a private endpoint.
const GOOGLE_TOKEN_AUDIENCE: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Credential labels used in binding errors.
pub const ANTHROPIC_API_KEY_LABEL: &str = "Anthropic API key";
pub const GCP_SA_KEY_LABEL: &str = "GCP service account key";

/// Errors from building a client or exchanging a token. `Endpoint` and
/// `Binding` are raised before any request is sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error("{0}")]
    Upstream(String),
}

/// AWS credentials for Bedrock.
#[derive(Clone)]
pub enum AwsAuth {
    /// The standard AWS chain (environment, web identity / IRSA, EKS Pod
    /// Identity, ECS, IMDS), resolved through `object_store`. The client
    /// options carry deckwatch's extra CAs for the STS / IMDS calls.
    Ambient(Box<object_store::ClientOptions>),
    /// Fixed credentials.
    Static {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
}

impl std::fmt::Debug for AwsAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AwsAuth::Ambient(_) => f.write_str("AwsAuth::Ambient"),
            AwsAuth::Static { access_key_id, .. } => f
                .debug_struct("AwsAuth::Static")
                .field("access_key_id", access_key_id)
                .finish_non_exhaustive(),
        }
    }
}

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
    /// AWS Bedrock `InvokeModel`.
    Bedrock {
        region: String,
        model_id: String,
        auth: AwsAuth,
    },
}

/// A minimal Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    provider: Provider,
    model: String,
    /// Fully resolved request URL.
    url: String,
}

impl AnthropicClient {
    /// Native Anthropic API at `api` (`/v1/messages` is appended). Fails if
    /// the key from `source` may not be sent to `api`.
    pub fn native(
        http: reqwest::Client,
        api: &Endpoint,
        api_key: String,
        source: &CredentialSource,
    ) -> Result<Self, ClientError> {
        authorize(ANTHROPIC_API_KEY_LABEL, source, api)?;
        Ok(Self {
            http,
            url: api.url(ANTHROPIC_MESSAGES_PATH),
            provider: Provider::Native { api_key },
            model: DEFAULT_MODEL.to_string(),
        })
    }

    /// Vertex AI `streamRawPredict` at `api` (`{region}` substituted).
    /// `source` is where the service account key behind `access_token`
    /// came from; the token is bound to the same hosts as the key.
    pub fn vertex(
        http: reqwest::Client,
        api: &Endpoint,
        project_id: String,
        region: String,
        access_token: String,
        source: &CredentialSource,
    ) -> Result<Self, ClientError> {
        authorize(GCP_SA_KEY_LABEL, source, api)?;
        let model = DEFAULT_MODEL.to_string();
        let url = api.url_for_region(
            &region,
            &format!(
                "/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models/{model}:streamRawPredict"
            ),
        )?;
        Ok(Self {
            http,
            url,
            provider: Provider::VertexAi {
                project_id,
                region,
                access_token,
            },
            model,
        })
    }

    /// Bedrock `InvokeModel` at `api` (`{region}` substituted). AWS
    /// credentials only go to AWS hosts (or loopback).
    pub fn bedrock(
        http: reqwest::Client,
        api: &Endpoint,
        region: String,
        model_id: String,
        auth: AwsAuth,
    ) -> Result<Self, ClientError> {
        let host = api.host_for_region(&region)?;
        authorize_aws(api, &host)?;
        let model_id = if model_id.is_empty() {
            DEFAULT_BEDROCK_MODEL.to_string()
        } else {
            model_id
        };
        let url = api.url_for_region(
            &region,
            &format!("/model/{}/invoke", encode_path_segment(&model_id)),
        )?;
        Ok(Self {
            http,
            url,
            provider: Provider::Bedrock {
                region,
                model_id: model_id.clone(),
                auth,
            },
            model: model_id,
        })
    }

    /// The request URL for the configured provider.
    fn endpoint_url(&self) -> &str {
        &self.url
    }

    /// Build the request body. Vertex AI and Bedrock omit the `model` field
    /// (it's in the URL path) and carry their own `anthropic_version`.
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
            Provider::Bedrock { .. } => {
                serde_json::json!({
                    "anthropic_version": BEDROCK_ANTHROPIC_VERSION,
                    "max_tokens": DEFAULT_MAX_TOKENS,
                    "messages": [{"role": "user", "content": prompt}]
                })
            }
            Provider::Native { .. } => {
                serde_json::json!({
                    "model": self.model,
                    "max_tokens": DEFAULT_MAX_TOKENS,
                    "stream": stream,
                    "messages": [{"role": "user", "content": prompt}]
                })
            }
        }
    }

    /// Build provider-specific auth headers. Bedrock's SigV4 headers are
    /// added per request by [`Self::bedrock_invoke`].
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
            Provider::Bedrock { .. } => {}
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
            return self.bedrock_invoke(prompt).await;
        }

        let body = self.build_body(prompt, false);

        let resp = self
            .http
            .post(self.endpoint_url())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "{} API request failed: {}",
                    self.provider_name(),
                    error_chain(&e)
                )
            })?;

        let json = self.success_json(resp).await?;
        Ok(response_text(&json))
    }

    /// Map a non-2xx response to an error and parse a 2xx body as JSON.
    async fn success_json(&self, resp: reqwest::Response) -> Result<serde_json::Value, String> {
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "{} API error {status}: {text}",
                self.provider_name()
            ));
        }
        resp.json()
            .await
            .map_err(|e| format!("failed to parse {} response: {e}", self.provider_name()))
    }

    /// Bedrock `InvokeModel`: sign with SigV4 and return the reply text.
    async fn bedrock_invoke(&self, prompt: &str) -> Result<String, String> {
        let Provider::Bedrock { region, auth, .. } = &self.provider else {
            return Err("not a Bedrock client".to_string());
        };
        let body = serde_json::to_vec(&self.build_body(prompt, false))
            .map_err(|e| format!("failed to encode Bedrock request: {e}"))?;
        let headers = sigv4_headers(auth, region, self.endpoint_url(), &body).await?;

        let resp = self
            .http
            .post(self.endpoint_url())
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Bedrock API request failed: {}", error_chain(&e)))?;

        let json = self.success_json(resp).await?;
        Ok(response_text(&json))
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
    /// Bedrock replies are fetched with the non-streaming `InvokeModel` call
    /// and forwarded as a single chunk.
    pub async fn message_stream(
        &self,
        prompt: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), String> {
        if matches!(&self.provider, Provider::Bedrock { .. }) {
            let text = self.bedrock_invoke(prompt).await?;
            // A dropped receiver means the client disconnected.
            let _ = tx.send(text).await;
            return Ok(());
        }

        let body = self.build_body(prompt, true);

        let resp = self
            .http
            .post(self.endpoint_url())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "{} API request failed: {}",
                    self.provider_name(),
                    error_chain(&e)
                )
            })?;

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
            let chunk =
                chunk_result.map_err(|e| format!("stream read error: {}", error_chain(&e)))?;
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

/// Concatenated text of a Messages API response's content blocks.
fn response_text(json: &serde_json::Value) -> String {
    json["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Percent-encode one URL path segment (Bedrock model IDs and ARNs contain
/// `:` and `/`). Only RFC 3986 unreserved characters pass through.
fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// SigV4 headers (`authorization`, `x-amz-date`, `x-amz-content-sha256`,
/// `x-amz-security-token` for temporary credentials) for a Bedrock POST.
async fn sigv4_headers(
    auth: &AwsAuth,
    region: &str,
    url: &str,
    body: &[u8],
) -> Result<HeaderMap, String> {
    let credential = match auth {
        AwsAuth::Static {
            access_key_id,
            secret_access_key,
            session_token,
        } => Arc::new(AwsCredential {
            key_id: access_key_id.clone(),
            secret_key: secret_access_key.clone(),
            token: session_token.clone(),
        }),
        AwsAuth::Ambient(options) => ambient_aws_credential(region, options).await?,
    };

    let mut request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri(url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .body(object_store::client::HttpRequestBody::from(body.to_vec()))
        .map_err(|e| format!("invalid Bedrock request: {e}"))?;
    AwsAuthorizer::new(&credential, "bedrock", region)
        .try_authorize(&mut request, None)
        .map_err(|e| format!("SigV4 signing failed: {e}"))?;
    Ok(request.headers().clone())
}

/// Credentials from the standard AWS chain. object_store only exposes that
/// chain through its S3 client, so one is built per region and only its
/// credential provider is used (the bucket is never contacted). Providers
/// are cached so temporary credentials are reused until they expire.
async fn ambient_aws_credential(
    region: &str,
    options: &object_store::ClientOptions,
) -> Result<Arc<AwsCredential>, String> {
    static PROVIDERS: OnceLock<Mutex<HashMap<String, AwsCredentialProvider>>> = OnceLock::new();
    let provider = {
        let mut providers = PROVIDERS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match providers.get(region) {
            Some(p) => p.clone(),
            None => {
                let s3 = crate::outbound::amazon_s3_builder_from_env(options.clone())
                    .with_bucket_name("deckwatch-bedrock-credentials")
                    .with_region(region)
                    .build()
                    .map_err(|e| format!("cannot set up AWS credentials: {e}"))?;
                let p = s3.credentials().clone();
                providers.insert(region.to_string(), p.clone());
                p
            }
        }
    };
    provider
        .get_credential()
        .await
        .map_err(|e| format!("cannot load AWS credentials: {e}"))
}

/// Exchange a GCP service account key JSON for an OAuth2 access token at
/// the `token` endpoint. Fails before sending if the key from `source` may
/// not be used with `token`.
///
/// Steps:
///   1. Parse the SA key JSON to extract `client_email` and `private_key`.
///   2. Create a JWT signed with RS256 (using `jsonwebtoken`).
///   3. POST it (form-encoded JWT bearer grant) to the token endpoint.
///   4. Return the `access_token` from the response.
pub async fn exchange_sa_key_for_token(
    http: &reqwest::Client,
    token: &Endpoint,
    sa_key_json: &str,
    source: &CredentialSource,
) -> Result<String, ClientError> {
    authorize(GCP_SA_KEY_LABEL, source, token)?;
    let upstream = ClientError::Upstream;

    #[derive(Deserialize)]
    struct SaKey {
        client_email: String,
        private_key: String,
    }

    let sa_key: SaKey = serde_json::from_str(sa_key_json)
        .map_err(|e| upstream(format!("failed to parse GCP SA key JSON: {e}")))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| upstream(format!("system clock error: {e}")))?
        .as_secs();

    let claims = serde_json::json!({
        "iss": sa_key.client_email,
        "scope": GOOGLE_CLOUD_PLATFORM_SCOPE,
        "aud": GOOGLE_TOKEN_AUDIENCE,
        "iat": now,
        "exp": now + 3600,
    });

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(sa_key.private_key.as_bytes())
        .map_err(|e| upstream(format!("failed to parse GCP SA private key: {e}")))?;
    let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
        .map_err(|e| upstream(format!("failed to sign GCP JWT: {e}")))?;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let resp = http
        .post(token.url(""))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await
        .map_err(|e| {
            upstream(format!(
                "GCP token exchange request failed: {}",
                error_chain(&e)
            ))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(upstream(format!(
            "GCP token exchange error {status}: {text}"
        )));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .map_err(|e| upstream(format!("failed to parse GCP token response: {e}")))?;

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
    use crate::outbound::{EndpointId, OutboundHttp, Purpose};

    fn http() -> reqwest::Client {
        OutboundHttp::default().client(Purpose::AiInference)
    }

    fn default_endpoint(id: EndpointId) -> Endpoint {
        Endpoint::default_for(id)
    }

    /// Credential saved before bindings existed: bound to the defaults.
    fn legacy() -> CredentialSource {
        CredentialSource::Stored(None)
    }

    fn native(key: &str) -> AnthropicClient {
        AnthropicClient::native(
            http(),
            &default_endpoint(EndpointId::AnthropicApi),
            key.to_string(),
            &legacy(),
        )
        .expect("default endpoint accepts a legacy key")
    }

    fn vertex(project: &str, region: &str, token: &str) -> AnthropicClient {
        AnthropicClient::vertex(
            http(),
            &default_endpoint(EndpointId::VertexApi),
            project.to_string(),
            region.to_string(),
            token.to_string(),
            &legacy(),
        )
        .expect("default endpoint accepts a legacy key")
    }

    fn bedrock(region: &str, model: &str) -> AnthropicClient {
        AnthropicClient::bedrock(
            http(),
            &default_endpoint(EndpointId::Bedrock),
            region.to_string(),
            model.to_string(),
            AwsAuth::Static {
                access_key_id: "AKIDEXAMPLE".to_string(),
                secret_access_key: "secret".to_string(),
                session_token: None,
            },
        )
        .expect("default Bedrock endpoint is an AWS host")
    }

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
    fn native_client_uses_default_model() {
        let client = native("sk-test-key");
        assert_eq!(client.model, DEFAULT_MODEL);
        assert!(matches!(client.provider, Provider::Native { .. }));
    }

    #[test]
    fn native_auth_headers_contain_required_fields() {
        let client = native("sk-ant-test123");
        let headers = client.auth_headers();
        assert_eq!(headers.get("x-api-key").unwrap(), "sk-ant-test123");
        assert_eq!(headers.get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn vertex_auth_headers_contain_bearer_token() {
        let client = vertex("my-project", "us-central1", "ya29.test-token");
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
        let client = bedrock("us-east-1", "anthropic.claude-sonnet-4-20250514-v1:0");
        let headers = client.auth_headers();
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        // SigV4 headers are added per request, not here.
        assert!(headers.get("x-api-key").is_none());
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn native_endpoint_url() {
        let client = native("key");
        assert_eq!(
            client.endpoint_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn native_endpoint_url_follows_configured_base() {
        let api = Endpoint::parse(
            EndpointId::AnthropicApi,
            "https://llm-gateway.example.com/anthropic/",
            &Default::default(),
        )
        .unwrap();
        let binding = crate::outbound::EndpointBinding::capture(
            &crate::outbound::EndpointSettings::default().with(
                EndpointId::AnthropicApi,
                "https://llm-gateway.example.com/anthropic",
            ),
            &[EndpointId::AnthropicApi],
            &Default::default(),
        )
        .unwrap();
        let client = AnthropicClient::native(
            http(),
            &api,
            "key".to_string(),
            &CredentialSource::Stored(Some(binding)),
        )
        .unwrap();
        assert_eq!(
            client.endpoint_url(),
            "https://llm-gateway.example.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn native_refuses_legacy_key_for_custom_host() {
        let api = Endpoint::parse(
            EndpointId::AnthropicApi,
            "https://llm-gateway.example.com",
            &Default::default(),
        )
        .unwrap();
        let err = AnthropicClient::native(http(), &api, "key".to_string(), &legacy())
            .err()
            .expect("legacy key is bound to api.anthropic.com");
        let msg = err.to_string();
        assert!(msg.contains("api.anthropic.com"), "{msg}");
        assert!(msg.contains("llm-gateway.example.com"), "{msg}");
        assert!(msg.contains("re-enter"), "{msg}");
    }

    #[test]
    fn vertex_endpoint_url() {
        let client = vertex("my-project", "us-central1", "token");
        let url = client.endpoint_url();
        assert!(url.starts_with("https://us-central1-aiplatform.googleapis.com/"));
        assert!(url.contains("/projects/my-project/"));
        assert!(url.contains("/locations/us-central1/"));
        assert!(url.contains(&format!("/models/{}:streamRawPredict", DEFAULT_MODEL)));
    }

    #[test]
    fn vertex_rejects_region_that_would_change_the_host() {
        let err = AnthropicClient::vertex(
            http(),
            &default_endpoint(EndpointId::VertexApi),
            "proj".to_string(),
            "evil.example.com#".to_string(),
            "tok".to_string(),
            &legacy(),
        )
        .err()
        .expect("region is interpolated into the host");
        assert!(matches!(
            err,
            ClientError::Endpoint(EndpointError::Region { .. })
        ));
    }

    #[test]
    fn bedrock_endpoint_url() {
        let client = bedrock("us-east-1", "anthropic.claude-sonnet-4-20250514-v1:0");
        assert_eq!(
            client.endpoint_url(),
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-sonnet-4-20250514-v1%3A0/invoke"
        );
    }

    #[test]
    fn bedrock_refuses_non_aws_host() {
        let api = Endpoint::parse(
            EndpointId::Bedrock,
            "https://bedrock.attacker.example",
            &Default::default(),
        )
        .unwrap();
        let err = AnthropicClient::bedrock(
            http(),
            &api,
            "us-east-1".to_string(),
            String::new(),
            AwsAuth::Static {
                access_key_id: "AKID".to_string(),
                secret_access_key: "secret".to_string(),
                session_token: None,
            },
        )
        .err()
        .expect("AWS credentials only go to AWS hosts");
        assert!(matches!(
            err,
            ClientError::Binding(BindingError::NotAwsHost { .. })
        ));
    }

    #[test]
    fn bedrock_accepts_vpc_endpoint_override() {
        let api = Endpoint::parse(
            EndpointId::Bedrock,
            "https://vpce-0123-abcd.bedrock-runtime.us-east-1.vpce.amazonaws.com",
            &Default::default(),
        )
        .unwrap();
        let client = AnthropicClient::bedrock(
            http(),
            &api,
            "us-east-1".to_string(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
            AwsAuth::Static {
                access_key_id: "AKID".to_string(),
                secret_access_key: "secret".to_string(),
                session_token: None,
            },
        )
        .unwrap();
        assert!(client.endpoint_url().starts_with(
            "https://vpce-0123-abcd.bedrock-runtime.us-east-1.vpce.amazonaws.com/model/"
        ));
    }

    #[test]
    fn encode_path_segment_escapes_reserved_characters() {
        assert_eq!(
            encode_path_segment("arn:aws:bedrock:us-east-1:1:inference-profile/us.anthropic.x"),
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A1%3Ainference-profile%2Fus.anthropic.x"
        );
        assert_eq!(encode_path_segment("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn native_body_includes_model() {
        let client = native("key");
        let body = client.build_body("hello", true);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn vertex_body_omits_model() {
        let client = vertex("proj", "region", "tok");
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
    fn bedrock_body_uses_bedrock_version_and_omits_model() {
        let client = bedrock("us-west-2", "anthropic.claude-sonnet-4-20250514-v1:0");
        let body = client.build_body("analyze this", true);
        assert!(body.get("model").is_none(), "model is in the Bedrock URL");
        assert!(body.get("stream").is_none(), "InvokeModel is not streamed");
        assert_eq!(body["anthropic_version"], BEDROCK_ANTHROPIC_VERSION);
        assert_eq!(body["messages"][0]["content"], "analyze this");
    }

    #[test]
    fn bedrock_default_model_id() {
        let client = bedrock("us-east-1", "");
        assert_eq!(client.model, DEFAULT_BEDROCK_MODEL);
    }

    #[test]
    fn test_message_request_body_shape() {
        // Construct the request body the same way the client does for
        // message_stream and verify its JSON structure.
        let client = native("key");
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
        assert_eq!(native("sk-test").model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn provider_name_native() {
        assert_eq!(native("key").provider_name(), "Anthropic");
    }

    #[test]
    fn provider_name_vertex() {
        assert_eq!(vertex("proj", "region", "tok").provider_name(), "Vertex AI");
    }

    #[test]
    fn provider_name_bedrock() {
        assert_eq!(bedrock("us-east-1", "model-id").provider_name(), "Bedrock");
    }

    #[test]
    fn aws_auth_debug_hides_the_secret() {
        let auth = AwsAuth::Static {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "very-secret".to_string(),
            session_token: Some("session".to_string()),
        };
        let debug = format!("{auth:?}");
        assert!(debug.contains("AKIDEXAMPLE"));
        assert!(!debug.contains("very-secret"));
        assert!(!debug.contains("session"));
    }

    #[test]
    fn response_text_joins_content_blocks() {
        let json = serde_json::json!({
            "content": [{"type": "text", "text": "Hello, "}, {"type": "text", "text": "world"}]
        });
        assert_eq!(response_text(&json), "Hello, world");
    }
}
