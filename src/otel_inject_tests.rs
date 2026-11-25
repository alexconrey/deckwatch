use super::*;

use std::collections::HashMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::FieldsV1;
use sea_orm::ActiveModelTrait;
use sea_orm::ActiveValue::Set;
use serde_json::{json, Value};

const OWNER: &str = "this-database";
const COLLECTOR: &str = "http://otel-collector.observability:4318";
const PROMETHEUS: &str = "http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics";

fn both() -> AppEndpoints {
    AppEndpoints {
        otlp: Some(COLLECTOR.to_string()),
        metrics: Some(PROMETHEUS.to_string()),
    }
}

fn on() -> OtelInjection {
    OtelInjection {
        enabled: true,
        ..Default::default()
    }
}

fn attr(key: &str, value: &str) -> ResourceAttribute {
    ResourceAttribute {
        key: key.to_string(),
        value: value.to_string(),
    }
}

fn target(kind: WorkloadKind, name: &str) -> Target<'_> {
    Target {
        namespace: "team",
        kind,
        name,
        application: "shop",
    }
}

fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The variables
// ---------------------------------------------------------------------------

#[test]
fn a_deployment_gets_both_endpoints_its_name_and_the_managed_attributes() {
    let env = desired_env(&both(), &on(), &target(WorkloadKind::Deployment, "web"));
    assert_eq!(
        env,
        env_map(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", COLLECTOR),
            ("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", PROMETHEUS),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
            ("OTEL_SERVICE_NAME", "web"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "k8s.namespace.name=team,k8s.deployment.name=web,deckwatch.application.name=shop"
            ),
        ])
    );
}

#[test]
fn every_workload_kind_gets_its_own_name_attribute() {
    for (kind, attribute) in [
        (WorkloadKind::Deployment, "k8s.deployment.name"),
        (WorkloadKind::StatefulSet, "k8s.statefulset.name"),
        (WorkloadKind::CronJob, "k8s.cronjob.name"),
    ] {
        let env = desired_env(&both(), &on(), &target(kind, "db"));
        assert_eq!(
            env["OTEL_RESOURCE_ATTRIBUTES"],
            format!("k8s.namespace.name=team,{attribute}=db,deckwatch.application.name=shop"),
            "{kind:?}"
        );
        assert_eq!(env["OTEL_SERVICE_NAME"], "db", "{kind:?}");
    }
}

#[test]
fn a_signal_without_an_endpoint_is_turned_off() {
    let collector_only = AppEndpoints {
        otlp: Some(COLLECTOR.to_string()),
        metrics: None,
    };
    let env = desired_env(
        &collector_only,
        &on(),
        &target(WorkloadKind::Deployment, "web"),
    );
    assert_eq!(env["OTEL_METRICS_EXPORTER"], "none");
    assert!(!env.contains_key("OTEL_TRACES_EXPORTER"));
    assert!(!env.contains_key("OTEL_LOGS_EXPORTER"));
    assert!(!env.contains_key("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"));
    assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], COLLECTOR);

    let metrics_only = AppEndpoints {
        otlp: None,
        metrics: Some(PROMETHEUS.to_string()),
    };
    let env = desired_env(
        &metrics_only,
        &on(),
        &target(WorkloadKind::CronJob, "nightly"),
    );
    assert_eq!(env["OTEL_TRACES_EXPORTER"], "none");
    assert_eq!(env["OTEL_LOGS_EXPORTER"], "none");
    assert!(!env.contains_key("OTEL_METRICS_EXPORTER"));
    assert!(!env.contains_key("OTEL_EXPORTER_OTLP_ENDPOINT"));
    assert_eq!(env["OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"], PROMETHEUS);
    assert_eq!(env["OTEL_EXPORTER_OTLP_PROTOCOL"], "http/protobuf");
}

#[test]
fn nothing_is_injected_without_any_endpoint() {
    let env = desired_env(
        &AppEndpoints::default(),
        &on(),
        &target(WorkloadKind::Deployment, "web"),
    );
    assert!(env.is_empty(), "{env:?}");
}

#[test]
fn the_override_and_the_extra_attributes_apply_to_every_workload() {
    let config = OtelInjection {
        enabled: true,
        service_name: Some("storefront".to_string()),
        resource_attributes: vec![
            attr("deployment.environment", "prod"),
            attr("team", "payments%20eu"),
        ],
    };
    let env = desired_env(&both(), &config, &target(WorkloadKind::StatefulSet, "db"));
    assert_eq!(env["OTEL_SERVICE_NAME"], "storefront");
    assert_eq!(
        env["OTEL_RESOURCE_ATTRIBUTES"],
        "k8s.namespace.name=team,k8s.statefulset.name=db,deckwatch.application.name=shop,\
         deployment.environment=prod,team=payments%20eu"
    );
}

#[test]
fn the_variables_are_deterministic() {
    let config = OtelInjection {
        enabled: true,
        service_name: None,
        resource_attributes: vec![attr("b", "2"), attr("a", "1")],
    };
    let first = desired_env(&both(), &config, &target(WorkloadKind::Deployment, "web"));
    for _ in 0..10 {
        assert_eq!(
            desired_env(&both(), &config, &target(WorkloadKind::Deployment, "web")),
            first
        );
    }
    // The extra attributes keep the order they were given in.
    assert!(first["OTEL_RESOURCE_ATTRIBUTES"].ends_with(",b=2,a=1"));

    let live = live_deployment("web", "app", vec![user_env("USER_VAR", "1")], None);
    let plan = plan_for(&live, &first);
    let body = patch_body(&live, &plan).to_string();
    for _ in 0..10 {
        let again = plan_for(&live, &first);
        assert_eq!(patch_body(&live, &again).to_string(), body);
    }
}

fn plan_for(live: &LiveWorkload, desired: &BTreeMap<String, String>) -> EnvPlan {
    super::plan(live, desired, OWNER, true).expect("a plan")
}

/// The plan that removes what this database set (injection off).
fn release(live: &LiveWorkload) -> EnvPlan {
    super::plan(live, &BTreeMap::new(), OWNER, false).expect("a plan")
}

