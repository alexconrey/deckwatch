//! deckwatch's own telemetry: OpenTelemetry export of what deckwatch, and the
//! libraries in its process, emit through `tracing`. That includes the
//! embedded mcp-k8s library and plugin log lines, which extism turns into
//! `tracing` events (target `extism::pdk`).
//!
//! Export is **off** unless an OTLP endpoint is set, through the standard
//! `OTEL_*` variables or in Settings → Observability; per signal, an endpoint
//! variable wins ([`endpoints`]). Without one, the subscriber writes exactly
//! the JSON log lines deckwatch always wrote. When it is on:
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
//! The endpoints are read at startup, in two phases ([`Telemetry`]): those
//! from the environment before anything else, those from settings once the
//! database is read. A change in settings applies at the next restart.
//!
//! The resource is `service.name=deckwatch` and `service.version` (the crate
//! version), unless `OTEL_SERVICE_NAME` or `OTEL_RESOURCE_ATTRIBUTES` say
//! otherwise. Export is OTLP over HTTP (http/protobuf) through the shared
//! [`Purpose::Observability`] client, so the extra CA bundle applies (see
//! [`ObservabilityClient`]).
//!
//! The rest of the standard configuration is read by the OpenTelemetry SDK:
//! headers, timeout and compression (`OTEL_EXPORTER_OTLP_*`),
//! `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`, `OTEL_BSP_*`,
//! `OTEL_BLRP_*` and `OTEL_METRIC_EXPORT_INTERVAL`. The retired
//! `tracing.otlp_endpoint` setting is *not* used: see docs/TRACING.md,
//! "deckwatch's own telemetry".

pub mod endpoints;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mcp_k8s::telemetry::{semconv as mcp_semconv, W3cTraceContext};
use opentelemetry::context::FutureExt as _;
use opentelemetry::logs::Severity;
use opentelemetry::metrics::{
    Counter, Gauge, Histogram, HistogramBuilder, InstrumentBuilder, InstrumentProvider, Meter,
    MeterProvider, NoopMeterProvider, UpDownCounter,
};
use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry::{InstrumentationScope, Key, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response, ResponseBodyTooLarge};
use opentelemetry_otlp::{Protocol, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{BatchLogProcessor, LogProcessor, SdkLogRecord, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{
    Aggregation, Instrument, PeriodicReader, SdkMeterProvider, Stream,
};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::resource::{EnvResourceDetector, ResourceDetector as _};
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider, SpanData, SpanProcessor};
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
use endpoints::{ObservabilitySettings, Plan, Signal, SignalStatus, Source};

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

    fn get(self, signal: Signal) -> bool {
        match signal {
            Signal::Traces => self.traces,
            Signal::Metrics => self.metrics,
            Signal::Logs => self.logs,
        }
    }

    fn set(&mut self, signal: Signal) {
        match signal {
            Signal::Traces => self.traces = true,
            Signal::Metrics => self.metrics = true,
            Signal::Logs => self.logs = true,
        }
    }
}

/// A configuration problem, logged once the subscriber is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Warn(String),
    Error(String),
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

