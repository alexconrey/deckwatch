//! Where deckwatch's own telemetry goes: the OTLP endpoint of each signal,
//! from the `OTEL_*` variables or from Settings → Observability
//! ([`ObservabilitySettings`]), and what turns a signal off.
//!
//! Per signal, an endpoint variable wins over settings ([`Plan::resolve`]):
//!
//! 1. `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT`, used as is;
//! 2. `OTEL_EXPORTER_OTLP_ENDPOINT`, with `/v1/<signal>` appended;
//! 3. settings: `otlp_endpoint` with `/v1/traces` or `/v1/logs` appended, and
//!    `otlp_metrics_endpoint` as is for metrics. The collector endpoint
//!    doesn't carry metrics: they go to Prometheus's OTLP receiver.
//!
//! Whatever the source, `OTEL_SDK_DISABLED`, `OTEL_<SIGNAL>_EXPORTER` and the
//! protocol variables still apply. Exporter headers
//! (`OTEL_EXPORTER_OTLP_[<SIGNAL>_]HEADERS`, usually credentials) are sent
//! only to an endpoint from the environment: settings can be changed through
//! the API, and that must not be a way to send those credentials elsewhere.

use reqwest::Url;
use serde::{Deserialize, Serialize};

use super::{Notice, PROTOCOL};
use crate::outbound;

/// The longest endpoint accepted in settings.
pub const MAX_ENDPOINT_LEN: usize = 2048;

/// Where deckwatch sends its own traces, log lines and metrics: the
/// `observability` block of the settings document (Settings →
/// Observability). Read when deckwatch starts, so a change applies at the
/// next restart. An endpoint variable wins over these, per signal (see the
/// module docs). Values are validated when saved ([`Self::validated`]) and
/// again at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilitySettings {
    /// OTLP/HTTP base URL of an OpenTelemetry Collector, for traces and logs:
    /// deckwatch appends `/v1/traces` and `/v1/logs`. Empty: not set.
    #[serde(default)]
    pub otlp_endpoint: String,
    /// Full URL of an OTLP metrics receiver, used as is: Prometheus's
    /// `/api/v1/otlp/v1/metrics` (`--web.enable-otlp-receiver`), or a
    /// collector's `/v1/metrics`. Empty: not set.
    #[serde(default)]
    pub otlp_metrics_endpoint: String,
}

/// The two endpoint fields of [`ObservabilitySettings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointField {
    /// `otlp_endpoint`: a base URL, for traces and logs.
    Otlp,
    /// `otlp_metrics_endpoint`: a full URL, for metrics.
    OtlpMetrics,
}

impl EndpointField {
    /// The field's name in the settings document.
    pub fn setting(self) -> &'static str {
        match self {
            EndpointField::Otlp => "observability.otlp_endpoint",
            EndpointField::OtlpMetrics => "observability.otlp_metrics_endpoint",
        }
    }
}

impl ObservabilitySettings {
    /// Neither endpoint is set.
    pub fn is_empty(&self) -> bool {
        self.otlp_endpoint.trim().is_empty() && self.otlp_metrics_endpoint.trim().is_empty()
    }

    /// The stored value of `field`.
    pub fn get(&self, field: EndpointField) -> &str {
        match field {
            EndpointField::Otlp => &self.otlp_endpoint,
            EndpointField::OtlpMetrics => &self.otlp_metrics_endpoint,
        }
    }

