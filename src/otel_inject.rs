//! OpenTelemetry env injection into application workloads: the
//! OpenTelemetry part of the application's Observability section (#398).
//!
//! When an application turns injection on, the reconciler ([`reconcile`],
//! run by the watcher every cycle) sets the standard `OTEL_*` variables on
//! the primary (first) container of every Deployment, StatefulSet and
//! CronJob labelled `deckwatch.io/application=<app>`, however the workload
//! was created or changed (REST API, MCP, YAML editor, kubectl, GitOps):
//!
//! * `OTEL_EXPORTER_OTLP_ENDPOINT`, the collector's OTLP/HTTP base URL, and
//!   `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`;
//! * `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, when a metrics endpoint is set;
//! * `OTEL_SERVICE_NAME`: the workload's name, or the application's
//!   override;
//! * `OTEL_RESOURCE_ATTRIBUTES`: `k8s.namespace.name`, `k8s.<kind>.name` and
//!   `deckwatch.application.name`, then the application's extra attributes;
//! * `OTEL_<SIGNAL>_EXPORTER=none` for each signal without an endpoint, so
//!   SDKs don't keep retrying exports nobody receives.
//!
//! **Endpoints** come from Settings → Observability only
//! (`observability.otlp_endpoint` and `otlp_metrics_endpoint`), never from
//! deckwatch's own `OTEL_EXPORTER_OTLP_*` variables, which win for
//! deckwatch's own export. Those variables can point at something only
//! deckwatch can reach (a sidecar, a host its NetworkPolicy allows, a CA in
//! its extra bundle), can carry credentials (exporter headers, a password in
//! the URL) and aren't validated. The settings endpoints are validated to
//! carry no credentials, and they're what the application's section shows.
//! Nothing secret is injected: no headers, no credentials, and every `$` is
//! escaped so Kubernetes can't expand a `$(NAME)` reference into a value.
//!
//! **The workload's own values win.** deckwatch sets a variable only if the
//! container's `env` doesn't already define it, and gives a variable up as
//! soon as someone else changes it, replaces it with a `valueFrom`, or
//! declares it too (another field manager's server-side apply, or a
//! client-side apply's last applied configuration). A workload that sets an
//! OTLP endpoint itself (`OTEL_EXPORTER_OTLP_[<SIGNAL>_]ENDPOINT`) keeps its
//! export setup: deckwatch then only sets `OTEL_SERVICE_NAME` and
//! `OTEL_RESOURCE_ATTRIBUTES`. Variables from `envFrom` aren't seen (that
//! would mean reading Secrets), and `env` wins over them.
//!
//! **What deckwatch set is recorded on the workload**, in the
//! [`RECORD_ANNOTATION`] annotation: the container, a hash of each value it
//! set, and the id of the database whose reconciler set them (the per-
//! database id the plugin field sweep uses, `watcher::plugin_fields_owner`).
//! A variable is deckwatch's while its value still matches the recorded
//! hash. Turning injection off, deleting the application or taking the
//! workload out of it removes exactly those variables and the annotation;
//! anything changed since stays. Only the database named in the record
//! removes them, so a database that doesn't have injection on for the
//! application, a fresh or reset one included, never strips them. One that
//! has it on takes another database's record over, once per workload: if
//! the record changes hands again, another deckwatch manages the workload
//! too, and it's left to that one rather than restarting its pods each
//! time they take turns.
//!
//! Server-side apply isn't used for the variables, unlike the plugin
//! fields: when a user changes the value of a variable an apply set (an
//! Update, such as `kubectl edit` or the YAML editor), the applier still
//! owns the list entry, and its next apply without that entry deletes it,
//! the user's value included. A strategic merge patch that sets and deletes
//! single entries, conditional on the object's `resourceVersion`, removes
//! only what is still deckwatch's.
//!
//! **Pods restart only when something changes.** The values are a pure
//! function of the settings and the workload, the patch is computed against
//! the live object and only sent when it changes something, and the
//! annotation sits on the workload, not on its pod template. The patch
//! fingerprint cache (`AppState::plugin_patch_fingerprints`) also keeps the
//! same patch from being sent again to an object that hasn't changed since
//! (an admission webhook that drops a variable would otherwise get a patch
//! every cycle).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::CronJob;
use k8s_openapi::api::core::v1::PodSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta};
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, Resource};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::entities::applications;
use crate::state::AppState;
use crate::telemetry::endpoints::{validate_endpoint, EndpointField, ObservabilitySettings};

/// Field manager of deckwatch's patches to the variables and the record.
pub const FIELD_MANAGER: &str = "deckwatch-observability";

/// Annotation recording what deckwatch set ([`Record`]). Not under
/// `deckwatch.io/`, whose annotations GitOps clean-up removes.
pub const RECORD_ANNOTATION: &str = "observability.deckwatch.io/otel-env";

/// Label that makes a workload a member of an application.
pub const APP_LABEL: &str = "deckwatch.io/application";

pub const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
pub const OTEL_EXPORTER_OTLP_METRICS_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT";
pub const OTEL_EXPORTER_OTLP_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_PROTOCOL";
pub const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
pub const OTEL_RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";

/// The protocol of every endpoint deckwatch injects (OTLP/HTTP).
pub const PROTOCOL: &str = crate::telemetry::PROTOCOL;

/// The longest service name override, in characters.
pub const MAX_SERVICE_NAME_CHARS: usize = 255;
/// The most extra resource attributes an application can add.
pub const MAX_RESOURCE_ATTRIBUTES: usize = 32;
/// The longest extra attribute key and value, in bytes.
const MAX_ATTRIBUTE_KEY_LEN: usize = 255;
const MAX_ATTRIBUTE_VALUE_LEN: usize = 1024;

/// Resource attributes deckwatch sets; extra attributes can't override them.
pub const MANAGED_ATTRIBUTES: &[&str] = &[
    "k8s.namespace.name",
    "k8s.deployment.name",
    "k8s.statefulset.name",
    "k8s.cronjob.name",
    "deckwatch.application.name",
];

// ---------------------------------------------------------------------------
// The application's settings
// ---------------------------------------------------------------------------

/// The application's Observability section as stored in
/// `applications.observability`. Prometheus monitoring isn't stored here:
/// it is the PodMonitor of each Deployment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationObservability {
    #[serde(default)]
    pub opentelemetry: OtelInjection,
}

impl ApplicationObservability {
    /// The stored column; NULL is the defaults (injection off).
    pub fn from_column(raw: Option<&str>) -> Result<ApplicationObservability, String> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(ApplicationObservability::default()),
            Some(raw) => serde_json::from_str(raw)
                .map_err(|e| format!("the stored observability settings are not valid: {e}")),
        }
    }

    /// The value to store: NULL for the defaults.
    pub fn to_column(&self) -> Option<String> {
        (*self != ApplicationObservability::default())
            .then(|| serde_json::to_string(self).expect("observability settings serialize"))
    }
}