/// deckwatch's OpenTelemetry pipeline: a provider per signal, behind the
/// layers [`subscriber`] installs.
///
/// The layers are in the subscriber from the first log line, each switched
/// off. [`Telemetry::attach`] gives a signal its exporter and switches its
/// layer on, once: at startup for an endpoint from the environment
/// ([`Telemetry::start`]), and once the database is read for one from
/// settings ([`Telemetry::apply_settings`]). A layer that is off sees
/// nothing, so a signal without an endpoint changes nothing: no
/// OpenTelemetry spans, no trace ids in the logs, no exporter threads.
///
/// Adding the layers later instead doesn't work:
///
/// * `tracing-opentelemetry` finds a span's OpenTelemetry data by
///   downcasting the subscriber to its layer (`OpenTelemetrySpanExt::context`
///   and `set_parent`, which MCP remote parents, [`trace_id`] and plugins'
///   `trace_context` rely on), and a `tracing_subscriber::reload` layer
///   can't be downcast through;
/// * a layer with a per-layer filter (ours, and the one inside
///   `MetricsLayer`) gets its filter id from the registry while the
///   subscriber is built, so it can't be added afterwards, and
///   `reload::Handle::reload` doesn't support it.
///
/// So the layers stay, and what is attached is behind them: the span and log
/// processors, and the meter provider. Hot reload of the endpoints would
/// build on that, replacing what is attached and shutting the old one down;
/// [`global`] reaches the pipeline from a handler. Today an endpoint change
/// takes a restart.
#[derive(Debug, Clone)]
pub struct Telemetry {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// Per signal ([`Signal::ALL`] order), whether its exporter is attached.
    /// The layers' filters read these.
    on: [AtomicBool; 3],
    resource: Resource,
    span_processor: LateSpanProcessor,
    tracer_provider: SdkTracerProvider,
    log_processor: LateLogProcessor,
    logger_provider: SdkLoggerProvider,
    meter_provider: LateMeterProvider,
    /// What each signal is exported to, for the status API.
    running: Mutex<[SignalStatus; 3]>,
    /// Why an exporter couldn't be built, per signal.
    failed: Mutex<[Option<String>; 3]>,
    /// Notices already logged, so the second phase logs only new ones.
    logged: Mutex<Vec<Notice>>,
}

