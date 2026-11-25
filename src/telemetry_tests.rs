use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use opentelemetry::logs::AnyValue;
use opentelemetry::trace::Status;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor};
use tower::ServiceExt as _;

use super::*;
use crate::logging::DEFAULT_LOG_FILTER;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// An environment holding only `vars`.
fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

/// The signals the `OTEL_*` variables alone ask for, and their configuration
/// problems.
fn signals_from(get: impl Fn(&str) -> Option<String>) -> (Signals, Vec<Notice>) {
    let plan = Plan::resolve(get, &ObservabilitySettings::default());
    (plan.signals(), plan.notices)
}

const NONE: Signals = Signals {
    traces: false,
    metrics: false,
    logs: false,
};
const ALL: Signals = Signals {
    traces: true,
    metrics: true,
    logs: true,
};
const ENDPOINT: (&str, &str) = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");

#[test]
fn nothing_is_exported_when_unconfigured() {
    let without_endpoint: [&[(&str, &str)]; 2] = [
        &[],
        // Every other OTEL_* variable, but no endpoint (a blank one is unset).
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "  "),
            ("OTEL_SERVICE_NAME", "deckwatch-dev"),
            ("OTEL_TRACES_EXPORTER", "otlp"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer x"),
            ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/ca.pem"),
        ],
    ];
    for vars in without_endpoint {
        assert_eq!(signals_from(env(vars)), (NONE, vec![]), "{vars:?}");
        // Nothing attached, so no exporter threads, and nothing to log.
        let telemetry = Telemetry::new();
        let plan = telemetry.start_with(env(vars));
        assert_eq!(telemetry.signals(), NONE, "{vars:?}");
        assert!(plan.notices.is_empty(), "{vars:?}");
        assert_eq!(
            telemetry.running(),
            Signal::ALL.map(|_| SignalStatus::unset())
        );
    }
}

#[test]
fn an_endpoint_turns_export_on() {
    assert_eq!(signals_from(env(&[ENDPOINT])), (ALL, vec![]));
    assert_eq!(
        signals_from(env(&[
            ENDPOINT,
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        ])),
        (ALL, vec![])
    );
    // A signal's own endpoint turns on only that signal.
    assert_eq!(
        signals_from(env(&[(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://tempo:4318/v1/traces"
        )])),
        (
            Signals {
                traces: true,
                ..NONE
            },
            vec![]
        )
    );
}

#[test]
fn signals_can_be_turned_off() {
    assert_eq!(
        signals_from(env(&[
            ENDPOINT,
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "NONE"),
        ])),
        (
            Signals {
                traces: true,
                ..NONE
            },
            vec![]
        )
    );
    assert_eq!(
        signals_from(env(&[ENDPOINT, ("OTEL_SDK_DISABLED", "true")])),
        (NONE, vec![])
    );
}

#[test]
fn unsupported_protocols_and_exporters_turn_a_signal_off() {
    let (signals, notices) = signals_from(env(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
        // A signal's own protocol wins over the generic one.
        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/protobuf"),
        ("OTEL_METRICS_EXPORTER", "prometheus"),
    ]));
    assert_eq!(signals, Signals { logs: true, ..NONE });
    assert_eq!(
        notices,
        [
            Notice::Error(
                "OTEL_EXPORTER_OTLP_PROTOCOL=grpc is not supported: deckwatch exports OTLP as \
                 http/protobuf only; traces are not exported"
                    .to_string()
            ),
            Notice::Error(
                "OTEL_METRICS_EXPORTER=prometheus is not supported (use otlp or none); metrics \
                 are not exported"
                    .to_string()
            ),
        ]
    );
}

#[test]
fn tls_variables_are_reported_as_ignored() {
    let (signals, notices) = signals_from(env(&[
        ENDPOINT,
        ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/etc/ca.pem"),
        ("OTEL_EXPORTER_OTLP_TRACES_CLIENT_KEY", "/etc/key.pem"),
    ]));
    assert_eq!(signals, ALL);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices.iter().all(|n| matches!(
        n,
        Notice::Warn(m) if m.contains("is ignored") && m.contains("DECKWATCH_EXTRA_CA_FILE")
    )));
}

#[test]
fn resource_names_deckwatch_and_its_version() {
    // OTEL_SERVICE_NAME and OTEL_RESOURCE_ATTRIBUTES win; skip if the
    // environment running the tests sets them.
    if std::env::var_os("OTEL_SERVICE_NAME").is_some()
        || std::env::var_os("OTEL_RESOURCE_ATTRIBUTES").is_some()
    {
        return;
    }
    let resource = resource();
    assert_eq!(
        resource.get(&Key::new("service.name")),
        Some("deckwatch".into())
    );
    assert_eq!(
        resource.get(&Key::new("service.version")),
        Some(env!("CARGO_PKG_VERSION").into())
    );
}