#[test]
fn endpoints_come_from_the_settings_validated_again() {
    let (endpoints, problems) = AppEndpoints::from_settings(&ObservabilitySettings {
        otlp_endpoint: format!("{COLLECTOR}/"),
        otlp_metrics_endpoint: PROMETHEUS.to_string(),
    });
    assert_eq!(endpoints, both());
    assert!(problems.is_empty());

    // Stored some other way: a URL with credentials is never injected.
    let (endpoints, problems) = AppEndpoints::from_settings(&ObservabilitySettings {
        otlp_endpoint: "http://user:secret@collector:4318".to_string(),
        otlp_metrics_endpoint: String::new(),
    });
    assert!(endpoints.is_empty(), "{endpoints:?}");
    assert_eq!(problems.len(), 1);
    assert!(!problems[0].contains("secret"), "{problems:?}");
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn the_service_name_is_trimmed_bounded_and_free_of_control_characters() {
    assert_eq!(
        validate_service_name("  shop  ").unwrap().as_deref(),
        Some("shop")
    );
    assert_eq!(validate_service_name("   ").unwrap(), None);
    assert_eq!(validate_service_name("").unwrap(), None);
    let longest = "é".repeat(MAX_SERVICE_NAME_CHARS);
    assert_eq!(
        validate_service_name(&longest).unwrap(),
        Some(longest.clone())
    );
    assert!(validate_service_name(&format!("{longest}x")).is_err());
    assert!(validate_service_name("a\nb").is_err());
    assert!(validate_service_name("a\u{7f}b").is_err());
}

#[test]
fn attribute_keys_and_values_follow_the_resource_attributes_format() {
    for key in ["team", "deployment.environment", "a-b_c.d", "K8S"] {
        assert!(is_attribute_key(key), "{key}");
    }
    for key in ["", "bad key", "a=b", "a,b", "café", &"k".repeat(256)] {
        assert!(!is_attribute_key(key), "{key}");
    }
    for value in [
        "prod",
        "payments%20eu",
        "a=b",
        "x%2Cy",
        "~!#$&'()*+-./:<>?@[]^_`{|}",
    ] {
        assert!(is_encoded_attribute_value(value), "{value}");
    }
    for value in [
        "",
        "two words",
        "\"q\"",
        "a,b",
        "a;b",
        "a\\b",
        "é",
        "50%",
        "%zz",
        "%2",
        "tab\t",
    ] {
        assert!(!is_encoded_attribute_value(value), "{value:?}");
    }
    assert!(!is_encoded_attribute_value(&"v".repeat(1025)));
}

#[test]
fn extra_attributes_are_trimmed_unique_and_never_override_deckwatchs() {
    let ok =
        validate_resource_attributes(&[attr(" team ", " payments "), attr("tier", "web")]).unwrap();
    assert_eq!(ok, vec![attr("team", "payments"), attr("tier", "web")]);

    for key in MANAGED_ATTRIBUTES {
        let problems = validate_resource_attributes(&[attr(key, "x")]).unwrap_err();
        assert!(problems[0].contains("set by deckwatch"), "{problems:?}");
    }
    let problems = validate_resource_attributes(&[attr("service.name", "x")]).unwrap_err();
    assert!(problems[0].contains("service name"), "{problems:?}");
    let problems = validate_resource_attributes(&[attr("a", "1"), attr("a", "2")]).unwrap_err();
    assert!(problems[0].contains("repeated"), "{problems:?}");
    let many: Vec<ResourceAttribute> = (0..=MAX_RESOURCE_ATTRIBUTES)
        .map(|i| attr(&format!("k{i}"), "v"))
        .collect();
    assert!(validate_resource_attributes(&many).is_err());
}

#[test]
fn validation_reports_every_problem() {
    let config = OtelInjection {
        enabled: true,
        service_name: Some("a\u{0}b".to_string()),
        resource_attributes: vec![attr("bad key", "x"), attr("ok", "has space")],
    };
    let error = config.validated().unwrap_err();
    assert!(error.contains("service_name"), "{error}");
    assert!(error.contains("\"bad key\""), "{error}");
    assert!(error.contains("value of ok"), "{error}");

    let normalized = OtelInjection {
        enabled: true,
        service_name: Some("  ".to_string()),
        resource_attributes: vec![attr(" a ", "1")],
    }
    .validated()
    .unwrap();
    assert_eq!(normalized.service_name, None);
    assert_eq!(normalized.resource_attributes, vec![attr("a", "1")]);
}

#[test]
fn the_stored_column_round_trips_and_null_is_off() {
    assert_eq!(
        ApplicationObservability::from_column(None).unwrap(),
        ApplicationObservability::default()
    );
    assert_eq!(
        ApplicationObservability::from_column(Some("  ")).unwrap(),
        ApplicationObservability::default()
    );
    assert_eq!(ApplicationObservability::default().to_column(), None);
    let stored = ApplicationObservability {
        opentelemetry: OtelInjection {
            enabled: true,
            service_name: Some("shop".to_string()),
            resource_attributes: vec![attr("team", "payments")],
        },
    };
    let column = stored.to_column().unwrap();
    assert_eq!(
        ApplicationObservability::from_column(Some(&column)).unwrap(),
        stored
    );
    assert!(ApplicationObservability::from_column(Some("{not json")).is_err());
}

// ---------------------------------------------------------------------------
// The record and managedFields
// ---------------------------------------------------------------------------

#[test]
fn the_record_encodes_deterministically_and_hashes_are_short() {
    let record = Record {
        owner: OWNER.to_string(),
        container: "app".to_string(),
        env: env_map(&[("B", &value_hash("2")), ("A", &value_hash("1"))]),
    };
    let encoded = record.encode();
    assert_eq!(Record::parse(&encoded), Some(record.clone()));
    assert!(encoded.find("\"A\"").unwrap() < encoded.find("\"B\"").unwrap());
    assert_eq!(value_hash("1"), value_hash("1"));
    assert_ne!(value_hash("1"), value_hash("2"));
    assert_eq!(value_hash("anything").len(), 16);
    assert_eq!(Record::parse("not a record"), None);
}

fn managed(manager: &str, operation: &str, fields: Value) -> ManagedFieldsEntry {
    ManagedFieldsEntry {
        manager: Some(manager.to_string()),
        operation: Some(operation.to_string()),
        fields_type: Some("FieldsV1".to_string()),
        fields_v1: Some(FieldsV1(fields)),
        ..Default::default()
    }
}

fn env_fields(path: &[&str], container: &str, names: &[&str]) -> Value {
    let env: serde_json::Map<String, Value> = names
        .iter()
        .map(|n| {
            (
                format!("k:{{\"name\":\"{n}\"}}"),
                json!({ ".": {}, "f:name": {}, "f:value": {} }),
            )
        })
        .collect();
    let mut node = json!({
        "f:containers": { format!("k:{{\"name\":\"{container}\"}}"): { "f:env": env } }
    });
    for field in path.iter().rev() {
        node = json!({ format!("f:{field}"): node });
    }
    node
}

#[test]
fn variables_other_field_managers_applied_are_read_from_managed_fields() {
    let deployment_path = ["spec", "template", "spec"];
    let entries = vec![
        managed(
            "argocd-controller",
            "Apply",
            env_fields(&deployment_path, "app", &["A", "B"]),
        ),
        // An Update doesn't claim anything; neither does deckwatch itself.
        managed(
            "kubectl-edit",
            "Update",
            env_fields(&deployment_path, "app", &["C"]),
        ),
        managed(
            FIELD_MANAGER,
            "Apply",
            env_fields(&deployment_path, "app", &["D"]),
        ),
        managed(
            "flux",
            "Apply",
            env_fields(&deployment_path, "proxy", &["E"]),
        ),
    ];
    let names = applied_env(Some(&entries), WorkloadKind::Deployment, "app");
    assert_eq!(names, ["A", "B"].map(String::from).into_iter().collect());
    assert_eq!(
        applied_env(Some(&entries), WorkloadKind::Deployment, "proxy"),
        ["E"].map(String::from).into_iter().collect()
    );
    // The same fields under a CronJob's path belong to a CronJob only.
    assert!(applied_env(Some(&entries), WorkloadKind::CronJob, "app").is_empty());
    let cronjob_path = ["spec", "jobTemplate", "spec", "template", "spec"];
    let entries = vec![managed(
        "flux",
        "Apply",
        env_fields(&cronjob_path, "job", &["F"]),
    )];
    assert_eq!(
        applied_env(Some(&entries), WorkloadKind::CronJob, "job"),
        ["F"].map(String::from).into_iter().collect()
    );
    assert!(applied_env(None, WorkloadKind::CronJob, "job").is_empty());
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

fn user_env(name: &str, value: &str) -> LiveEnv {
    LiveEnv {
        name: name.to_string(),
        value: Some(value.to_string()),
        value_from: false,
    }
}

fn live_deployment(
    name: &str,
    container: &str,
    env: Vec<LiveEnv>,
    record: Option<Record>,
) -> LiveWorkload {
    LiveWorkload {
        kind: WorkloadKind::Deployment,
        namespace: "team".to_string(),
        name: name.to_string(),
        resource_version: Some("7".to_string()),
        application: Some("shop".to_string()),
        record: record.map(|r| r.encode()),
        containers: vec![
            LiveContainer {
                name: container.to_string(),
                env,
            },
            LiveContainer {
                name: "proxy".to_string(),
                env: vec![user_env("PROXY_VAR", "1")],
            },
        ],
        applied_by_others: BTreeMap::new(),
    }
}

/// `live` after `plan`, the way the API server applies the patch: values set
/// in place or appended, deleted entries dropped, the record replaced.
fn applied(live: &LiveWorkload, plan: &EnvPlan) -> LiveWorkload {
    let mut after = live.clone();
    for container in &mut after.containers {
        if let Some(names) = plan.remove.get(&container.name) {
            container.env.retain(|e| !names.contains(&e.name));
        }
        if container.name == plan.primary {
            for (name, value) in &plan.set {
                match container.env.iter_mut().find(|e| &e.name == name) {
                    Some(entry) => entry.value = Some(value.clone()),
                    None => container.env.push(user_env(name, value)),
                }
            }
        }
    }
    after.record = plan.record.as_ref().map(Record::encode);
    after
}

fn desired_web() -> BTreeMap<String, String> {
    desired_env(&both(), &on(), &target(WorkloadKind::Deployment, "web"))
}

#[test]
fn a_fresh_workload_gets_every_variable_on_its_primary_container_only() {
    let live = live_deployment("web", "app", vec![user_env("USER_VAR", "1")], None);
    let plan = plan_for(&live, &desired_web());
    assert_eq!(plan.primary, "app");
    assert_eq!(plan.set, desired_web());
    assert!(plan.remove.is_empty());
    assert!(plan.kept.is_empty());
    let record = plan.record.clone().unwrap();
    assert_eq!(record.owner, OWNER);
    assert_eq!(record.container, "app");
    assert_eq!(
        record.env.keys().collect::<Vec<_>>(),
        desired_web().keys().collect::<Vec<_>>()
    );

    let after = applied(&live, &plan);
    assert_eq!(
        after.containers[1], live.containers[1],
        "the sidecar is untouched"
    );
    assert_eq!(after.containers[0].env[0], user_env("USER_VAR", "1"));
    // Once applied, there's nothing left to do.
    let again = plan_for(&after, &desired_web());
    assert!(again.is_noop(), "{again:?}");
    assert_eq!(
        again.owned,
        desired_web().into_keys().collect::<BTreeSet<_>>()
    );
}

#[test]
fn a_variable_the_workload_sets_itself_wins() {
    let live = live_deployment(
        "web",
        "app",
        vec![
            user_env("OTEL_SERVICE_NAME", "checkout"),
            LiveEnv {
                name: "OTEL_RESOURCE_ATTRIBUTES".to_string(),
                value: None,
                value_from: true,
            },
        ],
        None,
    );
    let plan = plan_for(&live, &desired_web());
    assert!(!plan.set.contains_key("OTEL_SERVICE_NAME"));
    assert!(!plan.set.contains_key("OTEL_RESOURCE_ATTRIBUTES"));
    assert_eq!(
        plan.kept,
        ["OTEL_RESOURCE_ATTRIBUTES", "OTEL_SERVICE_NAME"]
            .map(String::from)
            .into_iter()
            .collect()
    );
    let record = plan.record.clone().unwrap();
    assert!(!record.env.contains_key("OTEL_SERVICE_NAME"));

    // Turning injection off later leaves them.
    let after = applied(&live, &plan);
    let off = release(&after);
    let after = applied(&after, &off);
    assert_eq!(after.containers[0].env, live.containers[0].env);
    assert_eq!(after.record, None);
}

#[test]
fn a_variable_changed_after_injection_becomes_the_workloads() {
    let live = live_deployment("web", "app", vec![], None);
    let injected = applied(&live, &plan_for(&live, &desired_web()));

    // Someone edits the service name (kubectl edit, the YAML editor).
    let mut edited = injected.clone();
    edited.containers[0]
        .env
        .iter_mut()
        .find(|e| e.name == "OTEL_SERVICE_NAME")
        .unwrap()
        .value = Some("my-service".to_string());
    let plan = plan_for(&edited, &desired_web());
    assert!(plan.set.is_empty(), "{plan:?}");
    assert!(plan.remove.is_empty(), "{plan:?}");
    assert!(plan.kept.contains("OTEL_SERVICE_NAME"));
    assert!(!plan.owned.contains("OTEL_SERVICE_NAME"));
    assert!(plan.record_changed, "the record drops the variable");
    let after = applied(&edited, &plan);
    assert!(plan_for(&after, &desired_web()).is_noop());

    // Off: everything else goes, the edited value stays.
    let off = release(&after);
    let after = applied(&after, &off);
    assert_eq!(
        after.containers[0].env,
        vec![user_env("OTEL_SERVICE_NAME", "my-service")]
    );
    assert_eq!(after.record, None);
}

#[test]
fn a_variable_replaced_with_value_from_or_applied_by_another_manager_is_theirs() {
    let live = live_deployment("web", "app", vec![], None);
    let injected = applied(&live, &plan_for(&live, &desired_web()));

    let mut from_secret = injected.clone();
    let entry = from_secret.containers[0]
        .env
        .iter_mut()
        .find(|e| e.name == "OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap();
    entry.value = None;
    entry.value_from = true;
    let off = release(&from_secret);
    assert!(!off.remove["app"].contains("OTEL_EXPORTER_OTLP_ENDPOINT"));

    // A GitOps tool now applies the same name with the same value.
    let mut applied_by_gitops = injected.clone();
    applied_by_gitops.applied_by_others.insert(
        "app".to_string(),
        ["OTEL_SERVICE_NAME".to_string()].into_iter().collect(),
    );
    let off = release(&applied_by_gitops);
    assert!(!off.remove["app"].contains("OTEL_SERVICE_NAME"));
    assert!(off.remove["app"].contains("OTEL_EXPORTER_OTLP_PROTOCOL"));
}

#[test]
fn enabling_then_disabling_leaves_nothing_behind() {
    for kind in [
        WorkloadKind::Deployment,
        WorkloadKind::StatefulSet,
        WorkloadKind::CronJob,
    ] {
        let mut live = live_deployment("web", "app", vec![user_env("USER_VAR", "1")], None);
        live.kind = kind;
        let desired = desired_env(&both(), &on(), &target(kind, "web"));
        let injected = applied(&live, &plan_for(&live, &desired));
        assert_eq!(injected.containers[0].env.len(), 1 + desired.len());
        let off = release(&injected);
        assert!(off.set.is_empty());
        assert_eq!(off.record, None);
        assert_eq!(applied(&injected, &off), live, "{kind:?}");
    }
}

#[test]
fn a_changed_endpoint_or_service_name_is_updated_in_place() {
    let live = live_deployment("web", "app", vec![], None);
    let injected = applied(&live, &plan_for(&live, &desired_web()));
    let moved = AppEndpoints {
        otlp: Some("http://collector.other:4318".to_string()),
        metrics: None,
    };
    let desired = desired_env(&moved, &on(), &target(WorkloadKind::Deployment, "web"));
    let plan = plan_for(&injected, &desired);
    assert_eq!(
        plan.set,
        env_map(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector.other:4318"),
            ("OTEL_METRICS_EXPORTER", "none"),
        ])
    );
    assert_eq!(
        plan.remove["app"],
        ["OTEL_EXPORTER_OTLP_METRICS_ENDPOINT".to_string()]
            .into_iter()
            .collect()
    );
    let after = applied(&injected, &plan);
    assert!(plan_for(&after, &desired).is_noop());
}

#[test]
fn removing_every_endpoint_removes_the_variables() {
    let live = live_deployment("web", "app", vec![user_env("USER_VAR", "1")], None);
    let injected = applied(&live, &plan_for(&live, &desired_web()));
    let none = desired_env(
        &AppEndpoints::default(),
        &on(),
        &target(WorkloadKind::Deployment, "web"),
    );
    let plan = plan_for(&injected, &none);
    assert_eq!(applied(&injected, &plan), live);
}

#[test]
fn a_workload_with_its_own_endpoint_keeps_its_export_setup() {
    for own_endpoint in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
    ] {
        // Metrics only in settings: deckwatch would turn traces and logs off.
        let metrics_only = AppEndpoints {
            otlp: None,
            metrics: Some(PROMETHEUS.to_string()),
        };
        let desired = desired_env(
            &metrics_only,
            &on(),
            &target(WorkloadKind::Deployment, "web"),
        );
        let live = live_deployment(
            "web",
            "app",
            vec![user_env(
                own_endpoint,
                "http://own-collector.example.com:4317",
            )],
            None,
        );
        let plan = plan_for(&live, &desired);
        // No protocol, endpoint or exporter: they'd change where its
        // signals go. Its identity is still deckwatch's to set.
        assert_eq!(
            plan.set.keys().collect::<Vec<_>>(),
            vec!["OTEL_RESOURCE_ATTRIBUTES", "OTEL_SERVICE_NAME"],
            "{own_endpoint}"
        );
        assert!(plan.exports_itself);
    }
}

#[test]
fn taking_over_the_endpoint_hands_over_the_export_setup() {
    let live = live_deployment("web", "app", vec![], None);
    let collector_only = AppEndpoints {
        otlp: Some(COLLECTOR.to_string()),
        metrics: None,
    };
    let desired = desired_env(
        &collector_only,
        &on(),
        &target(WorkloadKind::Deployment, "web"),
    );
    let mut edited = applied(&live, &plan_for(&live, &desired));
    // Someone points the workload at another collector.
    edited.containers[0]
        .env
        .iter_mut()
        .find(|e| e.name == "OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap()
        .value = Some("http://own-collector.example.com:4318".to_string());
    let plan = plan_for(&edited, &desired);
    assert!(plan.set.is_empty(), "{plan:?}");
    assert_eq!(
        plan.remove["app"],
        ["OTEL_EXPORTER_OTLP_PROTOCOL", "OTEL_METRICS_EXPORTER"]
            .map(String::from)
            .into_iter()
            .collect()
    );
    let after = applied(&edited, &plan);
    assert_eq!(
        after.containers[0]
            .env
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_RESOURCE_ATTRIBUTES",
            "OTEL_SERVICE_NAME"
        ]
    );
}

#[test]
fn a_dollar_is_escaped_so_kubernetes_expands_nothing() {
    let config = OtelInjection {
        enabled: true,
        service_name: Some("shop$(SHARD)".to_string()),
        resource_attributes: vec![attr("secret", "$(DB_PASSWORD)"), attr("price", "5$")],
    };
    let env = desired_env(&both(), &config, &target(WorkloadKind::Deployment, "web"));
    assert_eq!(env["OTEL_SERVICE_NAME"], "shop$$(SHARD)");
    assert!(
        env["OTEL_RESOURCE_ATTRIBUTES"].ends_with(",secret=$$(DB_PASSWORD),price=5$$"),
        "{}",
        env["OTEL_RESOURCE_ATTRIBUTES"]
    );
    // What deckwatch set is still recognized as its own.
    let live = live_deployment("web", "app", vec![], None);
    let after = applied(&live, &plan_for(&live, &env));
    assert!(plan_for(&after, &env).is_noop());
}

#[test]
fn another_databases_record_is_left_alone_unless_adopted() {
    let live = live_deployment("web", "app", vec![], None);
    let theirs = applied(
        &live,
        &super::plan(&live, &desired_web(), "other-database", true).unwrap(),
    );
    assert_eq!(super::plan(&theirs, &BTreeMap::new(), OWNER, false), None);

    // An application with injection on here takes the record over without
    // touching the variables (no restart).
    let adopted = super::plan(&theirs, &desired_web(), OWNER, true).unwrap();
    assert!(
        adopted.set.is_empty() && adopted.remove.is_empty(),
        "{adopted:?}"
    );
    assert!(adopted.record_changed);
    assert_eq!(adopted.record.unwrap().owner, OWNER);
}

#[test]
fn a_new_primary_container_gets_the_variables_and_the_old_one_loses_them() {
    let live = live_deployment("web", "app", vec![], None);
    let injected = applied(&live, &plan_for(&live, &desired_web()));
    let mut reordered = injected.clone();
    reordered.containers.insert(
        0,
        LiveContainer {
            name: "main".to_string(),
            env: vec![],
        },
    );
    let plan = plan_for(&reordered, &desired_web());
    assert_eq!(plan.primary, "main");
    assert_eq!(plan.set, desired_web());
    assert_eq!(
        plan.remove["app"],
        desired_web().into_keys().collect::<BTreeSet<_>>()
    );
    let after = applied(&reordered, &plan);
    assert!(after.containers[1].env.is_empty());
    assert_eq!(
        after
            .record
            .as_deref()
            .and_then(Record::parse)
            .unwrap()
            .container,
        "main"
    );
}

#[test]
fn duplicate_entries_and_garbage_records_are_never_deckwatchs() {
    let live = live_deployment("web", "app", vec![], None);
    let mut injected = applied(&live, &plan_for(&live, &desired_web()));
    injected.containers[0]
        .env
        .push(user_env("OTEL_SERVICE_NAME", "web"));
    let plan = plan_for(&injected, &desired_web());
    assert!(!plan.owned.contains("OTEL_SERVICE_NAME"));
    assert!(plan.kept.contains("OTEL_SERVICE_NAME"));

    let mut garbage = live_deployment(
        "web",
        "app",
        vec![user_env("OTEL_SERVICE_NAME", "web")],
        None,
    );
    garbage.record = Some("{not a record".to_string());
    let plan = plan_for(&garbage, &desired_web());
    assert!(plan.owned.is_empty());
    assert!(plan.kept.contains("OTEL_SERVICE_NAME"));
}

#[test]
fn a_renamed_primary_container_keeps_deckwatchs_variables_deckwatchs() {
    let live = live_deployment("web", "app", vec![user_env("USER_VAR", "1")], None);
    let mut renamed = applied(&live, &plan_for(&live, &desired_web()));
    renamed.containers[0].name = "main".to_string();
    let off = release(&renamed);
    assert_eq!(
        off.remove["main"],
        desired_web().into_keys().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        applied(&renamed, &off).containers[0].env,
        live.containers[0].env
    );
}

#[test]
fn a_client_side_apply_declaring_a_variable_makes_it_theirs() {
    let deployment: Deployment = serde_json::from_value(json!({
        "metadata": {
            "name": "web",
            "namespace": "team",
            "annotations": {
                "kubectl.kubernetes.io/last-applied-configuration": json!({
                    "apiVersion": "apps/v1",
                    "kind": "Deployment",
                    "spec": { "template": { "spec": { "containers": [
                        { "name": "app", "env": [{ "name": "OTEL_SERVICE_NAME", "value": "web" }] },
                    ] } } },
                }).to_string(),
            },
        },
        "spec": {
            "selector": {},
            "template": { "spec": { "containers": [{ "name": "app" }, { "name": "proxy" }] } },
        },
    }))
    .unwrap();
    let live = LiveWorkload::from_deployment(&deployment).unwrap();
    assert_eq!(
        live.applied_by_others.get("app"),
        Some(&["OTEL_SERVICE_NAME".to_string()].into_iter().collect())
    );
    assert_eq!(live.applied_by_others.get("proxy"), None);
}

#[test]
fn a_workload_without_containers_gets_nothing() {
    let mut live = live_deployment("web", "app", vec![], None);
    live.containers.clear();
    assert!(plan_for(&live, &desired_web()).is_noop());
}

// ---------------------------------------------------------------------------
// The patch
// ---------------------------------------------------------------------------

#[test]
fn the_patch_sets_and_deletes_single_entries_by_name() {
    let live = live_deployment("web", "app", vec![], None);
    let plan = EnvPlan {
        primary: "app".to_string(),
        set: env_map(&[("B", "2"), ("A", "1")]),
        remove: [("app".to_string(), ["C".to_string()].into_iter().collect())]
            .into_iter()
            .collect(),
        record: Some(Record {
            owner: OWNER.to_string(),
            container: "app".to_string(),
            env: env_map(&[("A", &value_hash("1")), ("B", &value_hash("2"))]),
        }),
        record_changed: true,
        ..Default::default()
    };
    let body = patch_body(&live, &plan);
    assert_eq!(
        body,
        json!({
            "metadata": { "annotations": { RECORD_ANNOTATION: plan.record.as_ref().unwrap().encode() } },
            "spec": { "template": { "spec": { "containers": [{
                "name": "app",
                "$setElementOrder/env": [{ "name": "A" }, { "name": "B" }],
                "env": [
                    { "name": "A", "value": "1" },
                    { "name": "B", "value": "2" },
                    { "name": "C", "$patch": "delete" },
                ],
            }] } } },
        })
    );
}

#[test]
fn the_values_follow_the_order_directive() {
    // OTEL_A is new and sorts before OTEL_B, which exists and changes: the
    // API server refuses a patch whose entries disagree with the directive.
    let live = live_deployment(
        "web",
        "app",
        vec![user_env("USER_VAR", "1"), user_env("OTEL_B", "old")],
        None,
    );
    let plan = EnvPlan {
        primary: "app".to_string(),
        set: env_map(&[("OTEL_A", "a"), ("OTEL_B", "new")]),
        remove: [(
            "app".to_string(),
            ["OTEL_GONE".to_string()].into_iter().collect(),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let body = patch_body(&live, &plan);
    let container = &body["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(
        container["$setElementOrder/env"],
        json!([{ "name": "USER_VAR" }, { "name": "OTEL_B" }, { "name": "OTEL_A" }])
    );
    assert_eq!(
        container["env"],
        json!([
            { "name": "OTEL_B", "value": "new" },
            { "name": "OTEL_A", "value": "a" },
            { "name": "OTEL_GONE", "$patch": "delete" },
        ])
    );
}

#[test]
fn new_variables_go_after_the_existing_ones() {
    let live = live_deployment(
        "web",
        "app",
        vec![user_env("NODE_NAME", "n"), user_env("OTEL_A", "old")],
        None,
    );
    let plan = EnvPlan {
        primary: "app".to_string(),
        set: env_map(&[("OTEL_Z", "z"), ("OTEL_A", "new"), ("OTEL_B", "b")]),
        ..Default::default()
    };
    let body = patch_body(&live, &plan);
    assert_eq!(
        body["spec"]["template"]["spec"]["containers"][0]["$setElementOrder/env"],
        json!([
            { "name": "NODE_NAME" },
            { "name": "OTEL_A" },
            { "name": "OTEL_B" },
            { "name": "OTEL_Z" },
        ])
    );

    // A list with a repeated name can't be ordered by name: no directive.
    let repeated = live_deployment(
        "web",
        "app",
        vec![user_env("X", "1"), user_env("X", "2")],
        None,
    );
    let body = patch_body(&repeated, &plan);
    assert!(body["spec"]["template"]["spec"]["containers"][0]
        .get("$setElementOrder/env")
        .is_none());
}

#[test]
fn a_cronjob_patch_reaches_its_job_template_and_removal_clears_the_record() {
    let mut live = live_deployment("nightly", "job", vec![], None);
    live.kind = WorkloadKind::CronJob;
    let plan = EnvPlan {
        primary: "job".to_string(),
        remove: [("job".to_string(), ["A".to_string()].into_iter().collect())]
            .into_iter()
            .collect(),
        record_changed: true,
        ..Default::default()
    };
    assert_eq!(
        patch_body(&live, &plan),
        json!({
            "metadata": { "annotations": { RECORD_ANNOTATION: null } },
            "spec": { "jobTemplate": { "spec": { "template": { "spec": { "containers": [{
                "name": "job",
                "$setElementOrder/env": [],
                "env": [{ "name": "A", "$patch": "delete" }],
            }] } } } } },
        })
    );

    // Only the record changes: no pod template in the patch, so no restart.
    let record_only = EnvPlan {
        primary: "job".to_string(),
        record_changed: true,
        ..Default::default()
    };
    let body = patch_body(&live, &record_only);
    assert!(body.get("spec").is_none(), "{body}");
}

// ---------------------------------------------------------------------------
// Sweep decisions
// ---------------------------------------------------------------------------

fn swept_meta(app: Option<&str>, record: Option<&str>) -> ObjectMeta {
    ObjectMeta {
        name: Some("web".to_string()),
        namespace: Some("team".to_string()),
        labels: app.map(|a| {
            [(APP_LABEL.to_string(), a.to_string())]
                .into_iter()
                .collect()
        }),
        annotations: record.map(|r| {
            [(RECORD_ANNOTATION.to_string(), r.to_string())]
                .into_iter()
                .collect()
        }),
        ..Default::default()
    }
}

fn record_of(owner: &str) -> String {
    Record {
        owner: owner.to_string(),
        container: "app".to_string(),
        env: env_map(&[("OTEL_SERVICE_NAME", &value_hash("web"))]),
    }
    .encode()
}

#[test]
fn the_sweep_removes_only_this_databases_variables_outside_enabled_applications() {
    let keep: BTreeSet<String> = ["team/shop".to_string()].into_iter().collect();
    let ours = record_of(OWNER);
    let theirs = record_of("other-database");
    // Still in an application with injection on (or unreadable settings).
    assert_eq!(
        stale_workload(&swept_meta(Some("shop"), Some(&ours)), OWNER, &keep),
        None
    );
    // Turned off or deleted application, or taken out of it.
    assert_eq!(
        stale_workload(&swept_meta(Some("blog"), Some(&ours)), OWNER, &keep),
        Some(("team", "web"))
    );
    assert_eq!(
        stale_workload(&swept_meta(None, Some(&ours)), OWNER, &keep),
        Some(("team", "web"))
    );
    // No evidence this database set them.
    assert_eq!(
        stale_workload(&swept_meta(None, Some(&theirs)), OWNER, &keep),
        None
    );
    assert_eq!(
        stale_workload(&swept_meta(None, Some("garbage")), OWNER, &keep),
        None
    );
    assert_eq!(
        stale_workload(&swept_meta(Some("blog"), None), OWNER, &keep),
        None
    );
}

#[test]
fn the_sweep_runs_first_after_a_dropped_workload_and_after_a_failure() {
    let mut sweep = Sweep::default();
    let seen: BTreeSet<String> = ["Deployment/team/web".to_string()].into_iter().collect();
    assert!(sweep.needed(&seen), "first cycle");
    sweep.injected = Some(seen.clone());
    assert!(!sweep.needed(&seen));
    let mut more = seen.clone();
    more.insert("CronJob/team/nightly".to_string());
    assert!(!sweep.needed(&more), "a new workload is no reason to sweep");
    assert!(sweep.needed(&BTreeSet::new()), "web dropped out");
    sweep.retry = true;
    assert!(sweep.needed(&seen));
}

#[test]
fn only_label_values_go_into_the_selector() {
    for value in ["shop", "a", "web-2", "a.b_c"] {
        assert!(is_label_value(value), "{value}");
    }
    for value in ["", "-shop", "shop-", "a,b", "a b", "a)b", &"a".repeat(64)] {
        assert!(!is_label_value(value), "{value}");
    }
}

#[test]
fn the_status_says_injected_pending_or_off() {
    let config = on();
    let live = live_deployment(
        "web",
        "app",
        vec![user_env("OTEL_SERVICE_NAME", "own")],
        None,
    );
    let pending = workload_status(&live, &both(), &config, "shop", OWNER);
    assert_eq!(pending.state, InjectionState::Pending);
    assert_eq!(pending.kept, vec!["OTEL_SERVICE_NAME".to_string()]);
    assert!(pending.injected.is_empty());

    let desired = live.desired(&both(), &config, "shop");
    let injected = applied(&live, &plan_for(&live, &desired));
    let status = workload_status(&injected, &both(), &config, "shop", OWNER);
    assert_eq!(status.state, InjectionState::Injected);
    assert_eq!(status.container.as_deref(), Some("app"));
    assert!(status
        .injected
        .contains(&"OTEL_EXPORTER_OTLP_ENDPOINT".to_string()));

    let off = OtelInjection::default();
    let removing = workload_status(&injected, &both(), &off, "shop", OWNER);
    assert_eq!(removing.state, InjectionState::Pending);
    let never = workload_status(&live, &both(), &off, "shop", OWNER);
    assert_eq!(never.state, InjectionState::Off);
}

// ---------------------------------------------------------------------------
// A fake API server for the reconciler
// ---------------------------------------------------------------------------
//
// Serves Deployments, StatefulSets and CronJobs: LISTs (label selectors
// with `=` and `in`, and metadata-only), GETs, and strategic merge patches
// of the shape `patch_body` builds: annotations set or removed, env entries
// of existing containers set or deleted by name, and the `resourceVersion`
// precondition (409 when stale). A patch that changes nothing keeps the
// resourceVersion, as the API server does.

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub body: Value,
}

#[derive(Default)]
struct FakeState {
    /// (plural, namespace, name) → object.
    objects: BTreeMap<(String, String, String), Value>,
    requests: Vec<Request>,
    /// Plurals the API server refuses to list (403).
    forbidden: BTreeSet<String>,
    /// The next patches answered with 409.
    conflicts: usize,
    /// An admission webhook that drops this variable from every patch.
    drops: Option<String>,
    next_resource_version: u64,
}

#[derive(Clone, Default)]
pub(crate) struct FakeCluster(std::sync::Arc<std::sync::Mutex<FakeState>>);

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| decode(v))
}

fn matches_selector(obj: &Value, selector: Option<&str>) -> bool {
    let Some(selector) = selector.filter(|s| !s.is_empty()) else {
        return true;
    };
    let labels = &obj["metadata"]["labels"];
    if let Some((key, rest)) = selector.split_once(" in (") {
        let values: Vec<&str> = rest.trim_end_matches(')').split(',').collect();
        return labels[key].as_str().is_some_and(|v| values.contains(&v));
    }
    selector.split(',').all(|term| match term.split_once('=') {
        Some((k, v)) => labels[k].as_str() == Some(v),
        None => labels.get(term).is_some(),
    })
}

fn kind_of(plural: &str) -> &'static str {
    match plural {
        "deployments" => "Deployment",
        "statefulsets" => "StatefulSet",
        "cronjobs" => "CronJob",
        other => panic!("the fake API server does not serve {other}"),
    }
}

