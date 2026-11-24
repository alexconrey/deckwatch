//! Upstream endpoints operators can override in Settings (stored in the
//! database), with the URLs deckwatch used before they were configurable as
//! defaults.
//!
//! Every value is validated the same way when it is saved and again when it
//! is used:
//!
//! * `https://` only. `http://` is accepted for loopback hosts (`localhost`,
//!   `127.0.0.0/8`, `::1`), or for any host when the deployment sets
//!   `DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true` (local development and tests).
//! * No userinfo (`user:pass@host`), query string or fragment.
//! * A valid host. `{region}` may appear in the host of the Vertex AI and
//!   Bedrock endpoints; it is replaced with the provider region, which must
//!   be a single DNS label.
//!
//! An empty value means "use the default". Values equal to the default are
//! normalized to empty so there is one spelling of "default".

use std::fmt;

use reqwest::Url;
use serde::{Deserialize, Serialize};

/// Placeholder replaced with the provider region (Vertex AI, Bedrock).
pub const REGION_PLACEHOLDER: &str = "{region}";

/// Stand-in for [`REGION_PLACEHOLDER`] while a template is parsed as a URL:
/// a valid DNS label that cannot collide with a real region.
const REGION_SENTINEL: &str = "deckwatch-region-placeholder";

/// Longest accepted endpoint value.
const MAX_ENDPOINT_LEN: usize = 2048;

/// The configurable upstream endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointId {
    /// Anthropic Messages API base (`/v1/messages` is appended).
    AnthropicApi,
    /// Google OAuth token endpoint used to exchange the Vertex AI service
    /// account key for an access token (full URL).
    VertexToken,
    /// Vertex AI API base; `{region}` in the host is the provider region.
    VertexApi,
    /// Bedrock runtime base; `{region}` in the host is the provider region.
    Bedrock,
    /// GitHub REST API base (`https://<host>/api/v3` for GitHub Enterprise
    /// Server).
    GithubApi,
    /// GitHub web base: identifies GitHub-hosted repositories and serves
    /// release downloads for plugins.
    GithubWeb,
    /// GitHub raw file base used for plugin downloads
    /// (`https://<host>/raw` for GitHub Enterprise Server).
    GithubRaw,
}

impl EndpointId {
    pub const ALL: [EndpointId; 7] = [
        EndpointId::AnthropicApi,
        EndpointId::VertexToken,
        EndpointId::VertexApi,
        EndpointId::Bedrock,
        EndpointId::GithubApi,
        EndpointId::GithubWeb,
        EndpointId::GithubRaw,
    ];

    /// The URL deckwatch used before endpoints were configurable.
    pub fn default_url(self) -> &'static str {
        match self {
            EndpointId::AnthropicApi => "https://api.anthropic.com",
            EndpointId::VertexToken => "https://oauth2.googleapis.com/token",
            EndpointId::VertexApi => "https://{region}-aiplatform.googleapis.com",
            EndpointId::Bedrock => "https://bedrock-runtime.{region}.amazonaws.com",
            EndpointId::GithubApi => "https://api.github.com",
            EndpointId::GithubWeb => "https://github.com",
            EndpointId::GithubRaw => "https://raw.githubusercontent.com",
        }
    }

    /// Human-readable name used in validation and binding errors.
    pub fn label(self) -> &'static str {
        match self {
            EndpointId::AnthropicApi => "Anthropic API",
            EndpointId::VertexToken => "Vertex AI token",
            EndpointId::VertexApi => "Vertex AI API",
            EndpointId::Bedrock => "Bedrock runtime",
            EndpointId::GithubApi => "GitHub API",
            EndpointId::GithubWeb => "GitHub web",
            EndpointId::GithubRaw => "GitHub raw content",
        }
    }

    /// Field name in [`EndpointSettings`] (and the settings JSON).
    pub fn setting_name(self) -> &'static str {
        match self {
            EndpointId::AnthropicApi => "anthropic_api_base",
            EndpointId::VertexToken => "vertex_token_url",
            EndpointId::VertexApi => "vertex_api_base",
            EndpointId::Bedrock => "bedrock_endpoint",
            EndpointId::GithubApi => "github_api_base",
            EndpointId::GithubWeb => "github_web_base",
            EndpointId::GithubRaw => "github_raw_base",
        }
    }

    /// Whether `{region}` may appear in the host.
    pub fn allows_region(self) -> bool {
        matches!(self, EndpointId::VertexApi | EndpointId::Bedrock)
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Deployment-level rules for endpoint URLs. Not stored in the database:
/// deckwatch has no authentication yet, so loosening them is reserved for
/// whoever controls the pod environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndpointPolicy {
    /// Accept `http://` for non-loopback hosts
    /// (`DECKWATCH_ALLOW_INSECURE_ENDPOINTS`).
    pub allow_insecure: bool,
}

