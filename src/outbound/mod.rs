//! Outbound HTTP: the one place deckwatch builds `reqwest::Client`s.
//!
//! Every request deckwatch makes to another service (Anthropic, Vertex AI,
//! Bedrock, GitHub and other git hosts, plugin downloads, notification
//! webhooks, Prometheus, tracing backends, Entra JWKS, registries) uses a
//! client from [`OutboundHttp::client`], so every client gets the same:
//!
//! * per-purpose connect and total timeouts ([`Purpose::timeouts`]);
//! * `User-Agent: deckwatch/<version>`;
//! * proxy settings from the environment (`HTTPS_PROXY`, `HTTP_PROXY`,
//!   `ALL_PROXY`, `NO_PROXY`): reqwest's default, unchanged;
//! * trust: the system roots **plus** the CAs in `DECKWATCH_EXTRA_CA_FILE`
//!   ([`tls::ExtraCaBundle`]). Extra CAs are appended; nothing removes or
//!   replaces a system root.
//!
//! Clients are built once (at startup, by [`OutboundHttp::new`]) and shared;
//! `reqwest::Client` is a cheap `Arc` clone. A broken CA bundle fails
//! [`OutboundHttp::from_env`], which `main` turns into a startup failure
//! naming the file.
//!
//! TLS-backend-specific code is confined to [`apply_extra_roots`]: reqwest
//! 0.12 (native-tls/OpenSSL) and 0.13 (rustls + rustls-platform-verifier)
//! both accept the same calls. See `docs/TLS.md`.
//!
//! [`endpoints`] holds the operator-configurable upstream URLs and
//! [`binding`] the rule that a stored credential only goes to the host it
//! was saved for.

pub mod binding;
pub mod endpoints;
pub mod github;
pub mod tls;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub use binding::{
    authorize, authorize_aws, display_origin, BindingError, CredentialSource, EndpointBinding,
};
pub use endpoints::{
    Endpoint, EndpointError, EndpointErrors, EndpointId, EndpointPolicy, EndpointSettings,
};
pub use tls::{ExtraCaBundle, ExtraCaError};

/// PEM bundle of CA certificates to trust in addition to the system roots.
pub const EXTRA_CA_FILE_ENV: &str = "DECKWATCH_EXTRA_CA_FILE";

/// `true` permits `http://` endpoints for non-loopback hosts (local
/// development and tests only).
pub const ALLOW_INSECURE_ENDPOINTS_ENV: &str = "DECKWATCH_ALLOW_INSECURE_ENDPOINTS";

/// `err` followed by its sources, joined with ": ". reqwest's `Display` stops
/// at "error sending request for url (...)"; the reason (unknown issuer,
/// expired certificate, connection refused) is only in the source chain.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = Some(err);
    while let Some(e) = current {
        let msg = e.to_string();
        // Some errors repeat their source's message in their own.
        if !msg.is_empty() && !parts.iter().any(|p| p.contains(&msg)) {
            parts.push(msg);
        }
        current = e.source();
    }
    parts.join(": ")
}

/// `deckwatch/<version>`, sent on every outbound request.
pub fn user_agent() -> String {
    format!("deckwatch/{}", env!("CARGO_PKG_VERSION"))
}

/// What a client is for. Each purpose gets its own client and timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Anthropic Messages API, Vertex AI `streamRawPredict`, Bedrock invoke.
    /// Responses stream for minutes.
    AiInference,
    /// OAuth token exchange for Vertex AI (service account JWT bearer).
    AiToken,
    /// Git smart-HTTP and the GitHub REST API (GitOps poller, branch picker,
    /// manual builds).
    Git,
    /// WASM plugin downloads.
    PluginDownload,
    /// Notification webhooks (Slack, Teams, generic).
    Webhook,
    /// Prometheus and tracing-backend queries proxied for the UI.
    Observability,
    /// Container registry probes (built image availability).
    Registry,
    /// Identity provider metadata (Entra ID JWKS).
    Identity,
}

/// Connect and total (whole request, body included) timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub total: Duration,
}

impl Purpose {
    pub const ALL: [Purpose; 8] = [
        Purpose::AiInference,
        Purpose::AiToken,
        Purpose::Git,
        Purpose::PluginDownload,
        Purpose::Webhook,
        Purpose::Observability,
        Purpose::Registry,
        Purpose::Identity,
    ];