/// OpenTelemetry injection for one application.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtelInjection {
    /// Inject the variables. Off by default.
    #[serde(default)]
    pub enabled: bool,
    /// `OTEL_SERVICE_NAME` for every workload of the application; unset,
    /// each workload's own name.
    #[serde(default)]
    pub service_name: Option<String>,
    /// Added to `OTEL_RESOURCE_ATTRIBUTES` after the attributes deckwatch
    /// sets, in this order.
    #[serde(default)]
    pub resource_attributes: Vec<ResourceAttribute>,
}

/// One extra resource attribute. The value goes into
/// `OTEL_RESOURCE_ATTRIBUTES` as is, so it must already be percent-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceAttribute {
    pub key: String,
    pub value: String,
}

impl OtelInjection {
    /// These settings validated and normalized: surrounding whitespace
    /// trimmed, and an empty service name unset. Every problem is reported,
    /// not just the first.
    pub fn validated(&self) -> Result<OtelInjection, String> {
        let service_name = validate_service_name(self.service_name.as_deref().unwrap_or(""));
        let attributes = validate_resource_attributes(&self.resource_attributes);
        match (service_name, attributes) {
            (Ok(service_name), Ok(resource_attributes)) => Ok(OtelInjection {
                enabled: self.enabled,
                service_name,
                resource_attributes,
            }),
            (service_name, attributes) => {
                let mut problems: Vec<String> = Vec::new();
                problems.extend(service_name.err());
                problems.extend(attributes.err().into_iter().flatten());
                Err(problems.join("; "))
            }
        }
    }
}

/// The service name override, trimmed; `None` when empty. At most
/// [`MAX_SERVICE_NAME_CHARS`] characters, no control characters.
pub fn validate_service_name(raw: &str) -> Result<Option<String>, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_SERVICE_NAME_CHARS {
        return Err(format!(
            "service_name: at most {MAX_SERVICE_NAME_CHARS} characters"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("service_name: must not contain control characters".to_string());
    }
    Ok(Some(name.to_string()))
}

/// A resource attribute key: letters, digits, `.`, `_` and `-`.
pub fn is_attribute_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_ATTRIBUTE_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A W3C baggage value as `OTEL_RESOURCE_ATTRIBUTES` takes it: baggage-octets
/// only (printable ASCII except space, `"`, `,`, `;` and `\`), so anything
/// else is percent-encoded, and every `%` starts a valid escape.
pub fn is_encoded_attribute_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_ATTRIBUTE_VALUE_LEN {
        return false;
    }
    let baggage_octets = bytes
        .iter()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b',' | b';' | b'\\'));
    let escapes_valid = bytes.iter().enumerate().all(|(i, b)| {
        *b != b'%'
            || bytes
                .get(i + 1..i + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
    });
    baggage_octets && escapes_valid
}

/// The extra resource attributes, trimmed and checked: valid keys, encoded
/// values, no repeated key, none of [`MANAGED_ATTRIBUTES`] and no
/// `service.name` (that's the service name setting). Returns every problem.
pub fn validate_resource_attributes(
    attributes: &[ResourceAttribute],
) -> Result<Vec<ResourceAttribute>, Vec<String>> {
    let mut problems = Vec::new();
    if attributes.len() > MAX_RESOURCE_ATTRIBUTES {
        problems.push(format!(
            "resource_attributes: at most {MAX_RESOURCE_ATTRIBUTES} attributes"
        ));
    }
    let mut out: Vec<ResourceAttribute> = Vec::with_capacity(attributes.len());
    for attribute in attributes {
        let key = attribute.key.trim();
        let value = attribute.value.trim();
        if !is_attribute_key(key) {
            problems.push(format!(
                "resource_attributes: the key {key:?} must be 1 to {MAX_ATTRIBUTE_KEY_LEN} \
                 letters, digits, '.', '_' or '-'"
            ));
            continue;
        }
        if MANAGED_ATTRIBUTES.contains(&key) {
            problems.push(format!(
                "resource_attributes: {key} is set by deckwatch and can't be overridden"
            ));
            continue;
        }
        if key == "service.name" {
            problems.push(
                "resource_attributes: set service.name with the service name, not as an \
                 attribute (OTEL_SERVICE_NAME wins over it)"
                    .to_string(),
            );
            continue;
        }
        if !is_encoded_attribute_value(value) {
            problems.push(format!(
                "resource_attributes: the value of {key} must be 1 to \
                 {MAX_ATTRIBUTE_VALUE_LEN} characters and percent-encoded: no spaces, quotes, \
                 commas, semicolons, backslashes or non-ASCII characters, and every % starts \
                 an escape such as %20"
            ));
            continue;
        }
        if out.iter().any(|a| a.key == key) {
            problems.push(format!("resource_attributes: {key} is repeated"));
            continue;
        }
        out.push(ResourceAttribute {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

// ---------------------------------------------------------------------------
// What a workload gets
// ---------------------------------------------------------------------------

/// Where application workloads send their telemetry: the endpoints in
/// Settings → Observability (see the module docs for why not deckwatch's
/// own variables).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppEndpoints {
    /// OTLP/HTTP base URL for traces and logs.
    pub otlp: Option<String>,
    /// Full URL for metrics, used as is.
    pub metrics: Option<String>,
}

impl AppEndpoints {
    /// The endpoints in `settings`, validated again (one stored some other
    /// way might not be): an invalid one is left out, with its problem in
    /// the second value.
    pub fn from_settings(settings: &ObservabilitySettings) -> (AppEndpoints, Vec<String>) {
        let mut problems = Vec::new();
        let mut endpoint =
            |field: EndpointField| match validate_endpoint(field, settings.get(field)) {
                Ok(value) => (!value.is_empty()).then_some(value),
                Err(e) => {
                    problems.push(e);
                    None
                }
            };
        let endpoints = AppEndpoints {
            otlp: endpoint(EndpointField::Otlp),
            metrics: endpoint(EndpointField::OtlpMetrics),
        };
        (endpoints, problems)
    }

    /// No endpoint at all: nothing is injected.
    pub fn is_empty(&self) -> bool {
        self.otlp.is_none() && self.metrics.is_none()
    }
}

/// The kinds of workload that get the variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    CronJob,
}

impl WorkloadKind {
    /// `Deployment`, `StatefulSet` or `CronJob`.
    pub fn name(self) -> &'static str {
        match self {
            WorkloadKind::Deployment => "Deployment",
            WorkloadKind::StatefulSet => "StatefulSet",
            WorkloadKind::CronJob => "CronJob",
        }
    }

    /// The semantic-convention resource attribute for the workload's name.
    pub fn name_attribute(self) -> &'static str {
        match self {
            WorkloadKind::Deployment => "k8s.deployment.name",
            WorkloadKind::StatefulSet => "k8s.statefulset.name",
            WorkloadKind::CronJob => "k8s.cronjob.name",
        }
    }

    /// The fields from the object to its pod spec.
    fn pod_spec_path(self) -> &'static [&'static str] {
        match self {
            WorkloadKind::Deployment | WorkloadKind::StatefulSet => &["spec", "template", "spec"],
            WorkloadKind::CronJob => &["spec", "jobTemplate", "spec", "template", "spec"],
        }
    }
}

