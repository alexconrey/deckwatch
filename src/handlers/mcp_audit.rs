//! SEC-22: an audit row for every MCP tool call that writes, deletes or
//! execs, or that a guardrail denied, written once at the MCP dispatch choke
//! point (`handlers::mcp`). Reads that went through aren't audited.
//!
//! A row holds only what telemetry may record: the action class, the target
//! (kind, namespace, name), the argument **keys** and the values mcp-k8s's
//! allowlist permits (`mcp_k8s::telemetry::ALLOWED_ARGUMENTS`), plus the
//! client's self-reported name and version. Never Secret data, env values,
//! commands, manifests or plugin config.
//!
//! Writes are fire-and-forget: the row is inserted in a background task, so
//! a slow or failing database never delays or fails a tool call.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use mcp_k8s::telemetry::{ActionClass, ClientInfo, ToolCallSummary};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{Map, Value};

use super::mcp_telemetry::{clip, DeckwatchTool, RequestContext};
use crate::audit::{self, McpCallRow};

/// The longest value copied into a row (as `summarize` cuts allowlisted
/// values). Kubernetes object names are at most 253 bytes.
const MAX_VALUE_LEN: usize = 256;
/// At most this many argument keys are recorded per call.
const MAX_KEYS: usize = 32;

/// How an audited call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    /// A guardrail refused the call; it didn't run.
    Denied,
    /// The request was dropped before the call finished (the client went
    /// away). The change may or may not have been made.
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Whether a call is audited: everything except reads that went through.
pub fn audited(action: ActionClass, denied: bool) -> bool {
    action != ActionClass::Read || denied
}

/// What an audit row says about one call, built from its redacted summary.
#[derive(Clone, Debug)]
pub struct CallRecord {
    tool: String,
    action: ActionClass,
    resource_type: String,
    resource_name: String,
    namespace: String,
    api_version: Option<String>,
    argument_keys: Vec<String>,
    arguments: Map<String, Value>,
    denied_rule: Option<String>,
    client: Option<ClientInfo>,
    session_id: Option<String>,
    trace_id: Option<String>,
}

impl CallRecord {
    /// The record of a call of `tool` with `args`.
    ///
    /// - `own`: the table entry if deckwatch serves the tool itself.
    /// - `summary`: the call's `mcp_k8s::telemetry::summarize` output (the
    ///   one its `ToolCall` records), the only source of argument values.
    pub fn new(
        tool: &str,
        own: Option<&DeckwatchTool>,
        action: ActionClass,
        summary: &ToolCallSummary,
        args: &Value,
        ctx: &RequestContext,
    ) -> Self {
        let target = &summary.target;
        let resource_type = target
            .kind
            .as_deref()
            .map(str::to_ascii_lowercase)
            .or_else(|| own.map(|t| t.resource.to_string()))
            .unwrap_or_else(|| resource_from_tool(tool).to_string());
        let resource_name = target
            .name
            .clone()
            .unwrap_or_else(|| own.map(|t| target_name(t, args)).unwrap_or_default());
        Self {
            tool: tool.to_string(),
            action,
            resource_type,
            resource_name,
            namespace: target.namespace.clone().unwrap_or_default(),
            api_version: target.api_version.clone(),
            argument_keys: argument_keys(tool, own.is_some(), args, summary),
            arguments: summary.arguments.clone(),
            denied_rule: None,
            client: ctx.session.as_ref().map(|s| s.client.clone()),
            session_id: ctx.session.as_ref().map(|s| s.id.clone()),
            trace_id: ctx.trace_id.clone(),
        }
    }

    /// Mark the call as refused by guardrail `rule`.
    pub fn deny(&mut self, rule: &str) {
        self.denied_rule = Some(rule.to_string());
    }

    /// The row for this call ending with `outcome` after `elapsed`.
    pub fn row(&self, outcome: Outcome, elapsed: Duration) -> McpCallRow {
        let client = self
            .client
            .as_ref()
            .filter(|c| c.name.is_some() || c.version.is_some());
        let detail = Detail {
            tool: &self.tool,
            outcome: outcome.as_str(),
            denied_rule: self.denied_rule.as_deref(),
            argument_keys: &self.argument_keys,
            arguments: &self.arguments,
            api_version: self.api_version.as_deref(),
            client: client.map(|c| ClientDetail {
                name: c.name.as_deref(),
                version: c.version.as_deref(),
                self_reported: true,
            }),
            session_id: self.session_id.as_deref(),
            trace_id: self.trace_id.as_deref(),
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        };
        McpCallRow {
            action: self.action.as_str().to_string(),
            resource_type: self.resource_type.clone(),
            resource_name: self.resource_name.clone(),
            namespace: self.namespace.clone(),
            detail: serde_json::to_string(&detail).unwrap_or_default(),
            tool: self.tool.clone(),
            outcome: outcome.as_str().to_string(),
            client_name: self
                .client
                .as_ref()
                .and_then(|c| c.name.clone())
                .unwrap_or_default(),
        }
    }
}