    /// Both endpoints validated ([`validate_endpoint`]) and normalized. Every
    /// problem is reported, not just the first.
    pub fn validated(&self) -> Result<ObservabilitySettings, String> {
        let otlp = validate_endpoint(EndpointField::Otlp, &self.otlp_endpoint);
        let metrics = validate_endpoint(EndpointField::OtlpMetrics, &self.otlp_metrics_endpoint);
        match (otlp, metrics) {
            (Ok(otlp_endpoint), Ok(otlp_metrics_endpoint)) => Ok(ObservabilitySettings {
                otlp_endpoint,
                otlp_metrics_endpoint,
            }),
            (otlp, metrics) => Err([otlp.err(), metrics.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ")),
        }
    }
}

/// Validate an endpoint from settings and return it normalized ("" when
/// empty):
///
/// * an absolute `http://` or `https://` URL with a host. Plain http is
///   accepted for any host, as in-cluster collectors use it: deckwatch sends
///   these endpoints telemetry only, never credentials;
/// * no username or password, query string or fragment, which are where
///   tokens end up in a URL;
/// * at most [`MAX_ENDPOINT_LEN`] characters;
/// * `otlp_endpoint` is a base URL: deckwatch appends `/v1/traces` and
///   `/v1/logs`, so a path that already ends in `/v1/<signal>` is refused,
///   and a trailing `/` is dropped;
/// * `otlp_metrics_endpoint` is used as is, so it needs a path (Prometheus:
///   `/api/v1/otlp/v1/metrics`).
pub fn validate_endpoint(field: EndpointField, raw: &str) -> Result<String, String> {
    let name = field.setting();
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.len() > MAX_ENDPOINT_LEN {
        return Err(format!("{name}: longer than {MAX_ENDPOINT_LEN} characters"));
    }
    let example = match field {
        EndpointField::Otlp => "http://otel-collector.observability:4318",
        EndpointField::OtlpMetrics => "http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics",
    };
    if !value.contains("://") {
        return Err(format!(
            "{name}: an http:// or https:// URL is required, such as {example} (deckwatch \
             exports OTLP over HTTP)"
        ));
    }
    let url = Url::parse(value).map_err(|e| format!("{name}: not a valid URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "{name}: the scheme must be http or https (got {}://)",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "{name}: must not contain a username or password (deckwatch sends no credentials \
             to an endpoint from settings)"
        ));
    }
    if url.query().is_some() || url.fragment().is_some() || value.contains(['?', '#']) {
        return Err(format!(
            "{name}: must not contain a query string or fragment"
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(format!("{name}: a host is required"));
    }
    let path = url.path().trim_end_matches('/');
    match field {
        EndpointField::Otlp => {
            if let Some(suffix) = Signal::ALL
                .iter()
                .map(|s| s.path())
                .find(|suffix| path.ends_with(suffix))
            {
                return Err(format!(
                    "{name}: enter the collector's base URL, without {suffix}: deckwatch \
                     appends /v1/traces and /v1/logs"
                ));
            }
            Ok(value.trim_end_matches('/').to_string())
        }
        EndpointField::OtlpMetrics => {
            if path.is_empty() {
                return Err(format!(
                    "{name}: enter the full URL of the metrics receiver, such as {example}; it \
                     is used as is"
                ));
            }
            Ok(value.to_string())
        }
    }
}

/// One of the signals deckwatch exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Traces,
    Metrics,
    Logs,
}

impl Signal {
    pub const ALL: [Signal; 3] = [Signal::Traces, Signal::Metrics, Signal::Logs];

    /// `traces`, `metrics` or `logs`.
    pub fn name(self) -> &'static str {
        match self {
            Signal::Traces => "traces",
            Signal::Metrics => "metrics",
            Signal::Logs => "logs",
        }
    }

    /// The signal in `OTEL_*` variable names: `TRACES`, `METRICS`, `LOGS`.
    pub fn var(self) -> &'static str {
        match self {
            Signal::Traces => "TRACES",
            Signal::Metrics => "METRICS",
            Signal::Logs => "LOGS",
        }
    }

    /// The path appended to an OTLP/HTTP base URL.
    pub fn path(self) -> &'static str {
        match self {
            Signal::Traces => "/v1/traces",
            Signal::Metrics => "/v1/metrics",
            Signal::Logs => "/v1/logs",
        }
    }

    /// The variable holding this signal's own endpoint.
    pub fn endpoint_var(self) -> String {
        format!("OTEL_EXPORTER_OTLP_{}_ENDPOINT", self.var())
    }

    /// The signal's position in [`Signal::ALL`].
    pub fn index(self) -> usize {
        match self {
            Signal::Traces => 0,
            Signal::Metrics => 1,
            Signal::Logs => 2,
        }
    }
}

/// Where a signal's endpoint comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// An `OTEL_EXPORTER_OTLP_*ENDPOINT` variable.
    Env,
    /// Settings → Observability.
    Settings,
    /// No endpoint: the signal isn't exported.
    Unset,
}