    /// Totals keep the values the call sites used before the factory
    /// existed (plugins 30 s, webhooks and observability 10 s); call sites
    /// that had none get a bound so a hung upstream cannot stall the GitOps
    /// poller or a request handler forever.
    pub fn timeouts(self) -> Timeouts {
        let (connect, total) = match self {
            Purpose::AiInference => (10, 600),
            Purpose::AiToken => (10, 30),
            Purpose::Git => (10, 60),
            Purpose::PluginDownload => (10, 30),
            Purpose::Webhook => (5, 10),
            Purpose::Observability => (5, 10),
            Purpose::Registry => (5, 15),
            Purpose::Identity => (5, 15),
        };
        Timeouts {
            connect: Duration::from_secs(connect),
            total: Duration::from_secs(total),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Purpose::AiInference => "ai-inference",
            Purpose::AiToken => "ai-token",
            Purpose::Git => "git",
            Purpose::PluginDownload => "plugin-download",
            Purpose::Webhook => "webhook",
            Purpose::Observability => "observability",
            Purpose::Registry => "registry",
            Purpose::Identity => "identity",
        }
    }
}

/// Startup configuration problems. `main` refuses to start on any of them.
#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error("{EXTRA_CA_FILE_ENV}: {0}")]
    ExtraCa(#[from] ExtraCaError),
    #[error("{ALLOW_INSECURE_ENDPOINTS_ENV}={0:?} is not a boolean (use true or false)")]
    InsecureFlag(String),
    #[error("cannot build the {purpose} HTTP client: {detail}")]
    Client {
        purpose: &'static str,
        detail: String,
    },
}

/// Inputs to [`OutboundHttp::new`].
#[derive(Debug, Clone, Default)]
pub struct OutboundConfig {
    /// Extra CAs trusted on top of the system roots.
    pub extra_ca: Option<ExtraCaBundle>,
    /// Endpoint URL rules (`DECKWATCH_ALLOW_INSECURE_ENDPOINTS`).
    pub policy: EndpointPolicy,
}

impl OutboundConfig {
    /// Read `DECKWATCH_EXTRA_CA_FILE` and `DECKWATCH_ALLOW_INSECURE_ENDPOINTS`.
    /// An unset or empty `DECKWATCH_EXTRA_CA_FILE` means no extra CAs.
    pub fn from_env() -> Result<Self, OutboundError> {
        Self::from_values(
            std::env::var_os(EXTRA_CA_FILE_ENV),
            std::env::var(ALLOW_INSECURE_ENDPOINTS_ENV).ok(),
        )
    }

    /// [`Self::from_env`] with the variable values passed in.
    pub fn from_values(
        extra_ca_file: Option<std::ffi::OsString>,
        allow_insecure: Option<String>,
    ) -> Result<Self, OutboundError> {
        let extra_ca = match extra_ca_file {
            Some(path) if !path.is_empty() => Some(ExtraCaBundle::load(PathBuf::from(path))?),
            _ => None,
        };
        let allow_insecure = match allow_insecure {
            None => false,
            Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                "" | "0" | "false" | "no" => false,
                "1" | "true" | "yes" => true,
                _ => return Err(OutboundError::InsecureFlag(v)),
            },
        };
        Ok(Self {
            extra_ca,
            policy: EndpointPolicy { allow_insecure },
        })
    }
}

/// The shared outbound HTTP clients. Cheap to clone.
#[derive(Clone)]
pub struct OutboundHttp {
    inner: Arc<Inner>,
}

struct Inner {
    config: OutboundConfig,
    clients: HashMap<Purpose, reqwest::Client>,
    object_store_roots: Vec<object_store::Certificate>,
}

impl std::fmt::Debug for OutboundHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundHttp")
            .field("extra_ca", &self.inner.config.extra_ca)
            .field("policy", &self.inner.config.policy)
            .finish()
    }
}

