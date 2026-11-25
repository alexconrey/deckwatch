//! Test-only fixture plugins, written in WebAssembly text.
//!
//! The host's plugin tests used to stop at `LoadedPlugin` values with fake
//! bytes, so nothing exercised a real extism call. These fixtures are real
//! extism plugins: each export either returns a fixed JSON document, logs
//! through the PDK first, reports an error the way `extism_pdk` does, traps,
//! or spins forever. wasmtime (built with its default `wat` feature) compiles
//! the text directly, so no toolchain or checked-in binaries are needed.
//!
//! ```ignore
//! let wasm = FixturePlugin::new()
//!     .export("apply", Behavior::output(r#"{"errors":["boom"]}"#))
//!     .export("provision", Behavior::Trap)
//!     .wasm();
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::plugins::{LoadedPlugin, PluginMetadata};

/// What one export of a fixture plugin does when called.
#[derive(Debug, Clone)]
pub enum Behavior {
    /// Log each `(level, line)` through the PDK, then return `output`.
    Output {
        logs: Vec<(LogLevel, String)>,
        output: String,
    },
    /// Log the call's input at info level through the PDK, then return
    /// `output`. Lets a test see exactly what the host sent.
    LogInputThenOutput(String),
    /// Report `message` through `error_set` and return 1, which is what
    /// `extism_pdk` does when a `#[plugin_fn]` returns `Err`.
    Fail(String),
    /// Execute `unreachable` (what a Rust panic compiles to).
    Trap,
    /// Loop forever, for timeout tests.
    Spin,
}

impl Behavior {
    pub fn output(json: impl Into<String>) -> Self {
        Self::Output {
            logs: Vec::new(),
            output: json.into(),
        }
    }

    pub fn logs_then_output(logs: &[(LogLevel, &str)], json: impl Into<String>) -> Self {
        Self::Output {
            logs: logs.iter().map(|(l, s)| (*l, s.to_string())).collect(),
            output: json.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn import(self) -> &'static str {
        match self {
            Self::Info => "$log_info",
            Self::Warn => "$log_warn",
            Self::Error => "$log_error",
        }
    }
}

/// Builder for a fixture plugin module.
#[derive(Debug, Clone, Default)]
pub struct FixturePlugin {
    exports: Vec<(String, Behavior)>,
}

impl FixturePlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn export(mut self, name: &str, behavior: Behavior) -> Self {
        self.exports.push((name.to_string(), behavior));
        self
    }