/// The JSON in an MCP row's `detail`.
#[derive(Serialize)]
struct Detail<'a> {
    tool: &'a str,
    /// `ok`, `error`, `denied` or `cancelled`.
    outcome: &'a str,
    /// The guardrail that refused the call: `namespace_allowlist`, or an
    /// mcp-k8s permission rule (`global:<action>`,
    /// `override:<resource>-<action>`, `apply_manifest:disabled`).
    #[serde(skip_serializing_if = "Option::is_none")]
    denied_rule: Option<&'a str>,
    /// The top-level argument keys, sorted; `?` stands for keys the tool
    /// doesn't declare.
    argument_keys: &'a [String],
    /// The allowlisted argument values (namespace, name, kind, image, ...).
    arguments: &'a Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_version: Option<&'a str>,
    /// What the client said about itself in `initialize`.
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<ClientDetail<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    /// The W3C trace id of the request, when deckwatch exports traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<&'a str>,
    duration_ms: u64,
}

#[derive(Serialize)]
struct ClientDetail<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    /// Always true: the client reports this itself, so it's a label, never
    /// an identity.
    self_reported: bool,
}

/// The name of the object a deckwatch tool targets, from the arguments its
/// table entry names (`deployment_name`, or `application_name` and
/// `plugin_name` joined with `/`). These identify the target, like the
/// `name` argument mcp-k8s's allowlist keeps; they go only into the row's
/// `resource_name`.
fn target_name(tool: &DeckwatchTool, args: &Value) -> String {
    tool.target
        .iter()
        .filter_map(|key| args.get(*key).and_then(Value::as_str))
        .map(|v| clip(v, MAX_VALUE_LEN))
        .collect::<Vec<_>>()
        .join("/")
}

/// The resource an mcp-k8s tool acts on, from its name:
/// `create_clusterrolebinding` → `clusterrolebinding`.
fn resource_from_tool(tool: &str) -> &str {
    const VERBS: &[&str] = &[
        "list_",
        "get_",
        "create_",
        "update_",
        "delete_",
        "scale_",
        "restart_",
        "rollback_",
        "approve_",
        "deny_",
        "cordon_",
        "uncordon_",
        "drain_",
        "evict_",
        "exec_",
        "apply_",
    ];
    VERBS
        .iter()
        .find_map(|verb| tool.strip_prefix(verb))
        .unwrap_or(tool)
}

/// The argument keys to record. For deckwatch's own tools, against
/// deckwatch's schema (mcp-k8s checks the ones it also defines, such as
/// `exec_pod` or `update_deployment`, against its own, which lacks
/// `pod_name` and `service_account`); otherwise the summary's.
fn argument_keys(tool: &str, own: bool, args: &Value, summary: &ToolCallSummary) -> Vec<String> {
    let (true, Some(declared), Some(obj)) = (own, declared_keys(tool), args.as_object()) else {
        return summary.argument_keys.clone();
    };
    let mut keys: Vec<String> = obj
        .keys()
        .filter(|k| declared.contains(k.as_str()))
        .cloned()
        .collect();
    keys.sort();
    keys.truncate(MAX_KEYS);
    if obj.keys().any(|k| !declared.contains(k.as_str())) {
        keys.push("?".to_string());
    }
    keys
}

/// Tool name → the top-level keys deckwatch's input schema declares.
fn declared_keys(tool: &str) -> Option<&'static HashSet<String>> {
    static SCHEMAS: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    SCHEMAS
        .get_or_init(|| {
            super::mcp::served_tool_definitions()
                .iter()
                .filter_map(|tool| {
                    let name = tool["name"].as_str()?.to_string();
                    let keys = tool["inputSchema"]["properties"]
                        .as_object()
                        .map(|props| props.keys().cloned().collect())
                        .unwrap_or_default();
                    Some((name, keys))
                })
                .collect()
        })
        .get(tool)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The audit row of a call in progress. [`PendingAudit::finish`] writes it
/// with the call's outcome; if the request is dropped first (the client
/// disconnected mid-call), it is written as `cancelled`, because the change
/// may have been made anyway.
pub struct PendingAudit {
    db: DatabaseConnection,
    record: Option<CallRecord>,
    started: Instant,
}

impl PendingAudit {
    pub fn new(db: &DatabaseConnection, record: CallRecord, started: Instant) -> Self {
        Self {
            db: db.clone(),
            record: Some(record),
            started,
        }
    }

    /// Write the row with `outcome`, in the background.
    pub fn finish(mut self, outcome: Outcome) {
        self.write(outcome);
    }

    /// Write nothing: no tool served the call after all.
    pub fn abandon(mut self) {
        self.record = None;
    }

    fn write(&mut self, outcome: Outcome) {
        if let Some(record) = self.record.take() {
            spawn_write(self.db.clone(), record.row(outcome, self.started.elapsed()));
        }
    }
}

impl Drop for PendingAudit {
    fn drop(&mut self) {
        self.write(Outcome::Cancelled);
    }
}

/// Insert `row` in a background task. A failure is logged and never reaches
/// the tool call.
fn spawn_write(db: DatabaseConnection, row: McpCallRow) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!(tool = %row.tool, "no runtime to write an MCP audit row");
        return;
    };
    runtime.spawn(async move {
        if let Err(e) = audit::insert_mcp_call(&db, &row).await {
            tracing::warn!(tool = %row.tool, error = %e, "failed to write an MCP audit row");
        }
    });
}

#[cfg(test)]
#[path = "mcp_audit_tests.rs"]
mod tests;
