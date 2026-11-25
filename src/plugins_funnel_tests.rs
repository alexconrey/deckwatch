//! The plugin call funnel against real extism calls (fixture plugins written
//! in WebAssembly text, see `plugin_fixture`).

use std::cell::RefCell;
use std::time::Duration;

use super::*;
use crate::plugin_events::{EventRateLimiter, PluginTelemetry, SECRET_MASK};
use crate::plugin_fixture::capture::capture;
use crate::plugin_fixture::{Behavior, FixturePlugin, LogLevel};

fn telemetry() -> PluginTelemetry {
    PluginTelemetry::default()
}

fn ctx() -> PluginContext {
    PluginContext {
        namespace: "shop".into(),
        deployment_name: "web".into(),
        ..Default::default()
    }
}

fn apply_with(plugin: &LoadedPlugin, t: &PluginTelemetry) -> CallOutcome<PluginResult> {
    let call = PluginCall::new(t, Trigger::DeploymentCreate)
        .namespace("shop")
        .application("shop")
        .deployment("web");
    run_plugin(plugin, &ctx(), &call)
}

fn apply(behavior: Behavior) -> CallOutcome<PluginResult> {
    let plugin = FixturePlugin::new()
        .export("apply", behavior)
        .loaded("fixture");
    apply_with(&plugin, &telemetry())
}

fn codes_of<T>(outcome: &CallOutcome<T>) -> Vec<&str> {
    outcome.events.iter().map(|e| e.code.as_str()).collect()
}

fn only_event<T>(outcome: &CallOutcome<T>) -> &PluginEvent {
    assert_eq!(outcome.events.len(), 1, "{:?}", outcome.events);
    &outcome.events[0]
}

// ── Results and events ───────────────────────────────────────────────────────

#[test]
fn apply_returns_the_result_and_its_events() {
    let outcome = apply(Behavior::output(
        r#"{"env_vars":[{"name":"DB_HOST","value":"db"}],
            "warnings":["tag missing"],
            "events":[{"severity":"info","code":"fixture.ready","message":"configured"}]}"#,
    ));
    let result = outcome.result.as_ref().expect("call succeeded");
    assert_eq!(result.env_vars[0].name, "DB_HOST");
    assert!(result.events.is_empty(), "events live on the outcome");
    assert_eq!(
        codes_of(&outcome),
        vec![codes::UNSPECIFIED, "fixture.ready"]
    );
    assert_eq!(outcome.health(), Health::Degraded);
    assert_eq!(outcome.plugin, "fixture");
    assert_eq!(outcome.export, Export::Apply);

    let reports = outcome.reports();
    assert_eq!(reports[0].plugin, "fixture");
    assert_eq!(reports[0].export, "apply");
}

#[test]
fn a_quiet_successful_call_is_healthy() {
    let outcome = apply(Behavior::output("{}"));
    assert!(outcome.result.is_some());
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.health(), Health::Healthy);
}

/// SDK 0.10.0 helpers copy each event into `errors`/`warnings`; it must be
/// reported once.
#[test]
fn sdk_legacy_copies_are_reported_once() {
    let outcome = apply(Behavior::output(
        r#"{"errors":["fixture.denied: AccessDenied"],
            "events":[{"severity":"error","code":"fixture.denied","message":"AccessDenied","retryable":true}]}"#,
    ));
    let e = only_event(&outcome);
    assert_eq!(e.code, "fixture.denied");
    assert!(e.retryable);
    assert_eq!(
        outcome.health(),
        Health::Failing,
        "an error event means failing"
    );
    assert!(outcome.result.is_some(), "but the result is still usable");
}

// ── Host-detected failures ───────────────────────────────────────────────────

#[test]
fn a_trap_is_plugin_trap() {
    let outcome = apply(Behavior::Trap);
    assert!(outcome.result.is_none());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::TRAP);
    assert_eq!(e.severity, EventSeverity::Error);
    assert!(e.message.contains("unreachable"), "{}", e.message);
    assert!(!e.message.contains("backtrace"), "{}", e.message);
    assert_eq!(outcome.health(), Health::Failing);
    assert_eq!(
        outcome.failure_summary(),
        format!("{}: {}", e.code, e.message)
    );
}

#[test]
fn running_past_the_time_limit_is_plugin_timeout() {
    let plugin = FixturePlugin::new()
        .export("apply", Behavior::Spin)
        .loaded("spinner");
    let t = PluginTelemetry::with_options(
        "",
        Some(Duration::from_millis(200)),
        EventRateLimiter::default(),
    );
    let outcome = apply_with(&plugin, &t);
    assert!(outcome.result.is_none());
    assert_eq!(only_event(&outcome).code, codes::TIMEOUT);
}