    /// The module as WebAssembly text. extism accepts it as-is.
    pub fn wat(&self) -> String {
        // Every string the module needs lives in its own memory as a data
        // segment; `$put` copies one into extism memory and returns the
        // handle that the PDK functions take.
        let mut data = String::new();
        let mut funcs = String::new();
        let mut offset = 0usize;
        let mut place = |s: &str, data: &mut String| -> (usize, usize) {
            let at = offset;
            let _ = writeln!(data, "  (data (i32.const {at}) \"{}\")", wat_escape(s));
            offset += s.len().max(1);
            (at, s.len())
        };

        for (name, behavior) in &self.exports {
            let mut body = String::new();
            match behavior {
                Behavior::Output { logs, output } => {
                    for (level, line) in logs {
                        let (at, len) = place(line, &mut data);
                        let _ = writeln!(
                            body,
                            "    (call {} (call $put (i32.const {at}) (i32.const {len})))",
                            level.import()
                        );
                    }
                    let (at, len) = place(output, &mut data);
                    let _ = writeln!(
                        body,
                        "    (call $output_set (call $put (i32.const {at}) (i32.const {len})) (i64.const {len}))\n    (i32.const 0)"
                    );
                }
                Behavior::LogInputThenOutput(output) => {
                    let (at, len) = place(output, &mut data);
                    let _ = writeln!(
                        body,
                        "    (call $log_info (call $put_input))\n    (call $output_set (call $put (i32.const {at}) (i32.const {len})) (i64.const {len}))\n    (i32.const 0)"
                    );
                }
                Behavior::Fail(message) => {
                    let (at, len) = place(message, &mut data);
                    let _ = writeln!(
                        body,
                        "    (call $error_set (call $put (i32.const {at}) (i32.const {len})))\n    (i32.const 1)"
                    );
                }
                Behavior::Trap => body.push_str("    unreachable\n"),
                Behavior::Spin => body.push_str("    (loop $l (br $l))\n    (i32.const 0)\n"),
            }
            let _ = writeln!(
                funcs,
                "  (func (export \"{}\") (result i32)\n{body}  )",
                wat_escape(name)
            );
        }

        format!(
            r#"(module
  (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
  (import "extism:host/env" "store_u8" (func $store_u8 (param i64 i32)))
  (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
  (import "extism:host/env" "error_set" (func $error_set (param i64)))
  (import "extism:host/env" "log_info" (func $log_info (param i64)))
  (import "extism:host/env" "log_warn" (func $log_warn (param i64)))
  (import "extism:host/env" "log_error" (func $log_error (param i64)))
  (import "extism:host/env" "input_length" (func $input_length (result i64)))
  (import "extism:host/env" "input_load_u8" (func $input_load_u8 (param i64) (result i32)))
  (memory 1)
{data}
  ;; Copy `len` bytes at `src` in this module's memory into a fresh extism
  ;; allocation and return its handle.
  (func $put (param $src i32) (param $len i32) (result i64)
    (local $h i64) (local $i i32)
    (local.set $h (call $alloc (i64.extend_i32_u (local.get $len))))
    (block $done
      (loop $copy
        (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
        (call $store_u8
          (i64.add (local.get $h) (i64.extend_i32_u (local.get $i)))
          (i32.load8_u (i32.add (local.get $src) (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $copy)))
    (local.get $h))
  ;; Copy the call's input into a fresh extism allocation.
  (func $put_input (result i64)
    (local $h i64) (local $n i64) (local $i i64)
    (local.set $n (call $input_length))
    (local.set $h (call $alloc (local.get $n)))
    (block $done
      (loop $copy
        (br_if $done (i64.ge_u (local.get $i) (local.get $n)))
        (call $store_u8
          (i64.add (local.get $h) (local.get $i))
          (call $input_load_u8 (local.get $i)))
        (local.set $i (i64.add (local.get $i) (i64.const 1)))
        (br $copy)))
    (local.get $h))
{funcs})"#
        )
    }

    pub fn wasm(&self) -> Vec<u8> {
        self.wat().into_bytes()
    }

    /// A `LoadedPlugin` running this module, named `name`.
    pub fn loaded(&self, name: &str) -> LoadedPlugin {
        let bytes = self.wasm();
        LoadedPlugin {
            name: name.to_string(),
            sha256: crate::plugins::sha256_hex(&bytes),
            wasm_bytes: Arc::new(bytes),
            allowed_hosts: vec![],
            config: BTreeMap::new(),
            inherit_env_keys: vec![],
            inherit_env_file_keys: BTreeMap::new(),
            metadata: PluginMetadata {
                name: name.to_string(),
                version: "0.0.1-fixture".to_string(),
                ..Default::default()
            },
            loaded_at: String::new(),
        }
    }
}

/// Escape a string for a WAT string literal: printable ASCII except `"` and
/// `\` passes through, everything else becomes `\hh` byte escapes.
fn wat_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' {
            out.push(b as char);
        } else {
            let _ = write!(out, "\\{b:02x}");
        }
    }
    out
}

/// Records tracing events with the spans they happened in, for asserting
/// that plugin log lines land inside the plugin call's span.
pub mod capture {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    #[derive(Debug, Clone)]
    pub struct CapturedSpan {
        pub name: String,
        pub fields: BTreeMap<String, String>,
    }

    #[derive(Debug, Clone)]
    pub struct CapturedEvent {
        pub target: String,
        pub level: tracing::Level,
        pub message: String,
        pub fields: BTreeMap<String, String>,
        /// Enclosing spans, outermost first.
        pub spans: Vec<CapturedSpan>,
    }

    impl CapturedEvent {
        /// The innermost enclosing span with this name.
        pub fn span(&self, name: &str) -> Option<&CapturedSpan> {
            self.spans.iter().rev().find(|s| s.name == name)
        }
    }

    #[derive(Default)]
    struct Fields(BTreeMap<String, String>);

    impl Visit for Fields {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    struct CaptureLayer {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl<S> Layer<S> for CaptureLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            if let Some(span) = ctx.span(id) {
                span.extensions_mut().insert(fields);
            }
        }

        fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                if let Some(fields) = span.extensions_mut().get_mut::<Fields>() {
                    values.record(fields);
                }
            }
        }

        fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            let message = fields.0.remove("message").unwrap_or_default();
            let spans = ctx
                .event_scope(event)
                .map(|scope| {
                    scope
                        .from_root()
                        .map(|s| CapturedSpan {
                            name: s.name().to_string(),
                            fields: s
                                .extensions()
                                .get::<Fields>()
                                .map(|f| f.0.clone())
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Ok(mut events) = self.events.lock() {
                events.push(CapturedEvent {
                    target: event.metadata().target().to_string(),
                    level: *event.metadata().level(),
                    message,
                    fields: fields.0,
                    spans,
                });
            }
        }
    }

    /// Registered for the life of the test binary; wants nothing.
    ///
    /// tracing caches, per callsite, whether any subscriber wants it. While
    /// exactly one dispatcher is registered, a callsite first hit on another
    /// thread computes that from *that thread's* default (none, in a test
    /// without capture) and caches "never", so a concurrently running capture
    /// silently loses the event. A second, permanent dispatcher answering
    /// "sometimes" turns that shortcut off: the cached interest then always
    /// includes every live capture.
    struct Sometimes;

    impl Subscriber for Sometimes {
        fn register_callsite(
            &self,
            _: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::sometimes()
        }
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            false
        }
        fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
            Some(tracing::level_filters::LevelFilter::TRACE)
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    static KEEPALIVE: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();

    /// [`capture`] for async code on a current-thread runtime (the
    /// `#[tokio::test]` default), where everything runs on this thread.
    pub async fn capture_async<R>(
        fut: impl std::future::Future<Output = R>,
    ) -> (R, Vec<CapturedEvent>) {
        KEEPALIVE.get_or_init(|| tracing::Dispatch::new(Sometimes));
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            events: events.clone(),
        });
        let guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();
        let result = fut.await;
        drop(guard);
        let captured = events.lock().map(|e| e.clone()).unwrap_or_default();
        (result, captured)
    }

    /// Run `f` with a subscriber that records every event on this thread.
    pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<CapturedEvent>) {
        KEEPALIVE.get_or_init(|| tracing::Dispatch::new(Sometimes));
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            events: events.clone(),
        });
        let result = tracing::subscriber::with_default(subscriber, || {
            // A callsite registered while this subscriber was being set up
            // may have cached a stale interest; recompute with it live.
            tracing::callsite::rebuild_interest_cache();
            f()
        });
        let captured = events.lock().map(|e| e.clone()).unwrap_or_default();
        (result, captured)
    }
}