#[test]
fn endpoints_are_logged_without_credentials() {
    assert_eq!(
        endpoint_for_log(Some(
            "https://user:secret@otlp.example.com:4318/otlp?token=x#y".into()
        )),
        "https://***@otlp.example.com:4318/otlp?token=***#***"
    );
    assert_eq!(
        endpoint_for_log(Some("https://otlp.example.com:4318".into())),
        "https://otlp.example.com:4318"
    );
    assert_eq!(endpoint_for_log(None), "(per-signal endpoints)");
    assert_eq!(endpoint_for_log(Some("not a url".into())), "(invalid URL)");
}

// ---------------------------------------------------------------------------
// Unconfigured: the JSON log output is unchanged
// ---------------------------------------------------------------------------

/// A `MakeWriter` that keeps everything written to it.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Buffer;
    fn make_writer(&'a self) -> Buffer {
        self.clone()
    }
}

impl Buffer {
    /// The JSON lines written, without their timestamps.
    fn json_lines(&self) -> Vec<serde_json::Value> {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| {
                let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                value.as_object_mut().unwrap().remove("timestamp");
                value
            })
            .collect()
    }
}

fn emit_sample() {
    tracing::info!(target: "deckwatch::sample", n = 1, "outside any span");
    let span = tracing::info_span!(target: "deckwatch::sample", "http", method = "GET", path = "/api/healthz");
    let _entered = span.enter();
    tracing::info!(target: "extism::pdk", plugin = "instance-1", "from a plugin");
    tracing::warn!(target: "deckwatch::sample", error = "boom", "inside the span");
    tracing::debug!(target: "deckwatch::sample", "dropped by the filter");
}

#[test]
fn unconfigured_log_output_is_unchanged() {
    // What run() installed before this module existed.
    let before = Buffer::default();
    tracing::subscriber::with_default(
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(DEFAULT_LOG_FILTER))
            .json()
            .with_writer(before.clone())
            .finish(),
        emit_sample,
    );
    // With the OpenTelemetry layers in place, all off.
    let after = Buffer::default();
    tracing::subscriber::with_default(
        subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), after.clone()).0,
        emit_sample,
    );

    let before = before.json_lines();
    assert_eq!(before.len(), 3, "{before:?}");
    assert_eq!(after.json_lines(), before);
}

// ---------------------------------------------------------------------------
// Configured: the layers, with in-memory exporters
// ---------------------------------------------------------------------------

/// In-memory exporters for `signals`, to attach to a [`Telemetry`].
struct InMemory {
    signals: Signals,
    spans: InMemorySpanExporter,
    logs: InMemoryLogExporter,
    metrics: InMemoryMetricExporter,
    meter_provider: Option<SdkMeterProvider>,
    /// The pipelines they are attached to, kept for as long as the test runs
    /// (`run` keeps its own for the life of the process). Dropping one shuts
    /// its providers down, and the in-memory exporters forget their records
    /// on shutdown.
    pipelines: Mutex<Vec<Telemetry>>,
}

fn in_memory(signals: Signals) -> InMemory {
    let metrics = InMemoryMetricExporter::default();
    InMemory {
        signals,
        spans: InMemorySpanExporter::default(),
        logs: InMemoryLogExporter::default(),
        meter_provider: signals.metrics.then(|| {
            SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(metrics.clone()).build())
                .with_resource(resource())
                .with_view(mcp_duration_view)
                .build()
        }),
        metrics,
        pipelines: Mutex::default(),
    }
}

impl InMemory {
    /// The exporters for [`Telemetry::attach`], behind simple (synchronous)
    /// processors instead of the batch ones.
    fn exporters(&self) -> Exporters {
        Exporters {
            traces: self.signals.traces.then(|| {
                Box::new(SimpleSpanProcessor::new(self.spans.clone())) as Box<dyn SpanProcessor>
            }),
            metrics: self.meter_provider.clone(),
            logs: self.signals.logs.then(|| {
                Box::new(SimpleLogProcessor::new(self.logs.clone())) as Box<dyn LogProcessor>
            }),
        }
    }

