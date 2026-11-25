//! deckwatch's own telemetry: OpenTelemetry export of what deckwatch, and the
//! libraries in its process, emit through `tracing`. That includes the
//! embedded mcp-k8s library and plugin log lines, which extism turns into
//! `tracing` events (target `extism::pdk`).
//!
//! Export is **off** unless an OTLP endpoint is set through the standard
//! `OTEL_*` variables ([`signals_from`]); without one, the subscriber writes
//! exactly the JSON log lines deckwatch always wrote. When it is on:
//!
//! * spans become traces (`tracing-opentelemetry`);
//! * events become OTLP log records, carrying the trace and span id of the
//!   span they happened in (`opentelemetry-appender-tracing`). When logs are
//!   not exported, events go out as span events instead, so a traces-only
//!   backend such as the chart's bundled Tempo or Jaeger still shows them;
//! * event fields named `monotonic_counter.*`, `counter.*` or `histogram.*`
//!   become metrics (`tracing_opentelemetry::MetricsLayer`). Prometheus
//!   `/metrics` is separate and unchanged.
//!
//! The resource is `service.name=deckwatch` and `service.version` (the crate
//! version), unless `OTEL_SERVICE_NAME` or `OTEL_RESOURCE_ATTRIBUTES` say
//! otherwise. Export is OTLP over HTTP (http/protobuf) through the shared
//! [`Purpose::Observability`] client, so the extra CA bundle applies (see
//! [`ObservabilityClient`]).
//!
//! The rest of the standard configuration is read by the OpenTelemetry SDK:
//! `OTEL_EXPORTER_OTLP_*` endpoints, headers, timeout and compression,
//! `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`, `OTEL_BSP_*`, `OTEL_BLRP_*`
//! and `OTEL_METRIC_EXPORT_INTERVAL`. `tracing.otlp_endpoint` in deckwatch's
//! settings is *not* used: see docs/TRACING.md, "deckwatch's own telemetry".

use std::sync::{Arc, Mutex};

use mcp_k8s::telemetry::{semconv as mcp_semconv, W3cTraceContext};
use opentelemetry::context::FutureExt as _;
use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry::{Key, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response, ResponseBodyTooLarge};
use opentelemetry_otlp::{Protocol, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{
    Aggregation, Instrument, PeriodicReader, SdkMeterProvider, Stream,
};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::resource::{EnvResourceDetector, ResourceDetector as _};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing::{Level, Metadata, Span, Subscriber};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

use crate::logging::is_log_line;
use crate::outbound::{self, Purpose};

/// `service.name` when neither `OTEL_SERVICE_NAME` nor
/// `OTEL_RESOURCE_ATTRIBUTES` sets one.
pub const DEFAULT_SERVICE_NAME: &str = "deckwatch";

/// The OTLP protocol deckwatch exports with. gRPC is not supported.
pub const PROTOCOL: &str = "http/protobuf";

/// The largest OTLP response body accepted, as in opentelemetry-http.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// The signals deckwatch exports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Signals {
    pub traces: bool,
    pub metrics: bool,
    pub logs: bool,
}

impl Signals {
    pub fn any(self) -> bool {
        self.traces || self.metrics || self.logs
    }
}

/// A configuration problem, logged once the subscriber is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Warn(String),
    Error(String),
}