/// The workload the variables are for.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    pub namespace: &'a str,
    pub kind: WorkloadKind,
    pub name: &'a str,
    pub application: &'a str,
}

/// The variables that send telemetry somewhere. A workload that sets one
/// itself has its own export setup ([`plan`]).
const ENDPOINT_VARS: [&str; 4] = [
    OTEL_EXPORTER_OTLP_ENDPOINT,
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
];

/// What deckwatch still sets on a workload with its own export setup.
const IDENTITY_VARS: [&str; 2] = [OTEL_SERVICE_NAME, OTEL_RESOURCE_ATTRIBUTES];

/// Every variable deckwatch would set on `target` with these endpoints and
/// settings, before the workload's own values are taken into account
/// ([`plan`]). Empty when there is no endpoint at all. Deterministic: the
/// same input always gives the same variables and values.
///
/// Every `$` is written `$$`, so Kubernetes doesn't expand a `$(NAME)` in a
/// value: an attribute such as `x=$(DB_PASSWORD)` would otherwise send
/// another variable's value, a Secret's for instance, to the telemetry
/// backend. The application sees the value as it was entered.
pub fn desired_env(
    endpoints: &AppEndpoints,
    config: &OtelInjection,
    target: &Target<'_>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if endpoints.is_empty() {
        return env;
    }
    if let Some(base) = &endpoints.otlp {
        env.insert(OTEL_EXPORTER_OTLP_ENDPOINT.to_string(), base.clone());
    }
    if let Some(metrics) = &endpoints.metrics {
        env.insert(
            OTEL_EXPORTER_OTLP_METRICS_ENDPOINT.to_string(),
            metrics.clone(),
        );
    }
    env.insert(
        OTEL_EXPORTER_OTLP_PROTOCOL.to_string(),
        PROTOCOL.to_string(),
    );
    env.insert(
        OTEL_SERVICE_NAME.to_string(),
        config
            .service_name
            .clone()
            .unwrap_or_else(|| target.name.to_string()),
    );
    let mut attributes = vec![
        format!("k8s.namespace.name={}", percent_encode(target.namespace)),
        format!(
            "{}={}",
            target.kind.name_attribute(),
            percent_encode(target.name)
        ),
        format!(
            "deckwatch.application.name={}",
            percent_encode(target.application)
        ),
    ];
    attributes.extend(
        config
            .resource_attributes
            .iter()
            .map(|a| format!("{}={}", a.key, a.value)),
    );
    env.insert(OTEL_RESOURCE_ATTRIBUTES.to_string(), attributes.join(","));
    for (exporter, has_endpoint) in [
        ("OTEL_TRACES_EXPORTER", endpoints.otlp.is_some()),
        ("OTEL_METRICS_EXPORTER", endpoints.metrics.is_some()),
        ("OTEL_LOGS_EXPORTER", endpoints.otlp.is_some()),
    ] {
        if !has_endpoint {
            env.insert(exporter.to_string(), "none".to_string());
        }
    }
    env.into_iter()
        .map(|(name, value)| (name, value.replace('$', "$$")))
        .collect()
}

/// Percent-encode everything but RFC 3986 unreserved characters, as the
/// OpenTelemetry spec asks of `OTEL_RESOURCE_ATTRIBUTES` values. Kubernetes
/// names only contain unreserved characters, so this is a safeguard.
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

// ---------------------------------------------------------------------------
// What deckwatch set: the record
// ---------------------------------------------------------------------------

/// The value of [`RECORD_ANNOTATION`]: what deckwatch set on the workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The id of the database whose reconciler set the variables.
    pub owner: String,
    /// The container they're on.
    pub container: String,
    /// Each variable's name and [`value_hash`].
    pub env: BTreeMap<String, String>,
}

impl Record {
    /// The annotation's record, or `None` if it isn't one.
    pub fn parse(raw: &str) -> Option<Record> {
        serde_json::from_str(raw).ok()
    }

    /// The annotation value: compact JSON with sorted keys, so the same
    /// record always encodes the same way.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("record serializes")
    }
}

/// A short hash of a variable's value, enough to tell whether it is still
/// the value deckwatch set. Not a secret: the values aren't either.
pub fn value_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// The live workload
// ---------------------------------------------------------------------------