    /// The subscriber `run` installs, with the default log filter and these
    /// exporters attached.
    fn subscriber(&self) -> impl Subscriber + Send + Sync {
        self.subscriber_with(EnvFilter::new(DEFAULT_LOG_FILTER))
    }

    fn subscriber_with(&self, filter: EnvFilter) -> impl Subscriber + Send + Sync {
        let (subscriber, telemetry) = subscriber(filter, std::io::sink);
        assert_eq!(telemetry.attach(self.exporters()), self.signals);
        self.pipelines.lock().unwrap().push(telemetry);
        subscriber
    }

    fn meter_provider(&self) -> &SdkMeterProvider {
        self.meter_provider.as_ref().expect("metrics are attached")
    }

    fn spans(&self) -> Vec<opentelemetry_sdk::trace::SpanData> {
        self.spans.get_finished_spans().unwrap()
    }

    fn log_records(&self) -> Vec<opentelemetry_sdk::logs::in_memory_exporter::LogDataWithResource> {
        self.logs.get_emitted_logs().unwrap()
    }
}

fn attribute<'a>(
    span: &'a opentelemetry_sdk::trace::SpanData,
    key: &str,
) -> Option<&'a opentelemetry::Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// An AppState whose kube client points at nothing (127.0.0.1:1).
async fn test_state() -> crate::state::AppState {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let kubeconfig: kube::config::Kubeconfig = serde_yaml::from_str(
        r#"
apiVersion: v1
kind: Config
current-context: dummy
clusters:
  - name: dummy
    cluster: { server: "https://127.0.0.1:1", insecure-skip-tls-verify: true }
contexts:
  - name: dummy
    context: { cluster: dummy, user: dummy }
users:
  - name: dummy
    user: {}
"#,
    )
    .unwrap();
    let config = kube::Config::from_custom_kubeconfig(
        kubeconfig,
        &kube::config::KubeConfigOptions::default(),
    )
    .await
    .unwrap();
    crate::state::AppState {
        kube_client: kube::Client::try_from(config).unwrap(),
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-config".to_string(),
        entitlements: Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db: crate::db::connect("sqlite::memory:").await.unwrap(),
        encryption_key: String::new(),
        plugins: Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: Arc::new(tokio::sync::Mutex::new(Default::default())),
        plugin_telemetry: Default::default(),
    }
}