/// The signals the `OTEL_*` variables ask for, read through `get`.
///
/// A signal is exported when an endpoint is set for it, through
/// `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT`,
/// and none of these turn it off:
///
/// * `OTEL_SDK_DISABLED=true` turns everything off;
/// * `OTEL_<SIGNAL>_EXPORTER` must be unset, `otlp` or `none`;
/// * the protocol (`OTEL_EXPORTER_OTLP_[<SIGNAL>_]PROTOCOL`) must be unset or
///   `http/protobuf`.
///
/// Without any endpoint nothing is exported and there are no notices: the
/// spec's default endpoint (`localhost:4318`) is never used implicitly.
pub fn signals_from(get: impl Fn(&str) -> Option<String>) -> (Signals, Vec<Notice>) {
    let get = |name: &str| {
        get(name)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let mut notices = Vec::new();
    if get("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return (Signals::default(), notices);
    }
    let generic_endpoint = get("OTEL_EXPORTER_OTLP_ENDPOINT").is_some();
    let mut signal = |signal: &str, name: &str| -> bool {
        if !generic_endpoint && get(&format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT")).is_none() {
            return false;
        }
        let exporter_var = format!("OTEL_{signal}_EXPORTER");
        match get(&exporter_var)
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("otlp") => {}
            Some("none") => return false,
            Some(other) => {
                notices.push(Notice::Error(format!(
                    "{exporter_var}={other} is not supported (use otlp or none); {name} are not exported"
                )));
                return false;
            }
        }
        let protocol_var = format!("OTEL_EXPORTER_OTLP_{signal}_PROTOCOL");
        let protocol = get(&protocol_var)
            .map(|v| (protocol_var.as_str(), v))
            .or_else(|| {
                get("OTEL_EXPORTER_OTLP_PROTOCOL").map(|v| ("OTEL_EXPORTER_OTLP_PROTOCOL", v))
            });
        match protocol {
            Some((var, value)) if !value.eq_ignore_ascii_case(PROTOCOL) => {
                notices.push(Notice::Error(format!(
                    "{var}={value} is not supported: deckwatch exports OTLP as {PROTOCOL} only; \
                     {name} are not exported"
                )));
                false
            }
            _ => true,
        }
    };
    let signals = Signals {
        traces: signal("TRACES", "traces"),
        metrics: signal("METRICS", "metrics"),
        logs: signal("LOGS", "logs"),
    };
    if signals.any() {
        for signal in ["", "TRACES_", "METRICS_", "LOGS_"] {
            for setting in ["CERTIFICATE", "CLIENT_CERTIFICATE", "CLIENT_KEY"] {
                let var = format!("OTEL_EXPORTER_OTLP_{signal}{setting}");
                if get(&var).is_some() {
                    notices.push(Notice::Warn(format!(
                        "{var} is ignored: OTLP export uses deckwatch's outbound TLS settings \
                         (the system roots plus {})",
                        outbound::EXTRA_CA_FILE_ENV
                    )));
                }
            }
        }
    }
    (signals, notices)
}

/// The resource for every signal: `service.name` (default
/// [`DEFAULT_SERVICE_NAME`]) and `service.version` (the crate version), plus
/// what `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES` set, which wins.
pub fn resource() -> Resource {
    let from_env = EnvResourceDetector::new().detect();
    let service_name_set = std::env::var("OTEL_SERVICE_NAME").is_ok_and(|v| !v.trim().is_empty())
        || from_env.get(&Key::new("service.name")).is_some();
    let mut builder = Resource::builder();
    if !service_name_set {
        builder = builder.with_service_name(DEFAULT_SERVICE_NAME);
    }
    if from_env.get(&Key::new("service.version")).is_none() {
        builder =
            builder.with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")));
    }
    builder.build()
}

