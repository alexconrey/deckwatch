//! Plugin events: what a plugin call reported, made safe to show.
//!
//! Every plugin call goes through the funnel in `plugins.rs` (`call_export`
//! and the fetch wrapper). The funnel hands this module three kinds of input:
//!
//! - the structured `events` a plugin returned (SDK `PluginEvent`);
//! - the legacy `errors`, `warnings` and deprovision `message` strings, which
//!   become events with code [`codes::UNSPECIFIED`];
//! - failures the host detected itself, with the reserved `plugin.*` codes.
//!
//! [`normalize`] merges them into one list of [`PluginEvent`]s. Everything a
//! plugin wrote is untrusted, so before an event is returned, stored, logged
//! or exported it passes the safety rules:
//!
//! - the values the host passed the plugin for secret-typed config fields are
//!   replaced with [`SECRET_MASK`] ([`SecretScrubber`]), and credentials in
//!   URLs are redacted;
//! - control characters are stripped and text is collapsed onto one line;
//! - messages are capped at [`MAX_MESSAGE_BYTES`] and a call keeps at most
//!   [`MAX_EVENTS_PER_CALL`] events;
//! - codes are limited to a safe character set, and the `plugin.*` namespace
//!   is reserved for the host;
//! - logging is rate limited per plugin ([`EventRateLimiter`]); the excess is
//!   dropped and counted in `deckwatch_plugin_events_dropped_total`.
//!
//! Text that reaches an LLM (MCP tool results) is additionally wrapped in the
//! untrusted-content fence from `log_sanitize` by [`fence_for_llm`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::plugins::{EventSeverity, KubernetesObjectRef, PluginEvent};

/// Longest message kept, in bytes. Longer text is cut at a character
/// boundary and ends with [`TRUNCATION_MARKER`].
pub const MAX_MESSAGE_BYTES: usize = 2048;
/// Longest hint kept, in bytes.
pub const MAX_HINT_BYTES: usize = 512;
/// Longest event code kept, in bytes.
pub const MAX_CODE_BYTES: usize = 128;
/// Longest resource id or object reference field kept (a Kubernetes name is
/// at most 253 bytes).
pub const MAX_NAME_BYTES: usize = 253;
/// Most events kept from one plugin call. Host events are kept first, then
/// the plugin's own, most severe first.
pub const MAX_EVENTS_PER_CALL: usize = 20;
/// Replaces every occurrence of a secret config value.
pub const SECRET_MASK: &str = "********";
/// Appended to text that was cut short.
pub const TRUNCATION_MARKER: &str = " ...[truncated]";

/// Event codes. Plugins use `<plugin-name>.<code>`; `plugin.*` is reserved for
/// the host, and the host rewrites any plugin event that tries to use it.
pub mod codes {
    /// A plugin reported something without a code (including every legacy
    /// `errors`/`warnings`/`message` string and `error_set` text).
    pub const UNSPECIFIED: &str = "plugin.unspecified";
    /// Downloading or reading the WASM binary failed.
    pub const FETCH_FAILED: &str = "plugin.fetch_failed";
    /// The downloaded binary doesn't match the configured `sha256`.
    pub const CHECKSUM_MISMATCH: &str = "plugin.checksum_mismatch";
    /// The module didn't compile or instantiate, lacks a required export, or
    /// needs a newer deckwatch.
    pub const LOAD_FAILED: &str = "plugin.load_failed";
    /// The plugin trapped (a Rust panic compiles to a trap) or ran out of
    /// memory.
    pub const TRAP: &str = "plugin.trap";
    /// The call ran past its time limit.
    pub const TIMEOUT: &str = "plugin.timeout";
    /// The plugin's output isn't JSON deckwatch can read. The message carries
    /// the serde error path, e.g. `env_vars[1].name`.
    pub const OUTPUT_INVALID: &str = "plugin.output_invalid";
    /// Applying a Kubernetes object the plugin returned failed.
    pub const K8S_APPLY_FAILED: &str = "plugin.k8s_apply_failed";
    /// Deckwatch refused something the plugin or its config asked for.
    pub const POLICY_REJECTED: &str = "plugin.policy_rejected";
    /// Informational: deckwatch released the fields provisioned resources had
    /// added to a workload, because its application has no provisioned
    /// resources left. Host-only; not part of the SDK's list.
    pub const FIELDS_RELEASED: &str = "plugin.fields_released";