/// What [`plan`] needs to know about a workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWorkload {
    pub kind: WorkloadKind,
    pub namespace: String,
    pub name: String,
    pub resource_version: Option<String>,
    /// The `deckwatch.io/application` label.
    pub application: Option<String>,
    /// The raw [`RECORD_ANNOTATION`].
    pub record: Option<String>,
    /// The pod template's containers, in order: the first is the primary.
    pub containers: Vec<LiveContainer>,
    /// Per container, the variables someone else declares: another field
    /// manager's server-side apply, or a client-side apply's
    /// `kubectl.kubernetes.io/last-applied-configuration`. Those are theirs,
    /// even with deckwatch's value.
    pub applied_by_others: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveContainer {
    pub name: String,
    pub env: Vec<LiveEnv>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEnv {
    pub name: String,
    pub value: Option<String>,
    /// Set from a Secret, ConfigMap or field reference.
    pub value_from: bool,
}

impl LiveWorkload {
    fn new(kind: WorkloadKind, meta: &ObjectMeta, pod_spec: Option<&PodSpec>) -> Option<Self> {
        let containers: Vec<LiveContainer> = pod_spec
            .map(|spec| {
                spec.containers
                    .iter()
                    .map(|c| LiveContainer {
                        name: c.name.clone(),
                        env: c
                            .env
                            .iter()
                            .flatten()
                            .map(|e| LiveEnv {
                                name: e.name.clone(),
                                value: e.value.clone(),
                                value_from: e.value_from.is_some(),
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let last_applied = meta
            .annotations
            .as_ref()
            .and_then(|a| a.get(LAST_APPLIED_ANNOTATION))
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        let applied_by_others = containers
            .iter()
            .map(|c| {
                let mut names = applied_env(meta.managed_fields.as_deref(), kind, &c.name);
                names.extend(last_applied_env(last_applied.as_ref(), kind, &c.name));
                (c.name.clone(), names)
            })
            .filter(|(_, names)| !names.is_empty())
            .collect();
        Some(LiveWorkload {
            kind,
            namespace: meta.namespace.clone()?,
            name: meta.name.clone()?,
            resource_version: meta.resource_version.clone(),
            application: meta.labels.as_ref().and_then(|l| l.get(APP_LABEL)).cloned(),
            record: meta
                .annotations
                .as_ref()
                .and_then(|a| a.get(RECORD_ANNOTATION))
                .cloned(),
            containers,
            applied_by_others,
        })
    }

    pub fn from_deployment(d: &Deployment) -> Option<Self> {
        let pod_spec = d.spec.as_ref().and_then(|s| s.template.spec.as_ref());
        Self::new(WorkloadKind::Deployment, &d.metadata, pod_spec)
    }

    pub fn from_statefulset(s: &StatefulSet) -> Option<Self> {
        let pod_spec = s.spec.as_ref().and_then(|s| s.template.spec.as_ref());
        Self::new(WorkloadKind::StatefulSet, &s.metadata, pod_spec)
    }

    pub fn from_cronjob(c: &CronJob) -> Option<Self> {
        let pod_spec = c
            .spec
            .as_ref()
            .and_then(|s| s.job_template.spec.as_ref())
            .and_then(|j| j.template.spec.as_ref());
        Self::new(WorkloadKind::CronJob, &c.metadata, pod_spec)
    }

    /// The primary container: the first one.
    pub fn primary(&self) -> Option<&LiveContainer> {
        self.containers.first()
    }

    fn container(&self, name: &str) -> Option<&LiveContainer> {
        self.containers.iter().find(|c| c.name == name)
    }

    /// The variables to set on this workload when its application is
    /// `application` ([`desired_env`]).
    pub fn desired(
        &self,
        endpoints: &AppEndpoints,
        config: &OtelInjection,
        application: &str,
    ) -> BTreeMap<String, String> {
        desired_env(
            endpoints,
            config,
            &Target {
                namespace: &self.namespace,
                kind: self.kind,
                name: &self.name,
                application,
            },
        )
    }

    /// `<kind>/<namespace>/<name>`, unique per workload.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.kind.name(), self.namespace, self.name)
    }

    /// The record is this database's.
    fn has_record_of(&self, owner: &str) -> bool {
        self.record
            .as_deref()
            .and_then(Record::parse)
            .is_some_and(|r| r.owner == owner)
    }
}

/// The env variables of `container` that a field manager other than
/// [`FIELD_MANAGER`] has applied with server-side apply, from `managedFields`.
fn applied_env(
    entries: Option<&[ManagedFieldsEntry]>,
    kind: WorkloadKind,
    container: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in entries.into_iter().flatten() {
        if entry.operation.as_deref() != Some("Apply")
            || entry.manager.as_deref() == Some(FIELD_MANAGER)
        {
            continue;
        }
        let Some(fields) = entry.fields_v1.as_ref() else {
            continue;
        };
        let containers = kind
            .pod_spec_path()
            .iter()
            .try_fold(&fields.0, |node, field| node.get(format!("f:{field}")))
            .and_then(|pod_spec| pod_spec.get("f:containers"))
            .and_then(|containers| containers.as_object());
        let Some(containers) = containers else {
            continue;
        };
        for (key, fields) in containers {
            if list_key_name(key).as_deref() != Some(container) {
                continue;
            }
            let Some(env) = fields.get("f:env").and_then(|e| e.as_object()) else {
                continue;
            };
            names.extend(env.keys().filter_map(|k| list_key_name(k)));
        }
    }
    names
}

/// The annotation `kubectl apply` (client-side) keeps its last applied
/// object in.
const LAST_APPLIED_ANNOTATION: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// The env variables of `container` in a client-side apply's last applied
/// object.
fn last_applied_env(
    last_applied: Option<&serde_json::Value>,
    kind: WorkloadKind,
    container: &str,
) -> BTreeSet<String> {
    let containers = last_applied
        .and_then(|object| {
            kind.pod_spec_path()
                .iter()
                .try_fold(object, |node, field| node.get(*field))
        })
        .and_then(|pod_spec| pod_spec.get("containers"))
        .and_then(|containers| containers.as_array());
    containers
        .into_iter()
        .flatten()
        .filter(|c| c.get("name").and_then(|n| n.as_str()) == Some(container))
        .filter_map(|c| c.get("env").and_then(|e| e.as_array()))
        .flatten()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect()
}

/// The `name` in a `managedFields` list key such as `k:{"name":"web"}`.
fn list_key_name(key: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(key.strip_prefix("k:")?).ok()?;
    json.get("name")?.as_str().map(str::to_string)
}

// ---------------------------------------------------------------------------
// The plan for one workload
// ---------------------------------------------------------------------------

/// What to change on one workload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvPlan {
    /// The primary container's name.
    pub primary: String,
    /// Variables to add to the primary container, or whose value to change.
    pub set: BTreeMap<String, String>,
    /// Variables to remove, per container: deckwatch's, no longer wanted
    /// (or on a container that is no longer the primary).
    pub remove: BTreeMap<String, BTreeSet<String>>,
    /// The record afterwards; `None` removes the annotation.
    pub record: Option<Record>,
    /// The record changes.
    pub record_changed: bool,
    /// The variables deckwatch has set on the workload now.
    pub owned: BTreeSet<String>,
    /// Variables deckwatch would set but leaves to the workload, which sets
    /// them itself.
    pub kept: BTreeSet<String>,
    /// The workload sets an OTLP endpoint itself, so deckwatch only sets its
    /// service name and resource attributes.
    pub exports_itself: bool,
}

impl EnvPlan {
    /// Nothing to change.
    pub fn is_noop(&self) -> bool {
        self.set.is_empty() && self.remove.is_empty() && !self.record_changed
    }
}

/// What to change on `live` so it carries `desired` (empty: nothing), minus
/// what the workload sets itself (see the module docs).
///
/// `adopt`: treat a record another database left as this one's. The
/// reconciler adopts for applications with injection on, and doesn't when
/// it removes leftovers. `None`: the workload has another database's record
/// and `adopt` is false, so leave it alone.
pub fn plan(
    live: &LiveWorkload,
    desired: &BTreeMap<String, String>,
    owner: &str,
    adopt: bool,
) -> Option<EnvPlan> {
    let record = live.record.as_deref().and_then(Record::parse);
    if record.as_ref().is_some_and(|r| r.owner != owner && !adopt) {
        return None;
    }
    let Some(primary) = live.primary() else {
        // No container to set anything on (and none to remove from).
        return Some(EnvPlan::default());
    };

    // Deckwatch's variables: recorded, still there once with the recorded
    // value (not a valueFrom), and not applied by another field manager.
    let mut owned: BTreeMap<String, String> = BTreeMap::new();
    let mut owned_container: Option<&str> = None;
    if let Some(record) = &record {
        // A recorded container that no longer exists was most likely
        // renamed: look for the variables on the primary.
        let container = live.container(&record.container).unwrap_or(primary);
        let applied_by_others = live.applied_by_others.get(&container.name);
        for (name, hash) in &record.env {
            let mut entries = container.env.iter().filter(|e| &e.name == name);
            let (Some(entry), None) = (entries.next(), entries.next()) else {
                continue;
            };
            let Some(value) = entry.value.as_deref().filter(|_| !entry.value_from) else {
                continue;
            };
            if value_hash(value) == *hash
                && !applied_by_others.is_some_and(|names| names.contains(name))
            {
                owned.insert(name.clone(), value.to_string());
            }
        }
        owned_container = Some(container.name.as_str());
    }
    let on_primary = owned_container == Some(primary.name.as_str());

    // The workload's own variables, on the primary container.
    let own: BTreeSet<&str> = primary
        .env
        .iter()
        .map(|e| e.name.as_str())
        .filter(|name| !(on_primary && owned.contains_key(*name)))
        .collect();
    // A workload that sends its telemetry somewhere itself keeps its whole
    // export setup: deckwatch's protocol, metrics endpoint or
    // `OTEL_<SIGNAL>_EXPORTER=none` would change where its signals go.
    let exports_itself = ENDPOINT_VARS.iter().any(|var| own.contains(var));
    let mut kept = BTreeSet::new();
    let mut wanted: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in desired {
        if own.contains(name.as_str()) {
            kept.insert(name.clone());
        } else if !exports_itself || IDENTITY_VARS.contains(&name.as_str()) {
            wanted.insert(name.clone(), value.clone());
        }
    }

    let mut set = BTreeMap::new();
    let mut remove: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, value) in &wanted {
        let current = on_primary.then(|| owned.get(name)).flatten();
        if current != Some(value) {
            set.insert(name.clone(), value.clone());
        }
    }
    if let Some(container) = owned_container {
        for name in owned.keys() {
            if !(on_primary && wanted.contains_key(name)) {
                remove
                    .entry(container.to_string())
                    .or_default()
                    .insert(name.clone());
            }
        }
    }

    let new_record = (!wanted.is_empty()).then(|| Record {
        owner: owner.to_string(),
        container: primary.name.clone(),
        env: wanted
            .iter()
            .map(|(name, value)| (name.clone(), value_hash(value)))
            .collect(),
    });
    let record_changed =
        live.record.as_deref() != new_record.as_ref().map(Record::encode).as_deref();
    Some(EnvPlan {
        primary: primary.name.clone(),
        set,
        remove,
        record: new_record,
        record_changed,
        owned: owned.into_keys().collect(),
        kept,
        exports_itself,
    })
}

/// The strategic merge patch that carries out `plan` on `live`, without the
/// `resourceVersion` precondition [`send`] adds. Deterministic.
///
/// Each changed env list also gets a `$setElementOrder/env` directive: the
/// existing entries in their order, then the new ones. Without it a
/// strategic merge puts new entries first, ahead of the workload's own. A
/// list with a repeated name gets no directive (its order can't be stated
/// by name).
pub fn patch_body(live: &LiveWorkload, plan: &EnvPlan) -> serde_json::Value {
    let mut containers: Vec<serde_json::Value> = Vec::new();
    // Every container named here exists on the live object: a strategic
    // merge patch would add a missing one.
    for container in &live.containers {
        let set = if container.name == plan.primary {
            Some(&plan.set).filter(|set| !set.is_empty())
        } else {
            None
        };
        let removed = plan.remove.get(&container.name);
        if set.is_none() && removed.is_none() {
            continue;
        }
        let existing: Vec<&str> = container.env.iter().map(|e| e.name.as_str()).collect();
        let unique: BTreeSet<&str> = existing.iter().copied().collect();
        // The values to set, in the order the list ends up in: entries that
        // exist where they are, then the new ones. The API server refuses a
        // patch whose entries are in another order than its
        // `$setElementOrder` directive.
        let mut set_order: Vec<&str> = existing
            .iter()
            .copied()
            .filter(|name| set.is_some_and(|set| set.contains_key(*name)))
            .collect();
        set_order.extend(
            set.into_iter()
                .flatten()
                .map(|(name, _)| name.as_str())
                .filter(|name| !unique.contains(name)),
        );
        let mut env: Vec<serde_json::Value> = set_order
            .iter()
            .filter_map(|name| {
                let value = set?.get(*name)?;
                Some(serde_json::json!({ "name": name, "value": value }))
            })
            .collect();
        env.extend(
            removed
                .into_iter()
                .flatten()
                .map(|name| serde_json::json!({ "name": name, "$patch": "delete" })),
        );
        let mut patch = serde_json::json!({ "name": container.name });
        if unique.len() == existing.len() {
            let mut order: Vec<&str> = existing
                .into_iter()
                .filter(|name| !removed.is_some_and(|r| r.contains(*name)))
                .collect();
            order.extend(
                set_order
                    .iter()
                    .copied()
                    .filter(|name| !unique.contains(name)),
            );
            let order: Vec<serde_json::Value> = order
                .into_iter()
                .map(|name| serde_json::json!({ "name": name }))
                .collect();
            patch["$setElementOrder/env"] = serde_json::json!(order);
        }
        patch["env"] = serde_json::json!(env);
        containers.push(patch);
    }
    let mut body = serde_json::json!({
        "metadata": {
            "annotations": {
                RECORD_ANNOTATION: plan.record.as_ref().map(Record::encode),
            },
        },
    });
    if !containers.is_empty() {
        // Nest the containers at the kind's pod spec path, then add the
        // outermost field (`spec`) next to `metadata`.
        let mut node = serde_json::json!({ "containers": containers });
        for field in live.kind.pod_spec_path().iter().rev() {
            let mut wrapper = serde_json::Map::new();
            wrapper.insert((*field).to_string(), node);
            node = serde_json::Value::Object(wrapper);
        }
        if let (Some(body), serde_json::Value::Object(spec)) = (body.as_object_mut(), node) {
            body.extend(spec);
        }
    }
    body
}

// ---------------------------------------------------------------------------
// Status, for the application's Observability section
// ---------------------------------------------------------------------------

/// Where a workload's injection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionState {
    /// It carries what it should and deckwatch has set variables on it.
    Injected,
    /// It doesn't carry what it should yet; the reconciler patches it within
    /// a poll interval.
    Pending,
    /// deckwatch has set nothing on it, and shouldn't.
    Off,
    /// Another deckwatch database set its variables. The reconciler of an
    /// application with injection on takes them over once.
    Foreign,
}

/// One workload in the application's Observability section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkloadInjection {
    pub kind: WorkloadKind,
    pub name: String,
    /// The primary container.
    pub container: Option<String>,
    pub state: InjectionState,
    /// The variables deckwatch has set on it.
    pub injected: Vec<String>,
    /// Variables deckwatch leaves to the workload, which sets them itself.
    pub kept: Vec<String>,
    /// The workload sets an OTLP endpoint itself, so deckwatch only sets its
    /// service name and resource attributes.
    pub exports_itself: bool,
}

/// The status of `live` in an application with these settings.
pub fn workload_status(
    live: &LiveWorkload,
    endpoints: &AppEndpoints,
    config: &OtelInjection,
    application: &str,
    owner: &str,
) -> WorkloadInjection {
    let desired = if config.enabled {
        live.desired(endpoints, config, application)
    } else {
        BTreeMap::new()
    };
    let foreign = live
        .record
        .as_deref()
        .and_then(Record::parse)
        .is_some_and(|r| r.owner != owner);
    let plan = plan(live, &desired, owner, config.enabled).unwrap_or_default();
    let state = if foreign {
        InjectionState::Foreign
    } else if !plan.is_noop() {
        InjectionState::Pending
    } else if plan.owned.is_empty() {
        InjectionState::Off
    } else {
        InjectionState::Injected
    };
    WorkloadInjection {
        kind: live.kind,
        name: live.name.clone(),
        container: live.primary().map(|c| c.name.clone()),
        state,
        injected: if foreign {
            Vec::new()
        } else {
            plan.owned.into_iter().collect()
        },
        kept: plan.kept.into_iter().collect(),
        exports_itself: plan.exports_itself,
    }
}

// ---------------------------------------------------------------------------
// The reconciler
// ---------------------------------------------------------------------------

/// The workload kinds as Kubernetes resources.
trait Workload:
    Resource<Scope = k8s_openapi::NamespaceResourceScope, DynamicType = ()>
    + Clone
    + serde::de::DeserializeOwned
    + std::fmt::Debug
    + Send
    + Sync
    + 'static
{
    const KIND: WorkloadKind;
    fn live(&self) -> Option<LiveWorkload>;
}

impl Workload for Deployment {
    const KIND: WorkloadKind = WorkloadKind::Deployment;
    fn live(&self) -> Option<LiveWorkload> {
        LiveWorkload::from_deployment(self)
    }
}

impl Workload for StatefulSet {
    const KIND: WorkloadKind = WorkloadKind::StatefulSet;
    fn live(&self) -> Option<LiveWorkload> {
        LiveWorkload::from_statefulset(self)
    }
}

impl Workload for CronJob {
    const KIND: WorkloadKind = WorkloadKind::CronJob;
    fn live(&self) -> Option<LiveWorkload> {
        LiveWorkload::from_cronjob(self)
    }
}

/// How long to stop listing StatefulSets after the API server refused
/// (deckwatch's RBAC predates StatefulSet injection).
const FORBIDDEN_BACKOFF: Duration = Duration::from_secs(600);

/// Cross-cycle state of [`reconcile`].
#[derive(Debug, Default)]
pub(crate) struct Sweep {
    /// Workloads that carried this database's record after the previous
    /// cycle; `None` until the first cycle.
    injected: Option<BTreeSet<String>>,
    /// The previous sweep failed part-way, so sweep again.
    retry: bool,
    /// This database's owner id, once read.
    owner: Option<String>,
    /// StatefulSets aren't listed until then: deckwatch may not.
    statefulsets_forbidden_until: Option<Instant>,
    /// The invalid endpoints last logged.
    endpoint_problems: Vec<String>,
    /// Workloads whose record another database left, and this process took
    /// over. If the other database's record comes back, another deckwatch
    /// manages them too: taking turns would restart their pods every cycle,
    /// so they're left to it.
    adopted: BTreeSet<String>,
    /// Those left to another deckwatch, already logged.
    left_alone: BTreeSet<String>,
}

impl Sweep {
    /// Sweep on the first cycle after startup (catches what changed while
    /// deckwatch was down), whenever a workload that carried a record isn't
    /// in an application with injection on anymore (turned off, application
    /// deleted, workload taken out of it or deleted), and after a sweep that
    /// failed part-way.
    fn needed(&self, seen: &BTreeSet<String>) -> bool {
        self.retry
            || self
                .injected
                .as_ref()
                .is_none_or(|previous| !previous.is_subset(seen))
    }

    fn skips(&self, kind: WorkloadKind) -> bool {
        kind == WorkloadKind::StatefulSet
            && self
                .statefulsets_forbidden_until
                .is_some_and(|until| Instant::now() < until)
    }

    /// The API server refused to list `kind`. Only StatefulSets are skipped
    /// for a while: the chart has always granted the others.
    fn forbidden(&mut self, kind: WorkloadKind, error: &kube::Error) -> bool {
        if kind != WorkloadKind::StatefulSet || !is_status(error, 403) {
            return false;
        }
        tracing::warn!(
            retry_in_seconds = FORBIDDEN_BACKOFF.as_secs(),
            "otel injection: deckwatch may not list StatefulSets, so it can't inject into \
             them; grant its ClusterRole get, list and patch on apps/statefulsets"
        );
        self.statefulsets_forbidden_until = Some(Instant::now() + FORBIDDEN_BACKOFF);
        true
    }
}

fn is_status(error: &kube::Error, code: u16) -> bool {
    matches!(error, kube::Error::Api(e) if e.code == code)
}

/// A label value, as Kubernetes accepts it (so it can go in a selector).
fn is_label_value(value: &str) -> bool {
    value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .last()
            .is_some_and(|b| b.is_ascii_alphanumeric())
}

/// Applications and their injection settings, from the database.
struct Applications {
    /// `<ns>/<name>` of each application with injection on.
    enabled: BTreeMap<String, OtelInjection>,
    /// Applications whose stored settings don't parse or validate: neither
    /// patched nor swept until they're fixed.
    unreadable: BTreeSet<String>,
}

async fn load_applications(state: &AppState) -> Result<Applications, sea_orm::DbErr> {
    let rows = applications::Entity::find().all(&state.db).await?;
    let mut apps = Applications {
        enabled: BTreeMap::new(),
        unreadable: BTreeSet::new(),
    };
    for row in rows {
        // Settings are only validated for applications with injection on:
        // one that's off has nothing to inject, and its leftovers are swept.
        let config =
            ApplicationObservability::from_column(row.observability.as_deref()).and_then(|o| {
                if o.opentelemetry.enabled {
                    o.opentelemetry.validated().map(Some)
                } else {
                    Ok(None)
                }
            });
        match config {
            Ok(Some(config)) => {
                apps.enabled.insert(row.id, config);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    application = %row.id,
                    error = %e,
                    "otel injection: the application's stored settings are invalid; leaving its workloads alone"
                );
                apps.unreadable.insert(row.id);
            }
        }
    }
    Ok(apps)
}

/// One reconcile cycle: patch the workloads of every application with
/// injection on, then, when needed ([`Sweep::needed`]), remove this
/// database's variables from workloads no longer in such an application.
///
/// A database error stops the cycle before anything is patched or removed,
/// so a transient error never strips variables.
pub(crate) async fn reconcile(state: &AppState, sweep: &mut Sweep) {
    let owner = match &sweep.owner {
        Some(owner) => owner.clone(),
        None => match crate::watcher::plugin_fields_owner(&state.db).await {
            Ok(owner) => sweep.owner.insert(owner).clone(),
            Err(e) => {
                tracing::warn!(error = %e, "otel injection: failed to read the database's owner id");
                return;
            }
        },
    };
    let apps = match load_applications(state).await {
        Ok(apps) => apps,
        Err(e) => {
            tracing::warn!(error = %e, "otel injection: failed to read the applications");
            return;
        }
    };
    let settings = match crate::handlers::settings::load_observability_strict(state).await {
        Ok(settings) => settings,
        Err(e) => {
            tracing::warn!(error = %e, "otel injection: failed to read the OTLP endpoints");
            return;
        }
    };
    let (endpoints, problems) = AppEndpoints::from_settings(&settings);
    // Logged when they change, not every cycle.
    if !apps.enabled.is_empty() && problems != sweep.endpoint_problems {
        for problem in &problems {
            tracing::warn!(
                problem = %problem,
                "otel injection: an OTLP endpoint in the settings is invalid and isn't injected"
            );
        }
    }
    sweep.endpoint_problems = problems;

    // Applications with injection on, by namespace: one LIST per kind and
    // namespace.
    let mut by_namespace: BTreeMap<&str, BTreeMap<&str, &OtelInjection>> = BTreeMap::new();
    for (app_id, config) in &apps.enabled {
        let Some((ns, app)) = app_id.split_once('/') else {
            continue;
        };
        if state.is_namespace_allowed(ns) && is_label_value(app) {
            by_namespace.entry(ns).or_default().insert(app, config);
        }
    }
    let mut cycle = Cycle {
        state,
        owner: &owner,
        endpoints: &endpoints,
        seen: BTreeSet::new(),
        injected: BTreeSet::new(),
    };
    for (ns, apps) in &by_namespace {
        cycle.reconcile_kind::<Deployment>(sweep, ns, apps).await;
        cycle.reconcile_kind::<StatefulSet>(sweep, ns, apps).await;
        cycle.reconcile_kind::<CronJob>(sweep, ns, apps).await;
    }

    if sweep.needed(&cycle.seen) {
        let keep: BTreeSet<String> = apps
            .enabled
            .keys()
            .chain(&apps.unreadable)
            .cloned()
            .collect();
        sweep.retry = !release_stale(state, sweep, &owner, &keep).await;
    }
    sweep.injected = Some(cycle.injected);
}

/// One cycle's shared inputs and what it saw.
struct Cycle<'a> {
    state: &'a AppState,
    owner: &'a str,
    endpoints: &'a AppEndpoints,
    /// Every workload of an application with injection on.
    seen: BTreeSet<String>,
    /// Those that carry this database's record.
    injected: BTreeSet<String>,
}