/// What [`Telemetry::attach`] connects, per signal: production attaches the
/// OTLP exporters, tests attach in-memory ones.
#[derive(Debug, Default)]
pub struct Exporters {
    pub traces: Option<Box<dyn SpanProcessor>>,
    pub metrics: Option<SdkMeterProvider>,
    pub logs: Option<Box<dyn LogProcessor>>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl Telemetry {
    /// The pipeline, with nothing attached.
    pub fn new() -> Telemetry {
        let resource = resource();
        let span_processor = LateSpanProcessor::default();
        let tracer_provider = SdkTracerProvider::builder()
            .with_span_processor(span_processor.clone())
            .with_resource(resource.clone())
            .build();
        let log_processor = LateLogProcessor::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_log_processor(log_processor.clone())
            .with_resource(resource.clone())
            .build();
        Telemetry {
            inner: Arc::new(Inner {
                on: Default::default(),
                resource,
                span_processor,
                tracer_provider,
                log_processor,
                logger_provider,
                meter_provider: LateMeterProvider::default(),
                running: Mutex::new(Signal::ALL.map(|_| SignalStatus::unset())),
                failed: Mutex::default(),
                logged: Mutex::default(),
            }),
        }
    }

    /// Whether `signal` has its exporter attached.
    pub fn is_on(&self, signal: Signal) -> bool {
        self.inner.is_on(signal)
    }

    /// The signals with an exporter attached.
    pub fn signals(&self) -> Signals {
        let mut signals = Signals::default();
        for signal in Signal::ALL.into_iter().filter(|s| self.is_on(*s)) {
            signals.set(signal);
        }
        signals
    }

    /// Attach `exporters`, each to its signal unless that one already has an
    /// exporter (which is then left as it is), and switch those layers on.
    /// Returns the signals attached.
    pub fn attach(&self, exporters: Exporters) -> Signals {
        let inner = &self.inner;
        let mut attached = Signals::default();
        if let Some(mut processor) = exporters.traces {
            processor.set_resource(&inner.resource);
            if inner.span_processor.0.set(processor).is_ok() {
                attached.traces = true;
            }
        }
        if let Some(provider) = exporters.metrics {
            if inner.meter_provider.0.set(provider).is_ok() {
                attached.metrics = true;
            }
        }
        if let Some(mut processor) = exporters.logs {
            processor.set_resource(&inner.resource);
            if inner.log_processor.0.set(processor).is_ok() {
                attached.logs = true;
            }
        }
        for signal in Signal::ALL.into_iter().filter(|s| attached.get(*s)) {
            inner.on[signal.index()].store(true, Ordering::Release);
        }
        if attached.any() {
            // The layers' filters answer differently now, and tracing caches
            // their answer per callsite.
            tracing::callsite::rebuild_interest_cache();
        }
        attached
    }

    /// What each signal is exported to, in [`Signal::ALL`] order: set by
    /// [`Telemetry::start`] and [`Telemetry::apply_settings`].
    pub fn running(&self) -> [SignalStatus; 3] {
        lock(&self.inner.running).clone()
    }

    /// Startup, before anything else: export the signals whose endpoint the
    /// `OTEL_*` variables set, and log the configuration problems and, when
    /// export is on, what is exported. Logs nothing when no endpoint variable
    /// is set. Call inside the Tokio runtime, which sends the export
    /// requests.
    pub fn start(&self) -> Plan {
        self.start_with(|name| std::env::var(name).ok())
    }

    fn start_with(&self, get: impl Fn(&str) -> Option<String>) -> Plan {
        let plan = Plan::resolve(&get, &ObservabilitySettings::default());
        let attached = self.attach_otlp(&plan, Source::Env);
        self.log_new_notices(&plan);
        if attached.any() {
            let signals = self.signals();
            tracing::info!(
                traces = signals.traces,
                metrics = signals.metrics,
                logs = signals.logs,
                endpoint = %endpoint_for_log(get("OTEL_EXPORTER_OTLP_ENDPOINT")),
                "exporting deckwatch's telemetry over OTLP ({PROTOCOL})"
            );
        }
        self.record(&plan);
        plan
    }

    /// Once the settings are read: also export the signals that the
    /// environment leaves to `settings` (an endpoint variable wins, per
    /// signal, see [`endpoints`]). Logs the problems not logged at startup
    /// and, when settings add signals, what they export.
    ///
    /// Called once, at startup: a signal is attached at most once, so a
    /// later change in settings applies at the next restart.
    pub fn apply_settings(&self, settings: &ObservabilitySettings) -> Plan {
        self.apply_settings_with(|name| std::env::var(name).ok(), settings)
    }

    fn apply_settings_with(
        &self,
        get: impl Fn(&str) -> Option<String>,
        settings: &ObservabilitySettings,
    ) -> Plan {
        let plan = Plan::resolve(&get, settings);
        let attached = self.attach_otlp(&plan, Source::Settings);
        self.log_new_notices(&plan);
        if attached.any() {
            let signals = self.signals();
            let used = |field_signals: &[Signal], value: &str| {
                if field_signals.iter().any(|s| attached.get(*s)) {
                    crate::redact::url(value)
                } else {
                    String::new()
                }
            };
            tracing::info!(
                traces = signals.traces,
                metrics = signals.metrics,
                logs = signals.logs,
                endpoint = %used(&[Signal::Traces, Signal::Logs], &settings.otlp_endpoint),
                metrics_endpoint = %used(&[Signal::Metrics], &settings.otlp_metrics_endpoint),
                "exporting deckwatch's telemetry over OTLP ({PROTOCOL}) to the endpoints in settings"
            );
        }
        self.record(&plan);
        plan
    }

    /// Build and attach the OTLP exporters of the signals `plan` exports with
    /// an endpoint from `source`, unless they already have one. The SDK reads
    /// the endpoint variables itself, so only an endpoint from settings is
    /// passed to it, and its requests go without exporter headers
    /// ([`ObservabilityClient::without_exporter_headers`]). A failure is
    /// logged and recorded for the status API.
    fn attach_otlp(&self, plan: &Plan, source: Source) -> Signals {
        let wanted: Vec<&endpoints::SignalPlan> = plan
            .all()
            .iter()
            .filter(|s| s.source == source && s.exported() && !self.is_on(s.signal))
            .collect();
        if wanted.is_empty() {
            return Signals::default();
        }
        let runtime = tokio::runtime::Handle::current();
        let mut exporters = Exporters::default();
        for signal_plan in wanted {
            let signal = signal_plan.signal;
            let endpoint = (source == Source::Settings)
                .then(|| signal_plan.endpoint.clone())
                .flatten();
            let client = || {
                let client = ObservabilityClient::new(runtime.clone());
                if endpoint.is_some() {
                    client.without_exporter_headers()
                } else {
                    client
                }
            };
            let built = match signal {
                Signal::Traces => {
                    let mut builder = opentelemetry_otlp::SpanExporter::builder()
                        .with_http()
                        .with_protocol(Protocol::HttpBinary)
                        .with_http_client(client());
                    if let Some(endpoint) = &endpoint {
                        builder = builder.with_endpoint(endpoint);
                    }
                    builder.build().map(|exporter| {
                        exporters.traces =
                            Some(Box::new(BatchSpanProcessor::builder(exporter).build()));
                    })
                }
                Signal::Metrics => {
                    let mut builder = opentelemetry_otlp::MetricExporter::builder()
                        .with_http()
                        .with_protocol(Protocol::HttpBinary)
                        .with_http_client(client());
                    if let Some(endpoint) = &endpoint {
                        builder = builder.with_endpoint(endpoint);
                    }
                    builder.build().map(|exporter| {
                        exporters.metrics = Some(
                            SdkMeterProvider::builder()
                                .with_reader(PeriodicReader::builder(exporter).build())
                                .with_resource(self.inner.resource.clone())
                                .with_view(mcp_duration_view)
                                .build(),
                        );
                    })
                }
                Signal::Logs => {
                    let mut builder = opentelemetry_otlp::LogExporter::builder()
                        .with_http()
                        .with_protocol(Protocol::HttpBinary)
                        .with_http_client(client());
                    if let Some(endpoint) = &endpoint {
                        builder = builder.with_endpoint(endpoint);
                    }
                    builder.build().map(|exporter| {
                        exporters.logs =
                            Some(Box::new(BatchLogProcessor::builder(exporter).build()));
                    })
                }
            };
            if let Err(e) = built {
                let message = format!("{} are not exported: {e}", signal.name());
                tracing::error!("{message}");
                lock(&self.inner.failed)[signal.index()] = Some(message);
            }
        }
        self.attach(exporters)
    }

    /// Log the notices of `plan` that weren't logged yet.
    fn log_new_notices(&self, plan: &Plan) {
        let mut logged = lock(&self.inner.logged);
        for notice in &plan.notices {
            if logged.contains(notice) {
                continue;
            }
            match notice {
                Notice::Warn(message) => tracing::warn!("{message}"),
                Notice::Error(message) => tracing::error!("{message}"),
            }
            logged.push(notice.clone());
        }
    }

    /// Record what each signal is exported to, after attaching for `plan`.
    fn record(&self, plan: &Plan) {
        let failed = lock(&self.inner.failed).clone();
        let mut running = lock(&self.inner.running);
        for signal in Signal::ALL {
            let i = signal.index();
            let mut status = plan.signal(signal).status();
            status.exported = self.is_on(signal);
            if !status.exported && status.reason.is_none() {
                status.reason.clone_from(&failed[i]);
            }
            running[i] = status;
        }
    }

    /// The layers that feed the providers, for the subscriber. Each one is
    /// off until its signal is attached.
    ///
    /// Every layer skips [`on_export_path`] targets. The trace layer takes
    /// spans; it takes events too (as span events) only when logs are not
    /// exported, except error events, which it always takes so they set the
    /// span's status. Metric events (mcp-k8s's duration histogram, see
    /// [`crate::logging::is_log_line`]) go to the metrics layer only.
    fn layer<S>(&self) -> Box<dyn Layer<S> + Send + Sync + 'static>
    where
        S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        let inner = &self.inner;
        let on = self.inner.clone();
        let spans_filter = filter_fn(move |meta: &Metadata<'_>| {
            on.is_on(Signal::Traces)
                && !on_export_path(meta.target())
                && (meta.is_span()
                    || (is_log_line(meta)
                        && (!on.is_on(Signal::Logs) || *meta.level() == Level::ERROR)))
        });
        let on = self.inner.clone();
        let metrics_filter = filter_fn(move |meta: &Metadata<'_>| {
            on.is_on(Signal::Metrics) && !on_export_path(meta.target())
        });
        let on = self.inner.clone();
        let logs_filter = filter_fn(move |meta: &Metadata<'_>| {
            on.is_on(Signal::Logs) && !on_export_path(meta.target()) && is_log_line(meta)
        });
        vec![
            tracing_opentelemetry::layer()
                .with_tracer(inner.tracer_provider.tracer(DEFAULT_SERVICE_NAME))
                .with_filter(spans_filter)
                .boxed(),
            tracing_opentelemetry::MetricsLayer::new(inner.meter_provider.clone())
                .with_filter(metrics_filter)
                .boxed(),
            OpenTelemetryTracingBridge::new(&inner.logger_provider)
                .with_filter(logs_filter)
                .boxed(),
        ]
        .boxed()
    }
}