    /// The reserved namespace.
    pub const RESERVED_PREFIX: &str = "plugin.";

    /// Every host code, for docs and tests.
    pub const ALL: &[&str] = &[
        UNSPECIFIED,
        FETCH_FAILED,
        CHECKSUM_MISMATCH,
        LOAD_FAILED,
        TRAP,
        TIMEOUT,
        OUTPUT_INVALID,
        K8S_APPLY_FAILED,
        POLICY_REJECTED,
        FIELDS_RELEASED,
    ];

    /// The metric label for a host code: the code without `plugin.`. Anything
    /// that isn't a host code maps to `reported_error`, so a plugin can never
    /// put its own string into a label.
    pub fn metric_label(code: &str) -> &'static str {
        ALL.iter()
            .find(|c| **c == code)
            .map(|c| &c[RESERVED_PREFIX.len()..])
            .unwrap_or("reported_error")
    }
}

// ── Health ───────────────────────────────────────────────────────────────────

/// The state one call (or a set of calls) leaves a plugin in.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// No warnings or errors.
    #[default]
    Healthy,
    /// The call worked but reported warnings.
    Degraded,
    /// The call failed, or reported an error.
    Failing,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Failing => "failing",
        }
    }

    /// The health implied by a list of events.
    pub fn from_events(events: &[PluginEvent]) -> Self {
        events
            .iter()
            .map(|e| match effective_severity(e.severity) {
                EventSeverity::Error => Self::Failing,
                EventSeverity::Info => Self::Healthy,
                _ => Self::Degraded,
            })
            .max()
            .unwrap_or(Self::Healthy)
    }
}

/// `Unknown` (a severity from a newer SDK) counts as `Warning`.
pub fn effective_severity(s: EventSeverity) -> EventSeverity {
    match s {
        EventSeverity::Info | EventSeverity::Warning | EventSeverity::Error => s,
        _ => EventSeverity::Warning,
    }
}

fn severity_rank(s: EventSeverity) -> u8 {
    match effective_severity(s) {
        EventSeverity::Error => 2,
        EventSeverity::Info => 0,
        _ => 1,
    }
}

pub fn severity_str(s: EventSeverity) -> &'static str {
    match effective_severity(s) {
        EventSeverity::Error => "error",
        EventSeverity::Info => "info",
        _ => "warning",
    }
}

// ── Secret scrubbing ─────────────────────────────────────────────────────────

/// Masks the secret values the host passed to one plugin call.
///
/// Built from the config the call actually received: for every field the
/// plugin's `config_schema` marks `secret`, the value in whatever form the
/// host passed it (today that can be the stored AES-GCM ciphertext, see
/// deckwatch#251), plus its decryption when it decrypts, so fixing #251 can't
/// start leaking plaintext. Values read from files (`inherit_env_file_keys`,
/// typically workload identity tokens) are treated as secrets too. Each value
/// is also matched in its JSON-escaped and percent-encoded forms, which is
/// how it shows up when a plugin echoes a request body or URL.
#[derive(Debug, Clone, Default)]
pub struct SecretScrubber {
    /// Longest first, so a secret that contains another is masked whole.
    needles: Vec<String>,
}