impl Cycle<'_> {
    async fn reconcile_kind<K: Workload>(
        &mut self,
        sweep: &mut Sweep,
        ns: &str,
        apps: &BTreeMap<&str, &OtelInjection>,
    ) {
        if sweep.skips(K::KIND) {
            return;
        }
        let names: Vec<&str> = apps.keys().copied().collect();
        let selector = format!("{APP_LABEL} in ({})", names.join(","));
        let api: Api<K> = Api::namespaced(self.state.kube_client.clone(), ns);
        let workloads = match api.list(&ListParams::default().labels(&selector)).await {
            Ok(list) => list.items,
            Err(e) => {
                if !sweep.forbidden(K::KIND, &e) {
                    tracing::warn!(
                        kind = K::KIND.name(),
                        namespace = %ns,
                        error = %e,
                        "otel injection: failed to list workloads"
                    );
                }
                return;
            }
        };
        for workload in workloads {
            let Some(live) = workload.live() else {
                continue;
            };
            let Some((app, config)) = live
                .application
                .as_deref()
                .and_then(|app| apps.get_key_value(app))
            else {
                continue;
            };
            let key = live.key();
            self.seen.insert(key.clone());
            let desired = live.desired(self.endpoints, config, app);
            let foreign = live
                .record
                .as_deref()
                .and_then(Record::parse)
                .is_some_and(|r| r.owner != self.owner);
            let adopt = !(foreign && sweep.adopted.contains(&key));
            let Some(plan) = plan(&live, &desired, self.owner, adopt) else {
                if sweep.left_alone.insert(key.clone()) {
                    tracing::warn!(
                        workload = %key,
                        "otel injection: another deckwatch database also manages this \
                         workload's OpenTelemetry variables; leaving them to it"
                    );
                }
                continue;
            };
            if plan.record.is_some() || live.has_record_of(self.owner) {
                self.injected.insert(key.clone());
            }
            if !plan.is_noop() {
                let sent = send::<K>(self.state, &api, &live, &plan, "updated").await;
                if sent.is_ok() && foreign {
                    sweep.adopted.insert(key);
                }
            }
        }
    }
}