fn status(code: u16, reason: &str) -> (u16, Value) {
    (
        code,
        json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {},
            "status": "Failure", "message": reason, "reason": reason, "code": code,
        }),
    )
}

/// The pod spec of a workload object (or patch).
fn pod_spec_mut<'a>(plural: &str, obj: &'a mut Value) -> &'a mut Value {
    match plural {
        "cronjobs" => &mut obj["spec"]["jobTemplate"]["spec"]["template"]["spec"],
        _ => &mut obj["spec"]["template"]["spec"],
    }
}

/// Apply the strategic merge patches `patch_body` builds; anything else in
/// the patch fails the test.
fn merge(plural: &str, obj: &mut Value, patch: &Value) -> Result<(), String> {
    let patch_obj = patch.as_object().expect("patch is an object");
    for key in patch_obj.keys() {
        assert!(
            matches!(key.as_str(), "metadata" | "spec"),
            "unexpected patch field {key}"
        );
    }
    for (key, value) in patch["metadata"].as_object().into_iter().flatten() {
        match key.as_str() {
            "resourceVersion" => {}
            "annotations" => {
                for (name, value) in value.as_object().expect("annotations are an object") {
                    let annotations = &mut obj["metadata"]["annotations"];
                    if annotations.is_null() {
                        *annotations = json!({});
                    }
                    let map = annotations.as_object_mut().unwrap();
                    if value.is_null() {
                        map.remove(name);
                    } else {
                        map.insert(name.clone(), value.clone());
                    }
                }
            }
            other => panic!("unexpected metadata field in patch: {other}"),
        }
    }
    if patch.get("spec").is_none() {
        return Ok(());
    }
    let mut patch = patch.clone();
    let pod_patch = pod_spec_mut(plural, &mut patch).take();
    let pod_patch = pod_patch
        .as_object()
        .expect("the patch reaches the pod spec");
    assert_eq!(pod_patch.keys().collect::<Vec<_>>(), vec!["containers"]);
    let containers = pod_spec_mut(plural, obj)["containers"]
        .as_array_mut()
        .expect("containers");
    for container_patch in pod_patch["containers"].as_array().unwrap() {
        let name = container_patch["name"].as_str().unwrap();
        let Some(container) = containers.iter_mut().find(|c| c["name"] == name) else {
            // A strategic merge patch would add a container without an
            // image, which the API server rejects.
            return Err(format!("spec.containers[{name}].image: Required value"));
        };
        if container["env"].is_null() {
            container["env"] = json!([]);
        }
        for key in container_patch.as_object().unwrap().keys() {
            assert!(
                matches!(key.as_str(), "name" | "env" | "$setElementOrder/env"),
                "unexpected container field in patch: {key}"
            );
        }
        // The API server's rule (validatePatchWithSetOrderList): the entries
        // that aren't deletions are listed in the directive, in its order.
        if let Some(order) = container_patch["$setElementOrder/env"].as_array() {
            let mut last = None;
            for entry in container_patch["env"].as_array().unwrap() {
                if entry["$patch"] == "delete" {
                    continue;
                }
                let Some(position) = order.iter().position(|o| o["name"] == entry["name"]) else {
                    return Err(format!("{} is not in $setElementOrder", entry["name"]));
                };
                if last.is_some_and(|last| position < last) {
                    return Err(
                        "The order in patch list doesn't match $setElementOrder list".to_string(),
                    );
                }
                last = Some(position);
            }
        }
        let env = container["env"].as_array_mut().unwrap();
        for entry in container_patch["env"].as_array().unwrap() {
            let var = entry["name"].as_str().unwrap();
            if entry["$patch"] == "delete" {
                env.retain(|e| e["name"] != var);
            } else {
                match env.iter_mut().find(|e| e["name"] == var) {
                    Some(existing) => existing["value"] = entry["value"].clone(),
                    None => env.push(json!({ "name": var, "value": entry["value"] })),
                }
            }
        }
        // The listed names first, in that order; then anything unlisted.
        if let Some(order) = container_patch["$setElementOrder/env"].as_array() {
            let position = |e: &Value| {
                order
                    .iter()
                    .position(|o| o["name"] == e["name"])
                    .unwrap_or(order.len())
            };
            env.sort_by_key(position);
        }
    }
    Ok(())
}