impl Inner {
    fn is_on(&self, signal: Signal) -> bool {
        self.on[signal.index()].load(Ordering::Acquire)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The process's pipeline, once `run` has set it up.
static GLOBAL: OnceLock<Telemetry> = OnceLock::new();

/// Make `telemetry` the process's pipeline ([`global`]). Only the first call
/// counts.
pub fn install_global(telemetry: Telemetry) -> bool {
    GLOBAL.set(telemetry).is_ok()
}

/// The process's pipeline, for the status API; `None` until `run` set it up
/// (in tests).
pub fn global() -> Option<&'static Telemetry> {
    GLOBAL.get()
}

/// The tracer provider's span processor: does nothing until a processor is
/// attached, then forwards to it.
#[derive(Debug, Clone, Default)]
struct LateSpanProcessor(Arc<OnceLock<Box<dyn SpanProcessor>>>);

impl SpanProcessor for LateSpanProcessor {
    fn on_start(&self, span: &mut opentelemetry_sdk::trace::Span, cx: &opentelemetry::Context) {
        if let Some(processor) = self.0.get() {
            processor.on_start(span, cx);
        }
    }

    fn on_end(&self, span: SpanData) {
        if let Some(processor) = self.0.get() {
            processor.on_end(span);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.get().map_or(Ok(()), |p| p.force_flush())
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0
            .get()
            .map_or(Ok(()), |p| p.shutdown_with_timeout(timeout))
    }
}

/// The logger provider's log processor: does nothing until a processor is
/// attached, then forwards to it.
#[derive(Debug, Clone, Default)]
struct LateLogProcessor(Arc<OnceLock<Box<dyn LogProcessor>>>);

impl LogProcessor for LateLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if let Some(processor) = self.0.get() {
            processor.emit(data, instrumentation);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.get().map_or(Ok(()), |p| p.force_flush())
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0
            .get()
            .map_or(Ok(()), |p| p.shutdown_with_timeout(timeout))
    }

    fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
        self.0
            .get()
            .is_some_and(|p| p.event_enabled(level, target, name))
    }
}

/// The meter provider `MetricsLayer` is built with, before metrics have an
/// exporter: its instruments come from the [`SdkMeterProvider`] attached
/// later. `MetricsLayer` creates an instrument the first time a metric is
/// recorded, and its filter lets no metric event through before the provider
/// is attached, so every instrument it keeps comes from that provider.
#[derive(Debug, Clone, Default)]
struct LateMeterProvider(Arc<OnceLock<SdkMeterProvider>>);

impl MeterProvider for LateMeterProvider {
    fn meter_with_scope(&self, scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(LateInstruments {
            provider: self.0.clone(),
            scope,
        }))
    }
}

/// Creates each instrument with the attached meter provider (a no-op one
/// before it is attached).
struct LateInstruments {
    provider: Arc<OnceLock<SdkMeterProvider>>,
    scope: InstrumentationScope,
}

impl LateInstruments {
    fn meter(&self) -> Meter {
        match self.provider.get() {
            Some(provider) => provider.meter_with_scope(self.scope.clone()),
            None => NoopMeterProvider::new().meter_with_scope(self.scope.clone()),
        }
    }
}

/// [`InstrumentProvider`] methods that create the instrument with
/// [`LateInstruments::meter`], copying what the builder carries.
macro_rules! late_instruments {
    ($($method:ident: $instrument:ty),* $(,)?) => {
        $(
            fn $method(&self, builder: InstrumentBuilder<'_, $instrument>) -> $instrument {
                let meter = self.meter();
                let mut late = meter.$method(builder.name);
                if let Some(description) = builder.description {
                    late = late.with_description(description);
                }
                if let Some(unit) = builder.unit {
                    late = late.with_unit(unit);
                }
                late.build()
            }
        )*
    };
}