#[test]
fn error_set_text_is_the_plugins_own_error() {
    let outcome = apply(Behavior::Fail("could not reach the AWS API".into()));
    assert!(outcome.result.is_none());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::UNSPECIFIED);
    assert_eq!(e.severity, EventSeverity::Error);
    assert!(
        e.message.ends_with("could not reach the AWS API"),
        "{}",
        e.message
    );
}

#[test]
fn a_missing_required_export_is_load_failed() {
    let plugin = FixturePlugin::new()
        .export("metadata", Behavior::output("{}"))
        .loaded("no-apply");
    let outcome = apply_with(&plugin, &telemetry());
    assert!(outcome.result.is_none());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::LOAD_FAILED);
    assert_eq!(e.message, missing_export_message(Export::Apply));
}

#[test]
fn a_missing_deprovision_export_is_success() {
    let plugin = FixturePlugin::new()
        .export("apply", Behavior::output("{}"))
        .loaded("no-deprovision");
    let t = telemetry();
    let outcome = run_deprovision(
        &plugin,
        &ResourceDeprovisionRequest::default(),
        &PluginCall::new(&t, Trigger::Deprovision),
    );
    assert!(outcome.result.is_some());
    assert!(outcome.events.is_empty());
}

#[test]
fn a_module_that_does_not_compile_is_load_failed() {
    let mut plugin = FixturePlugin::new().loaded("broken");
    plugin.wasm_bytes = Arc::new(b"(module (func".to_vec());
    let outcome = apply_with(&plugin, &telemetry());
    assert_eq!(only_event(&outcome).code, codes::LOAD_FAILED);
    assert!(outcome.result.is_none());
}

#[test]
fn unreadable_output_is_output_invalid_with_the_serde_path() {
    let outcome = apply(Behavior::output(
        r#"{"env_vars":[{"name":"A"},{"name":7}]}"#,
    ));
    assert!(outcome.result.is_none());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::OUTPUT_INVALID);
    assert!(e.message.contains("env_vars[1].name"), "{}", e.message);
}

#[test]
fn output_that_is_not_json_is_output_invalid() {
    let outcome = apply(Behavior::output("this is not json"));
    assert!(outcome.result.is_none());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::OUTPUT_INVALID);
    assert!(e.message.contains("not valid JSON"), "{}", e.message);
}

/// One malformed event costs only itself: the result and the other events
/// survive, and the bad one is reported with its path.
#[test]
fn a_malformed_event_costs_only_itself() {
    let outcome = apply(Behavior::output(
        r#"{"env_vars":[{"name":"A","value":"1"}],
            "events":[{"severity":"warning","code":"fixture.ok","message":"fine"},
                      {"severity":"warning","code":"fixture.bad","message":42}]}"#,
    ));
    let result = outcome.result.as_ref().expect("result kept");
    assert_eq!(result.env_vars.len(), 1);
    assert_eq!(
        codes_of(&outcome),
        vec![codes::OUTPUT_INVALID, "fixture.ok"]
    );
    assert!(
        outcome.events[0].message.contains("events[1].message"),
        "{}",
        outcome.events[0].message
    );
}

#[test]
fn plugin_events_using_reserved_codes_are_moved_to_the_plugins_namespace() {
    let outcome = apply(Behavior::output(
        r#"{"events":[{"severity":"error","code":"plugin.k8s_apply_failed","message":"spoofed"}]}"#,
    ));
    assert_eq!(only_event(&outcome).code, "fixture.k8s_apply_failed");
}

// ── Secrets ──────────────────────────────────────────────────────────────────

fn secret_plugin(behavior: Behavior, stored: &str) -> LoadedPlugin {
    let mut plugin = FixturePlugin::new().export("apply", behavior).loaded("aws");
    plugin.metadata.config_schema = vec![ConfigField {
        key: "AWS_SECRET_ACCESS_KEY".into(),
        label: "Secret key".into(),
        field_type: ConfigFieldType::Secret,
        ..Default::default()
    }];
    plugin
        .config
        .insert("AWS_SECRET_ACCESS_KEY".into(), stored.into());
    plugin
        .config
        .insert("AWS_REGION".into(), "us-gov-west-1".into());
    plugin
}

/// Whatever form the host passed the secret in (plaintext, or ciphertext
/// while #251 is open) is masked in every event field.
#[test]
fn secret_config_values_are_masked_wherever_the_plugin_echoes_them() {
    let key = "fixture-encryption-key";
    let ciphertext = crate::crypto::encrypt(key, "wJalrXUtnFEMI").unwrap();
    for stored in ["wJalrXUtnFEMI", ciphertext.as_str()] {
        let output = format!(
            r#"{{"errors":["signature mismatch for wJalrXUtnFEMI"],
                "warnings":["config was {stored}"],
                "events":[{{"severity":"error","code":"aws.denied","message":"key wJalrXUtnFEMI",
                            "hint":"rotate {stored}","resource_id":"wJalrXUtnFEMI"}}]}}"#
        );
        let plugin = secret_plugin(Behavior::output(output), stored);
        let t = PluginTelemetry::new(key);
        let outcome = apply_with(&plugin, &t);
        let all = serde_json::to_string(&outcome.reports()).unwrap();
        assert!(!all.contains("wJalrXUtnFEMI"), "{all}");
        assert!(!all.contains(&ciphertext), "{all}");
        assert!(all.contains(SECRET_MASK), "{all}");
    }
}

