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