/// A Kubernetes API server stand-in for handler tests: every request is
/// recorded, and `handler` decides the reply. Unhandled requests get 404.
pub mod fake_kube {
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    pub struct Recorded {
        pub method: String,
        pub path: String,
        pub query: String,
        pub body: Value,
    }

    type Handler = dyn Fn(&str, &str, &str, &Value) -> Option<(u16, Value)> + Send + Sync;

    #[derive(Clone)]
    pub struct FakeKube {
        requests: Arc<Mutex<Vec<Recorded>>>,
        handler: Arc<Handler>,
    }

    pub fn status(code: u16, reason: &str) -> (u16, Value) {
        (
            code,
            serde_json::json!({
                "kind": "Status", "apiVersion": "v1", "metadata": {},
                "status": "Failure", "message": reason, "reason": reason, "code": code,
            }),
        )
    }

    impl FakeKube {
        /// `handler(method, path, body)`; `None` means 404.
        pub fn new(
            handler: impl Fn(&str, &str, &Value) -> Option<(u16, Value)> + Send + Sync + 'static,
        ) -> Self {
            Self::with_query(move |method, path, _query, body| handler(method, path, body))
        }

        /// Like [`Self::new`], with the decoded query string:
        /// `handler(method, path, query, body)`.
        pub fn with_query(
            handler: impl Fn(&str, &str, &str, &Value) -> Option<(u16, Value)> + Send + Sync + 'static,
        ) -> Self {
            Self {
                requests: Arc::default(),
                handler: Arc::new(handler),
            }
        }