impl SecretScrubber {
    pub fn new<I, S>(values: I, encryption_key: &str) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut needles: Vec<String> = Vec::new();
        for value in values {
            let value = value.as_ref();
            if value.trim().is_empty() {
                continue;
            }
            let mut forms = vec![value.to_string()];
            if !encryption_key.is_empty() {
                if let Ok(plain) = crate::crypto::decrypt(encryption_key, value) {
                    if !plain.trim().is_empty() {
                        forms.push(plain);
                    }
                }
            }
            for form in forms {
                let json = serde_json::to_string(&form).unwrap_or_default();
                let json = json
                    .strip_prefix('"')
                    .and_then(|j| j.strip_suffix('"'))
                    .unwrap_or_default()
                    .to_string();
                let pct = percent_encode(&form);
                needles.push(form);
                needles.push(json);
                needles.push(pct);
            }
        }
        needles.retain(|n| !n.is_empty());
        needles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        needles.dedup();
        Self { needles }
    }

    pub fn scrub<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(text);
        for needle in &self.needles {
            if out.contains(needle.as_str()) {
                out = Cow::Owned(out.replace(needle.as_str(), SECRET_MASK));
            }
        }
        out
    }
}

/// RFC 3986 percent-encoding of everything but unreserved characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── Text sanitization ────────────────────────────────────────────────────────

/// Replace the userinfo of any `scheme://user:pass@host` URL with `***`.
/// Plugin source URLs and plugin error messages can carry credentials.
pub fn redact_url_userinfo(text: &str) -> Cow<'_, str> {
    if !text.contains("://") || !text.contains('@') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        // The authority ends at the first '/', '?', '#' or whitespace.
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Make plugin text safe to store and show: mask secrets and URL
/// credentials, strip control characters (ANSI escapes included), collapse
/// whitespace onto one line, and cap the length at `max_bytes`.
///
/// One line matters: nothing a plugin writes can start a new line in a log,
/// a Kubernetes Event or the fenced block an LLM sees, so it can't fake a
/// fence marker or a log record.
pub fn sanitize_text(text: &str, max_bytes: usize, scrubber: &SecretScrubber) -> String {
    let scrubbed = scrubber.scrub(text);
    let redacted = redact_url_userinfo(&scrubbed);
    let stripped = crate::log_sanitize::strip_control_bytes(&redacted);
    let one_line = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&one_line, max_bytes)
}

/// Cut `s` to at most `max_bytes` bytes (marker included) at a character
/// boundary.
pub fn truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let budget = max_bytes.saturating_sub(TRUNCATION_MARKER.len());
    let mut cut = budget;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &s[..cut])
}

/// The configured plugin name as a code namespace: lowercase, with anything
/// outside `[a-z0-9_-]` replaced by `-`.
pub fn plugin_slug(plugin: &str) -> String {
    let slug: String = plugin
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    match slug.as_str() {
        "" => "unnamed".to_string(),
        // `plugin.<code>` would land in the reserved namespace.
        "plugin" => "plugin_".to_string(),
        _ => slug,
    }
}

/// Validate a plugin-supplied code. Empty becomes [`codes::UNSPECIFIED`]; a
/// code in the reserved `plugin.*` namespace, or without any namespace, is
/// moved under the plugin's own (`<plugin>.<code>`). Characters outside
/// `[a-z0-9._-]` become `_`.
pub fn sanitize_plugin_code(code: &str, plugin: &str) -> String {
    let cleaned: String = code
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.');
    if cleaned.is_empty() || cleaned == "plugin" {
        return codes::UNSPECIFIED.to_string();
    }
    let namespaced = if let Some(rest) = cleaned.strip_prefix(codes::RESERVED_PREFIX) {
        format!("{}.{rest}", plugin_slug(plugin))
    } else if cleaned.contains('.') {
        cleaned.to_string()
    } else {
        format!("{}.{cleaned}", plugin_slug(plugin))
    };
    truncate_plain(&namespaced, MAX_CODE_BYTES)
}

fn truncate_plain(s: &str, max: usize) -> String {
    // Codes are ASCII, so any byte index is a char boundary.
    s.chars().take(max).collect()
}