impl FakeCluster {
    pub(crate) fn with_objects(objects: Vec<Value>) -> Self {
        let cluster = FakeCluster::default();
        cluster.add(objects);
        cluster
    }

    pub(crate) fn add(&self, objects: Vec<Value>) {
        let mut st = self.0.lock().unwrap();
        for obj in objects {
            let plural = match obj["kind"].as_str() {
                Some("Deployment") => "deployments",
                Some("StatefulSet") => "statefulsets",
                Some("CronJob") => "cronjobs",
                other => panic!("unsupported fixture kind {other:?}"),
            };
            let ns = obj["metadata"]["namespace"].as_str().unwrap().to_string();
            let name = obj["metadata"]["name"].as_str().unwrap().to_string();
            st.objects.insert((plural.to_string(), ns, name), obj);
        }
    }

    pub(crate) fn object(&self, plural: &str, ns: &str, name: &str) -> Value {
        self.0.lock().unwrap().objects[&(plural.to_string(), ns.to_string(), name.to_string())]
            .clone()
    }

    /// Change an object as another client would: `edit` it, and bump its
    /// resourceVersion.
    pub(crate) fn edit(&self, plural: &str, ns: &str, name: &str, edit: impl FnOnce(&mut Value)) {
        let mut st = self.0.lock().unwrap();
        st.next_resource_version += 1;
        let rv = format!("u{}", st.next_resource_version);
        let obj = st
            .objects
            .get_mut(&(plural.to_string(), ns.to_string(), name.to_string()))
            .unwrap();
        edit(obj);
        obj["metadata"]["resourceVersion"] = json!(rv);
    }