/// Key into `AppState::plugin_patch_fingerprints`.
fn fingerprint_key(live: &LiveWorkload) -> String {
    format!("otel:{}", live.key())
}

/// The fingerprint of `body` sent to the object at `resource_version`.
fn fingerprint(body: &serde_json::Value, resource_version: Option<&str>) -> u64 {
    crate::watcher::fingerprint_patch(&serde_json::json!({
        "patch": body,
        "resourceVersion": resource_version,
    }))
}

/// Patch `live` as `plan` says, unless the same patch already went to the
/// object as it is now. `verb` describes the change in the log line.
async fn send<K: Workload>(
    state: &AppState,
    api: &Api<K>,
    live: &LiveWorkload,
    plan: &EnvPlan,
    verb: &str,
) -> Result<(), kube::Error> {
    let body = patch_body(live, plan);
    let key = fingerprint_key(live);
    let sent_before = state
        .plugin_patch_fingerprints
        .lock()
        .await
        .get(&key)
        .copied()
        == Some(fingerprint(&body, live.resource_version.as_deref()));
    if sent_before {
        tracing::debug!(
            workload = %live.key(),
            "otel injection: this patch already went to the workload as it is now; not sending it again"
        );
        return Ok(());
    }
    // Conditional on the object not having changed since it was read.
    let mut conditional = body.clone();
    if let Some(version) = &live.resource_version {
        conditional["metadata"]["resourceVersion"] = serde_json::json!(version);
    }
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        ..Default::default()
    };
    match api
        .patch(&live.name, &params, &Patch::Strategic(&conditional))
        .await
    {
        Ok(patched) => {
            let mut fingerprints = state.plugin_patch_fingerprints.lock().await;
            if plan.record.is_some() {
                let after = patched.meta().resource_version.as_deref();
                fingerprints.insert(key, fingerprint(&body, after));
            } else {
                fingerprints.remove(&key);
            }
            tracing::info!(
                kind = live.kind.name(),
                namespace = %live.namespace,
                name = %live.name,
                container = %plan.primary,
                set = plan.set.len(),
                removed = plan.remove.values().map(BTreeSet::len).sum::<usize>(),
                kept = plan.kept.len(),
                "otel injection: {verb} OpenTelemetry variables"
            );
            Ok(())
        }
        Err(e) => {
            // 409: the workload changed since it was read; the next cycle
            // reads it again. 404: it's gone.
            if is_status(&e, 409) || is_status(&e, 404) {
                tracing::debug!(workload = %live.key(), error = %e, "otel injection: patch skipped");
            } else {
                tracing::warn!(
                    workload = %live.key(),
                    error = %e,
                    "otel injection: failed to patch the workload"
                );
            }
            Err(e)
        }
    }
}