#[test]
fn secrets_are_masked_in_the_plugins_own_error_text() {
    let plugin = secret_plugin(
        Behavior::Fail("bad key s3cr3t-value".into()),
        "s3cr3t-value",
    );
    let outcome = apply_with(&plugin, &telemetry());
    let e = only_event(&outcome);
    assert!(!e.message.contains("s3cr3t-value"), "{}", e.message);
    assert!(e.message.contains(SECRET_MASK));
}

#[test]
fn refusing_to_pass_deckwatchs_own_secrets_is_a_policy_rejection() {
    let mut plugin = FixturePlugin::new()
        .export("apply", Behavior::output("{}"))
        .loaded("greedy");
    plugin.inherit_env_keys = vec!["DECKWATCH_ENCRYPTION_KEY".into()];
    let outcome = apply_with(&plugin, &telemetry());
    let e = only_event(&outcome);
    assert_eq!(e.code, codes::POLICY_REJECTED);
    assert_eq!(e.severity, EventSeverity::Warning);
    assert!(e.message.contains("DECKWATCH_ENCRYPTION_KEY"));
    assert_eq!(outcome.health(), Health::Degraded);
}

// ── Spans and logs ───────────────────────────────────────────────────────────

/// extism turns the plugin's `log_*` calls into `extism::pdk` events on the
/// calling thread; they must sit inside the `plugin apply` span with its
/// fields, and so must the funnel's own event log lines.
#[test]
fn plugin_log_lines_are_inside_the_plugin_span() {
    let plugin = FixturePlugin::new()
        .export(
            "apply",
            Behavior::logs_then_output(
                &[(LogLevel::Warn, "assuming role arn:aws:iam::1:role/x")],
                r#"{"warnings":["slow API"]}"#,
            ),
        )
        .loaded("aws");
    let (outcome, events) = capture(|| apply_with(&plugin, &telemetry()));
    assert_eq!(outcome.health(), Health::Degraded);

    let pdk = events
        .iter()
        .find(|e| e.target == "extism::pdk" && e.message.contains("assuming role"))
        .expect("plugin log line captured");
    let span = pdk.span("plugin apply").expect("inside the plugin span");
    assert_eq!(span.fields["deckwatch.plugin.name"], "aws");
    assert_eq!(span.fields["deckwatch.plugin.version"], "0.0.1-fixture");
    assert_eq!(span.fields["deckwatch.plugin.export"], "apply");
    assert_eq!(span.fields["deckwatch.plugin.trigger"], "deployment_create");
    assert_eq!(span.fields["deckwatch.application"], "shop");
    assert_eq!(span.fields["k8s.namespace.name"], "shop");
    assert_eq!(span.fields["k8s.deployment.name"], "web");
    assert!(pdk.fields.contains_key("plugin"), "extism's instance id");

    let funnel = events
        .iter()
        .find(|e| e.target == "deckwatch::plugin_events")
        .expect("event logged");
    assert!(funnel.message.contains("slow API"));
    assert_eq!(funnel.level, tracing::Level::WARN);
    assert!(funnel.span("plugin apply").is_some());
}

#[test]
fn a_failed_call_marks_its_span_as_an_error() {
    let plugin = FixturePlugin::new()
        .export("provision", Behavior::Trap)
        .loaded("aws");
    let t = telemetry();
    let call = PluginCall::new(&t, Trigger::Provision)
        .namespace("shop")
        .application("shop")
        .resource("rds");
    let (_, events) =
        capture(|| run_provision(&plugin, &ResourceProvisionRequest::default(), &call));
    let logged = events
        .iter()
        .find(|e| e.target == "deckwatch::plugin_events")
        .unwrap_or_else(|| panic!("trap not logged; captured: {events:#?}"));
    let span = logged.span("plugin provision").unwrap();
    assert_eq!(span.fields["deckwatch.plugin.resource_id"], "rds");
    assert_eq!(span.fields["deckwatch.plugin.outcome"], "failing");
    assert_eq!(span.fields["otel.status_code"], "error");
    assert_eq!(span.fields["error.type"], codes::TRAP);
}

// ── Trace context ────────────────────────────────────────────────────────────

