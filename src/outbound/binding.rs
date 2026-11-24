//! Credential binding: a stored credential is only ever sent to the endpoint
//! host it was saved for.
//!
//! Deckwatch has no authentication yet, so anyone who can reach it can change
//! an endpoint in Settings. Without binding, pointing the Anthropic endpoint
//! at another host would make deckwatch hand that host the stored API key.
//! The rules:
//!
//! * When a credential is saved, the current origins
//!   (`scheme://host[:port]`) of every endpoint it may be sent to are recorded
//!   next to it ([`EndpointBinding::capture`]).
//! * Before a request carrying the credential is built, the configured
//!   endpoint's origin must equal the recorded one ([`authorize`]). If it
//!   differs nothing is sent, and the error tells the operator to re-enter
//!   the credential for the new host.
//! * Credentials stored before binding existed (no record) are bound to the
//!   compiled-in defaults: endpoints could not be changed back then.
//! * Credentials read from Kubernetes Secrets carry no record and are only
//!   sent to the default endpoints.
//! * AWS credentials (IRSA, pod identity, environment; never stored by
//!   deckwatch) are only sent to AWS-owned hosts or loopback.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::endpoints::{
    is_aws_host, Endpoint, EndpointError, EndpointId, EndpointPolicy, EndpointSettings,
};

/// Endpoint origins a stored credential may be sent to, recorded when it was
/// saved. Serialized as `{"anthropic_api": "https://api.anthropic.com"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EndpointBinding(BTreeMap<EndpointId, String>);

impl EndpointBinding {
    /// Record the current origins of `ids`.
    pub fn capture(
        endpoints: &EndpointSettings,
        ids: &[EndpointId],
        policy: &EndpointPolicy,
    ) -> Result<Self, EndpointError> {
        let mut map = BTreeMap::new();
        for id in ids {
            let ep = endpoints.resolve(*id, policy)?;
            map.insert(*id, ep.origin().to_string());
        }
        Ok(Self(map))
    }

    /// The binding of a credential saved before endpoints were configurable:
    /// the default origins of `ids`.
    pub fn defaults(ids: &[EndpointId]) -> Self {
        Self(
            ids.iter()
                .map(|id| (*id, Endpoint::default_for(*id).origin().to_string()))
                .collect(),
        )
    }

    /// Recorded origin for `id`, if any.
    pub fn origin(&self, id: EndpointId) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EndpointId, &str)> {
        self.0.iter().map(|(id, origin)| (*id, origin.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Where a credential came from, which decides where it may be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Stored encrypted in the deckwatch database with the binding recorded
    /// when it was saved. `None`: saved before bindings existed, so bound to
    /// the defaults.
    Stored(Option<EndpointBinding>),
    /// Read from the named Kubernetes Secret: default endpoints only.
    KubernetesSecret(String),
}

impl CredentialSource {
    /// The origin this credential may be sent to for `id`.
    fn bound_origin(&self, id: EndpointId) -> String {
        let recorded = match self {
            CredentialSource::Stored(Some(binding)) => binding.origin(id),
            CredentialSource::Stored(None) | CredentialSource::KubernetesSecret(_) => None,
        };
        recorded
            .map(str::to_string)
            .unwrap_or_else(|| Endpoint::default_for(id).origin().to_string())
    }
}

/// A credential would have been sent somewhere it is not bound to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    #[error(
        "the stored {credential} was saved for {bound}, but the {endpoint} endpoint is now \
         {configured}. Deckwatch does not send stored credentials to a different host: \
         re-enter the {credential} in Settings to use it with {configured}"
    )]
    HostMismatch {
        credential: String,
        endpoint: EndpointId,
        bound: String,
        configured: String,
    },
    #[error(
        "the {credential} comes from Kubernetes Secret '{secret}', which deckwatch only sends \
         to the default {endpoint} endpoint ({default}). The endpoint is now {configured}: \
         save the {credential} in Settings to use it with {configured}"
    )]
    SecretOnlyForDefault {
        credential: String,
        secret: String,
        endpoint: EndpointId,
        default: String,
        configured: String,
    },
    #[error(
        "AWS credentials are only sent to AWS endpoints (*.amazonaws.com, *.amazonaws.com.cn, \
         *.api.aws) or localhost; the {endpoint} endpoint host is {host}"
    )]
    NotAwsHost { endpoint: EndpointId, host: String },
}

/// Display form of an origin: just `host[:port]` for https, the full origin
/// otherwise so a downgrade to http is visible.
pub fn display_origin(origin: &str) -> &str {
    origin.strip_prefix("https://").unwrap_or(origin)
}

/// Check that `credential` (a label such as "Anthropic API key") from
/// `source` may be sent to `endpoint`. Call before building the request.
pub fn authorize(
    credential: &str,
    source: &CredentialSource,
    endpoint: &Endpoint,
) -> Result<(), BindingError> {
    let bound = source.bound_origin(endpoint.id());
    if bound == endpoint.origin() {
        return Ok(());
    }
    let configured = display_origin(endpoint.origin()).to_string();
    Err(match source {
        CredentialSource::KubernetesSecret(secret) => BindingError::SecretOnlyForDefault {
            credential: credential.to_string(),
            secret: secret.clone(),
            endpoint: endpoint.id(),
            default: display_origin(&bound).to_string(),
            configured,
        },
        CredentialSource::Stored(_) => BindingError::HostMismatch {
            credential: credential.to_string(),
            endpoint: endpoint.id(),
            bound: display_origin(&bound).to_string(),
            configured,
        },
    })
}

/// Check that AWS credentials (ambient IRSA / pod identity / environment
/// credentials, which deckwatch never stores) may be sent to `host`, the
/// concrete host of `endpoint` with `{region}` substituted.
pub fn authorize_aws(endpoint: &Endpoint, host: &str) -> Result<(), BindingError> {
    let bare = host_without_port(host);
    if endpoint.is_loopback() || is_aws_host(bare) {
        Ok(())
    } else {
        Err(BindingError::NotAwsHost {
            endpoint: endpoint.id(),
            host: host.to_string(),
        })
    }
}

fn host_without_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        // [v6]:port
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.split(':').next().unwrap_or(authority)
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod tests;
