//! Kubernetes Events for plugin failures and notices.
//!
//! The status worker (`plugin_status`) turns call outcomes into Events on
//! the affected workloads, published through kube-rs
//! `runtime::events::Recorder` (events.k8s.io/v1, reporting controller
//! `deckwatch`). They show up in `kubectl describe deployment`, in
//! deckwatch's event views and in MCP `get_events`, so people and agents see
//! plugin failures where they already look.
//!
//! Reasons are a fixed set ([`reasons`]). Repeats are folded twice:
//!
//! - **Here**, an Event with the same target, reason and action is published
//!   at most once per [`REPUBLISH_INTERVAL`], so the 10-second reconcile loop
//!   doesn't call the API server every cycle.
//! - **By the Recorder**, which turns a repeat within its 6-minute cache
//!   window into a series (`series.count` + 1, one PATCH) instead of a new
//!   Event. The republish interval is shorter than that window, so a failure
//!   that persists stays one Event whose count keeps going up.
//!
//! The action carries the event code (e.g. `Apply plugin.trap`), so a
//! different failure starts a new Event instead of hiding behind the first
//! one's note. Notes are sanitized plugin text, cut to the 1 KB the Events
//! API allows. Publishing never blocks a plugin call: it runs off the status
//! worker, at most [`MAX_IN_FLIGHT`] at a time, and drops (and counts) the
//! rest.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::ObjectReference;
use kube::api::ListParams;
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::Api;
use tokio::sync::Semaphore;

/// Event reasons deckwatch publishes for plugins.
pub mod reasons {
    pub const APPLY_FAILED: &str = "PluginApplyFailed";
    pub const APPLY_WARNING: &str = "PluginApplyWarning";
    pub const PROVISION_FAILED: &str = "PluginProvisionFailed";
    pub const PROVISION_WARNING: &str = "PluginProvisionWarning";
    pub const PROVISIONED: &str = "PluginProvisioned";
    pub const DEPROVISION_FAILED: &str = "PluginDeprovisionFailed";
    pub const DEPROVISIONED: &str = "PluginDeprovisioned";
    pub const RECOVERED: &str = "PluginRecovered";
    pub const FIELDS_RELEASED: &str = "PluginFieldsReleased";
}

/// The same Event isn't republished more often than this.
pub const REPUBLISH_INTERVAL: Duration = Duration::from_secs(300);
/// Events being published at once; more are dropped.
pub const MAX_IN_FLIGHT: usize = 8;
/// The Events API's limit for `note`.
pub const MAX_NOTE_BYTES: usize = 1024;
/// The Events API's limit for `reason` and `action`.
const MAX_ACTION_BYTES: usize = 128;

/// A workload an Event is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkloadRef {
    pub api_version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    /// `kubectl describe` only shows Events whose `regarding` carries the
    /// object's UID; without it the sink looks the object up first.
    pub uid: Option<String>,
}

impl WorkloadRef {
    pub fn deployment(namespace: &str, name: &str, uid: Option<String>) -> Self {
        Self {
            api_version: "apps/v1".to_string(),
            kind: "Deployment".to_string(),
            namespace: namespace.to_string(),
            name: name.to_string(),
            uid,
        }
    }

    pub fn of_deployment(dep: &Deployment) -> Option<Self> {
        Some(Self::deployment(
            dep.metadata.namespace.as_deref()?,
            dep.metadata.name.as_deref()?,
            dep.metadata.uid.clone(),
        ))
    }

    fn object_reference(&self) -> ObjectReference {
        ObjectReference {
            api_version: Some(self.api_version.clone()),
            kind: Some(self.kind.clone()),
            namespace: Some(self.namespace.clone()),
            name: Some(self.name.clone()),
            uid: self.uid.clone(),
            ..Default::default()
        }
    }
}

/// One Event, before it's attached to its targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeEvent {
    pub warning: bool,
    /// One of [`reasons`].
    pub reason: &'static str,
    /// What was being done, plus the event code when there is one.
    pub action: String,
    /// Sanitized text for `kubectl describe`'s Message column.
    pub note: String,
}

/// An Event and what it's about. With no `targets`, it goes on every
/// Deployment of `application_id` (`"{ns}/{app}"`).
#[derive(Debug, Clone)]
pub struct KubeEventRequest {
    pub application_id: String,
    pub targets: Vec<WorkloadRef>,
    pub event: KubeEvent,
}