/// The OpenTelemetry providers, one per exported signal.
#[derive(Debug, Default)]
pub struct Telemetry {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

/// What [`Telemetry::from_env`] found.
#[derive(Debug, Default)]
pub struct TelemetryInit {
    telemetry: Option<Telemetry>,
    notices: Vec<Notice>,
}

impl Telemetry {
    /// The OTLP pipeline the `OTEL_*` variables ask for, if any. Must be
    /// called inside the Tokio runtime, which sends the export requests.
    pub fn from_env() -> TelemetryInit {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> TelemetryInit {
        let (signals, mut notices) = signals_from(get);
        if !signals.any() {
            return TelemetryInit {
                telemetry: None,
                notices,
            };
        }
        let runtime = tokio::runtime::Handle::current();
        let resource = resource();
        let mut telemetry = Telemetry::default();

        if signals.traces {
            match opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpBinary)
                .with_http_client(ObservabilityClient::new(runtime.clone()))
                .build()
            {
                Ok(exporter) => {
                    telemetry.tracer_provider = Some(
                        SdkTracerProvider::builder()
                            .with_batch_exporter(exporter)
                            .with_resource(resource.clone())
                            .build(),
                    )
                }
                Err(e) => notices.push(Notice::Error(format!("traces are not exported: {e}"))),
            }
        }
        if signals.metrics {
            match opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpBinary)
                .with_http_client(ObservabilityClient::new(runtime.clone()))
                .build()
            {
                Ok(exporter) => {
                    telemetry.meter_provider = Some(
                        SdkMeterProvider::builder()
                            .with_reader(PeriodicReader::builder(exporter).build())
                            .with_resource(resource.clone())
                            .with_view(mcp_duration_view)
                            .build(),
                    )
                }
                Err(e) => notices.push(Notice::Error(format!("metrics are not exported: {e}"))),
            }
        }
        if signals.logs {
            match opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpBinary)
                .with_http_client(ObservabilityClient::new(runtime))
                .build()
            {
                Ok(exporter) => {
                    telemetry.logger_provider = Some(
                        SdkLoggerProvider::builder()
                            .with_batch_exporter(exporter)
                            .with_resource(resource)
                            .build(),
                    )
                }
                Err(e) => notices.push(Notice::Error(format!("logs are not exported: {e}"))),
            }
        }

        TelemetryInit {
            telemetry: telemetry.signals().any().then_some(telemetry),
            notices,
        }
    }

    /// The signals with a provider.
    pub fn signals(&self) -> Signals {
        Signals {
            traces: self.tracer_provider.is_some(),
            metrics: self.meter_provider.is_some(),
            logs: self.logger_provider.is_some(),
        }
    }

    /// The layers that feed the providers, for the subscriber.
    ///
    /// Every layer skips [`on_export_path`] targets. The trace layer takes
    /// spans; it takes events too (as span events) only when logs are not
    /// exported, except error events, which it always takes so they set the
    /// span's status. Metric events (mcp-k8s's duration histogram, see
    /// [`crate::logging::is_log_line`]) go to the metrics layer only.
    pub fn layer<S>(&self) -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
    where
        S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        let logs_exported = self.logger_provider.is_some();
        let mut layers: Vec<Box<dyn Layer<S> + Send + Sync + 'static>> = Vec::new();
        if let Some(provider) = &self.tracer_provider {
            let spans_filter = filter_fn(move |meta: &Metadata<'_>| {
                !on_export_path(meta.target())
                    && (meta.is_span()
                        || (is_log_line(meta) && (!logs_exported || *meta.level() == Level::ERROR)))
            });
            layers.push(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer(DEFAULT_SERVICE_NAME))
                    .with_filter(spans_filter)
                    .boxed(),
            );
        }
        if let Some(provider) = &self.meter_provider {
            layers.push(
                tracing_opentelemetry::MetricsLayer::new(provider.clone())
                    .with_filter(filter_fn(|meta| !on_export_path(meta.target())))
                    .boxed(),
            );
        }
        if let Some(provider) = &self.logger_provider {
            layers.push(
                OpenTelemetryTracingBridge::new(provider)
                    .with_filter(filter_fn(|meta| {
                        !on_export_path(meta.target()) && is_log_line(meta)
                    }))
                    .boxed(),
            );
        }
        (!layers.is_empty()).then(|| layers.boxed())
    }
}

impl TelemetryInit {
    pub fn telemetry(&self) -> Option<&Telemetry> {
        self.telemetry.as_ref()
    }

    /// Log the configuration problems and, when export is on, what is
    /// exported. Call once the subscriber is installed. Logs nothing when no
    /// OTLP endpoint is configured.
    pub fn log(&self) {
        for notice in &self.notices {
            match notice {
                Notice::Warn(message) => tracing::warn!("{message}"),
                Notice::Error(message) => tracing::error!("{message}"),
            }
        }
        if let Some(telemetry) = &self.telemetry {
            let signals = telemetry.signals();
            tracing::info!(
                traces = signals.traces,
                metrics = signals.metrics,
                logs = signals.logs,
                endpoint = %endpoint_for_log(std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()),
                "exporting deckwatch's telemetry over OTLP ({PROTOCOL})"
            );
        }
    }
}