        pub fn client(&self) -> kube::Client {
            let fake = self.clone();
            let service = tower::service_fn(move |req: axum::http::Request<kube::client::Body>| {
                let fake = fake.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let bytes = body.collect_bytes().await.unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let method = parts.method.to_string();
                    let path = parts.uri.path().to_string();
                    let query = parts.uri.query().unwrap_or_default().to_string();
                    fake.requests.lock().unwrap().push(Recorded {
                        method: method.clone(),
                        path: path.clone(),
                        query: query.clone(),
                        body: body.clone(),
                    });
                    let query = query
                        .replace("%3D", "=")
                        .replace("%2F", "/")
                        .replace('+', " ");
                    let (code, json) = (fake.handler)(&method, &path, &query, &body)
                        .unwrap_or_else(|| status(404, "NotFound"));
                    let response = axum::http::Response::builder()
                        .status(code)
                        .header("content-type", "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&json).unwrap()))
                        .unwrap();
                    Ok::<_, std::convert::Infallible>(response)
                }
            });
            kube::Client::new(service, "default")
        }

        pub fn requests(&self) -> Vec<Recorded> {
            self.requests.lock().unwrap().clone()
        }
    }
}

/// A DB-backed `AppState` for handler tests: SQLite in `dir` (a file, so
/// every pooled connection sees the same data), `plugins` loaded, and
/// `kube` as the cluster (a client pointing nowhere if `None`).
pub async fn test_state(
    dir: &std::path::Path,
    plugins: Vec<LoadedPlugin>,
    kube: Option<kube::Client>,
) -> crate::state::AppState {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let kube_client = match kube {
        Some(client) => client,
        None => fake_kube::FakeKube::new(|_, _, _| None).client(),
    };
    let url = format!("sqlite://{}?mode=rwc", dir.join("deckwatch.db").display());
    let db = crate::db::connect(&url).await.expect("test database");
    crate::state::AppState {
        kube_client,
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-config".to_string(),
        entitlements: Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: Arc::new(tokio::sync::RwLock::new(plugins)),
        plugin_patch_fingerprints: Arc::default(),
        plugin_telemetry: Default::default(),
    }
}

/// Serve `bytes` over plain HTTP on localhost; returns the URL.
pub async fn serve_bytes(bytes: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/plugin.wasm",
        axum::routing::get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/plugin.wasm")
}