/// Why an endpoint value was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    #[error("{endpoint} endpoint: not a valid URL ({detail})")]
    Invalid {
        endpoint: EndpointId,
        detail: String,
    },
    #[error("{endpoint} endpoint: longer than {MAX_ENDPOINT_LEN} characters")]
    TooLong { endpoint: EndpointId },
    #[error("{endpoint} endpoint: scheme must be https (got {scheme}://)")]
    Scheme {
        endpoint: EndpointId,
        scheme: String,
    },
    #[error(
        "{endpoint} endpoint: http:// is only allowed for localhost, unless the deployment \
         sets DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true; use https://{host}"
    )]
    Insecure { endpoint: EndpointId, host: String },
    #[error("{endpoint} endpoint: must not contain a username or password")]
    UserInfo { endpoint: EndpointId },
    #[error("{endpoint} endpoint: must not contain a query string or fragment")]
    QueryOrFragment { endpoint: EndpointId },
    #[error("{endpoint} endpoint: a host is required")]
    MissingHost { endpoint: EndpointId },
    #[error("{endpoint} endpoint: {detail}")]
    RegionPlaceholder {
        endpoint: EndpointId,
        detail: &'static str,
    },
    #[error("invalid region {region:?}: expected a region name such as us-east-1 or us-central1")]
    Region { region: String },
}

/// Overrides for the upstream endpoints, as stored in settings. Empty fields
/// use [`EndpointId::default_url`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointSettings {
    #[serde(default)]
    pub anthropic_api_base: String,
    #[serde(default)]
    pub vertex_token_url: String,
    #[serde(default)]
    pub vertex_api_base: String,
    #[serde(default)]
    pub bedrock_endpoint: String,
    #[serde(default)]
    pub github_api_base: String,
    #[serde(default)]
    pub github_web_base: String,
    #[serde(default)]
    pub github_raw_base: String,
}

impl EndpointSettings {
    /// The configured value ("" = default).
    pub fn get(&self, id: EndpointId) -> &str {
        match id {
            EndpointId::AnthropicApi => &self.anthropic_api_base,
            EndpointId::VertexToken => &self.vertex_token_url,
            EndpointId::VertexApi => &self.vertex_api_base,
            EndpointId::Bedrock => &self.bedrock_endpoint,
            EndpointId::GithubApi => &self.github_api_base,
            EndpointId::GithubWeb => &self.github_web_base,
            EndpointId::GithubRaw => &self.github_raw_base,
        }
    }

    fn get_mut(&mut self, id: EndpointId) -> &mut String {
        match id {
            EndpointId::AnthropicApi => &mut self.anthropic_api_base,
            EndpointId::VertexToken => &mut self.vertex_token_url,
            EndpointId::VertexApi => &mut self.vertex_api_base,
            EndpointId::Bedrock => &mut self.bedrock_endpoint,
            EndpointId::GithubApi => &mut self.github_api_base,
            EndpointId::GithubWeb => &mut self.github_web_base,
            EndpointId::GithubRaw => &mut self.github_raw_base,
        }
    }

    /// Builder-style setter (tests, callers assembling settings in code).
    pub fn with(mut self, id: EndpointId, value: impl Into<String>) -> Self {
        *self.get_mut(id) = value.into();
        self
    }

    /// True when every endpoint uses its default.
    pub fn is_default(&self) -> bool {
        EndpointId::ALL
            .iter()
            .all(|id| self.get(*id).trim().is_empty())
    }

    /// Validate every endpoint and return the normalized settings. All
    /// problems are reported, not just the first.
    pub fn validated(&self, policy: &EndpointPolicy) -> Result<EndpointSettings, EndpointErrors> {
        let mut out = EndpointSettings::default();
        let mut errors = Vec::new();
        for id in EndpointId::ALL {
            match Endpoint::parse(id, self.get(id), policy) {
                Ok(ep) if ep.is_default() => {}
                Ok(ep) => *out.get_mut(id) = ep.base,
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() {
            Ok(out)
        } else {
            Err(EndpointErrors(errors))
        }
    }

    /// The effective, validated endpoint for `id`.
    pub fn resolve(
        &self,
        id: EndpointId,
        policy: &EndpointPolicy,
    ) -> Result<Endpoint, EndpointError> {
        Endpoint::parse(id, self.get(id), policy)
    }
}

/// One or more invalid endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointErrors(pub Vec<EndpointError>);

impl fmt::Display for EndpointErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let messages: Vec<String> = self.0.iter().map(|e| e.to_string()).collect();
        f.write_str(&messages.join("; "))
    }
}

impl std::error::Error for EndpointErrors {}

/// A validated endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    id: EndpointId,
    /// Normalized base: lowercase scheme and host, default port and trailing
    /// slash removed. May contain `{region}` in the host.
    base: String,
    /// `scheme://host[:port]` of `base` (still templated).
    origin: String,
    /// `host[:port]` of `base` (still templated).
    host: String,
    loopback: bool,
    is_default: bool,
}

impl Endpoint {
    /// The default endpoint for `id`.
    pub fn default_for(id: EndpointId) -> Endpoint {
        Endpoint::parse(id, "", &EndpointPolicy::default())
            .expect("compiled-in endpoint defaults are valid")
    }