    pub(crate) fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.0.lock().unwrap().requests)
    }

    fn forbid(&self, plural: &str) {
        self.0.lock().unwrap().forbidden.insert(plural.to_string());
    }

    fn conflict_next(&self, n: usize) {
        self.0.lock().unwrap().conflicts = n;
    }

    fn drop_on_admission(&self, var: &str) {
        self.0.lock().unwrap().drops = Some(var.to_string());
    }

    pub(crate) fn client(&self) -> kube::Client {
        let cluster = self.clone();
        let service = tower::service_fn(move |req: axum::http::Request<kube::client::Body>| {
            let cluster = cluster.clone();
            async move {
                let (parts, body) = req.into_parts();
                let bytes = body.collect_bytes().await.unwrap_or_default();
                let (code, json) = cluster.handle(&parts, &bytes);
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

    fn handle(&self, parts: &axum::http::request::Parts, body: &[u8]) -> (u16, Value) {
        let path = parts.uri.path().to_string();
        let query = parts.uri.query().unwrap_or("").to_string();
        let body: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).expect("request body is JSON")
        };
        let mut st = self.0.lock().unwrap();
        st.requests.push(Request {
            method: parts.method.to_string(),
            path: path.clone(),
            body: body.clone(),
        });

        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (group_version, rest) = match segs.as_slice() {
            ["apis", group, version, rest @ ..] => (format!("{group}/{version}"), rest.to_vec()),
            // ConfigMaps (the settings migration), discovery: not served.
            _ => return status(404, "NotFound"),
        };
        let (namespace, plural, name) = match rest.as_slice() {
            [plural] => (None, *plural, None),
            ["namespaces", ns, plural] => (Some(*ns), *plural, None),
            ["namespaces", ns, plural, name] => (Some(*ns), *plural, Some(*name)),
            _ => return status(404, "NotFound"),
        };
        if !matches!(plural, "deployments" | "statefulsets" | "cronjobs") {
            return status(404, "NotFound");
        }
        let kind = kind_of(plural);

        match (parts.method.as_str(), name) {
            ("GET", None) => {
                if st.forbidden.contains(plural) {
                    return status(403, "Forbidden");
                }
                let selector = query_param(&query, "labelSelector");
                let items: Vec<Value> = st
                    .objects
                    .iter()
                    .filter(|((p, ns, _), obj)| {
                        p == plural
                            && namespace.is_none_or(|want| want == ns)
                            && matches_selector(obj, selector.as_deref())
                    })
                    .map(|(_, obj)| obj.clone())
                    .collect();
                let metadata_only = parts
                    .headers
                    .get("accept")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("PartialObjectMetadataList"));
                if metadata_only {
                    let items: Vec<Value> = items
                        .iter()
                        .map(|obj| {
                            json!({
                                "apiVersion": "meta.k8s.io/v1",
                                "kind": "PartialObjectMetadata",
                                "metadata": obj["metadata"],
                            })
                        })
                        .collect();
                    (
                        200,
                        json!({
                            "apiVersion": "meta.k8s.io/v1",
                            "kind": "PartialObjectMetadataList",
                            "metadata": { "resourceVersion": "1" },
                            "items": items,
                        }),
                    )
                } else {
                    (
                        200,
                        json!({
                            "apiVersion": group_version,
                            "kind": format!("{kind}List"),
                            "metadata": { "resourceVersion": "1" },
                            "items": items,
                        }),
                    )
                }
            }
            ("GET", Some(name)) => {
                let key = (
                    plural.to_string(),
                    namespace.unwrap_or_default().to_string(),
                    name.to_string(),
                );
                match st.objects.get(&key) {
                    Some(obj) => (200, obj.clone()),
                    None => status(404, "NotFound"),
                }
            }
            ("PATCH", Some(name)) => {
                assert_eq!(
                    parts
                        .headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok()),
                    Some("application/strategic-merge-patch+json"),
                    "deckwatch patches workloads with strategic merge patches"
                );
                assert_eq!(
                    query_param(&query, "fieldManager").as_deref(),
                    Some(FIELD_MANAGER)
                );
                if st.conflicts > 0 {
                    st.conflicts -= 1;
                    return status(409, "Conflict");
                }
                let key = (
                    plural.to_string(),
                    namespace.unwrap_or_default().to_string(),
                    name.to_string(),
                );
                let drops = st.drops.clone();
                st.next_resource_version += 1;
                let next = format!("p{}", st.next_resource_version);
                let Some(obj) = st.objects.get_mut(&key) else {
                    return status(404, "NotFound");
                };
                if let Some(want) = body["metadata"]["resourceVersion"].as_str() {
                    if obj["metadata"]["resourceVersion"].as_str() != Some(want) {
                        return status(409, "Conflict");
                    }
                }
                let before = obj.clone();
                let mut after = obj.clone();
                if let Err(e) = merge(plural, &mut after, &body) {
                    return status(422, &e);
                }
                if let Some(var) = drops {
                    if let Some(env) =
                        pod_spec_mut(plural, &mut after)["containers"][0]["env"].as_array_mut()
                    {
                        env.retain(|e| e["name"] != var.as_str());
                    }
                }
                if after != before {
                    after["metadata"]["resourceVersion"] = json!(next);
                }
                *obj = after;
                (200, obj.clone())
            }
            _ => status(405, "MethodNotAllowed"),
        }
    }
}

