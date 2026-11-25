use std::sync::{Arc, Mutex};

use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

use super::*;

/// Records the target of every event that reaches it.
#[derive(Clone, Default)]
struct Targets(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> Layer<S> for Targets {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target().to_string();
        self.0.lock().unwrap().push(target);
    }
}

/// The targets of the events `emit` sends that the default filter keeps.
fn kept_by_default_filter(emit: impl FnOnce()) -> Vec<String> {
    let targets = Targets::default();
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new(DEFAULT_LOG_FILTER))
        .with(targets.clone());
    tracing::subscriber::with_default(subscriber, emit);
    let kept = targets.0.lock().unwrap().clone();
    kept
}

#[test]
fn default_filter_parses() {
    EnvFilter::try_new(DEFAULT_LOG_FILTER).expect("DEFAULT_LOG_FILTER should parse");
}

#[test]
fn default_filter_keeps_plugin_and_mcp_k8s_logs() {
    let kept = kept_by_default_filter(|| {
        // What extism 1.41 emits for a plugin's PDK log_info / log_warn calls.
        tracing::info!(target: "extism::pdk", plugin = "instance-id", "from a plugin");
        tracing::warn!(target: "extism::pdk", plugin = "instance-id", "from a plugin");
        // An mcp-k8s library module (the Secret decode notice lives here).
        tracing::info!(target: "mcp_k8s::resources::secret", "secret values decoded");
        tracing::info!(target: "deckwatch::handlers::mcp", "mcp tool call");
        tracing::info!(target: "tower_http::trace::on_response", "finished processing request");
        // The OpenTelemetry SDK's export errors (its crates' names are the targets).
        tracing::error!(target: "opentelemetry_sdk", "BatchSpanProcessor.ExportError");
        tracing::warn!(target: "opentelemetry-otlp", "export failed");
    });
    assert_eq!(
        kept,
        [
            "extism::pdk",
            "extism::pdk",
            "mcp_k8s::resources::secret",
            "deckwatch::handlers::mcp",
            "tower_http::trace::on_response",
            "opentelemetry_sdk",
            "opentelemetry-otlp",
        ]
    );
}

#[test]
fn default_filter_drops_debug_and_other_targets() {
    let kept = kept_by_default_filter(|| {
        tracing::debug!(target: "extism::pdk", plugin = "instance-id", "plugin debug");
        tracing::debug!(target: "mcp_k8s::mcp::handlers", "library debug");
        tracing::debug!(target: "deckwatch::plugins", "deckwatch debug");
        tracing::info!(target: "extism::plugin", "extism internals");
        tracing::info!(target: "kube_client::client", "dependency");
        tracing::info!(target: "hyper_util::client::legacy", "dependency");
        tracing::info!(target: "opentelemetry_sdk", "BatchSpanProcessor.ThreadStarted");
    });
    assert!(kept.is_empty(), "unexpected events kept: {kept:?}");
}

/// A `MakeWriter` collecting everything written to it.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
    type Writer = Buffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn metric_events_stay_out_of_the_console_but_reach_other_layers() {
    let console = Buffer::default();
    // Stands in for an OpenTelemetry MetricsLayer next to the console layer.
    let metrics_layer = Targets::default();
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new(DEFAULT_LOG_FILTER))
        .with(json_layer(console.clone()))
        .with(metrics_layer.clone());
    tracing::subscriber::with_default(subscriber, || {
        // What mcp-k8s emits for one MCP request.
        tracing::event!(
            target: "mcp_k8s::telemetry::metrics",
            tracing::Level::INFO,
            {
                { mcp_k8s::telemetry::semconv::HISTOGRAM_MCP_SERVER_OPERATION_DURATION } = 0.25,
                { mcp_k8s::telemetry::semconv::MCP_METHOD_NAME } = "tools/call",
            }
        );
        tracing::info!(target: "mcp_k8s::telemetry", "tool call finished");
    });

    let console = String::from_utf8(console.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = console.lines().collect();
    assert_eq!(lines.len(), 1, "{console}");
    assert!(lines[0].contains("tool call finished"), "{console}");
    assert!(!console.contains("histogram."), "{console}");
    assert_eq!(
        *metrics_layer.0.lock().unwrap(),
        ["mcp_k8s::telemetry::metrics", "mcp_k8s::telemetry"]
    );
}

/// mcp-k8s's `audit` events duplicate deckwatch's own MCP audit rows, so no
/// log output writes them, even when `RUST_LOG` asks for them.
#[test]
fn mcp_k8s_audit_events_are_never_written() {
    let console = Buffer::default();
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info,mcp_k8s::audit=trace"))
        .with(json_layer(console.clone()));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "mcp_k8s::audit", tool = "delete_pod", "audit");
        tracing::info!(target: "mcp_k8s::telemetry", "tool call finished");
    });

    let console = String::from_utf8(console.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = console.lines().collect();
    assert_eq!(lines.len(), 1, "{console}");
    assert!(lines[0].contains("tool call finished"), "{console}");
    assert_eq!(
        SUPPRESSED_TARGETS,
        [mcp_k8s::telemetry::semconv::AUDIT_TARGET]
    );
    assert_eq!(mcp_k8s::telemetry::semconv::AUDIT_TARGET, "mcp_k8s::audit");
}