/// `endpoint` without userinfo, query or fragment, for the startup log.
fn endpoint_for_log(endpoint: Option<String>) -> String {
    let Some(endpoint) = endpoint.filter(|e| !e.trim().is_empty()) else {
        return "(per-signal endpoints)".to_string();
    };
    match reqwest::Url::parse(endpoint.trim()) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "(invalid URL)".to_string(),
    }
}

/// Targets on the export path itself: the HTTP and TLS stack the exporter
/// sends through, and the OpenTelemetry SDK (prefix match). The
/// OpenTelemetry layers skip them even when `RUST_LOG` enables them, so an
/// export never produces telemetry that the next export carries. They still
/// reach the JSON logs.
pub fn on_export_path(target: &str) -> bool {
    ["hyper", "h2", "reqwest", "rustls", "opentelemetry"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

/// `MetricsLayer` creates instruments without a unit or buckets, so this view
/// gives mcp-k8s's `mcp.server.operation.duration` histogram the MCP
/// conventions' unit (`s`) and bucket boundaries, and keeps only its
/// low-cardinality attributes: even if a field is added to the metric event,
/// it can't become an attribute.
pub fn mcp_duration_view(instrument: &Instrument) -> Option<Stream> {
    if instrument.name() != mcp_semconv::MCP_SERVER_OPERATION_DURATION {
        return None;
    }
    Stream::builder()
        .with_unit(mcp_semconv::MCP_SERVER_OPERATION_DURATION_UNIT)
        .with_description(
            "Duration of MCP requests as observed by deckwatch, from receipt to response.",
        )
        .with_aggregation(Aggregation::ExplicitBucketHistogram {
            boundaries: mcp_semconv::MCP_SERVER_OPERATION_DURATION_BUCKETS.to_vec(),
            record_min_max: true,
        })
        .with_allowed_attribute_keys(
            mcp_semconv::METRIC_ATTRIBUTES
                .iter()
                .map(|k| Key::from_static_str(k)),
        )
        .build()
        .ok()
}

/// Make the W3C trace context an MCP request carries the remote parent of
/// its server span. `params._meta.traceparent` (the MCP conventions) wins;
/// the HTTP `traceparent` header is the fallback, and becomes a link when
/// both are set. Call it before `span` is first entered. A no-op unless
/// traces are exported.
pub fn set_remote_parent(
    span: &Span,
    meta: Option<&W3cTraceContext>,
    http: Option<&W3cTraceContext>,
) {
    let propagator = TraceContextPropagator::new();
    let extract = |c: &W3cTraceContext| {
        let mut carrier = std::collections::HashMap::new();
        carrier.insert("traceparent".to_string(), c.traceparent.clone());
        if let Some(state) = &c.tracestate {
            carrier.insert("tracestate".to_string(), state.clone());
        }
        let cx = propagator.extract(&carrier);
        cx.span().span_context().is_valid().then_some(cx)
    };
    match (meta.and_then(extract), http.and_then(extract)) {
        (Some(parent), http) => {
            let _ = span.set_parent(parent);
            if let Some(http) = http {
                span.add_link(http.span().span_context().clone());
            }
        }
        (None, Some(parent)) => {
            let _ = span.set_parent(parent);
        }
        (None, None) => {}
    }
}

/// The W3C trace id of `span`, when traces are exported. Reading it starts
/// the OpenTelemetry span, so call it after [`set_remote_parent`].
pub fn trace_id(span: &Span) -> Option<String> {
    let cx = span.context();
    let span_context = cx.span().span_context().clone();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

/// deckwatch's subscriber: `filter`, one JSON line per event to `writer`
/// (stdout in production), and the OpenTelemetry layers when `telemetry` is
/// set. Without them it formats exactly what
/// `tracing_subscriber::fmt().with_env_filter(filter).json()` did.
pub fn subscriber<W>(
    filter: EnvFilter,
    telemetry: Option<&Telemetry>,
    writer: W,
) -> impl Subscriber + for<'a> LookupSpan<'a> + Send + Sync + 'static
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let base = tracing_subscriber::registry()
        .with(filter)
        .with(crate::logging::json_layer(writer));
    let otel = telemetry.and_then(|t| t.layer());
    base.with(otel)
}

/// The OTLP exporters' HTTP client: requests go through the shared
/// [`Purpose::Observability`] client from [`crate::outbound`]. So OTLP export
/// gets the same trust as every other outbound call (the system roots plus
/// `DECKWATCH_EXTRA_CA_FILE`), the same proxy settings, and that purpose's
/// timeouts (5 s connect, 10 s total). The exporter sets its own
/// `User-Agent`.
///
/// It is a bridge rather than the client itself for two reasons:
///
/// * **reqwest version.** opentelemetry-http 0.33 implements [`HttpClient`]
///   for reqwest 0.13; deckwatch is on reqwest 0.12 until #239.
/// * **Runtime.** The SDK's batch processors and periodic reader export from
///   their own threads with `futures_executor::block_on`, and a reqwest
///   client needs a Tokio runtime. Each request therefore runs as a task on
///   deckwatch's runtime while the export thread waits for it. This part
///   stays after #239.
///
/// The task carries the export thread's OpenTelemetry context, in which the
/// SDK suppresses telemetry, so events emitted while sending are not
/// exported in turn ([`on_export_path`] covers the connection tasks reqwest
/// spawns).
///
/// Each exporter gets its own client, which logs a failed request (the TLS
/// or connection reason, or the HTTP status) when the failure changes, and
/// logs again once requests succeed. The SDK's own export error only says
/// "network error" and repeats on every retry.
#[derive(Debug)]
pub struct ObservabilityClient {
    runtime: tokio::runtime::Handle,
    failures: Arc<FailureLog>,
}

impl ObservabilityClient {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            failures: Arc::default(),
        }
    }
}