/// How one signal is exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalPlan {
    pub signal: Signal,
    pub source: Source,
    /// The URL export requests go to, `None` when unset. One from the
    /// environment can hold credentials: display [`SignalPlan::status`].
    pub endpoint: Option<String>,
    /// The variable or setting the endpoint comes from.
    pub from: Option<String>,
    /// Why the signal isn't exported although it has an endpoint.
    pub off: Option<String>,
}

impl SignalPlan {
    /// The signal has an endpoint and nothing turns it off.
    pub fn exported(&self) -> bool {
        self.endpoint.is_some() && self.off.is_none()
    }

    /// This plan as the API reports it, the endpoint's credentials masked.
    pub fn status(&self) -> SignalStatus {
        SignalStatus {
            source: self.source,
            endpoint: self
                .endpoint
                .as_deref()
                .map(crate::redact::url)
                .unwrap_or_default(),
            from: self.from.clone(),
            exported: self.exported(),
            reason: self.off.clone(),
        }
    }
}

/// A signal's export as the API reports it (`GET /api/settings/observability`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalStatus {
    pub source: Source,
    /// Where it is sent, credentials masked ([`crate::redact::url`]). Empty
    /// when unset.
    pub endpoint: String,
    /// The variable or setting the endpoint comes from.
    pub from: Option<String>,
    pub exported: bool,
    /// Why it isn't exported although it has an endpoint: a variable that
    /// turns it off, or an exporter that couldn't be built.
    pub reason: Option<String>,
}

impl SignalStatus {
    /// No endpoint.
    pub fn unset() -> SignalStatus {
        SignalStatus {
            source: Source::Unset,
            endpoint: String::new(),
            from: None,
            exported: false,
            reason: None,
        }
    }
}

/// Where each signal goes, and the configuration problems to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    signals: [SignalPlan; 3],
    pub notices: Vec<Notice>,
}

