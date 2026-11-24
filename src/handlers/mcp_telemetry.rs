//! Telemetry for the MCP endpoint (`POST /mcp`), built on
//! `mcp_k8s::telemetry` so deckwatch's own tools report exactly like the
//! mcp-k8s tools it embeds.
//!
//! Per request, [`super::mcp::handle_mcp`] opens one SERVER span with
//! [`request_span`]. Inside it:
//!
//! - deckwatch's own tools ([`DECKWATCH_TOOLS`]) run in an
//!   `mcp_k8s::telemetry::ToolCall`, with the action class from the table;
//! - every other tool goes to `mcp_k8s::mcp::handle_tool`, which instruments
//!   itself into the same span;
//! - other methods, and tool calls that never reach a tool, finish with
//!   `mcp_k8s::telemetry::finish_operation`.
//!
//! So every request yields one span, one `mcp.server.operation.duration`
//! metric event and one log line (`tool call finished` or `request
//! finished`, target `mcp_k8s::telemetry`). An interceptor that hands its
//! call to `handle_tool` (`create_deployment` and friends) is still one
//! call: the library reports into the running `ToolCall`.
//!
//! The client's self-description from `initialize` is kept per session
//! ([`McpSessions`]) so later requests can be labelled with it. It is
//! self-reported: a label, never an identity.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use mcp_k8s::telemetry::{self, semconv, ActionClass, ClientInfo};
use serde_json::Value;
use tracing::Span;

// ---------------------------------------------------------------------------
// Tool classification
// ---------------------------------------------------------------------------

/// A tool deckwatch serves itself rather than leaving to mcp-k8s.
#[derive(Debug)]
pub struct DeckwatchTool {
    pub name: &'static str,
    /// What the tool does, as recorded in `mcp_k8s.action`. mcp-k8s
    /// classifies tools by name prefix (`list_`, `create_`, `delete_`…) and
    /// calls everything else a read, so names like `trigger_build` need an
    /// explicit class.
    pub action: ActionClass,
}

const fn tool(name: &'static str, action: ActionClass) -> DeckwatchTool {
    DeckwatchTool { name, action }
}

use ActionClass::{Delete, Exec, Read, Write};

/// Every tool deckwatch dispatches itself, with its action class. The tests
/// check that every dispatched or listed deckwatch tool is in here, and that
/// the classes agree with mcp-k8s for names it knows.
///
/// - `write`: creates or changes cluster or deckwatch state.
/// - `delete`: deletes or tears down (`deprovision_resource` destroys
///   plugin-managed infrastructure; `disable_monitoring` deletes the
///   PodMonitor).
/// - `exec`: runs code: `exec_pod` in a container, `validate_plugin` a
///   caller-supplied WASM plugin with caller-chosen config and hosts.
pub const DECKWATCH_TOOLS: &[DeckwatchTool] = &[
    // Applications
    tool("create_application", Write),
    tool("list_applications", Read),
    tool("get_application", Read),
    tool("restart_application", Write),
    tool("scale_application", Write),
    tool("list_templates", Read),
    // GitOps and builds
    tool("set_gitops", Write),
    tool("get_gitops", Read),
    tool("trigger_build", Write),
    tool("list_builds", Read),
    tool("get_build_log", Read),
    tool("watch_build", Read),
    tool("generate_local_build", Read),
    // Ingresses and ingress templates
    tool("create_ingress", Write),
    tool("update_ingress", Write),
    tool("list_ingress_templates", Read),
    tool("create_ingress_template", Write),
    tool("update_ingress_template", Write),
    tool("delete_ingress_template", Delete),
    // Monitoring
    tool("get_monitoring", Read),
    tool("enable_monitoring", Write),
    tool("disable_monitoring", Delete),
    // Plugins
    tool("list_plugins", Read),
    tool("enable_plugin", Write),
    tool("disable_plugin", Write),
    tool("update_plugin_config", Write),
    tool("get_plugin_config", Read),
    tool("validate_plugin", Exec),
    tool("associate_plugin", Write),
    tool("disassociate_plugin", Write),
    tool("list_application_resources", Read),
    tool("provision_resource", Write),
    tool("deprovision_resource", Delete),
    tool("refresh_resource", Write),
    // Agent feedback
    tool("submit_agent_feedback", Write),
    // mcp-k8s tools deckwatch intercepts. The deployment tools still hand
    // the call to mcp-k8s; update_deployment and create_pod add a service
    // account, apply_manifest accepts YAML, and exec_pod is deckwatch's own
    // non-interactive implementation.
    tool("create_deployment", Write),
    tool("update_deployment", Write),
    tool("delete_deployment", Delete),
    tool("scale_deployment", Write),
    tool("restart_deployment", Write),
    tool("create_pod", Write),
    tool("apply_manifest", Write),
    tool("exec_pod", Exec),
];

/// The deckwatch tool named `name`, if deckwatch serves it itself.
pub fn deckwatch_tool(name: &str) -> Option<&'static DeckwatchTool> {
    DECKWATCH_TOOLS.iter().find(|t| t.name == name)
}

/// The arguments telemetry summarizes for a call of `tool`.
///
/// deckwatch's `apply_manifest` accepts YAML, but `summarize` reads the
/// target (kind, apiVersion, name, namespace) only from a JSON manifest, so
/// a YAML manifest is parsed into an object here. Nothing else in it is read
/// or recorded.
pub fn telemetry_arguments<'a>(tool: &str, args: &'a Value) -> Cow<'a, Value> {
    /// The size `summarize` parses a JSON manifest string up to.
    const MAX_MANIFEST_LEN: usize = 4 * 1024 * 1024;
    if tool == "apply_manifest" {
        if let Some(manifest) = args["manifest"].as_str() {
            if manifest.len() <= MAX_MANIFEST_LEN {
                if let Ok(parsed @ Value::Object(_)) = serde_yaml::from_str::<Value>(manifest) {
                    let mut args = args.clone();
                    args["manifest"] = parsed;
                    return Cow::Owned(args);
                }
            }
        }
    }
    Cow::Borrowed(args)
}