impl OutboundHttp {
    /// Build one client per [`Purpose`]. Fails if the TLS backend rejects an
    /// extra CA or a client cannot be built.
    pub fn new(config: OutboundConfig) -> Result<Self, OutboundError> {
        let (roots, object_store_roots) = match &config.extra_ca {
            Some(bundle) => (
                bundle.reqwest_certificates()?,
                bundle.object_store_certificates()?,
            ),
            None => (Vec::new(), Vec::new()),
        };
        let mut clients = HashMap::new();
        for purpose in Purpose::ALL {
            let client = build_client(purpose, &roots).map_err(|e| OutboundError::Client {
                purpose: purpose.name(),
                detail: e.to_string(),
            })?;
            clients.insert(purpose, client);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                clients,
                object_store_roots,
            }),
        })
    }

    /// [`OutboundConfig::from_env`] + [`Self::new`].
    pub fn from_env() -> Result<Self, OutboundError> {
        Self::new(OutboundConfig::from_env()?)
    }

    /// The shared client for `purpose`.
    pub fn client(&self, purpose: Purpose) -> reqwest::Client {
        self.inner.clients[&purpose].clone()
    }

    /// Endpoint URL rules for this deployment.
    pub fn policy(&self) -> EndpointPolicy {
        self.inner.config.policy
    }

    /// The extra CA bundle, if one is configured.
    pub fn extra_ca(&self) -> Option<&ExtraCaBundle> {
        self.inner.config.extra_ca.as_ref()
    }

    /// `object_store` client options carrying the same extra CAs, for the S3
    /// registry backend and the AWS credential providers used by Bedrock
    /// (object_store builds its own reqwest 0.13 clients).
    pub fn object_store_client_options(&self) -> object_store::ClientOptions {
        self.inner
            .object_store_roots
            .iter()
            .cloned()
            .fold(object_store::ClientOptions::new(), |opts, cert| {
                opts.with_root_certificate(cert)
            })
    }
}

/// `AmazonS3Builder::from_env()`, but starting from `options` (deckwatch's
/// extra CAs) instead of default client options. `from_env` applies `AWS_*`
/// variables (including client settings such as `AWS_ALLOW_HTTP` or
/// `AWS_PROXY_URL`) on top of the builder's options; calling
/// `with_client_options` afterwards would discard them.
pub fn amazon_s3_builder_from_env(
    options: object_store::ClientOptions,
) -> object_store::aws::AmazonS3Builder {
    let mut builder = object_store::aws::AmazonS3Builder::new().with_client_options(options);
    for (key, value) in std::env::vars_os() {
        if let (Some(key), Some(value)) = (key.to_str(), value.to_str()) {
            if key.starts_with("AWS_") {
                if let Ok(config_key) = key.to_ascii_lowercase().parse() {
                    builder = builder.with_config(config_key, value);
                }
            }
        }
    }
    builder
}

impl Default for OutboundHttp {
    /// No extra CAs, default policy.
    fn default() -> Self {
        Self::new(OutboundConfig::default()).expect("default outbound HTTP clients build")
    }
}

fn build_client(
    purpose: Purpose,
    roots: &[reqwest::Certificate],
) -> reqwest::Result<reqwest::Client> {
    let timeouts = purpose.timeouts();
    let builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(timeouts.connect)
        .timeout(timeouts.total);
    apply_extra_roots(builder, roots).build()
}

/// Append `roots` to the client's trust store. The only TLS-backend-specific
/// call in deckwatch:
///
/// * reqwest 0.12 / native-tls: OpenSSL keeps its default verify paths (the
///   system bundle, or `SSL_CERT_FILE` / `SSL_CERT_DIR`) and adds these.
/// * reqwest 0.13 / rustls: `rustls-platform-verifier` is built with these
///   as extra roots next to the OS store.
///
/// reqwest 0.13 deprecates `add_root_certificate` in favour of
/// `tls_certs_merge`, which does the same thing; `allow(deprecated)` keeps
/// this compiling warning-free on both until the call is switched.
#[allow(deprecated)]
fn apply_extra_roots(
    mut builder: reqwest::ClientBuilder,
    roots: &[reqwest::Certificate],
) -> reqwest::ClientBuilder {
    for cert in roots {
        builder = builder.add_root_certificate(cert.clone());
    }
    builder
}

static GLOBAL: OnceLock<OutboundHttp> = OnceLock::new();

/// Install the process-wide clients. `main` calls this at startup, before
/// anything else can build a client. Returns false if they were already set.
///
/// Like the rustls `CryptoProvider` that `main` installs, TLS trust is a
/// process-wide setting, so the clients are too.
pub fn install_global(http: OutboundHttp) -> bool {
    GLOBAL.set(http).is_ok()
}

/// The process-wide clients. Falls back to [`OutboundHttp::from_env`] when
/// [`install_global`] was not called (unit tests), panicking if that fails.
pub fn global() -> &'static OutboundHttp {
    GLOBAL.get_or_init(|| {
        OutboundHttp::from_env()
            .unwrap_or_else(|e| panic!("invalid outbound HTTP configuration: {e}"))
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