impl Plan {
    /// The plan for the `OTEL_*` variables, read through `get`, and the
    /// endpoints in `settings`. See the module docs for the precedence.
    ///
    /// A signal with an endpoint is still not exported when:
    ///
    /// * `OTEL_SDK_DISABLED=true` (this turns everything off);
    /// * `OTEL_<SIGNAL>_EXPORTER` is set to anything but `otlp`;
    /// * the protocol (`OTEL_EXPORTER_OTLP_[<SIGNAL>_]PROTOCOL`) is set to
    ///   anything but `http/protobuf`.
    ///
    /// Without any endpoint nothing is exported and there are no notices: the
    /// spec's default endpoint (`localhost:4318`) is never used implicitly.
    /// An endpoint in settings that no longer validates (it was stored some
    /// other way) is reported and not used.
    pub fn resolve(get: impl Fn(&str) -> Option<String>, settings: &ObservabilitySettings) -> Plan {
        let get = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut notices = Vec::new();
        let mut from_settings = |field: EndpointField| -> Option<String> {
            match validate_endpoint(field, settings.get(field)) {
                Ok(value) => (!value.is_empty()).then_some(value),
                Err(e) => {
                    notices.push(Notice::Error(format!("{e}; it is not used")));
                    None
                }
            }
        };
        let base = from_settings(EndpointField::Otlp);
        let metrics = from_settings(EndpointField::OtlpMetrics);

        let sdk_disabled = get("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let generic = get("OTEL_EXPORTER_OTLP_ENDPOINT");
        let signals = Signal::ALL.map(|signal| {
            let own_var = signal.endpoint_var();
            let (source, endpoint, from) = if let Some(url) = get(&own_var) {
                (Source::Env, Some(url), Some(own_var))
            } else if let Some(url) = &generic {
                (
                    Source::Env,
                    Some(join(url, signal.path())),
                    Some("OTEL_EXPORTER_OTLP_ENDPOINT".to_string()),
                )
            } else {
                let (url, field) = match signal {
                    Signal::Traces | Signal::Logs => (
                        base.as_deref().map(|b| join(b, signal.path())),
                        EndpointField::Otlp,
                    ),
                    Signal::Metrics => (metrics.clone(), EndpointField::OtlpMetrics),
                };
                match url {
                    Some(url) => (
                        Source::Settings,
                        Some(url),
                        Some(field.setting().to_string()),
                    ),
                    None => (Source::Unset, None, None),
                }
            };
            let off = match source {
                Source::Unset => None,
                _ if sdk_disabled => Some("OTEL_SDK_DISABLED=true".to_string()),
                _ => switched_off(&get, signal, &mut notices),
            };
            SignalPlan {
                signal,
                source,
                endpoint,
                from,
                off,
            }
        });

        if signals.iter().any(SignalPlan::exported) {
            for signal in ["", "TRACES_", "METRICS_", "LOGS_"] {
                for setting in ["CERTIFICATE", "CLIENT_CERTIFICATE", "CLIENT_KEY"] {
                    let var = format!("OTEL_EXPORTER_OTLP_{signal}{setting}");
                    if get(&var).is_some() {
                        notices.push(Notice::Warn(format!(
                            "{var} is ignored: OTLP export uses deckwatch's outbound TLS \
                             settings (the system roots plus {})",
                            outbound::EXTRA_CA_FILE_ENV
                        )));
                    }
                }
            }
        }
        for s in signals
            .iter()
            .filter(|s| s.source == Source::Settings && s.exported())
        {
            let own = format!("OTEL_EXPORTER_OTLP_{}_HEADERS", s.signal.var());
            let headers_var = [own, "OTEL_EXPORTER_OTLP_HEADERS".to_string()]
                .into_iter()
                .find(|var| get(var).is_some());
            if let Some(var) = headers_var {
                notices.push(Notice::Warn(format!(
                    "{var} is not sent to the {} endpoint from settings: exporter headers go \
                     only to an endpoint set in the environment",
                    s.signal.name()
                )));
            }
        }
        Plan { signals, notices }
    }

    /// The plan for `signal`.
    pub fn signal(&self, signal: Signal) -> &SignalPlan {
        &self.signals[signal.index()]
    }

    /// Every signal's plan, in [`Signal::ALL`] order.
    pub fn all(&self) -> &[SignalPlan; 3] {
        &self.signals
    }

    /// The exported signals.
    #[cfg(test)]
    pub fn signals(&self) -> super::Signals {
        super::Signals {
            traces: self.signal(Signal::Traces).exported(),
            metrics: self.signal(Signal::Metrics).exported(),
            logs: self.signal(Signal::Logs).exported(),
        }
    }
}

/// Why a signal with an endpoint is off: `OTEL_<SIGNAL>_EXPORTER`, then the
/// protocol. An unsupported value is also an error notice.
fn switched_off(
    get: &impl Fn(&str) -> Option<String>,
    signal: Signal,
    notices: &mut Vec<Notice>,
) -> Option<String> {
    let name = signal.name();
    let exporter_var = format!("OTEL_{}_EXPORTER", signal.var());
    match get(&exporter_var)
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("otlp") => {}
        Some("none") => return Some(format!("{exporter_var}=none")),
        Some(other) => {
            notices.push(Notice::Error(format!(
                "{exporter_var}={other} is not supported (use otlp or none); {name} are not exported"
            )));
            return Some(format!("{exporter_var}={other} is not supported"));
        }
    }
    let protocol_var = format!("OTEL_EXPORTER_OTLP_{}_PROTOCOL", signal.var());
    let protocol = get(&protocol_var)
        .map(|v| (protocol_var.as_str(), v))
        .or_else(|| get("OTEL_EXPORTER_OTLP_PROTOCOL").map(|v| ("OTEL_EXPORTER_OTLP_PROTOCOL", v)));
    match protocol {
        Some((var, value)) if !value.eq_ignore_ascii_case(PROTOCOL) => {
            notices.push(Notice::Error(format!(
                "{var}={value} is not supported: deckwatch exports OTLP as {PROTOCOL} only; \
                 {name} are not exported"
            )));
            Some(format!("{var}={value} is not supported"))
        }
        _ => None,
    }
}

/// `base` with `path` appended, as the OpenTelemetry SDK joins
/// `OTEL_EXPORTER_OTLP_ENDPOINT` and a signal's path (one `/` between them).
pub fn join(base: &str, path: &str) -> String {
    match base.strip_suffix('/') {
        Some(base) => format!("{base}{path}"),
        None => format!("{base}{path}"),
    }
}

#[cfg(test)]
#[path = "endpoints_tests.rs"]
mod tests;