/// The current failure of one exporter's requests, so that each failure is
/// logged once rather than on every retry.
#[derive(Debug, Default)]
struct FailureLog(Mutex<Option<String>>);

#[derive(Debug, PartialEq, Eq)]
enum Report {
    Failed(String),
    Recovered,
}

impl FailureLog {
    /// Record a request's outcome (`None` for success) and say what to log.
    fn record(&self, failure: Option<String>) -> Option<Report> {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match failure {
            Some(reason) if current.as_deref() == Some(reason.as_str()) => None,
            Some(reason) => {
                *current = Some(reason.clone());
                Some(Report::Failed(reason))
            }
            None => current.take().map(|_| Report::Recovered),
        }
    }
}

#[async_trait::async_trait]
impl HttpClient for ObservabilityClient {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        // Looked up per request: the exporter is built before `run` installs
        // the process-wide clients.
        let client = outbound::global().client(Purpose::Observability);
        let failures = self.failures.clone();
        let task = async move {
            let result = send(client, request).await;
            let failure = match &result {
                Ok(response) if response.status().is_success() => None,
                Ok(response) => Some(format!("HTTP {}", response.status())),
                Err(e) => Some(outbound::error_chain(e.as_ref())),
            };
            match failures.record(failure) {
                Some(Report::Failed(reason)) => {
                    tracing::warn!(error = %reason, "OTLP export request failed")
                }
                Some(Report::Recovered) => tracing::info!("OTLP export requests succeed again"),
                None => {}
            }
            result
        };
        self.runtime
            .spawn(task.with_context(opentelemetry::Context::current()))
            .await?
    }
}

/// One OTLP request through a reqwest 0.12 client, reading at most
/// [`MAX_RESPONSE_BYTES`] of response body.
async fn send(
    client: reqwest::Client,
    request: Request<Bytes>,
) -> Result<Response<Bytes>, HttpError> {
    let mut response = client.execute(reqwest::Request::try_from(request)?).await?;
    let status = response.status();
    let headers = std::mem::take(response.headers_mut());
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Box::new(ResponseBodyTooLarge::new()));
        }
        body.extend_from_slice(&chunk);
    }
    let mut out = Response::builder().status(status).body(Bytes::from(body))?;
    *out.headers_mut() = headers;
    Ok(out)
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod telemetry_tests;