fn sanitize_opt(text: &Option<String>, max: usize, scrubber: &SecretScrubber) -> Option<String> {
    text.as_deref()
        .map(|t| sanitize_text(t, max, scrubber))
        .filter(|t| !t.is_empty())
}

/// Sanitize a Kubernetes object reference taken from plugin output.
pub fn sanitize_object_ref(
    object: &KubernetesObjectRef,
    scrubber: &SecretScrubber,
) -> KubernetesObjectRef {
    KubernetesObjectRef {
        api_version: sanitize_text(&object.api_version, MAX_NAME_BYTES, scrubber),
        kind: sanitize_text(&object.kind, MAX_NAME_BYTES, scrubber),
        name: sanitize_text(&object.name, MAX_NAME_BYTES, scrubber),
    }
}

/// Sanitize an event a plugin returned.
pub fn sanitize_plugin_event(
    plugin: &str,
    event: &PluginEvent,
    scrubber: &SecretScrubber,
) -> PluginEvent {
    let mut message = sanitize_text(&event.message, MAX_MESSAGE_BYTES, scrubber);
    if message.is_empty() {
        message = "(no message)".to_string();
    }
    PluginEvent {
        severity: effective_severity(event.severity),
        code: sanitize_plugin_code(&event.code, plugin),
        message,
        resource_id: sanitize_opt(&event.resource_id, MAX_NAME_BYTES, scrubber),
        object: event
            .object
            .as_ref()
            .map(|o| sanitize_object_ref(o, scrubber)),
        retryable: event.retryable,
        hint: sanitize_opt(&event.hint, MAX_HINT_BYTES, scrubber),
    }
}

/// An event the host raises. `code` should be one of [`codes`]. The message
/// is sanitized like plugin text, because host messages quote plugin output
/// (serde errors, `error_set` text, URLs).
pub fn host_event(
    severity: EventSeverity,
    code: &str,
    message: impl AsRef<str>,
    scrubber: &SecretScrubber,
) -> PluginEvent {
    PluginEvent {
        severity: effective_severity(severity),
        code: code.to_string(),
        message: sanitize_text(message.as_ref(), MAX_MESSAGE_BYTES, scrubber),
        ..Default::default()
    }
}

/// The legacy plain-string fields of a plugin result.
#[derive(Debug, Default, Clone, Copy)]
pub struct LegacyNotices<'a> {
    pub errors: &'a [String],
    pub warnings: &'a [String],
    /// `ResourceDeprovisionResult.message`: what was and wasn't cleaned up.
    pub info: Option<&'a str>,
}

/// The legacy strings that aren't SDK copies of structured events.
///
/// The SDK's result helpers (0.10.0) copy every event into the legacy fields
/// for older hosts, as `PluginEvent::legacy_text()`: error events into
/// `errors`, warning and unknown events into `warnings` (info events get no
/// copy), and on a deprovision result every non-error event onto the end of
/// `message`, joined with `"; "`. Comparing raw text, this drops each
/// `errors`/`warnings` entry equal to an event's `legacy_text()`, and strips
/// those copies from the end of `message`. What's left is a plugin's own
/// legacy reporting.
fn legacy_without_copies<'a>(
    structured: &[PluginEvent],
    legacy: LegacyNotices<'a>,
) -> (Vec<&'a str>, Vec<&'a str>, Option<&'a str>) {
    let copies: Vec<String> = structured.iter().map(PluginEvent::legacy_text).collect();
    let keep = |entries: &'a [String]| -> Vec<&'a str> {
        entries
            .iter()
            .map(String::as_str)
            .filter(|e| !copies.iter().any(|c| c == e))
            .collect()
    };
    let info = legacy.info.and_then(|message| {
        let appended: Vec<String> = structured
            .iter()
            .filter(|e| severity_rank(e.severity) < 2)
            .map(PluginEvent::legacy_text)
            .collect();
        if appended.is_empty() {
            return Some(message);
        }
        let suffix = appended.join("; ");
        if message == suffix {
            None
        } else if let Some(own) = message.strip_suffix(suffix.as_str()) {
            match own.strip_suffix("; ") {
                Some(own) => Some(own),
                // The suffix matched mid-text; it isn't the SDK's copy.
                None => Some(message),
            }
        } else {
            Some(message)
        }
    });
    (keep(legacy.errors), keep(legacy.warnings), info)
}