thread_local! {
    static TEST_TRACEPARENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn test_trace_context(_span: &tracing::Span) -> Option<String> {
    TEST_TRACEPARENT.with(|t| t.borrow().clone())
}

/// `trace_context` reaches the plugin only while a trace-context provider
/// (the OpenTelemetry layer) returns one, and only if it's a valid
/// traceparent. The provider is process-wide, so it reads a thread-local
/// here and other tests never see a context.
#[test]
fn trace_context_is_sent_only_when_tracing_is_on() {
    plugin_events::install_trace_context_provider(test_trace_context);
    let plugin = FixturePlugin::new()
        .export("apply", Behavior::LogInputThenOutput("{}".into()))
        .loaded("echo");
    let input_of = |events: &[crate::plugin_fixture::capture::CapturedEvent]| {
        events
            .iter()
            .find(|e| e.target == "extism::pdk")
            .map(|e| e.message.clone())
            .expect("input logged")
    };

    let (outcome, events) = capture(|| apply_with(&plugin, &telemetry()));
    assert!(!input_of(&events).contains("trace_context"));
    assert_eq!(outcome.trace_id, None);

    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    TEST_TRACEPARENT.with(|t| *t.borrow_mut() = Some(tp.to_string()));
    let (outcome, events) = capture(|| apply_with(&plugin, &telemetry()));
    assert!(
        input_of(&events).contains(&format!(r#""trace_context":"{tp}""#)),
        "{}",
        input_of(&events)
    );
    assert_eq!(
        outcome.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );

    TEST_TRACEPARENT.with(|t| *t.borrow_mut() = Some("not-a-traceparent".into()));
    let (outcome, events) = capture(|| apply_with(&plugin, &telemetry()));
    assert!(!input_of(&events).contains("trace_context"));
    assert_eq!(outcome.trace_id, None);
    TEST_TRACEPARENT.with(|t| *t.borrow_mut() = None);
}

// ── apply_plugins ────────────────────────────────────────────────────────────

#[test]
fn apply_plugins_keeps_going_past_a_failing_plugin_and_reports_both() {
    let good = FixturePlugin::new()
        .export(
            "apply",
            Behavior::output(
                r#"{"env_vars":[{"name":"FROM_GOOD","value":"1"}],"warnings":["heads up"]}"#,
            ),
        )
        .loaded("good");
    let bad = FixturePlugin::new()
        .export("apply", Behavior::Trap)
        .loaded("bad");
    let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
        containers: vec![Container {
            name: "web".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut annotations = BTreeMap::new();
    let t = telemetry();
    let call = PluginCall::new(&t, Trigger::DeploymentUpdate)
        .namespace("shop")
        .deployment("web");
    let report = apply_plugins(&[bad, good], &ctx(), &mut pod_spec, &mut annotations, &call);

    let env = pod_spec.containers[0].env.as_ref().unwrap();
    assert_eq!(env[0].name, "FROM_GOOD");
    assert_eq!(report.outcomes.len(), 2);
    let healths: Vec<_> = report
        .outcomes
        .iter()
        .map(|o| (o.plugin.as_str(), o.health()))
        .collect();
    assert_eq!(
        healths,
        vec![("bad", Health::Failing), ("good", Health::Degraded)]
    );
    let reports = report.reports();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].event.code, codes::TRAP);
    assert_eq!(reports[1].event.message, "heads up");
}

#[test]
fn a_service_account_conflict_is_a_policy_rejection_but_reapplying_is_not() {
    let plugin = FixturePlugin::new()
        .export(
            "apply",
            Behavior::output(r#"{"wants_service_account":{"name":"aws-sa"}}"#),
        )
        .loaded("aws");
    let t = telemetry();
    let call = PluginCall::new(&t, Trigger::DeploymentUpdate);

    for (current, conflict) in [
        (Some("team-sa"), true),
        (Some("aws-sa"), false),
        (None, false),
    ] {
        let mut pod_spec = k8s_openapi::api::core::v1::PodSpec {
            service_account_name: current.map(String::from),
            ..Default::default()
        };
        let report = apply_plugins(
            std::slice::from_ref(&plugin),
            &ctx(),
            &mut pod_spec,
            &mut BTreeMap::new(),
            &call,
        );
        let codes: Vec<_> = report
            .reports()
            .iter()
            .map(|r| r.event.code.clone())
            .collect();
        if conflict {
            assert_eq!(codes, vec![codes::POLICY_REJECTED], "{current:?}");
            assert_eq!(pod_spec.service_account_name.as_deref(), Some("team-sa"));
        } else {
            assert!(codes.is_empty(), "{current:?}: {codes:?}");
            assert_eq!(pod_spec.service_account_name.as_deref(), Some("aws-sa"));
        }
        assert_eq!(report.wanted_service_accounts.len(), 1);
    }
}