    /// Validate and normalize `raw` for `id`. An empty `raw` is the default.
    pub fn parse(
        id: EndpointId,
        raw: &str,
        policy: &EndpointPolicy,
    ) -> Result<Endpoint, EndpointError> {
        let raw = raw.trim();
        let (value, is_default) = if raw.is_empty() {
            (id.default_url(), true)
        } else {
            (raw, false)
        };
        if value.len() > MAX_ENDPOINT_LEN {
            return Err(EndpointError::TooLong { endpoint: id });
        }

        let templated = value.contains(REGION_PLACEHOLDER);
        if templated {
            if !id.allows_region() {
                return Err(EndpointError::RegionPlaceholder {
                    endpoint: id,
                    detail: "{region} is only supported for the Vertex AI and Bedrock endpoints",
                });
            }
            if value.matches(REGION_PLACEHOLDER).count() > 1 {
                return Err(EndpointError::RegionPlaceholder {
                    endpoint: id,
                    detail: "{region} may appear only once",
                });
            }
        }
        let parseable = value.replace(REGION_PLACEHOLDER, REGION_SENTINEL);
        let url = Url::parse(&parseable).map_err(|e| EndpointError::Invalid {
            endpoint: id,
            detail: e.to_string(),
        })?;

        let scheme = url.scheme().to_string();
        if scheme != "https" && scheme != "http" {
            return Err(EndpointError::Scheme {
                endpoint: id,
                scheme,
            });
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::UserInfo { endpoint: id });
        }
        if url.query().is_some() || url.fragment().is_some() || parseable.contains(['?', '#']) {
            return Err(EndpointError::QueryOrFragment { endpoint: id });
        }
        let host = match url.host_str() {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => return Err(EndpointError::MissingHost { endpoint: id }),
        };
        if templated && !host.contains(REGION_SENTINEL) {
            return Err(EndpointError::RegionPlaceholder {
                endpoint: id,
                detail: "{region} is only allowed in the host name",
            });
        }
        let loopback = is_loopback(&url);
        if scheme == "http" && !loopback && !policy.allow_insecure {
            return Err(EndpointError::Insecure {
                endpoint: id,
                host: host.replace(REGION_SENTINEL, REGION_PLACEHOLDER),
            });
        }

        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let path = url.path().trim_end_matches('/');
        let untemplate = |s: String| s.replace(REGION_SENTINEL, REGION_PLACEHOLDER);
        let origin = untemplate(format!("{scheme}://{authority}"));
        let base = format!("{origin}{}", untemplate(path.to_string()));
        let is_default = is_default || base == id.default_url();

        Ok(Endpoint {
            id,
            base,
            origin,
            host: untemplate(authority),
            loopback,
            is_default,
        })
    }

    pub fn id(&self) -> EndpointId {
        self.id
    }

    /// Normalized base URL (may contain `{region}`).
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `scheme://host[:port]`, the value stored credentials are bound to.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// `host[:port]`, for messages ("API key saved for api.anthropic.com").
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn is_default(&self) -> bool {
        self.is_default
    }

    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    pub fn is_templated(&self) -> bool {
        self.base.contains(REGION_PLACEHOLDER)
    }

    /// `base + path` for endpoints without `{region}`. `path` should start
    /// with `/` (or be empty).
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// `base + path` with `{region}` replaced. Fails for an invalid region
    /// even when the endpoint is not templated, so a malformed region never
    /// reaches a URL path either.
    pub fn url_for_region(&self, region: &str, path: &str) -> Result<String, EndpointError> {
        validate_region(region)?;
        Ok(format!(
            "{}{path}",
            self.base.replacen(REGION_PLACEHOLDER, region, 1)
        ))
    }

    /// Host with `{region}` replaced (for scope checks on the concrete host).
    pub fn host_for_region(&self, region: &str) -> Result<String, EndpointError> {
        validate_region(region)?;
        Ok(self.host.replacen(REGION_PLACEHOLDER, region, 1))
    }
}

/// A region is interpolated into a host name, so it must be one DNS label:
/// lowercase letters, digits and inner hyphens.
pub fn validate_region(region: &str) -> Result<(), EndpointError> {
    let ok = !region.is_empty()
        && region.len() <= 63
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !region.starts_with('-')
        && !region.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(EndpointError::Region {
            region: region.to_string(),
        })
    }
}

fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // IPv6 literals come back bracketed ("[::1]").
    let ip = host.trim_start_matches('[').trim_end_matches(']');
    ip.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `host` (no port) belongs to AWS: `*.amazonaws.com`,
/// `*.amazonaws.com.cn` or `*.api.aws`.
pub fn is_aws_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ["amazonaws.com", "amazonaws.com.cn", "api.aws"]
        .iter()
        .any(|suffix| {
            host.strip_suffix(suffix)
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.'))
        })
}

#[cfg(test)]
#[path = "endpoints_tests.rs"]
mod tests;