/// Merge host events, a plugin's structured events and its legacy strings
/// into one sanitized list, and return it with the number of events dropped
/// by the per-call cap.
///
/// - Host events (already built with [`host_event`]) come first.
/// - The SDK's legacy copies of structured events are dropped
///   ([`legacy_without_copies`]); the remaining legacy strings become
///   [`codes::UNSPECIFIED`] events.
/// - Exact duplicates are removed.
/// - Plugin events are ordered most severe first (stable), then the list is
///   capped at [`MAX_EVENTS_PER_CALL`].
pub fn normalize(
    plugin: &str,
    host: Vec<PluginEvent>,
    structured: &[PluginEvent],
    legacy: LegacyNotices<'_>,
    scrubber: &SecretScrubber,
) -> (Vec<PluginEvent>, usize) {
    let (errors, warnings, info) = legacy_without_copies(structured, legacy);
    let mut plugin_events: Vec<PluginEvent> = structured
        .iter()
        .map(|e| sanitize_plugin_event(plugin, e, scrubber))
        .collect();

    let legacy_events = errors
        .into_iter()
        .map(|t| (EventSeverity::Error, t))
        .chain(warnings.into_iter().map(|t| (EventSeverity::Warning, t)))
        .chain(info.map(|t| (EventSeverity::Info, t)));
    for (severity, text) in legacy_events {
        let message = sanitize_text(text, MAX_MESSAGE_BYTES, scrubber);
        if !message.is_empty() {
            plugin_events.push(PluginEvent {
                severity,
                code: codes::UNSPECIFIED.to_string(),
                message,
                ..Default::default()
            });
        }
    }

    plugin_events.sort_by_key(|e| std::cmp::Reverse(severity_rank(e.severity)));

    let mut out: Vec<PluginEvent> = Vec::with_capacity(host.len() + plugin_events.len());
    for event in host.into_iter().chain(plugin_events) {
        if !out.contains(&event) {
            out.push(event);
        }
    }
    let dropped = out.len().saturating_sub(MAX_EVENTS_PER_CALL);
    out.truncate(MAX_EVENTS_PER_CALL);
    (out, dropped)
}

// ── Reporting ────────────────────────────────────────────────────────────────

/// An event as returned to API and MCP clients: which plugin reported it,
/// from which export, and the trace id of the call when tracing is on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginEventReport {
    pub plugin: String,
    pub export: String,
    #[serde(flatten)]
    pub event: PluginEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// The first error (or failing) event as `code: message`, for error replies.
pub fn failure_summary(events: &[PluginEventReport]) -> Option<String> {
    events
        .iter()
        .find(|r| effective_severity(r.event.severity) == EventSeverity::Error)
        .or_else(|| events.first())
        .map(|r| format!("{}: {}", r.event.code, r.event.message))
}