/// The namespace and name of a workload whose variables the sweep removes:
/// it carries this database's record (positive evidence it set them), and
/// it isn't in an application of `keep` (injection on, or settings that
/// don't read).
pub(crate) fn stale_workload<'a>(
    meta: &'a ObjectMeta,
    owner: &str,
    keep: &BTreeSet<String>,
) -> Option<(&'a str, &'a str)> {
    let ns = meta.namespace.as_deref()?;
    let name = meta.name.as_deref()?;
    let record = Record::parse(meta.annotations.as_ref()?.get(RECORD_ANNOTATION)?)?;
    if record.owner != owner {
        return None;
    }
    let app = meta.labels.as_ref().and_then(|l| l.get(APP_LABEL));
    if app.is_some_and(|app| keep.contains(&format!("{ns}/{app}"))) {
        return None;
    }
    Some((ns, name))
}

/// Remove this database's variables from every workload [`stale_workload`]
/// picks, in the allowed namespaces (or everywhere). Returns `false` if a
/// list, read or patch failed, so the caller sweeps again next cycle.
async fn release_stale(
    state: &AppState,
    sweep: &mut Sweep,
    owner: &str,
    keep: &BTreeSet<String>,
) -> bool {
    let scopes: Vec<Option<&str>> = if state.allowed_namespaces.is_empty() {
        vec![None]
    } else {
        state
            .allowed_namespaces
            .iter()
            .map(|ns| Some(ns.as_str()))
            .collect()
    };
    let mut released_all = true;
    for scope in scopes {
        released_all &= release_kind::<Deployment>(state, sweep, scope, owner, keep).await;
        released_all &= release_kind::<StatefulSet>(state, sweep, scope, owner, keep).await;
        released_all &= release_kind::<CronJob>(state, sweep, scope, owner, keep).await;
    }
    released_all
}