/// Just enough of the Kubernetes API for plugin Events: Deployments to
/// attach them to (typed and metadata reads), events.k8s.io/v1 writes (the
/// Recorder's POST and series PATCH), and core/v1 Event LISTs that serve the
/// stored Events back the way the API server converts them, for `get_events`
/// and deckwatch's event views.
pub mod fake_events {
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};

    use super::fake_kube::{status, FakeKube};

    #[derive(Default)]
    struct State {
        deployments: Vec<Value>,
        events: Vec<Value>,
    }

    #[derive(Clone, Default)]
    pub struct FakeEventsApi {
        state: Arc<Mutex<State>>,
    }

    impl FakeEventsApi {
        /// `deployments` are `(namespace, name, application label)`; each gets
        /// UID `uid-{name}`.
        pub fn with_deployments(deployments: &[(&str, &str, &str)]) -> Self {
            let api = Self::default();
            api.state.lock().unwrap().deployments = deployments
                .iter()
                .map(|(ns, name, app)| {
                    json!({
                        "apiVersion": "apps/v1",
                        "kind": "Deployment",
                        "metadata": {
                            "name": name, "namespace": ns, "uid": format!("uid-{name}"),
                            "labels": { "deckwatch.io/application": app },
                        },
                        "spec": {
                            "selector": { "matchLabels": { "app": name } },
                            "template": { "spec": { "containers": [{ "name": "app", "image": "app:1" }] } },
                        },
                    })
                })
                .collect();
            api
        }

        /// The events.k8s.io/v1 Events written so far.
        pub fn events(&self) -> Vec<Value> {
            self.state.lock().unwrap().events.clone()
        }

        pub fn kube(&self) -> FakeKube {
            let api = self.clone();
            FakeKube::with_query(move |method, path, query, body| {
                api.handle(method, path, query, body)
            })
        }

        fn handle(
            &self,
            method: &str,
            path: &str,
            query: &str,
            body: &Value,
        ) -> Option<(u16, Value)> {
            let mut st = self.state.lock().unwrap();
            let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
            // Only `deckwatch.io/application=<app>` selectors are needed.
            let app_selector = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("labelSelector="))
                .and_then(|sel| sel.strip_prefix("deckwatch.io/application="))
                .map(str::to_string);
            match (method, segments.as_slice()) {
                ("GET", ["apis", "apps", "v1", "namespaces", ns, "deployments"]) => {
                    // Full objects: they also read as metadata-only items.
                    let items: Vec<Value> = st
                        .deployments
                        .iter()
                        .filter(|d| d["metadata"]["namespace"] == *ns)
                        .filter(|d| {
                            app_selector.as_deref().is_none_or(|app| {
                                d["metadata"]["labels"]["deckwatch.io/application"] == app
                            })
                        })
                        .cloned()
                        .collect();
                    Some((
                        200,
                        json!({
                            "apiVersion": "apps/v1",
                            "kind": "DeploymentList",
                            "metadata": { "resourceVersion": "1" },
                            "items": items,
                        }),
                    ))
                }
                // The plugin reconciler's server-side apply.
                ("PATCH", ["apis", "apps", "v1", "namespaces", ns, "deployments", name]) => st
                    .deployments
                    .iter()
                    .find(|d| d["metadata"]["namespace"] == *ns && d["metadata"]["name"] == *name)
                    .map(|d| (200, d.clone())),
                ("GET", ["apis", "batch", "v1", "namespaces", _ns, "cronjobs"]) => Some((
                    200,
                    json!({
                        "apiVersion": "batch/v1", "kind": "CronJobList",
                        "metadata": { "resourceVersion": "1" }, "items": [],
                    }),
                )),
                ("GET", ["apis", "apps", "v1", "namespaces", ns, "deployments", name]) => st
                    .deployments
                    .iter()
                    .find(|d| d["metadata"]["namespace"] == *ns && d["metadata"]["name"] == *name)
                    .map(|d| {
                        (
                            200,
                            json!({
                                "apiVersion": "meta.k8s.io/v1",
                                "kind": "PartialObjectMetadata",
                                "metadata": d["metadata"],
                            }),
                        )
                    }),
                ("POST", ["apis", "events.k8s.io", "v1", "namespaces", _ns, "events"]) => {
                    let mut event = body.clone();
                    event["metadata"]["resourceVersion"] = json!("1");
                    st.events.push(event.clone());
                    Some((201, event))
                }
                ("PATCH", ["apis", "events.k8s.io", "v1", "namespaces", _ns, "events", name]) => {
                    let stored = st
                        .events
                        .iter_mut()
                        .find(|e| e["metadata"]["name"] == *name)?;
                    if let Some(series) = body.get("series") {
                        stored["series"] = series.clone();
                    }
                    Some((200, stored.clone()))
                }
                ("GET", ["api", "v1", "namespaces", ns, "events"]) => {
                    let items: Vec<Value> = st
                        .events
                        .iter()
                        .filter(|e| e["metadata"]["namespace"] == *ns)
                        .map(core_event)
                        .collect();
                    Some((
                        200,
                        json!({
                            "apiVersion": "v1", "kind": "EventList",
                            "metadata": { "resourceVersion": "1" }, "items": items,
                        }),
                    ))
                }
                ("GET", _) => Some(status(404, "NotFound")),
                _ => None,
            }
        }
    }

    /// An events.k8s.io/v1 Event as core/v1 serves it.
    fn core_event(e: &Value) -> Value {
        let mut core = json!({
            "apiVersion": "v1",
            "kind": "Event",
            "metadata": e["metadata"],
            "involvedObject": e["regarding"],
            "reason": e["reason"],
            "message": e["note"],
            "type": e["type"],
            "action": e["action"],
            "eventTime": e["eventTime"],
            "reportingComponent": e["reportingController"],
            "reportingInstance": e["reportingInstance"],
        });
        if let Some(series) = e.get("series").filter(|s| !s.is_null()) {
            core["series"] = json!({
                "count": series["count"],
                "lastObservedTime": series["lastObservedTime"],
            });
        }
        core
    }
}

/// A webhook receiver on localhost that records every JSON body POSTed to
/// it. Returns its URL and the bodies received so far.
pub async fn serve_webhook() -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    let received: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(body);
                "ok"
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), received)
}

/// Save notification settings that send every event to `webhook_url`.
pub async fn enable_notifications(state: &crate::state::AppState, webhook_url: &str) {
    let mut settings = crate::handlers::settings::load_settings_from_db(state).await;
    settings.notifications = Some(crate::handlers::settings::NotificationSettings {
        enabled: true,
        webhook_url: webhook_url.to_string(),
        event_types: vec![],
        namespaces: vec![],
    });
    crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
        .await
        .unwrap();
}