// ---------------------------------------------------------------------------
// Metric labels
// ---------------------------------------------------------------------------

/// The `method` label for `deckwatch_mcp_requests_total`: the JSON-RPC
/// method if it is one the MCP conventions know, otherwise `_OTHER`. The
/// method is client input, so it can't become a label as is.
pub fn metric_method(method: &str) -> &str {
    if semconv::KNOWN_METHODS.contains(&method) {
        method
    } else {
        semconv::OTHER
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// The HTTP header carrying the MCP session id (Streamable HTTP transport).
pub const SESSION_HEADER: &str = "mcp-session-id";

/// How many sessions are remembered. The oldest is forgotten first.
const MAX_SESSIONS: usize = 1024;

/// A session unused for this long is forgotten.
const SESSION_IDLE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// MCP sessions, for labelling requests with the client that started them.
///
/// `initialize` issues a random `Mcp-Session-Id` and remembers the client's
/// self-reported [`ClientInfo`] under it; clients send the id back on every
/// later request. Sessions are attribution only: a missing, unknown or
/// expired id is never refused (that request just has no client info), so a
/// restart or a full table changes nothing for clients. The table is bounded
/// in size and idle time.
pub struct McpSessions {
    sessions: Mutex<HashMap<String, SessionEntry>>,
    capacity: usize,
    idle_ttl: Duration,
}

struct SessionEntry {
    client: ClientInfo,
    last_seen: Instant,
}

impl McpSessions {
    pub fn new(capacity: usize, idle_ttl: Duration) -> Self {
        Self {
            sessions: Mutex::default(),
            capacity,
            idle_ttl,
        }
    }

    /// Start a session for `client` and return its id.
    pub fn start(&self, client: ClientInfo) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| now.duration_since(s.last_seen) < self.idle_ttl);
        while sessions.len() >= self.capacity.max(1) {
            let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, s)| s.last_seen)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            sessions.remove(&oldest);
        }
        sessions.insert(
            id.clone(),
            SessionEntry {
                client,
                last_seen: now,
            },
        );
        id
    }

    /// The client of session `id`, if it is known and not expired.
    pub fn client(&self, id: &str) -> Option<ClientInfo> {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let session = sessions.get_mut(id)?;
        if now.duration_since(session.last_seen) >= self.idle_ttl {
            sessions.remove(id);
            return None;
        }
        session.last_seen = now;
        Some(session.client.clone())
    }
}

/// The process-wide session table.
pub fn sessions() -> &'static McpSessions {
    static SESSIONS: LazyLock<McpSessions> =
        LazyLock::new(|| McpSessions::new(MAX_SESSIONS, SESSION_IDLE_TTL));
    &SESSIONS
}

// ---------------------------------------------------------------------------
// Request context and span
// ---------------------------------------------------------------------------

/// A known session: its id (issued by deckwatch) and client.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub client: ClientInfo,
}

/// What the HTTP request says about an MCP request, beyond the JSON-RPC
/// message.
#[derive(Clone, Debug, Default)]
pub struct RequestContext {
    /// The session named by a known `Mcp-Session-Id` header.
    pub session: Option<Session>,
    /// A valid `MCP-Protocol-Version` header.
    pub protocol_version: Option<String>,
}

impl RequestContext {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let session = header(SESSION_HEADER).and_then(|id| {
            sessions().client(id).map(|client| Session {
                id: id.to_string(),
                client,
            })
        });
        Self {
            session,
            protocol_version: header("mcp-protocol-version")
                .filter(|v| telemetry::valid_protocol_version(v))
                .map(str::to_string),
        }
    }
}

/// The SERVER span for one MCP request, with what the request says about
/// itself recorded on it.
///
/// `target` is the tool (`tools/call`) or prompt (`prompts/get`) name, and
/// only if deckwatch serves it: a client-supplied name is unbounded.
pub fn request_span(
    method: &str,
    target: Option<&str>,
    id: Option<&Value>,
    ctx: &RequestContext,
) -> Span {
    let span = telemetry::server_span(method, target);
    span.record(
        semconv::MCP_K8S_REQUEST_ID,
        uuid::Uuid::new_v4().to_string().as_str(),
    );
    let id = match id {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(clip(s, 128)),
        _ => None,
    };
    if let Some(id) = id {
        span.record(semconv::JSONRPC_REQUEST_ID, id.as_str());
    }
    span.record(semconv::NETWORK_TRANSPORT, "tcp");
    span.record(semconv::NETWORK_PROTOCOL_NAME, "http");
    // initialize records the version it negotiates and the session it
    // starts itself (each attribute may be recorded only once).
    if method != "initialize" {
        if let Some(version) = &ctx.protocol_version {
            span.record(semconv::MCP_PROTOCOL_VERSION, version.as_str());
        }
        if let Some(session) = &ctx.session {
            span.record(semconv::MCP_SESSION_ID, session.id.as_str());
            session.client.record(&span);
        }
    }
    span
}

/// `s` without control characters, cut to at most `max` bytes.
fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
#[path = "mcp_telemetry_tests.rs"]
mod tests;