/// `export` as the PascalCase verb used in actions and reasons.
pub fn verb(export: &str) -> &'static str {
    match export {
        "provision" => "Provision",
        "deprovision" => "Deprovision",
        "fetch" => "Fetch",
        "metadata" => "Metadata",
        "validate" => "Validate",
        _ => "Apply",
    }
}

/// The action for an Event: the verb, and the event code if any.
pub fn action(verb: &str, code: Option<&str>) -> String {
    let action = match code {
        Some(code) if !code.is_empty() => format!("{verb} {code}"),
        _ => verb.to_string(),
    };
    crate::plugin_events::truncate(&action, MAX_ACTION_BYTES)
}

/// Cut a note to the Events API limit.
pub fn note(text: &str) -> String {
    crate::plugin_events::truncate(text, MAX_NOTE_BYTES)
}

/// `code: message` of the first event of `severity`, plus how many others
/// there are.
fn first(
    events: &[crate::plugin_events::PluginEventReport],
    severity: crate::plugins::EventSeverity,
) -> Option<(&str, String)> {
    let matching: Vec<_> = events
        .iter()
        .filter(|r| crate::plugin_events::effective_severity(r.event.severity) == severity)
        .collect();
    let e = &matching.first()?.event;
    let more = events.len() - 1;
    let more = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    Some((e.code.as_str(), format!("{}: {}{more}", e.code, e.message)))
}