pub(crate) fn patches(requests: &[Request]) -> Vec<&Request> {
    requests.iter().filter(|r| r.method == "PATCH").collect()
}

fn workload(kind: &str, name: &str, app: Option<&str>, env: Value) -> Value {
    let (api_version, container) = match kind {
        "CronJob" => ("batch/v1", "job"),
        _ => ("apps/v1", "app"),
    };
    let mut labels = serde_json::Map::new();
    if let Some(app) = app {
        labels.insert(APP_LABEL.to_string(), json!(app));
    }
    // An API server omits an empty field rather than returning null.
    let mut primary = json!({ "name": container, "image": "shop:1" });
    if !env.is_null() {
        primary["env"] = env;
    }
    let pod_spec = json!({
        "containers": [
            primary,
            { "name": "proxy", "image": "proxy:1", "env": [{ "name": "PROXY_VAR", "value": "1" }] },
        ],
    });
    let spec = match kind {
        "CronJob" => json!({
            "schedule": "0 3 * * *",
            "jobTemplate": { "spec": { "template": { "spec": pod_spec } } },
        }),
        "StatefulSet" => json!({
            "serviceName": name,
            "selector": { "matchLabels": { "app": name } },
            "template": { "metadata": { "labels": { "app": name } }, "spec": pod_spec },
        }),
        _ => json!({
            "selector": { "matchLabels": { "app": name } },
            "template": { "metadata": { "labels": { "app": name } }, "spec": pod_spec },
        }),
    };
    json!({
        "apiVersion": api_version,
        "kind": kind,
        "metadata": {
            "name": name,
            "namespace": "team",
            "resourceVersion": "1",
            "labels": labels,
            "annotations": { "team.example.com/owner": "payments" },
        },
        "spec": spec,
    })
}

