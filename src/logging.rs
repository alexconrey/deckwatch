//! Log filter and console log layer for deckwatch's own log output.

use tracing::{Metadata, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

/// The log filter used when `RUST_LOG` is unset or invalid.
///
/// - `deckwatch`: the server itself.
/// - `tower_http`: one line per HTTP response, from the trace layer.
/// - `mcp_k8s`: the embedded mcp-k8s library, which implements most MCP tools.
/// - `extism::pdk`: plugin log lines. extism turns a plugin's PDK `log_*`
///   calls into host `tracing` events with this target and a `plugin` field
///   holding the extism instance id.
/// - `opentelemetry` (warnings and errors): the OpenTelemetry SDK's own
///   reports, such as failed OTLP exports and dropped spans. It logs nothing
///   unless OTLP export is configured (`src/telemetry.rs`).
pub const DEFAULT_LOG_FILTER: &str =
    "deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=info,opentelemetry=warn";

/// `RUST_LOG` when it is set and valid, otherwise [`DEFAULT_LOG_FILTER`].
pub fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER))
}

/// Targets whose events are metric observations, not log lines: mcp-k8s
/// emits its `mcp.server.operation.duration` histogram as `histogram.*`
/// events here, for an OpenTelemetry `MetricsLayer`.
pub const METRIC_EVENT_TARGETS: &[&str] = &[mcp_k8s::telemetry::semconv::METRICS_TARGET];

/// Whether `meta` is a log line rather than a metric event. Log outputs (the
/// JSON console, and OTLP logs if exported) filter on this per layer; it must
/// not be a global filter, because a metrics layer needs the metric events.
pub fn is_log_line(meta: &Metadata<'_>) -> bool {
    !METRIC_EVENT_TARGETS.contains(&meta.target())
}

/// The console log layer: one JSON line per log event to `writer` (stdout
/// in production), without metric events. Under the global [`env_filter`]
/// it writes what `tracing_subscriber::fmt().with_env_filter(..).json()`
/// wrote.
pub fn json_layer<S, W>(writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_writer(writer)
        .with_filter(filter_fn(is_log_line))
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod logging_tests;
