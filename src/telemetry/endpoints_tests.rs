use super::*;
use crate::telemetry::Signals;

/// An environment holding only `vars`.
fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

const COLLECTOR: &str = "http://otel-collector.observability:4318";
const PROMETHEUS: &str = "http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics";

fn settings(otlp_endpoint: &str, otlp_metrics_endpoint: &str) -> ObservabilitySettings {
    ObservabilitySettings {
        otlp_endpoint: otlp_endpoint.to_string(),
        otlp_metrics_endpoint: otlp_metrics_endpoint.to_string(),
    }
}

/// `(source, endpoint, from)` of every signal, in traces, metrics, logs order.
fn sources(plan: &Plan) -> Vec<(Source, Option<&str>, Option<&str>)> {
    plan.all()
        .iter()
        .map(|s| (s.source, s.endpoint.as_deref(), s.from.as_deref()))
        .collect()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn endpoints_in_settings_are_http_urls_without_credentials() {
    for (field, value, normalized) in [
        (EndpointField::Otlp, "", ""),
        (EndpointField::Otlp, "   ", ""),
        (EndpointField::Otlp, COLLECTOR, COLLECTOR),
        (
            EndpointField::Otlp,
            " http://otel-collector.observability:4318/ ",
            COLLECTOR,
        ),
        (
            EndpointField::Otlp,
            "https://otlp.example.com/otlp",
            "https://otlp.example.com/otlp",
        ),
        (
            EndpointField::Otlp,
            "http://[::1]:4318",
            "http://[::1]:4318",
        ),
        (EndpointField::OtlpMetrics, PROMETHEUS, PROMETHEUS),
        (
            EndpointField::OtlpMetrics,
            "https://otel-collector.example.com/v1/metrics",
            "https://otel-collector.example.com/v1/metrics",
        ),
    ] {
        assert_eq!(
            validate_endpoint(field, value).as_deref(),
            Ok(normalized),
            "{field:?} {value:?}"
        );
    }
}

#[test]
fn invalid_endpoints_say_what_is_wrong() {
    let too_long = format!("http://otel-collector.example.com/{}", "a".repeat(2048));
    for (field, value, expected) in [
        // The retired gRPC setting's format.
        (
            EndpointField::Otlp,
            "deckwatch-tempo.deckwatch.svc:4317",
            "an http:// or https:// URL is required",
        ),
        (
            EndpointField::Otlp,
            "grpc://collector:4317",
            "the scheme must be http or https",
        ),
        (
            EndpointField::Otlp,
            "ftp://collector",
            "the scheme must be http or https",
        ),
        (
            EndpointField::Otlp,
            "https://user:secret@otlp.example.com",
            "must not contain a username or password",
        ),
        (
            EndpointField::Otlp,
            "https://token@otlp.example.com",
            "must not contain a username or password",
        ),
        (
            EndpointField::Otlp,
            "https://otlp.example.com/?api-key=secret",
            "must not contain a query string or fragment",
        ),
        (
            EndpointField::Otlp,
            "https://otlp.example.com/#x",
            "must not contain a query string or fragment",
        ),
        (
            EndpointField::Otlp,
            "http://otel-collector:4318/v1/traces",
            "without /v1/traces: deckwatch appends /v1/traces and /v1/logs",
        ),
        (
            EndpointField::Otlp,
            "http://otel-collector:4318/v1/metrics/",
            "without /v1/metrics",
        ),
        (
            EndpointField::Otlp,
            &too_long,
            "longer than 2048 characters",
        ),
        (EndpointField::Otlp, "http://", "not a valid URL"),
        (
            EndpointField::OtlpMetrics,
            "http://prometheus.monitoring:9090",
            "enter the full URL of the metrics receiver",
        ),
        (
            EndpointField::OtlpMetrics,
            "http://prometheus.monitoring:9090/",
            "enter the full URL of the metrics receiver",
        ),
    ] {
        let err = validate_endpoint(field, value).expect_err(value);
        assert!(err.starts_with(field.setting()), "{err}");
        assert!(err.contains(expected), "{value}: {err}");
    }
}

#[test]
fn settings_validation_reports_both_fields() {
    let err = settings("collector:4317", "http://prometheus:9090")
        .validated()
        .unwrap_err();
    assert!(err.contains("observability.otlp_endpoint: "), "{err}");
    assert!(
        err.contains("; observability.otlp_metrics_endpoint: "),
        "{err}"
    );

    assert_eq!(
        settings(&format!("{COLLECTOR}/"), PROMETHEUS).validated(),
        Ok(settings(COLLECTOR, PROMETHEUS))
    );
    assert!(ObservabilitySettings::default().is_empty());
    assert!(!settings("", PROMETHEUS).is_empty());
}

#[test]
fn signal_paths_join_like_the_sdk() {
    assert_eq!(
        join(COLLECTOR, "/v1/traces"),
        format!("{COLLECTOR}/v1/traces")
    );
    assert_eq!(
        join("http://collector:4318/", "/v1/logs"),
        "http://collector:4318/v1/logs"
    );
    assert_eq!(
        join("https://gateway.example.com/otlp", "/v1/traces"),
        "https://gateway.example.com/otlp/v1/traces"
    );
}

// ---------------------------------------------------------------------------
// Precedence
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_exported_without_an_endpoint() {
    let plan = Plan::resolve(env(&[]), &ObservabilitySettings::default());
    assert_eq!(sources(&plan), [(Source::Unset, None, None); 3], "{plan:?}");
    assert!(!plan.signals().any());
    assert!(plan.notices.is_empty());
}

#[test]
fn settings_alone_configure_every_signal() {
    let plan = Plan::resolve(env(&[]), &settings(COLLECTOR, PROMETHEUS));
    let traces = format!("{COLLECTOR}/v1/traces");
    let logs = format!("{COLLECTOR}/v1/logs");
    assert_eq!(
        sources(&plan),
        [
            (
                Source::Settings,
                Some(traces.as_str()),
                Some("observability.otlp_endpoint")
            ),
            (
                Source::Settings,
                Some(PROMETHEUS),
                Some("observability.otlp_metrics_endpoint")
            ),
            (
                Source::Settings,
                Some(logs.as_str()),
                Some("observability.otlp_endpoint")
            ),
        ]
    );
    assert!(plan.all().iter().all(SignalPlan::exported));
    assert!(plan.notices.is_empty(), "{:?}", plan.notices);
}

#[test]
fn the_collector_endpoint_does_not_carry_metrics() {
    let plan = Plan::resolve(env(&[]), &settings(COLLECTOR, ""));
    assert_eq!(plan.signal(Signal::Traces).source, Source::Settings);
    assert_eq!(plan.signal(Signal::Logs).source, Source::Settings);
    assert_eq!(plan.signal(Signal::Metrics).source, Source::Unset);

    let plan = Plan::resolve(env(&[]), &settings("", PROMETHEUS));
    assert_eq!(
        plan.signals(),
        Signals {
            metrics: true,
            ..Signals::default()
        }
    );
}

#[test]
fn the_generic_endpoint_variable_wins_over_settings_for_every_signal() {
    let plan = Plan::resolve(
        env(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://env-collector:4318/")]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    let from = Some("OTEL_EXPORTER_OTLP_ENDPOINT");
    assert_eq!(
        sources(&plan),
        [
            (
                Source::Env,
                Some("http://env-collector:4318/v1/traces"),
                from
            ),
            (
                Source::Env,
                Some("http://env-collector:4318/v1/metrics"),
                from
            ),
            (Source::Env, Some("http://env-collector:4318/v1/logs"), from),
        ]
    );
}

#[test]
fn a_signal_variable_wins_only_for_its_signal() {
    let plan = Plan::resolve(
        env(&[(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://tempo.observability:4318/v1/traces",
        )]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    let logs = format!("{COLLECTOR}/v1/logs");
    assert_eq!(
        sources(&plan),
        [
            (
                Source::Env,
                Some("http://tempo.observability:4318/v1/traces"),
                Some("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            ),
            (
                Source::Settings,
                Some(PROMETHEUS),
                Some("observability.otlp_metrics_endpoint")
            ),
            (
                Source::Settings,
                Some(logs.as_str()),
                Some("observability.otlp_endpoint")
            ),
        ]
    );

    // A signal's own variable also wins over the generic one.
    let plan = Plan::resolve(
        env(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://env-collector:4318"),
            (
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                "http://prometheus:9090/api/v1/otlp/v1/metrics",
            ),
        ]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    assert_eq!(
        plan.signal(Signal::Metrics).endpoint.as_deref(),
        Some("http://prometheus:9090/api/v1/otlp/v1/metrics")
    );
    assert_eq!(plan.signal(Signal::Logs).source, Source::Env);
}

#[test]
fn a_blank_variable_leaves_the_signal_to_settings() {
    let plan = Plan::resolve(
        env(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "  "),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", ""),
        ]),
        &settings(COLLECTOR, ""),
    );
    assert_eq!(plan.signal(Signal::Traces).source, Source::Settings);
    assert_eq!(plan.signal(Signal::Logs).source, Source::Settings);
}

#[test]
fn the_environment_can_turn_off_signals_from_settings() {
    let plan = Plan::resolve(
        env(&[("OTEL_LOGS_EXPORTER", "none")]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    let logs = plan.signal(Signal::Logs);
    assert_eq!(logs.source, Source::Settings);
    assert!(!logs.exported());
    assert_eq!(logs.off.as_deref(), Some("OTEL_LOGS_EXPORTER=none"));
    assert!(plan.signal(Signal::Traces).exported());
    assert!(plan.notices.is_empty());

    let plan = Plan::resolve(
        env(&[("OTEL_SDK_DISABLED", "true")]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    assert!(!plan.signals().any());
    assert!(plan
        .all()
        .iter()
        .all(|s| s.off.as_deref() == Some("OTEL_SDK_DISABLED=true")));
    assert!(plan.notices.is_empty());
}

#[test]
fn an_unsupported_protocol_applies_to_endpoints_from_settings_too() {
    let plan = Plan::resolve(
        env(&[("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")]),
        &settings(COLLECTOR, ""),
    );
    assert!(!plan.signals().any());
    assert_eq!(
        plan.notices,
        ["traces", "logs"].map(|name| Notice::Error(format!(
            "OTEL_EXPORTER_OTLP_PROTOCOL=grpc is not supported: deckwatch exports OTLP as \
             http/protobuf only; {name} are not exported"
        )))
    );
}

#[test]
fn exporter_headers_are_not_sent_to_endpoints_from_settings() {
    // Traces from the environment get the headers; logs, from settings, don't.
    let plan = Plan::resolve(
        env(&[
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "https://otlp.example.com/v1/traces",
            ),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer secret"),
        ]),
        &settings(COLLECTOR, ""),
    );
    assert_eq!(
        plan.notices,
        [Notice::Warn(
            "OTEL_EXPORTER_OTLP_HEADERS is not sent to the logs endpoint from settings: \
             exporter headers go only to an endpoint set in the environment"
                .to_string()
        )]
    );

    // A signal's own headers variable is the one the SDK would send.
    let plan = Plan::resolve(
        env(&[("OTEL_EXPORTER_OTLP_METRICS_HEADERS", "x-api-key=secret")]),
        &settings("", PROMETHEUS),
    );
    assert_eq!(plan.notices.len(), 1);
    assert!(
        matches!(&plan.notices[0], Notice::Warn(m) if m.starts_with("OTEL_EXPORTER_OTLP_METRICS_HEADERS is not sent to the metrics endpoint")),
        "{:?}",
        plan.notices
    );

    // No signal from settings, no warning.
    let plan = Plan::resolve(
        env(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otlp.example.com"),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer secret"),
        ]),
        &settings(COLLECTOR, PROMETHEUS),
    );
    assert!(plan.notices.is_empty(), "{:?}", plan.notices);
}

#[test]
fn an_invalid_endpoint_stored_in_settings_is_reported_and_not_used() {
    let plan = Plan::resolve(env(&[]), &settings("collector:4317", PROMETHEUS));
    assert_eq!(plan.signal(Signal::Traces).source, Source::Unset);
    assert_eq!(plan.signal(Signal::Metrics).source, Source::Settings);
    assert_eq!(plan.notices.len(), 1, "{:?}", plan.notices);
    assert!(
        matches!(&plan.notices[0], Notice::Error(m) if m.starts_with("observability.otlp_endpoint: ") && m.ends_with("; it is not used")),
        "{:?}",
        plan.notices
    );
}

#[test]
fn the_status_masks_credentials_from_the_environment() {
    let plan = Plan::resolve(
        env(&[(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "https://user:secret@otlp.example.com:4318/otlp",
        )]),
        &ObservabilitySettings::default(),
    );
    let status = plan.signal(Signal::Traces).status();
    assert_eq!(
        status,
        SignalStatus {
            source: Source::Env,
            endpoint: "https://***@otlp.example.com:4318/otlp/v1/traces".to_string(),
            from: Some("OTEL_EXPORTER_OTLP_ENDPOINT".to_string()),
            exported: true,
            reason: None,
        }
    );
    assert!(!serde_json::to_string(&status).unwrap().contains("secret"));
    assert_eq!(
        Plan::resolve(env(&[]), &ObservabilitySettings::default())
            .signal(Signal::Logs)
            .status(),
        SignalStatus::unset()
    );
}

#[test]
fn statuses_serialize_for_the_api() {
    let plan = Plan::resolve(
        env(&[("OTEL_TRACES_EXPORTER", "none")]),
        &settings(COLLECTOR, ""),
    );
    assert_eq!(
        serde_json::to_value(plan.signal(Signal::Traces).status()).unwrap(),
        serde_json::json!({
            "source": "settings",
            "endpoint": format!("{COLLECTOR}/v1/traces"),
            "from": "observability.otlp_endpoint",
            "exported": false,
            "reason": "OTEL_TRACES_EXPORTER=none",
        })
    );
    assert_eq!(
        serde_json::to_value(Signal::ALL).unwrap(),
        serde_json::json!(["traces", "metrics", "logs"])
    );
}