pub(crate) fn shop_workloads() -> Vec<Value> {
    vec![
        workload(
            "Deployment",
            "web",
            Some("shop"),
            json!([{ "name": "USER_VAR", "value": "1" }]),
        ),
        workload("StatefulSet", "db", Some("shop"), json!([])),
        workload("CronJob", "nightly", Some("shop"), Value::Null),
    ]
}

pub(crate) async fn test_state(cluster: &FakeCluster) -> AppState {
    let db = crate::db::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    AppState {
        kube_client: cluster.client(),
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-settings".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        plugin_telemetry: Default::default(),
    }
}

pub(crate) async fn set_endpoints(state: &AppState, otlp: &str, metrics: &str) {
    let settings = crate::handlers::settings::DeckwatchSettings {
        observability: Some(ObservabilitySettings {
            otlp_endpoint: otlp.to_string(),
            otlp_metrics_endpoint: metrics.to_string(),
        }),
        ..Default::default()
    };
    crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
        .await
        .expect("save settings");
}

pub(crate) async fn set_injection(state: &AppState, ns: &str, app: &str, config: OtelInjection) {
    crate::db::ensure_application(&state.db, ns, app)
        .await
        .expect("ensure application");
    let row = applications::Entity::find_by_id(format!("{ns}/{app}"))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: applications::ActiveModel = row.into();
    active.observability = Set(ApplicationObservability {
        opentelemetry: config,
    }
    .to_column());
    active.update(&state.db).await.expect("update application");
}

/// The primary container's env of a fake object, as name → value.
fn primary_env(cluster: &FakeCluster, plural: &str, name: &str) -> BTreeMap<String, String> {
    let mut obj = cluster.object(plural, "team", name);
    pod_spec_mut(plural, &mut obj)["containers"][0]["env"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["value"].as_str().unwrap_or("<valueFrom>").to_string(),
            )
        })
        .collect()
}

/// A fake object without what only the API server changes, to compare
/// before and after.
fn comparable(mut obj: Value) -> Value {
    obj["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("resourceVersion");
    obj
}

async fn owner_of(state: &AppState) -> String {
    crate::watcher::plugin_fields_owner(&state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn turning_injection_on_patches_every_kind_once() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();

    reconcile(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    let sent = patches(&requests);
    assert_eq!(sent.len(), 3, "{sent:#?}");
    for (plural, name, attribute) in [
        ("deployments", "web", "k8s.deployment.name=web"),
        ("statefulsets", "db", "k8s.statefulset.name=db"),
        ("cronjobs", "nightly", "k8s.cronjob.name=nightly"),
    ] {
        let env = primary_env(&cluster, plural, name);
        assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], COLLECTOR, "{plural}");
        assert_eq!(
            env["OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"], PROMETHEUS,
            "{plural}"
        );
        assert_eq!(
            env["OTEL_EXPORTER_OTLP_PROTOCOL"], "http/protobuf",
            "{plural}"
        );
        assert_eq!(env["OTEL_SERVICE_NAME"], name, "{plural}");
        assert!(
            env["OTEL_RESOURCE_ATTRIBUTES"].contains(attribute),
            "{plural}"
        );
        let mut obj = cluster.object(plural, "team", name);
        let record = Record::parse(
            obj["metadata"]["annotations"][RECORD_ANNOTATION]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(record.owner, owner_of(&state).await);
        assert_eq!(
            obj["metadata"]["annotations"]["team.example.com/owner"],
            "payments"
        );
        // Only the primary container.
        assert_eq!(
            pod_spec_mut(plural, &mut obj)["containers"][1]["env"],
            json!([{ "name": "PROXY_VAR", "value": "1" }])
        );
        // The pod template's metadata is untouched: only the env changed.
        assert!(obj["spec"]["template"]["metadata"]["annotations"].is_null());
    }
    assert_eq!(primary_env(&cluster, "deployments", "web")["USER_VAR"], "1");

    // Nothing changed, so nothing is patched (and nothing restarts).
    reconcile(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert!(patches(&requests).is_empty(), "{requests:#?}");
}

#[tokio::test]
async fn turning_injection_off_restores_every_workload() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let originals: Vec<Value> = shop_workloads().into_iter().map(comparable).collect();
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, "").await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_METRICS_EXPORTER"],
        "none"
    );
    cluster.take_requests();

    set_injection(&state, "team", "shop", OtelInjection::default()).await;
    reconcile(&state, &mut sweep).await;
    assert!(!sweep.retry);
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 3, "{requests:#?}");
    for (plural, name) in [
        ("deployments", "web"),
        ("statefulsets", "db"),
        ("cronjobs", "nightly"),
    ] {
        // The CronJob's primary container had no env at all; an empty list
        // is the same thing.
        let without_empty_env = |mut obj: Value| {
            let primary = &mut pod_spec_mut(plural, &mut obj)["containers"][0];
            if primary["env"] == json!([]) {
                primary.as_object_mut().unwrap().remove("env");
            }
            obj
        };
        let now = without_empty_env(comparable(cluster.object(plural, "team", name)));
        let original = originals
            .iter()
            .find(|o| o["metadata"]["name"] == name)
            .unwrap();
        assert_eq!(now, without_empty_env(original.clone()), "{plural}");
    }

    // And the sweep doesn't run again.
    reconcile(&state, &mut sweep).await;
    assert!(cluster.take_requests().is_empty());
}

#[tokio::test]
async fn values_the_user_set_win_and_outlive_injection() {
    let cluster = FakeCluster::with_objects(vec![workload(
        "Deployment",
        "web",
        Some("shop"),
        json!([{ "name": "OTEL_SERVICE_NAME", "value": "checkout" }]),
    )]);
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_SERVICE_NAME"],
        "checkout"
    );

    // The user then changes a variable deckwatch set, with an Update.
    cluster.edit("deployments", "team", "web", |obj| {
        for e in obj["spec"]["template"]["spec"]["containers"][0]["env"]
            .as_array_mut()
            .unwrap()
        {
            if e["name"] == "OTEL_EXPORTER_OTLP_PROTOCOL" {
                e["value"] = json!("grpc");
            }
        }
    });
    cluster.take_requests();
    reconcile(&state, &mut sweep).await;
    // Only the record changes (it lets go of the protocol); the value stays.
    let requests = cluster.take_requests();
    let sent = patches(&requests);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].body.get("spec").is_none(), "{:#?}", sent[0].body);
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_EXPORTER_OTLP_PROTOCOL"],
        "grpc"
    );

    set_injection(&state, "team", "shop", OtelInjection::default()).await;
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web"),
        env_map(&[
            ("OTEL_SERVICE_NAME", "checkout"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
        ])
    );
}

#[tokio::test]
async fn deleting_the_application_or_leaving_it_removes_the_variables() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;

    // The StatefulSet leaves the application.
    cluster.edit("statefulsets", "team", "db", |obj| {
        obj["metadata"]["labels"]
            .as_object_mut()
            .unwrap()
            .remove(APP_LABEL);
    });
    reconcile(&state, &mut sweep).await;
    assert!(primary_env(&cluster, "statefulsets", "db").is_empty());
    assert!(!primary_env(&cluster, "deployments", "web").is_empty());

    // The application is deleted.
    applications::Entity::delete_by_id("team/shop".to_string())
        .exec(&state.db)
        .await
        .unwrap();
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web"),
        env_map(&[("USER_VAR", "1")])
    );
    assert!(primary_env(&cluster, "cronjobs", "nightly").is_empty());
}

#[tokio::test]
async fn a_fresh_database_never_strips_another_databases_variables() {
    let foreign = Record {
        owner: "other-database".to_string(),
        container: "app".to_string(),
        env: env_map(&[("OTEL_SERVICE_NAME", &value_hash("web"))]),
    };
    let mut theirs = workload(
        "Deployment",
        "web",
        Some("shop"),
        json!([{ "name": "OTEL_SERVICE_NAME", "value": "web" }]),
    );
    theirs["metadata"]["annotations"][RECORD_ANNOTATION] = json!(foreign.encode());
    let cluster = FakeCluster::with_objects(vec![theirs]);
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    let mut sweep = Sweep::default();

    reconcile(&state, &mut sweep).await;
    assert!(patches(&cluster.take_requests()).is_empty());

    // Turning injection on here takes the record over.
    set_injection(&state, "team", "shop", on()).await;
    reconcile(&state, &mut sweep).await;
    let obj = cluster.object("deployments", "team", "web");
    let record = Record::parse(
        obj["metadata"]["annotations"][RECORD_ANNOTATION]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record.owner, owner_of(&state).await);
    assert!(record.env.contains_key("OTEL_SERVICE_NAME"));
}