/// The Event a status update calls for, if any. Plugin-wide rows (fetch and
/// load) have no workload to attach one to.
///
/// | Outcome | Event |
/// |---|---|
/// | failing | Warning `PluginApplyFailed` / `PluginProvisionFailed` / `PluginDeprovisionFailed` |
/// | back from failing | Normal `PluginRecovered` |
/// | degraded | Warning `PluginApplyWarning` / `PluginProvisionWarning` |
/// | an operator's provision or refresh worked | Normal `PluginProvisioned` |
pub fn event_for(
    update: &crate::plugin_status::StatusUpdate,
    transition: Option<crate::plugin_status::Transition>,
) -> Option<KubeEvent> {
    use crate::plugin_events::Health;
    use crate::plugins::EventSeverity;
    if update.key.application_id.is_empty() {
        return None;
    }
    let plugin = &update.key.plugin;
    let verb = verb(&update.export);
    let resource = if update.key.resource_id.is_empty() {
        String::new()
    } else {
        format!(" ({})", update.key.resource_id)
    };
    if transition == Some(crate::plugin_status::Transition::Recovered) {
        return Some(KubeEvent {
            warning: false,
            reason: reasons::RECOVERED,
            action: action(verb, None),
            note: note(&format!(
                "plugin {plugin}{resource}: {}() works again",
                update.export
            )),
        });
    }
    match update.health {
        Health::Failing => {
            let (code, text) = first(&update.events, EventSeverity::Error).unwrap_or((
                crate::plugin_events::codes::UNSPECIFIED,
                "failed".to_string(),
            ));
            Some(KubeEvent {
                warning: true,
                reason: match update.export.as_str() {
                    "provision" => reasons::PROVISION_FAILED,
                    "deprovision" => reasons::DEPROVISION_FAILED,
                    _ => reasons::APPLY_FAILED,
                },
                action: action(verb, Some(code)),
                note: note(&format!("plugin {plugin}{resource}: {text}")),
            })
        }
        Health::Degraded => {
            let (code, text) = first(&update.events, EventSeverity::Warning).unwrap_or((
                crate::plugin_events::codes::UNSPECIFIED,
                "warning".to_string(),
            ));
            Some(KubeEvent {
                warning: true,
                reason: match update.export.as_str() {
                    "provision" => reasons::PROVISION_WARNING,
                    _ => reasons::APPLY_WARNING,
                },
                action: action(verb, Some(code)),
                note: note(&format!("plugin {plugin}{resource}: {text}")),
            })
        }
        Health::Healthy
            if update.export == "provision"
                && matches!(update.trigger.as_str(), "provision" | "refresh") =>
        {
            Some(KubeEvent {
                warning: false,
                reason: reasons::PROVISIONED,
                action: action(verb, None),
                note: note(&format!("plugin {plugin} provisioned{resource}")),
            })
        }
        Health::Healthy => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ThrottleKey {
    target: WorkloadRef,
    reason: &'static str,
    action: String,
}

/// Publishes plugin Events. Cheap to clone.
#[derive(Clone)]
pub struct KubeEventSink {
    client: kube::Client,
    recorder: Recorder,
    last_published: Arc<Mutex<HashMap<ThrottleKey, Instant>>>,
    republish_after: Duration,
    in_flight: Arc<Semaphore>,
}

impl KubeEventSink {
    pub fn new(client: kube::Client) -> Self {
        Self::with_interval(client, REPUBLISH_INTERVAL)
    }

    /// A sink that republishes an unchanged Event after `republish_after`.
    pub fn with_interval(client: kube::Client, republish_after: Duration) -> Self {
        let reporter = match std::env::var("POD_NAME") {
            Ok(pod) if !pod.is_empty() => Reporter {
                controller: "deckwatch".to_string(),
                instance: Some(pod),
            },
            // Falls back to the hostname, which is the pod name in a pod.
            _ => Reporter::from("deckwatch"),
        };
        Self {
            recorder: Recorder::new(client.clone(), reporter),
            client,
            last_published: Arc::default(),
            republish_after,
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    /// Wait until every Event being published has been.
    pub async fn idle(&self) {
        let _ = self.in_flight.acquire_many(MAX_IN_FLIGHT as u32).await;
    }

    /// Publish in the background. Never waits: past [`MAX_IN_FLIGHT`]
    /// Events, the request is dropped and counted.
    pub fn spawn_publish(&self, request: KubeEventRequest) {
        let Ok(permit) = self.in_flight.clone().try_acquire_owned() else {
            crate::metrics::record_plugin_k8s_event("dropped");
            return;
        };
        let sink = self.clone();
        tokio::spawn(async move {
            sink.publish(request).await;
            drop(permit);
        });
    }

    /// Publish now, on every target that isn't throttled.
    pub async fn publish(&self, request: KubeEventRequest) {
        let targets = if request.targets.is_empty() {
            self.application_deployments(&request.application_id).await
        } else {
            request.targets
        };
        for target in targets {
            if !self.due(&target, &request.event) {
                crate::metrics::record_plugin_k8s_event("folded");
                continue;
            }
            let target = self.with_uid(target).await;
            let event = Event {
                type_: if request.event.warning {
                    EventType::Warning
                } else {
                    EventType::Normal
                },
                reason: request.event.reason.to_string(),
                note: Some(request.event.note.clone()),
                action: request.event.action.clone(),
                secondary: None,
            };
            match self
                .recorder
                .publish(&event, &target.object_reference())
                .await
            {
                Ok(()) => crate::metrics::record_plugin_k8s_event("published"),
                Err(e) => {
                    crate::metrics::record_plugin_k8s_event("error");
                    tracing::warn!(
                        namespace = %target.namespace,
                        name = %target.name,
                        reason = request.event.reason,
                        error = %e,
                        "failed to publish plugin Kubernetes Event"
                    );
                }
            }
        }
    }

    /// Whether this Event is due on `target`; records it as published if so.
    fn due(&self, target: &WorkloadRef, event: &KubeEvent) -> bool {
        let now = Instant::now();
        let Ok(mut last) = self.last_published.lock() else {
            return true;
        };
        // Forget what's long past, so the map stays small.
        last.retain(|_, at| now.duration_since(*at) < Duration::from_secs(3600));
        let key = ThrottleKey {
            target: WorkloadRef {
                uid: None,
                ..target.clone()
            },
            reason: event.reason,
            action: event.action.clone(),
        };
        match last.get(&key) {
            Some(at) if now.duration_since(*at) < self.republish_after => false,
            _ => {
                last.insert(key, now);
                true
            }
        }
    }

    /// Fill in a missing UID, so `kubectl describe` shows the Event.
    async fn with_uid(&self, mut target: WorkloadRef) -> WorkloadRef {
        if target.uid.is_none() && target.kind == "Deployment" {
            let api: Api<Deployment> = Api::namespaced(self.client.clone(), &target.namespace);
            if let Ok(dep) = api.get_metadata(&target.name).await {
                target.uid = dep.metadata.uid;
            }
        }
        target
    }

    async fn application_deployments(&self, application_id: &str) -> Vec<WorkloadRef> {
        let Some((ns, app)) = application_id.split_once('/') else {
            return Vec::new();
        };
        let api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);
        let lp = ListParams::default().labels(&format!("deckwatch.io/application={app}"));
        match api.list_metadata(&lp).await {
            Ok(list) => list
                .items
                .iter()
                .filter_map(|d| {
                    Some(WorkloadRef::deployment(
                        ns,
                        d.metadata.name.as_deref()?,
                        d.metadata.uid.clone(),
                    ))
                })
                .collect(),
            Err(e) => {
                tracing::warn!(application = %application_id, error = %e, "failed to list Deployments for plugin Kubernetes Events");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
#[path = "plugin_k8s_events_tests.rs"]
mod tests;
