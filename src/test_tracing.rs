//! Test support: capture the `tracing` spans and events a future emits.
//!
//! The capture layer is installed once as the process-wide subscriber,
//! rather than per test with a scoped one: `tracing` caches callsite
//! interest per process, and while a scoped subscriber exists, another test
//! thread reaching a callsite first can cache it as disabled for everyone.
//! What it sees is kept per thread, and only inside [`capture`], so tests
//! running in parallel don't see each other's telemetry. `#[tokio::test]`
//! runs everything (including spawned tasks) on the test's own thread.
//!
//! It captures what deckwatch exports by default and more: every span and
//! event at `INFO` and above, and `deckwatch` and `mcp_k8s` ones at any
//! level.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Once;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

/// A span: its creation fields merged with everything recorded later.
#[derive(Clone, Debug)]
pub struct CapturedSpan {
    pub name: String,
    pub target: String,
    pub fields: BTreeMap<String, String>,
}

/// An event, with the index (in [`Captured::spans`]) of the span it
/// happened in.
#[derive(Clone, Debug)]
pub struct CapturedEvent {
    pub target: String,
    pub message: String,
    pub span: Option<usize>,
    pub fields: BTreeMap<String, String>,
}

/// Everything captured, in order.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    pub spans: Vec<CapturedSpan>,
    pub events: Vec<CapturedEvent>,
    /// Span id → index of the latest span with that id (the registry
    /// reuses ids of closed spans).
    ids: HashMap<u64, usize>,
}

impl Captured {
    /// Spans named `name` on target `target`.
    pub fn spans_named(&self, target: &str, name: &str) -> Vec<&CapturedSpan> {
        self.spans
            .iter()
            .filter(|s| s.target == target && s.name == name)
            .collect()
    }

    /// Events on target `target`.
    pub fn events_on(&self, target: &str) -> Vec<&CapturedEvent> {
        self.events.iter().filter(|e| e.target == target).collect()
    }

    /// Every string captured: span and event names, targets, field names
    /// and values.
    pub fn haystack(&self) -> String {
        let mut out = String::new();
        let mut add = |name: &str, target: &str, fields: &BTreeMap<String, String>| {
            out.push_str(name);
            out.push('\n');
            out.push_str(target);
            out.push('\n');
            for (k, v) in fields {
                out.push_str(k);
                out.push('=');
                out.push_str(v);
                out.push('\n');
            }
        };
        for s in &self.spans {
            add(&s.name, &s.target, &s.fields);
        }
        for e in &self.events {
            add(&e.message, &e.target, &e.fields);
        }
        out
    }
}

thread_local! {
    static CAPTURE: RefCell<Option<Captured>> = const { RefCell::new(None) };
}

/// Run `fut` and return what it emitted.
pub async fn capture<F: Future>(fut: F) -> (F::Output, Captured) {
    install();
    CAPTURE.with(|c| *c.borrow_mut() = Some(Captured::default()));
    let output = fut.await;
    let captured = CAPTURE.with(|c| c.borrow_mut().take()).unwrap_or_default();
    (output, captured)
}

fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(CaptureLayer))
            .expect("no other test installs a global subscriber");
        tracing::callsite::rebuild_interest_cache();
    });
}

fn wanted(meta: &Metadata<'_>) -> bool {
    *meta.level() <= tracing::Level::INFO
        || meta.target().starts_with("deckwatch")
        || meta.target().starts_with("mcp_k8s")
}

fn with_capture(f: impl FnOnce(&mut Captured)) {
    CAPTURE.with(|c| {
        if let Some(captured) = c.borrow_mut().as_mut() {
            f(captured);
        }
    });
}

struct FieldVisitor<'a> {
    fields: &'a mut BTreeMap<String, String>,
    message: &'a mut String,
}

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let value = format!("{value:?}");
        if field.name() == "message" {
            *self.message = value.clone();
        }
        self.fields.insert(field.name().to_string(), value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            *self.message = value.to_string();
        }
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

struct CaptureLayer;

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if wanted(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        wanted(meta)
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        with_capture(|captured| {
            let mut fields = BTreeMap::new();
            let mut message = String::new();
            attrs.record(&mut FieldVisitor {
                fields: &mut fields,
                message: &mut message,
            });
            captured.spans.push(CapturedSpan {
                name: attrs.metadata().name().to_string(),
                target: attrs.metadata().target().to_string(),
                fields,
            });
            let index = captured.spans.len() - 1;
            captured.ids.insert(id.into_u64(), index);
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        with_capture(|captured| {
            let Some(&index) = captured.ids.get(&id.into_u64()) else {
                return;
            };
            let mut message = String::new();
            values.record(&mut FieldVisitor {
                fields: &mut captured.spans[index].fields,
                message: &mut message,
            });
        });
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        with_capture(|captured| {
            let mut fields = BTreeMap::new();
            let mut message = String::new();
            event.record(&mut FieldVisitor {
                fields: &mut fields,
                message: &mut message,
            });
            let span = ctx
                .event_span(event)
                .and_then(|s| captured.ids.get(&s.id().into_u64()).copied());
            captured.events.push(CapturedEvent {
                target: event.metadata().target().to_string(),
                message,
                span,
                fields,
            });
        });
    }
}