/// Render events for an LLM: one line per event inside the untrusted-content
/// fence. Returns an empty string for no events.
///
/// The text was already sanitized onto one line per event, and every line
/// starts with `- [`, so plugin text can't imitate a fence marker even
/// before SEC-13 is fixed.
pub fn fence_for_llm(events: &[PluginEventReport]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut body = String::new();
    for r in events {
        let e = &r.event;
        body.push_str(&format!(
            "- [{}] {} ({}): {}: {}",
            severity_str(e.severity),
            r.plugin,
            r.export,
            e.code,
            e.message
        ));
        if let Some(id) = &e.resource_id {
            body.push_str(&format!(" (resource: {id})"));
        }
        if let Some(o) = &e.object {
            body.push_str(&format!(
                " (object: {}/{} {})",
                o.api_version, o.kind, o.name
            ));
        }
        if e.retryable {
            body.push_str(" (retryable)");
        }
        if let Some(hint) = &e.hint {
            body.push_str(&format!(" (hint: {hint})"));
        }
        if let Some(t) = &r.trace_id {
            body.push_str(&format!(" (trace: {t})"));
        }
        body.push('\n');
    }
    crate::log_sanitize::fence_untrusted("PLUGIN OUTPUT", "deckwatch plugins", &body)
}

// ── Rate limiting ────────────────────────────────────────────────────────────

/// Token bucket per plugin for logging (and exporting) events. A plugin that
/// reports on every 10-second reconcile across many Deployments can't flood
/// the logs or the API server; what's over the limit is dropped and counted.
#[derive(Debug)]
pub struct EventRateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

/// Burst size per plugin.
pub const EVENT_BURST: u32 = 60;
/// Sustained rate per plugin.
pub const EVENTS_PER_SEC: f64 = 1.0;

impl Default for EventRateLimiter {
    fn default() -> Self {
        Self::new(EVENT_BURST, EVENTS_PER_SEC)
    }
}

impl EventRateLimiter {
    pub fn new(burst: u32, per_sec: f64) -> Self {
        Self {
            capacity: f64::from(burst),
            refill_per_sec: per_sec,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Take one token for `plugin`. Never blocks for long and never fails:
    /// a poisoned lock lets the event through.
    pub fn allow(&self, plugin: &str) -> bool {
        self.allow_at(plugin, Instant::now())
    }

    fn allow_at(&self, plugin: &str, now: Instant) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            return true;
        };
        let (tokens, last) = buckets
            .entry(plugin.to_string())
            .or_insert((self.capacity, now));
        let elapsed = now.saturating_duration_since(*last).as_secs_f64();
        *tokens = (*tokens + elapsed * self.refill_per_sec).min(self.capacity);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ── Telemetry handle ─────────────────────────────────────────────────────────

/// Shared plugin-telemetry state, held in `AppState`. Cheap to clone.
#[derive(Clone, Default)]
pub struct PluginTelemetry {
    inner: Arc<TelemetryInner>,
}

#[derive(Default)]
struct TelemetryInner {
    limiter: EventRateLimiter,
    /// `DECKWATCH_ENCRYPTION_KEY`, to recognise decrypted secret values.
    encryption_key: String,
    /// Upper bound for one plugin call. `None` (the default) means no limit.
    call_timeout: Option<Duration>,
    /// The events of each plugin's most recent load, by configured name.
    /// Bounded by the number of configured plugins.
    last_loads: Mutex<HashMap<String, Vec<PluginEventReport>>>,
    /// The status worker's queue, once `plugin_status::spawn_worker` ran.
    sink: OnceLock<tokio::sync::mpsc::Sender<crate::plugin_status::SinkMsg>>,
}

impl std::fmt::Debug for PluginTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginTelemetry")
            .field("call_timeout", &self.inner.call_timeout)
            .finish_non_exhaustive()
    }
}

impl PluginTelemetry {
    pub fn new(encryption_key: impl Into<String>) -> Self {
        Self::with_options(encryption_key, None, EventRateLimiter::default())
    }

    pub fn with_options(
        encryption_key: impl Into<String>,
        call_timeout: Option<Duration>,
        limiter: EventRateLimiter,
    ) -> Self {
        Self {
            inner: Arc::new(TelemetryInner {
                limiter,
                encryption_key: encryption_key.into(),
                call_timeout,
                last_loads: Mutex::default(),
                sink: OnceLock::new(),
            }),
        }
    }

    pub fn encryption_key(&self) -> &str {
        &self.inner.encryption_key
    }

