//! Log filter for deckwatch's own log output.

use tracing_subscriber::EnvFilter;

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

#[cfg(test)]
#[path = "logging_tests.rs"]
mod logging_tests;