#[tokio::test]
async fn a_request_produces_a_span_and_its_log_lines_carry_the_trace_id() {
    let otel = in_memory(Signals {
        traces: true,
        logs: true,
        ..NONE
    });
    let _guard = tracing::subscriber::set_default(otel.subscriber());

    let app = crate::routes::build_router(
        test_state().await,
        None,
        "/nonexistent",
        "/nonexistent",
        crate::auth::AuthConfig::from_settings(None),
    );
    let response = app
        .oneshot(
            axum::http::Request::get("/api/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The request span ends with the response body.
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    let spans = otel.spans();
    let span = spans.iter().find(|s| s.name == "http").expect("http span");
    assert_eq!(attribute(span, "method"), Some(&"GET".into()));
    assert_eq!(attribute(span, "path"), Some(&"/api/healthz".into()));

    // tower_http's response line is a log record of that span.
    let records = otel.log_records();
    let response_line = records
        .iter()
        .find(|r| r.record.target().map(|t| t.as_ref()) == Some("tower_http::trace::on_response"))
        .expect("response log record");
    let cx = response_line.record.trace_context().expect("trace context");
    assert_eq!(cx.trace_id, span.span_context.trace_id());
    assert_eq!(cx.span_id, span.span_context.span_id());
}

#[test]
fn plugin_log_lines_carry_the_trace_id_of_their_span() {
    let otel = in_memory(Signals {
        traces: true,
        logs: true,
        ..NONE
    });
    tracing::subscriber::with_default(otel.subscriber(), || {
        let span = tracing::info_span!("plugin apply", deckwatch.plugin.name = "otel");
        let _entered = span.enter();
        // What extism 1.41 emits for a plugin's PDK log_info call (src/pdk.rs).
        tracing::info!(target: "extism::pdk", plugin = "3f0c-instance", "provisioned bucket");
    });

    let spans = otel.spans();
    assert_eq!(spans.len(), 1);
    let span = &spans[0];
    // With logs exported, the event is a log record and not also a span event.
    assert!(span.events.is_empty(), "{:?}", span.events);

    let records = otel.log_records();
    assert_eq!(records.len(), 1);
    let record = &records[0].record;
    assert_eq!(record.target().map(|t| t.as_ref()), Some("extism::pdk"));
    assert_eq!(record.body(), Some(&AnyValue::from("provisioned bucket")));
    assert!(record
        .attributes_iter()
        .any(|(k, v)| k.as_str() == "plugin" && *v == AnyValue::from("3f0c-instance")));
    let cx = record.trace_context().expect("trace context");
    assert_eq!(cx.trace_id, span.span_context.trace_id());
    assert_eq!(cx.span_id, span.span_context.span_id());
    assert_eq!(
        records[0].resource.get(&Key::new("service.version")),
        Some(env!("CARGO_PKG_VERSION").into())
    );
}

#[test]
fn without_log_export_events_are_span_events() {
    // A traces-only backend, such as the chart's bundled Tempo or Jaeger.
    let otel = in_memory(Signals {
        traces: true,
        ..NONE
    });
    tracing::subscriber::with_default(otel.subscriber(), || {
        let span = tracing::info_span!("plugin apply");
        let _entered = span.enter();
        tracing::info!(target: "extism::pdk", plugin = "3f0c-instance", "provisioned bucket");
    });

    let spans = otel.spans();
    assert_eq!(spans.len(), 1);
    let names: Vec<&str> = spans[0].events.iter().map(|e| e.name.as_ref()).collect();
    assert_eq!(names, ["provisioned bucket"]);
}

#[test]
fn error_events_mark_their_span_as_failed() {
    let otel = in_memory(Signals {
        traces: true,
        logs: true,
        ..NONE
    });
    tracing::subscriber::with_default(otel.subscriber(), || {
        let span = tracing::info_span!("reconcile");
        let _entered = span.enter();
        tracing::error!("plugin apply() failed");
    });

    let spans = otel.spans();
    assert!(
        matches!(spans[0].status, Status::Error { .. }),
        "{:?}",
        spans[0].status
    );
    assert_eq!(otel.log_records().len(), 1);
}

#[test]
fn the_export_path_is_never_exported() {
    let otel = in_memory(Signals {
        traces: true,
        logs: true,
        ..NONE
    });
    // As if RUST_LOG=trace.
    tracing::subscriber::with_default(otel.subscriber_with(EnvFilter::new("trace")), || {
        let span = tracing::info_span!("request");
        let _entered = span.enter();
        tracing::debug!(target: "hyper_util::client::legacy::pool", "connecting");
        tracing::trace!(target: "reqwest::connect", "starting new connection");
        tracing::debug!(target: "h2::codec", "send frame");
        tracing::debug!(target: "rustls::client", "resumed session");
        tracing::error!(name: "BatchSpanProcessor.ExportError", target: "opentelemetry_sdk", "failed");
        tracing::info!("kept");
    });

    let bodies: Vec<AnyValue> = otel
        .log_records()
        .iter()
        .filter_map(|r| r.record.body().cloned())
        .collect();
    assert_eq!(bodies, [AnyValue::from("kept")]);
    assert!(otel.spans()[0].events.is_empty());
}

#[test]
fn metric_events_become_metrics() {
    let otel = in_memory(Signals {
        metrics: true,
        ..NONE
    });
    tracing::subscriber::with_default(otel.subscriber(), || {
        tracing::info!(
            monotonic_counter.deckwatch_telemetry_test_events = 2_u64,
            "counted"
        );
    });
    otel.meter_provider().force_flush().unwrap();

    let exported = otel.metrics.get_finished_metrics().unwrap();
    let names: Vec<&str> = exported
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .map(|m| m.name())
        .collect();
    assert!(
        names.contains(&"deckwatch_telemetry_test_events"),
        "{names:?}"
    );
}

// ---------------------------------------------------------------------------
// Two phases: the layers start off and switch on when attached
// ---------------------------------------------------------------------------

#[test]
fn layers_switch_on_when_their_exporter_is_attached() {
    let otel = in_memory(Signals {
        traces: true,
        logs: true,
        ..NONE
    });
    let (subscriber, telemetry) = subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), std::io::sink);
    tracing::subscriber::with_default(subscriber, || {
        // Before: nothing reaches OpenTelemetry, and spans have no trace id.
        let before = tracing::info_span!("before attach");
        before.in_scope(|| tracing::info!("logged before attach"));
        assert_eq!(trace_id(&before), None);

        assert_eq!(
            telemetry.attach(otel.exporters()),
            Signals {
                traces: true,
                logs: true,
                ..NONE
            }
        );
        assert_eq!(telemetry.signals(), otel.signals);

        let after = tracing::info_span!("after attach");
        after.in_scope(|| tracing::info!("logged after attach"));
        assert!(trace_id(&after).is_some());
        // A span that started before is still not exported.
        before.in_scope(|| tracing::info!("logged in a span from before"));
    });

    let spans = otel.spans();
    let names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();
    assert_eq!(names, ["after attach"]);
    let records = otel.log_records();
    let bodies: Vec<_> = records.iter().filter_map(|r| r.record.body()).collect();
    assert_eq!(
        bodies,
        [
            &AnyValue::from("logged after attach"),
            &AnyValue::from("logged in a span from before")
        ]
    );
    let cx = records[0].record.trace_context().expect("trace context");
    assert_eq!(cx.trace_id, spans[0].span_context.trace_id());
    assert!(records[1].record.trace_context().is_none());
}