macro_rules! late_histograms {
    ($($method:ident: $instrument:ty),* $(,)?) => {
        $(
            fn $method(&self, builder: HistogramBuilder<'_, $instrument>) -> $instrument {
                let meter = self.meter();
                let mut late = meter.$method(builder.name);
                if let Some(description) = builder.description {
                    late = late.with_description(description);
                }
                if let Some(unit) = builder.unit {
                    late = late.with_unit(unit);
                }
                if let Some(boundaries) = builder.boundaries {
                    late = late.with_boundaries(boundaries);
                }
                late.build()
            }
        )*
    };
}

/// The synchronous instruments `MetricsLayer` creates; it has no observable
/// ones.
impl InstrumentProvider for LateInstruments {
    late_instruments! {
        u64_counter: Counter<u64>,
        f64_counter: Counter<f64>,
        i64_up_down_counter: UpDownCounter<i64>,
        f64_up_down_counter: UpDownCounter<f64>,
        u64_gauge: Gauge<u64>,
        i64_gauge: Gauge<i64>,
        f64_gauge: Gauge<f64>,
    }
    late_histograms! {
        u64_histogram: Histogram<u64>,
        f64_histogram: Histogram<f64>,
    }
}

/// `endpoint` with its credentials masked ([`crate::redact::url`]), for the
/// startup log.
fn endpoint_for_log(endpoint: Option<String>) -> String {
    let Some(endpoint) = endpoint.filter(|e| !e.trim().is_empty()) else {
        return "(per-signal endpoints)".to_string();
    };
    match reqwest::Url::parse(endpoint.trim()) {
        Ok(_) => crate::redact::url(endpoint.trim()),
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
/// (stdout in production), and the OpenTelemetry layers, each off until
/// [`Telemetry::attach`] switches it on. With every layer off it formats
/// exactly what `tracing_subscriber::fmt().with_env_filter(filter).json()`
/// did.
pub fn subscriber<W>(
    filter: EnvFilter,
    writer: W,
) -> (
    impl Subscriber + for<'a> LookupSpan<'a> + Send + Sync + 'static,
    Telemetry,
)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let telemetry = Telemetry::new();
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(crate::logging::json_layer(writer))
        .with(telemetry.layer());
    (subscriber, telemetry)
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
    without_exporter_headers: bool,
}

impl ObservabilityClient {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            failures: Arc::default(),
            without_exporter_headers: false,
        }
    }

    /// Send only the content type and encoding of each request: the client
    /// for an endpoint from settings. The SDK adds the
    /// `OTEL_EXPORTER_OTLP_[<SIGNAL>_]HEADERS` to every request, and those
    /// headers (usually credentials) are meant for the endpoint set in the
    /// environment. Settings can be changed through the API, so they must not
    /// be able to send them elsewhere. The `User-Agent` is then the outbound
    /// client's.
    pub fn without_exporter_headers(mut self) -> Self {
        self.without_exporter_headers = true;
        self
    }
}

/// The headers of an OTLP request that aren't exporter headers.
const PROTOCOL_HEADERS: [axum::http::header::HeaderName; 2] = [
    axum::http::header::CONTENT_TYPE,
    axum::http::header::CONTENT_ENCODING,
];

/// `request` with only its [`PROTOCOL_HEADERS`].
fn without_exporter_headers(mut request: Request<Bytes>) -> Request<Bytes> {
    let headers = request.headers_mut();
    let kept: Vec<_> = PROTOCOL_HEADERS
        .iter()
        .filter_map(|name| Some((name.clone(), headers.get(name)?.clone())))
        .collect();
    headers.clear();
    headers.extend(kept);
    request
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
        let request = if self.without_exporter_headers {
            without_exporter_headers(request)
        } else {
            request
        };
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