    pub fn call_timeout(&self) -> Option<Duration> {
        self.inner.call_timeout
    }

    /// Remember what a plugin's latest load reported, so the action that
    /// triggered the load (e.g. MCP `enable_plugin`) can return it.
    pub fn record_load(&self, plugin: &str, events: Vec<PluginEventReport>) {
        if let Ok(mut loads) = self.inner.last_loads.lock() {
            loads.insert(plugin.to_string(), events);
        }
    }

    /// The events of `plugin`'s latest load, if it was loaded since startup.
    pub fn last_load(&self, plugin: &str) -> Option<Vec<PluginEventReport>> {
        self.inner.last_loads.lock().ok()?.get(plugin).cloned()
    }

    /// Attach the status worker's queue. Returns false if one was attached
    /// already.
    pub(crate) fn connect(
        &self,
        tx: tokio::sync::mpsc::Sender<crate::plugin_status::SinkMsg>,
    ) -> bool {
        self.inner.sink.set(tx).is_ok()
    }

    /// Queue a message for the status worker without waiting. Returns false
    /// if there's no worker or its queue is full.
    fn send(&self, msg: crate::plugin_status::SinkMsg) -> bool {
        match self.inner.sink.get() {
            Some(tx) => tx.try_send(msg).is_ok(),
            None => false,
        }
    }

    /// Record a call's outcome in the plugin status table. Never waits and
    /// never fails: with the worker behind, the update is dropped and counted.
    pub fn report(&self, update: crate::plugin_status::StatusUpdate) {
        if self.inner.sink.get().is_none() {
            return;
        }
        let plugin = update.key.plugin.clone();
        if !self.send(crate::plugin_status::SinkMsg::Status(update)) {
            crate::metrics::record_plugin_events_dropped(&plugin, "queue_full", 1);
        }
    }

    /// Publish a Kubernetes Event that isn't tied to a status change
    /// (a deprovision, released fields). Never waits.
    pub fn kube_event(&self, request: crate::plugin_k8s_events::KubeEventRequest) {
        self.send(crate::plugin_status::SinkMsg::KubeEvent(request));
    }

    /// Remove a target's status (its resource was deprovisioned).
    pub fn forget(&self, key: crate::plugin_status::StatusKey) {
        self.send(crate::plugin_status::SinkMsg::Forget(key));
    }

    /// Remove an application's status rows (it was deleted).
    pub fn forget_application(&self, application_id: &str) {
        self.send(crate::plugin_status::SinkMsg::ForgetApplication(
            application_id.to_string(),
        ));
    }

    /// Remove the status rows of every plugin not in `enabled`.
    pub fn keep_plugins(&self, enabled: Vec<String>) {
        self.send(crate::plugin_status::SinkMsg::KeepPlugins(enabled));
    }

    /// Wait until everything queued so far has been applied. For tests and
    /// for callers that read the status table right after reporting.
    pub async fn flush(&self) {
        let (done, wait) = tokio::sync::oneshot::channel();
        if let Some(tx) = self.inner.sink.get() {
            if tx
                .send(crate::plugin_status::SinkMsg::Flush(done))
                .await
                .is_ok()
            {
                let _ = wait.await;
            }
        }
    }