#[test]
fn a_signal_attached_later_changes_what_the_others_take() {
    // Traces first, as when they come from the environment and logs from
    // settings: events are span events until logs are attached.
    let traces = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let logs = in_memory(Signals { logs: true, ..NONE });
    let (subscriber, telemetry) = subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), std::io::sink);
    tracing::subscriber::with_default(subscriber, || {
        telemetry.attach(traces.exporters());
        tracing::info_span!("first").in_scope(|| tracing::info!("a span event"));
        telemetry.attach(logs.exporters());
        tracing::info_span!("second").in_scope(|| tracing::info!("a log record"));
    });

    let spans = traces.spans();
    let events = |name: &str| -> Vec<String> {
        spans
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .events
            .iter()
            .map(|e| e.name.to_string())
            .collect()
    };
    assert_eq!(events("first"), ["a span event"]);
    assert!(events("second").is_empty());
    let records = logs.log_records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record.body(),
        Some(&AnyValue::from("a log record"))
    );
}

#[test]
fn metric_events_count_once_metrics_are_attached() {
    let otel = in_memory(Signals {
        metrics: true,
        ..NONE
    });
    let (subscriber, telemetry) = subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), std::io::sink);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            monotonic_counter.deckwatch_telemetry_late_events = 5_u64,
            "before"
        );
        telemetry.attach(otel.exporters());
        tracing::info!(
            monotonic_counter.deckwatch_telemetry_late_events = 2_u64,
            "after"
        );
    });
    otel.meter_provider().force_flush().unwrap();

    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    let exported = otel.metrics.get_finished_metrics().unwrap();
    let counter = exported
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .find(|m| m.name() == "deckwatch_telemetry_late_events")
        .expect("the counter");
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = counter.data() else {
        panic!("not a u64 sum: {:?}", counter.data());
    };
    let values: Vec<u64> = sum.data_points().map(|p| p.value()).collect();
    assert_eq!(values, [2]);
}

#[test]
fn a_signal_is_attached_once() {
    let first = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let second = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let (subscriber, telemetry) = subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), std::io::sink);
    assert!(telemetry.attach(first.exporters()).traces);
    assert_eq!(telemetry.attach(second.exporters()), NONE);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("exported once").in_scope(|| {});
    });
    assert_eq!(first.spans().len(), 1);
    assert!(second.spans().is_empty());
}

// ---------------------------------------------------------------------------
// MCP requests (mcp-k8s telemetry through deckwatch's pipeline)
// ---------------------------------------------------------------------------

/// The caller's span in the W3C examples.
const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const OTHER_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// POST a `tools/call` of `list_templates` to the real `/mcp` handler.
async fn mcp_tool_call(params_meta: Option<&str>, header: Option<&str>) {
    let mut params = serde_json::json!({ "name": "list_templates", "arguments": {} });
    if let Some(traceparent) = params_meta {
        params["_meta"] = serde_json::json!({ "traceparent": traceparent });
    }
    let request = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params,
    }))
    .unwrap();
    let mut headers = HeaderMap::new();
    if let Some(traceparent) = header {
        headers.insert("traceparent", traceparent.parse().unwrap());
    }
    let response = crate::handlers::mcp::handle_mcp(
        axum::extract::State(test_state().await),
        headers,
        axum::Json(request),
    )
    .await;
    let response = axum::response::IntoResponse::into_response(response);
    assert_eq!(response.status(), StatusCode::OK);
}

fn mcp_span(otel: &InMemory) -> opentelemetry_sdk::trace::SpanData {
    let spans = otel.spans();
    let mcp: Vec<_> = spans
        .into_iter()
        .filter(|s| s.name == "tools/call list_templates")
        .collect();
    assert_eq!(mcp.len(), 1, "one MCP server span");
    mcp.into_iter().next().unwrap()
}

