use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use opentelemetry::logs::AnyValue;
use opentelemetry::trace::Status;
use opentelemetry_sdk::logs::InMemoryLogExporter;
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::trace::InMemorySpanExporter;
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
        // No providers, so no exporter threads, and nothing to log.
        let init = Telemetry::from_lookup(env(vars));
        assert!(init.telemetry().is_none(), "{vars:?}");
        assert!(init.notices.is_empty(), "{vars:?}");
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
            "https://user:secret@otlp.example.com:4318/otlp?token=x".into()
        )),
        "https://otlp.example.com:4318/otlp"
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
    let after = Buffer::default();
    tracing::subscriber::with_default(
        subscriber(EnvFilter::new(DEFAULT_LOG_FILTER), None, after.clone()),
        emit_sample,
    );

    let before = before.json_lines();
    assert_eq!(before.len(), 3, "{before:?}");
    assert_eq!(after.json_lines(), before);
}

// ---------------------------------------------------------------------------
// Configured: the layers, with in-memory exporters
// ---------------------------------------------------------------------------

/// [`Telemetry`] with in-memory exporters for `signals`, behind simple
/// (synchronous) processors instead of the batch ones.
struct InMemory {
    spans: InMemorySpanExporter,
    logs: InMemoryLogExporter,
    metrics: InMemoryMetricExporter,
    telemetry: Telemetry,
}

fn in_memory(signals: Signals) -> InMemory {
    let spans = InMemorySpanExporter::default();
    let logs = InMemoryLogExporter::default();
    let metrics = InMemoryMetricExporter::default();
    let telemetry = Telemetry {
        tracer_provider: signals.traces.then(|| {
            SdkTracerProvider::builder()
                .with_simple_exporter(spans.clone())
                .with_resource(resource())
                .build()
        }),
        meter_provider: signals.metrics.then(|| {
            SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(metrics.clone()).build())
                .with_resource(resource())
                .build()
        }),
        logger_provider: signals.logs.then(|| {
            SdkLoggerProvider::builder()
                .with_simple_exporter(logs.clone())
                .with_resource(resource())
                .build()
        }),
    };
    InMemory {
        spans,
        logs,
        metrics,
        telemetry,
    }
}

impl InMemory {
    /// The subscriber `run` installs, with the default log filter.
    fn subscriber(&self) -> impl Subscriber + Send + Sync {
        subscriber(
            EnvFilter::new(DEFAULT_LOG_FILTER),
            Some(&self.telemetry),
            std::io::sink,
        )
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
    let subscriber = subscriber(
        EnvFilter::new("trace"),
        Some(&otel.telemetry),
        std::io::sink,
    );
    tracing::subscriber::with_default(subscriber, || {
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
    otel.telemetry
        .meter_provider
        .as_ref()
        .unwrap()
        .force_flush()
        .unwrap();

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
    let client = ObservabilityClient::new(tokio::runtime::Handle::current());
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