async fn release_kind<K: Workload>(
    state: &AppState,
    sweep: &mut Sweep,
    scope: Option<&str>,
    owner: &str,
    keep: &BTreeSet<String>,
) -> bool {
    if sweep.skips(K::KIND) {
        return true;
    }
    let list_api: Api<K> = match scope {
        Some(ns) => Api::namespaced(state.kube_client.clone(), ns),
        None => Api::all(state.kube_client.clone()),
    };
    let workloads = match list_api.list_metadata(&ListParams::default()).await {
        Ok(list) => list.items,
        Err(e) => {
            if sweep.forbidden(K::KIND, &e) {
                return true;
            }
            tracing::warn!(
                kind = K::KIND.name(),
                namespace = scope.unwrap_or("*"),
                error = %e,
                "otel injection: failed to list workloads to remove variables from"
            );
            return false;
        }
    };
    let mut released_all = true;
    for workload in &workloads {
        let Some((ns, name)) = stale_workload(&workload.metadata, owner, keep) else {
            continue;
        };
        if !state.is_namespace_allowed(ns) {
            continue;
        }
        let api: Api<K> = Api::namespaced(state.kube_client.clone(), ns);
        let live = match api.get_opt(name).await {
            Ok(Some(object)) => object.live(),
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(kind = K::KIND.name(), namespace = %ns, name = %name, error = %e, "otel injection: failed to read a workload");
                released_all = false;
                continue;
            }
        };
        let Some(live) = live else {
            continue;
        };
        let Some(plan) = plan(&live, &BTreeMap::new(), owner, false) else {
            continue;
        };
        if !plan.is_noop()
            && send::<K>(state, &api, &live, &plan, "removed")
                .await
                .is_err()
        {
            released_all = false;
        }
    }
    released_all
}

/// The workloads of one application, for its Observability section.
#[derive(Debug, Default)]
pub struct ApplicationWorkloads {
    pub deployments: Vec<Deployment>,
    pub statefulsets: Vec<StatefulSet>,
    pub cronjobs: Vec<CronJob>,
    /// deckwatch may not list StatefulSets, so `statefulsets` is empty.
    pub statefulsets_forbidden: bool,
}

impl ApplicationWorkloads {
    /// The workloads labelled with application `app` in `ns`.
    pub async fn list(
        state: &AppState,
        ns: &str,
        app: &str,
    ) -> Result<ApplicationWorkloads, kube::Error> {
        let lp = ListParams::default().labels(&format!("{APP_LABEL}={app}"));
        let client = state.kube_client.clone();
        let deployments: Api<Deployment> = Api::namespaced(client.clone(), ns);
        let statefulsets: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
        let cronjobs: Api<CronJob> = Api::namespaced(client, ns);
        let (deployments, statefulsets, cronjobs) = futures::future::join3(
            deployments.list(&lp),
            statefulsets.list(&lp),
            cronjobs.list(&lp),
        )
        .await;
        let (statefulsets, statefulsets_forbidden) = match statefulsets {
            Err(e) if is_status(&e, 403) => (Vec::new(), true),
            other => (other?.items, false),
        };
        Ok(ApplicationWorkloads {
            deployments: deployments?.items,
            statefulsets,
            cronjobs: cronjobs?.items,
            statefulsets_forbidden,
        })
    }

    /// Every workload as [`plan`] sees it, by kind and then name.
    pub fn live(&self) -> Vec<LiveWorkload> {
        let mut workloads: Vec<LiveWorkload> = Vec::new();
        workloads.extend(
            self.deployments
                .iter()
                .filter_map(LiveWorkload::from_deployment),
        );
        workloads.extend(
            self.statefulsets
                .iter()
                .filter_map(LiveWorkload::from_statefulset),
        );
        workloads.extend(self.cronjobs.iter().filter_map(LiveWorkload::from_cronjob));
        workloads.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        workloads
    }
}

#[cfg(test)]
#[path = "otel_inject_tests.rs"]
pub(crate) mod tests;