#[tokio::test]
async fn an_mcp_request_joins_the_trace_in_its_meta() {
    let otel = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let _guard = tracing::subscriber::set_default(otel.subscriber());
    mcp_tool_call(Some(TRACEPARENT), None).await;

    let span = mcp_span(&otel);
    assert_eq!(span.span_kind, opentelemetry::trace::SpanKind::Server);
    assert_eq!(
        span.span_context.trace_id().to_string(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");
    assert!(span.parent_span_is_remote);
    assert_eq!(
        attribute(&span, "trace_id"),
        Some(&"4bf92f3577b34da6a3ce929d0e0e4736".into())
    );
    assert_eq!(
        attribute(&span, "gen_ai.tool.name"),
        Some(&"list_templates".into())
    );
    assert!(span.links.is_empty());
}

#[tokio::test]
async fn the_traceparent_header_is_the_fallback_and_otherwise_a_link() {
    let otel = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let _guard = tracing::subscriber::set_default(otel.subscriber());

    mcp_tool_call(None, Some(TRACEPARENT)).await;
    let span = mcp_span(&otel);
    assert_eq!(
        span.span_context.trace_id().to_string(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    otel.spans.reset();

    // Both: _meta is the parent (the MCP conventions), the header a link.
    mcp_tool_call(Some(OTHER_TRACEPARENT), Some(TRACEPARENT)).await;
    let span = mcp_span(&otel);
    assert_eq!(
        span.span_context.trace_id().to_string(),
        "0af7651916cd43dd8448eb211c80319c"
    );
    let links: Vec<String> = span
        .links
        .iter()
        .map(|l| l.span_context.trace_id().to_string())
        .collect();
    assert_eq!(links, ["4bf92f3577b34da6a3ce929d0e0e4736"]);
    otel.spans.reset();

    // A malformed traceparent starts a new trace.
    mcp_tool_call(Some("00-not-a-trace-01"), None).await;
    let span = mcp_span(&otel);
    assert!(span.span_context.is_valid());
    assert!(!span.parent_span_is_remote);
}

#[tokio::test]
async fn mcp_metric_events_become_the_duration_histogram_and_nothing_else() {
    let otel = in_memory(ALL);
    let _guard = tracing::subscriber::set_default(otel.subscriber());
    mcp_tool_call(None, None).await;
    otel.meter_provider().force_flush().unwrap();

    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    let exported = otel.metrics.get_finished_metrics().unwrap();
    let duration = exported
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .find(|m| m.name() == mcp_semconv::MCP_SERVER_OPERATION_DURATION)
        .expect("duration histogram");
    assert_eq!(duration.unit(), "s");
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = duration.data() else {
        panic!("not an f64 histogram: {:?}", duration.data());
    };
    let point = histogram.data_points().next().expect("one point");
    assert_eq!(point.count(), 1);
    assert_eq!(
        point.bounds().collect::<Vec<_>>(),
        mcp_semconv::MCP_SERVER_OPERATION_DURATION_BUCKETS
    );
    for kv in point.attributes() {
        assert!(
            mcp_semconv::METRIC_ATTRIBUTES.contains(&kv.key.as_str()),
            "attribute {} on the metric",
            kv.key
        );
    }

    // The metric event is neither a log record nor a span event.
    for record in otel.log_records() {
        assert_ne!(
            record.record.target().map(|t| t.as_ref()),
            Some(mcp_semconv::METRICS_TARGET)
        );
    }
    let span = mcp_span(&otel);
    assert!(
        span.events.is_empty(),
        "span events with log export on: {:?}",
        span.events
    );
}

#[tokio::test]
async fn without_log_export_mcp_metric_events_are_not_span_events() {
    let otel = in_memory(Signals {
        traces: true,
        ..NONE
    });
    let _guard = tracing::subscriber::set_default(otel.subscriber());
    mcp_tool_call(None, None).await;

    let span = mcp_span(&otel);
    let names: Vec<&str> = span.events.iter().map(|e| e.name.as_ref()).collect();
    // The log line arrives as a span event; the metric event doesn't.
    assert_eq!(names, ["tool call finished"]);
}

// ---------------------------------------------------------------------------
// The exporter's HTTP client
// ---------------------------------------------------------------------------

type Received = Arc<Mutex<Vec<(HeaderMap, Bytes)>>>;

/// A collector stand-in on 127.0.0.1 that records `POST /v1/traces` and
/// answers `body`.
async fn collector(body: Vec<u8>) -> (std::net::SocketAddr, Received) {
    let received = Received::default();
    let app = axum::Router::new().route(
        "/v1/traces",
        axum::routing::post({
            let received = received.clone();
            move |headers: HeaderMap, request: Bytes| async move {
                received.lock().unwrap().push((headers, request));
                body
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, received)
}

/// `request` through [`ObservabilityClient`] from a plain OS thread with no
/// Tokio runtime, like the SDK's batch processors.
async fn send_from_plain_thread(request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
    send_from_plain_thread_with(
        ObservabilityClient::new(tokio::runtime::Handle::current()),
        request,
    )
    .await
}

async fn send_from_plain_thread_with(
    client: ObservabilityClient,
    request: Request<Bytes>,
) -> Result<Response<Bytes>, HttpError> {
    tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || futures::executor::block_on(client.send_bytes(request)))
            .join()
            .unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_exporter_sends_through_the_shared_outbound_client() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (addr, received) = collector(b"accepted".to_vec()).await;

    let request = Request::post(format!("http://{addr}/v1/traces"))
        .header("content-type", "application/x-protobuf")
        .header("authorization", "Bearer otlp-token")
        .body(Bytes::from_static(b"spans"))
        .unwrap();
    let response = send_from_plain_thread(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body().as_ref(), b"accepted");
    let received = received.lock().unwrap();
    let (headers, body) = &received[0];
    assert_eq!(body.as_ref(), b"spans");
    assert_eq!(headers["content-type"], "application/x-protobuf");
    assert_eq!(headers["authorization"], "Bearer otlp-token");
    // A request without its own User-Agent gets the outbound factory's, so it
    // went through the shared client. (The OTLP exporter sets its own.)
    assert_eq!(headers["user-agent"], outbound::user_agent().as_str());
}

#[test]
fn each_failure_is_reported_once() {
    let failures = FailureLog::default();
    let refused = || Some("connection refused".to_string());
    let outcomes = [
        refused(),
        refused(),
        Some("HTTP 401 Unauthorized".to_string()),
        None,
        None,
        refused(),
    ];
    let reports: Vec<Option<Report>> = outcomes.into_iter().map(|o| failures.record(o)).collect();
    assert_eq!(
        reports,
        [
            Some(Report::Failed("connection refused".to_string())),
            None,
            Some(Report::Failed("HTTP 401 Unauthorized".to_string())),
            Some(Report::Recovered),
            None,
            Some(Report::Failed("connection refused".to_string())),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_exporter_refuses_oversized_responses() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (addr, _) = collector(vec![b'x'; MAX_RESPONSE_BYTES + 1]).await;

    let request = Request::post(format!("http://{addr}/v1/traces"))
        .body(Bytes::from_static(b"spans"))
        .unwrap();
    let err = send_from_plain_thread(request).await.unwrap_err();
    assert!(
        err.downcast_ref::<ResponseBodyTooLarge>().is_some(),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_endpoint_from_settings_gets_no_exporter_headers() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (addr, received) = collector(b"accepted".to_vec()).await;

    // What the SDK sends when OTEL_EXPORTER_OTLP_HEADERS is set.
    let request = Request::post(format!("http://{addr}/v1/traces"))
        .header("content-type", "application/x-protobuf")
        .header("content-encoding", "gzip")
        .header("user-agent", "OTel-OTLP-Exporter-Rust/0.33.1")
        .header("authorization", "Bearer otlp-token")
        .header("x-api-key", "otlp-key")
        .body(Bytes::from_static(b"spans"))
        .unwrap();
    let client =
        ObservabilityClient::new(tokio::runtime::Handle::current()).without_exporter_headers();
    let response = send_from_plain_thread_with(client, request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let received = received.lock().unwrap();
    let (headers, body) = &received[0];
    assert_eq!(body.as_ref(), b"spans");
    assert_eq!(headers["content-type"], "application/x-protobuf");
    assert_eq!(headers["content-encoding"], "gzip");
    assert!(headers.get("authorization").is_none(), "{headers:?}");
    assert!(headers.get("x-api-key").is_none(), "{headers:?}");
    assert_eq!(headers["user-agent"], outbound::user_agent().as_str());
}

type Posted = Arc<Mutex<Vec<(String, HeaderMap)>>>;

/// An OTLP receiver stand-in on 127.0.0.1 that records the path and headers
/// of every POST and accepts it.
async fn otlp_receiver() -> (std::net::SocketAddr, Posted) {
    let posted = Posted::default();
    let app = axum::Router::new().fallback({
        let posted = posted.clone();
        move |uri: axum::http::Uri, headers: HeaderMap| async move {
            posted
                .lock()
                .unwrap()
                .push((uri.path().to_string(), headers));
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, posted)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_second_phase_exports_to_the_endpoints_in_settings() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (addr, posted) = otlp_receiver().await;
    let (subscriber, telemetry) = subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), std::io::sink);
    let _guard = tracing::subscriber::set_default(subscriber);

    // Startup: no variable sets an endpoint, so nothing is exported yet.
    assert!(!telemetry.start_with(env(&[])).signals().any());
    assert_eq!(telemetry.signals(), NONE);

    // Once settings are read: the collector endpoint carries traces and logs.
    let settings = ObservabilitySettings {
        otlp_endpoint: format!("http://{addr}/otlp/"),
        otlp_metrics_endpoint: String::new(),
    };
    telemetry.apply_settings_with(env(&[]), &settings);
    assert_eq!(
        telemetry.signals(),
        Signals {
            traces: true,
            logs: true,
            ..NONE
        }
    );

    tracing::info_span!("settings phase").in_scope(|| tracing::info!("sent to the collector"));
    let flushed = telemetry.clone();
    tokio::task::spawn_blocking(move || {
        flushed.inner.tracer_provider.force_flush().unwrap();
        flushed.inner.logger_provider.force_flush().unwrap();
    })
    .await
    .unwrap();

    let posted = posted.lock().unwrap();
    let mut paths: Vec<&str> = posted.iter().map(|(path, _)| path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(paths, ["/otlp/v1/logs", "/otlp/v1/traces"]);
    for (_, headers) in posted.iter() {
        assert_eq!(headers["content-type"], "application/x-protobuf");
    }

    let running = telemetry.running();
    assert_eq!(
        running[0],
        SignalStatus {
            source: Source::Settings,
            endpoint: format!("http://{addr}/otlp/v1/traces"),
            from: Some("observability.otlp_endpoint".to_string()),
            exported: true,
            reason: None,
        }
    );
    assert_eq!(running[1], SignalStatus::unset());
    assert_eq!(running[2].endpoint, format!("http://{addr}/otlp/v1/logs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_environment_keeps_its_signals_in_the_second_phase() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (addr, _) = otlp_receiver().await;
    let traces_endpoint = format!("http://{addr}/env/v1/traces");
    let vars = [(
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        traces_endpoint.as_str(),
    )];
    let telemetry = Telemetry::new();

    let plan = telemetry.start_with(env(&vars));
    assert_eq!(
        telemetry.signals(),
        Signals {
            traces: true,
            ..NONE
        }
    );
    assert_eq!(plan.signal(Signal::Traces).source, Source::Env);

    let settings = ObservabilitySettings {
        otlp_endpoint: format!("http://{addr}/collector"),
        otlp_metrics_endpoint: format!("http://{addr}/api/v1/otlp/v1/metrics"),
    };
    telemetry.apply_settings_with(env(&vars), &settings);
    assert_eq!(telemetry.signals(), ALL);
    let running = telemetry.running();
    assert_eq!(running[0].source, Source::Env);
    assert_eq!(running[0].endpoint, traces_endpoint);
    assert_eq!(
        running[0].from.as_deref(),
        Some("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
    );
    assert_eq!(running[1].source, Source::Settings);
    assert_eq!(
        running[1].endpoint,
        format!("http://{addr}/api/v1/otlp/v1/metrics")
    );
    assert_eq!(running[2].source, Source::Settings);
    assert_eq!(
        running[2].endpoint,
        format!("http://{addr}/collector/v1/logs")
    );
}

#[test]
fn signals_the_environment_turns_off_are_reported_as_running_nowhere() {
    let telemetry = Telemetry::new();
    telemetry.start_with(env(&[("OTEL_SDK_DISABLED", "true")]));
    let settings = ObservabilitySettings {
        otlp_endpoint: "http://otel-collector.observability:4318".to_string(),
        otlp_metrics_endpoint: String::new(),
    };
    // Nothing to attach, so no runtime is needed.
    telemetry.apply_settings_with(env(&[("OTEL_SDK_DISABLED", "true")]), &settings);
    assert_eq!(telemetry.signals(), NONE);
    let traces = &telemetry.running()[0];
    assert_eq!(traces.source, Source::Settings);
    assert!(!traces.exported);
    assert_eq!(traces.reason.as_deref(), Some("OTEL_SDK_DISABLED=true"));
}