#[tokio::test]
async fn the_same_patch_is_not_sent_again_to_an_unchanged_object() {
    let cluster =
        FakeCluster::with_objects(vec![workload("Deployment", "web", Some("shop"), json!([]))]);
    // An admission webhook drops the service name from every patch.
    cluster.drop_on_admission("OTEL_SERVICE_NAME");
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();

    reconcile(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 1);
    // The variable is missing, so it is added again, once: the object
    // doesn't change.
    reconcile(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 1);
    for _ in 0..3 {
        reconcile(&state, &mut sweep).await;
        assert!(patches(&cluster.take_requests()).is_empty());
    }
    // Once someone changes the object, deckwatch tries again.
    cluster.edit("deployments", "team", "web", |_| {});
    reconcile(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 1);
}

#[tokio::test]
async fn a_conflict_is_retried_with_the_object_read_again() {
    let cluster =
        FakeCluster::with_objects(vec![workload("Deployment", "web", Some("shop"), json!([]))]);
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    cluster.conflict_next(1);
    reconcile(&state, &mut sweep).await;
    assert!(primary_env(&cluster, "deployments", "web").is_empty());
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_SERVICE_NAME"],
        "web"
    );
    // Every patch carries the version it was computed from.
    let requests = cluster.take_requests();
    for patch in patches(&requests) {
        assert!(
            patch.body["metadata"]["resourceVersion"].is_string(),
            "{patch:#?}"
        );
    }
}

#[tokio::test]
async fn unreadable_settings_patch_nothing() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    cluster.take_requests();

    // A settings row that doesn't parse must not read as "no endpoints".
    use crate::entities::settings as settings_entity;
    let row = settings_entity::Entity::find_by_id("main".to_string())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: settings_entity::ActiveModel = row.into();
    active.value = Set("{broken".to_string());
    active.update(&state.db).await.unwrap();
    reconcile(&state, &mut sweep).await;
    assert!(cluster.take_requests().is_empty());
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_SERVICE_NAME"],
        "web"
    );

    // Neither do application settings that don't parse.
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    let row = applications::Entity::find_by_id("team/shop".to_string())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: applications::ActiveModel = row.into();
    active.observability = Set(Some("{broken".to_string()));
    active.update(&state.db).await.unwrap();
    let mut fresh = Sweep::default();
    reconcile(&state, &mut fresh).await;
    assert!(patches(&cluster.take_requests()).is_empty());
}

#[tokio::test]
async fn turning_off_with_settings_that_no_longer_validate_still_cleans_up() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    assert!(!primary_env(&cluster, "deployments", "web").is_empty());

    // Off, with an attribute stored before a rule existed: nothing is
    // injected, so nothing needs validating, and the leftovers go.
    let stale = OtelInjection {
        enabled: false,
        service_name: None,
        resource_attributes: vec![attr("k8s.namespace.name", "other")],
    };
    set_injection(&state, "team", "shop", stale).await;
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web"),
        env_map(&[("USER_VAR", "1")])
    );
}

#[tokio::test]
async fn adding_and_changing_variables_in_one_patch_is_accepted() {
    let cluster = FakeCluster::with_objects(vec![workload(
        "Deployment",
        "web",
        Some("shop"),
        json!([{ "name": "USER_VAR", "value": "1" }]),
    )]);
    let state = test_state(&cluster).await;
    set_endpoints(
        &state,
        "",
        "http://prometheus.monitoring:9090/old/v1/metrics",
    )
    .await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_TRACES_EXPORTER"],
        "none"
    );
    cluster.take_requests();

    // One save adds the collector (a new variable that sorts first) and
    // moves the metrics endpoint (an existing one).
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    reconcile(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert_eq!(patches(&requests).len(), 1, "{requests:#?}");
    let env = primary_env(&cluster, "deployments", "web");
    assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], COLLECTOR);
    assert_eq!(env["OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"], PROMETHEUS);
    assert!(!env.contains_key("OTEL_TRACES_EXPORTER"));
    assert!(!env.contains_key("OTEL_LOGS_EXPORTER"));
    // The workload's own variable stays first.
    let obj = cluster.object("deployments", "team", "web");
    assert_eq!(
        obj["spec"]["template"]["spec"]["containers"][0]["env"][0]["name"],
        "USER_VAR"
    );
    reconcile(&state, &mut sweep).await;
    assert!(patches(&cluster.take_requests()).is_empty());
}

#[tokio::test]
async fn a_variable_a_manifest_now_declares_outlives_injection() {
    let cluster =
        FakeCluster::with_objects(vec![workload("Deployment", "web", Some("shop"), json!([]))]);
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;

    // The workload's manifest, applied with `kubectl apply`, now declares
    // the service name deckwatch set, with the same value.
    cluster.edit("deployments", "team", "web", |obj| {
        let manifest = json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "spec": { "template": { "spec": { "containers": [
                { "name": "app", "env": [{ "name": "OTEL_SERVICE_NAME", "value": "web" }] },
            ] } } },
        });
        obj["metadata"]["annotations"]["kubectl.kubernetes.io/last-applied-configuration"] =
            json!(manifest.to_string());
    });
    set_injection(&state, "team", "shop", OtelInjection::default()).await;
    reconcile(&state, &mut sweep).await;
    assert_eq!(
        primary_env(&cluster, "deployments", "web"),
        env_map(&[("OTEL_SERVICE_NAME", "web")])
    );
}

#[tokio::test]
async fn another_databases_record_is_taken_over_once() {
    let foreign = |cluster: &FakeCluster| {
        cluster.edit("deployments", "team", "web", |obj| {
            let raw = obj["metadata"]["annotations"][RECORD_ANNOTATION]
                .as_str()
                .unwrap()
                .to_string();
            let mut record = Record::parse(&raw).unwrap();
            record.owner = "other-database".to_string();
            obj["metadata"]["annotations"][RECORD_ANNOTATION] = json!(record.encode());
        });
    };
    let cluster =
        FakeCluster::with_objects(vec![workload("Deployment", "web", Some("shop"), json!([]))]);
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;

    // Another database claims the workload: taken over once (a record-only
    // patch, the values are the same)...
    foreign(&cluster);
    cluster.take_requests();
    reconcile(&state, &mut sweep).await;
    assert_eq!(patches(&cluster.take_requests()).len(), 1);
    // ...and left to it when it claims it again, rather than taking turns.
    foreign(&cluster);
    cluster.take_requests();
    for _ in 0..2 {
        reconcile(&state, &mut sweep).await;
        assert!(patches(&cluster.take_requests()).is_empty());
    }
    let obj = cluster.object("deployments", "team", "web");
    let record = Record::parse(
        obj["metadata"]["annotations"][RECORD_ANNOTATION]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record.owner, "other-database");
    let live = LiveWorkload::from_deployment(&serde_json::from_value(obj).unwrap()).unwrap();
    let status = workload_status(&live, &both(), &on(), "shop", &owner_of(&state).await);
    assert_eq!(status.state, InjectionState::Foreign);
}

#[tokio::test]
async fn a_failing_database_patches_and_removes_nothing() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    cluster.take_requests();

    // Every query fails from now on: neither "no applications" nor "no
    // endpoints" may be read into it.
    state.db.clone().close().await.unwrap();
    reconcile(&state, &mut sweep).await;
    let mut fresh = Sweep::default();
    reconcile(&state, &mut fresh).await;
    assert!(cluster.take_requests().is_empty());
    assert_eq!(
        primary_env(&cluster, "deployments", "web")["OTEL_SERVICE_NAME"],
        "web"
    );
}

#[tokio::test]
async fn statefulsets_deckwatch_may_not_list_dont_stop_the_others() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    cluster.forbid("statefulsets");
    let state = test_state(&cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "team", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    assert!(
        !sweep.retry,
        "a refusal is no reason to sweep again every cycle"
    );
    assert!(!primary_env(&cluster, "deployments", "web").is_empty());
    assert!(!primary_env(&cluster, "cronjobs", "nightly").is_empty());
    assert!(primary_env(&cluster, "statefulsets", "db").is_empty());
    cluster.take_requests();
    // StatefulSets aren't asked for again for a while.
    reconcile(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert!(
        requests.iter().all(|r| !r.path.contains("statefulsets")),
        "{requests:#?}"
    );
}

#[tokio::test]
async fn only_allowed_namespaces_are_touched() {
    let mut elsewhere = workload("Deployment", "web", Some("shop"), json!([]));
    elsewhere["metadata"]["namespace"] = json!("other");
    let cluster = FakeCluster::with_objects(vec![elsewhere]);
    let mut state = test_state(&cluster).await;
    state.allowed_namespaces = vec!["team".to_string()];
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    set_injection(&state, "other", "shop", on()).await;
    let mut sweep = Sweep::default();
    reconcile(&state, &mut sweep).await;
    let requests = cluster.take_requests();
    assert!(patches(&requests).is_empty(), "{requests:#?}");
    assert!(
        requests
            .iter()
            .all(|r| r.path.contains("/namespaces/team/")),
        "the sweep lists allowed namespaces only: {requests:#?}"
    );
}
