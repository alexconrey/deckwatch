//! Builds the Anthropic client for the AI provider selected in Settings,
//! shared by AI diagnostics and AI fix.
//!
//! Credentials are looked up in this order:
//!   1. the encrypted credential in the database (bound to the endpoint
//!      hosts recorded when it was saved);
//!   2. the `deckwatch-anthropic-api-key` / `deckwatch-gcp-sa-key`
//!      Kubernetes Secret in the workload's namespace;
//!   3. (native only) the Secret named by an override env var or by
//!      `ai_provider.api_key_secret`.
//!
//! Secret-sourced credentials are only sent to the default endpoints; see
//! `crate::outbound::binding`. Every binding check runs before a request is
//! built.

use crate::anthropic::{exchange_sa_key_for_token, AnthropicClient, AwsAuth};
use crate::error::AppError;
use crate::handlers::settings::{load_settings_from_db, AiProviderConfig, DeckwatchSettings};
use crate::outbound::{CredentialSource, EndpointId, EndpointSettings, OutboundHttp, Purpose};
use crate::state::AppState;

/// The two credential kinds stored by `POST /api/settings/credentials`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialKind {
    AnthropicApiKey,
    GcpServiceAccountKey,
}

impl CredentialKind {
    /// Default Secret name and data key for the Kubernetes fallback.
    fn default_secret(self) -> (&'static str, &'static str) {
        match self {
            CredentialKind::AnthropicApiKey => ("deckwatch-anthropic-api-key", "api-key"),
            CredentialKind::GcpServiceAccountKey => ("deckwatch-gcp-sa-key", "gcp-sa-key"),
        }
    }
}

/// Read a single key from a Kubernetes Secret.
async fn read_secret_key(
    state: &AppState,
    ns: &str,
    secret_name: &str,
    data_key: &str,
) -> Result<String, AppError> {
    let secrets_api = state.secrets_api(ns)?;
    let secret = secrets_api.get(secret_name).await.map_err(|_| {
        AppError::BadRequest(
            "AI provider credentials are not configured. \
             Go to Settings \u{2192} AI Providers to set up your API key."
                .to_string(),
        )
    })?;
    secret
        .data
        .as_ref()
        .and_then(|d| d.get(data_key))
        .map(|v| String::from_utf8_lossy(&v.0).to_string())
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Secret '{secret_name}' is missing the '{data_key}' key"
            ))
        })
}

/// The encrypted credential from settings, decrypted, with its binding.
fn stored_credential(
    state: &AppState,
    settings: &DeckwatchSettings,
    kind: CredentialKind,
) -> Option<(String, CredentialSource)> {
    let creds = settings.credentials.as_ref()?;
    let (encrypted, binding) = match kind {
        CredentialKind::AnthropicApiKey => (
            creds.anthropic_api_key.as_deref(),
            &creds.anthropic_api_key_binding,
        ),
        CredentialKind::GcpServiceAccountKey => {
            (creds.gcp_sa_key.as_deref(), &creds.gcp_sa_key_binding)
        }
    };
    let plaintext = crate::crypto::decrypt(&state.encryption_key, encrypted?).ok()?;
    Some((plaintext, CredentialSource::Stored(binding.clone())))
}

/// Stored credential first, then the default-named Kubernetes Secret.
async fn read_credential(
    state: &AppState,
    ns: &str,
    settings: &DeckwatchSettings,
    kind: CredentialKind,
) -> Result<(String, CredentialSource), AppError> {
    if let Some(found) = stored_credential(state, settings, kind) {
        return Ok(found);
    }
    let (secret_name, data_key) = kind.default_secret();
    let value = read_secret_key(state, ns, secret_name, data_key).await?;
    Ok((
        value,
        CredentialSource::KubernetesSecret(secret_name.to_string()),
    ))
}

fn bad_request(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(e.to_string())
}

/// Build an [`AnthropicClient`] for the provider selected in settings.
///
/// `secret_env_overrides` name env vars that may hold an alternative Secret
/// name for the native API key (checked in order).
pub async fn build_client(
    state: &AppState,
    http: &OutboundHttp,
    ns: &str,
    secret_env_overrides: &[&str],
) -> Result<AnthropicClient, AppError> {
    let settings = load_settings_from_db(state).await;
    let endpoints: EndpointSettings = settings.endpoints.clone().unwrap_or_default();
    let policy = http.policy();

    match &settings.ai_provider {
        AiProviderConfig::Native { api_key_secret } => {
            let api = endpoints
                .resolve(EndpointId::AnthropicApi, &policy)
                .map_err(bad_request)?;
            let (key, source) = match read_credential(
                state,
                ns,
                &settings,
                CredentialKind::AnthropicApiKey,
            )
            .await
            {
                Ok(found) => found,
                Err(_) => {
                    let secret_name = secret_env_overrides
                        .iter()
                        .find_map(|var| std::env::var(var).ok())
                        .unwrap_or_else(|| api_key_secret.clone());
                    let key = read_secret_key(state, ns, &secret_name, "api-key").await?;
                    (key, CredentialSource::KubernetesSecret(secret_name))
                }
            };
            AnthropicClient::native(http.client(Purpose::AiInference), &api, key, &source)
                .map_err(bad_request)
        }
        AiProviderConfig::VertexAi {
            project_id,
            region,
            sa_key_secret,
        } => {
            let token_endpoint = endpoints
                .resolve(EndpointId::VertexToken, &policy)
                .map_err(bad_request)?;
            let api = endpoints
                .resolve(EndpointId::VertexApi, &policy)
                .map_err(bad_request)?;
            let (sa_json, source) =
                match read_credential(state, ns, &settings, CredentialKind::GcpServiceAccountKey)
                    .await
                {
                    Ok(found) => found,
                    Err(_) => {
                        let key = read_secret_key(state, ns, sa_key_secret, "gcp-sa-key").await?;
                        (
                            key,
                            CredentialSource::KubernetesSecret(sa_key_secret.clone()),
                        )
                    }
                };
            let token = exchange_sa_key_for_token(
                &http.client(Purpose::AiToken),
                &token_endpoint,
                &sa_json,
                &source,
            )
            .await
            .map_err(|e| AppError::BadRequest(format!("GCP token exchange failed: {e}")))?;
            AnthropicClient::vertex(
                http.client(Purpose::AiInference),
                &api,
                project_id.clone(),
                region.clone(),
                token,
                &source,
            )
            .map_err(bad_request)
        }
        AiProviderConfig::Bedrock { region, model_id } => {
            // Bedrock uses the pod's AWS identity (IRSA / pod identity) --
            // no stored secret.
            let api = endpoints
                .resolve(EndpointId::Bedrock, &policy)
                .map_err(bad_request)?;
            AnthropicClient::bedrock(
                http.client(Purpose::AiInference),
                &api,
                region.clone(),
                model_id.clone(),
                AwsAuth::Ambient(Box::new(http.object_store_client_options())),
            )
            .map_err(bad_request)
        }
    }
}