    /// Log `events` at their severity in the current span (the funnel calls
    /// this inside the plugin span), subject to the per-plugin rate limit.
    /// Returns how many were dropped.
    pub fn log_events(&self, plugin: &str, export: &str, events: &[PluginEvent]) -> usize {
        let mut dropped = 0;
        for event in events {
            let severity = severity_str(event.severity);
            let source = if event.code.starts_with(codes::RESERVED_PREFIX)
                && event.code != codes::UNSPECIFIED
            {
                "host"
            } else {
                "plugin"
            };
            crate::metrics::record_plugin_event(plugin, severity, source);
            if !self.inner.limiter.allow(plugin) {
                dropped += 1;
                continue;
            }
            match effective_severity(event.severity) {
                EventSeverity::Error => tracing::error!(
                    target: "deckwatch::plugin_events",
                    plugin,
                    export,
                    code = %event.code,
                    retryable = event.retryable,
                    resource_id = event.resource_id.as_deref().unwrap_or(""),
                    "plugin event: {}",
                    event.message
                ),
                EventSeverity::Info => tracing::info!(
                    target: "deckwatch::plugin_events",
                    plugin,
                    export,
                    code = %event.code,
                    retryable = event.retryable,
                    resource_id = event.resource_id.as_deref().unwrap_or(""),
                    "plugin event: {}",
                    event.message
                ),
                _ => tracing::warn!(
                    target: "deckwatch::plugin_events",
                    plugin,
                    export,
                    code = %event.code,
                    retryable = event.retryable,
                    resource_id = event.resource_id.as_deref().unwrap_or(""),
                    "plugin event: {}",
                    event.message
                ),
            }
        }
        if dropped > 0 {
            crate::metrics::record_plugin_events_dropped(plugin, "rate_limited", dropped as u64);
        }
        dropped
    }
}

// ── Trace context ────────────────────────────────────────────────────────────

/// Returns the W3C `traceparent` of a span, when an OpenTelemetry layer is
/// installed.
pub type TraceContextFn = fn(&tracing::Span) -> Option<String>;

static TRACE_CONTEXT: OnceLock<TraceContextFn> = OnceLock::new();

/// Called once at startup when deckwatch exports traces (see `lib.rs`), with
/// [`otel_traceparent`]. Until then, and whenever trace export is off,
/// plugin requests carry no `trace_context` and events no trace id. Returns
/// false if a provider was already installed.
pub fn install_trace_context_provider(provider: TraceContextFn) -> bool {
    TRACE_CONTEXT.set(provider).is_ok()
}

/// The W3C `traceparent` of `span` as the OpenTelemetry layer sees it, or
/// `None` when the layer doesn't track it (export off, or the span filtered
/// out).
pub fn otel_traceparent(span: &tracing::Span) -> Option<String> {
    use opentelemetry::trace::TraceContextExt as _;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    let context = span.context();
    let otel_span = context.span();
    let span_context = otel_span.span_context();
    span_context.is_valid().then(|| {
        format!(
            "00-{}-{}-{:02x}",
            span_context.trace_id(),
            span_context.span_id(),
            span_context.trace_flags().to_u8()
        )
    })
}

/// The `traceparent` for `span`, or `None` when no OpenTelemetry layer is
/// active or the provider returns something that isn't a valid traceparent.
pub fn trace_context(span: &tracing::Span) -> Option<String> {
    let provider = TRACE_CONTEXT.get()?;
    provider(span).filter(|tp| trace_id_of(tp).is_some())
}

/// The trace id of a W3C `traceparent`, if it's a valid version-00 one:
/// `00-<32 lowercase hex>-<16 lowercase hex>-<2 lowercase hex>`, neither id
/// all zeros. The same check as the SDK's `trace_headers()`, so deckwatch
/// only ever sends a `trace_context` a plugin will forward.
pub fn trace_id_of(traceparent: &str) -> Option<&str> {
    let b = traceparent.as_bytes();
    let lower_hex = |s: &[u8]| s.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    let not_all_zeros = |s: &[u8]| s.iter().any(|&c| c != b'0');
    let valid = b.len() == 55
        && b.starts_with(b"00-")
        && b[35] == b'-'
        && b[52] == b'-'
        && lower_hex(&b[3..35])
        && not_all_zeros(&b[3..35])
        && lower_hex(&b[36..52])
        && not_all_zeros(&b[36..52])
        && lower_hex(&b[53..55]);
    valid.then(|| &traceparent[3..35])
}

#[cfg(test)]
#[path = "plugin_events_tests.rs"]
mod tests;
