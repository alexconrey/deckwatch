use super::*;
use crate::plugin_events::{codes, Health, PluginEventReport};
use crate::plugin_fixture::fake_events::FakeEventsApi;
use crate::plugin_status::{StatusKey, StatusUpdate, Transition};
use crate::plugins::{EventSeverity, PluginEvent};

fn report(severity: EventSeverity, code: &str, message: &str) -> PluginEventReport {
    PluginEventReport {
        plugin: "aws".into(),
        export: "apply".into(),
        event: PluginEvent {
            severity,
            code: code.into(),
            message: message.into(),
            ..Default::default()
        },
        trace_id: None,
    }
}

fn update(
    export: &str,
    trigger: &str,
    health: Health,
    events: Vec<PluginEventReport>,
) -> StatusUpdate {
    StatusUpdate {
        key: StatusKey::resource(
            "shop/web",
            "aws",
            if export == "apply" { "" } else { "rds" },
        ),
        export: export.into(),
        trigger: trigger.into(),
        health,
        events,
        ..Default::default()
    }
}

// ── Which Event an outcome calls for ─────────────────────────────────────────

#[test]
fn a_failure_is_a_warning_named_after_the_export_with_the_code_in_the_action() {
    let trap = report(
        EventSeverity::Error,
        codes::TRAP,
        "apply() trapped: unreachable",
    );
    let e = event_for(
        &update("apply", "reconcile", Health::Failing, vec![trap.clone()]),
        Some(Transition::Failing),
    )
    .unwrap();
    assert!(e.warning);
    assert_eq!(e.reason, reasons::APPLY_FAILED);
    assert_eq!(e.action, "Apply plugin.trap");
    assert_eq!(
        e.note,
        "plugin aws: plugin.trap: apply() trapped: unreachable"
    );

    let e = event_for(
        &update(
            "provision",
            "status_poll",
            Health::Failing,
            vec![trap.clone(), trap],
        ),
        None,
    )
    .unwrap();
    assert_eq!(e.reason, reasons::PROVISION_FAILED);
    assert_eq!(
        e.note,
        "plugin aws (rds): plugin.trap: apply() trapped: unreachable (+1 more)"
    );
}

#[test]
fn recovery_is_a_normal_event() {
    let e = event_for(
        &update("apply", "reconcile", Health::Healthy, vec![]),
        Some(Transition::Recovered),
    )
    .unwrap();
    assert!(!e.warning);
    assert_eq!(e.reason, reasons::RECOVERED);
    assert_eq!(e.action, "Apply");
}

#[test]
fn warnings_are_warning_events() {
    let slow = report(EventSeverity::Warning, "aws.slow", "throttled");
    let e = event_for(
        &update(
            "apply",
            "deployment_update",
            Health::Degraded,
            vec![slow.clone()],
        ),
        None,
    )
    .unwrap();
    assert_eq!((e.warning, e.reason), (true, reasons::APPLY_WARNING));
    let e = event_for(
        &update("provision", "provision", Health::Degraded, vec![slow]),
        None,
    )
    .unwrap();
    assert_eq!(e.reason, reasons::PROVISION_WARNING);
}

#[test]
fn an_operators_successful_provision_is_announced_but_quiet_polls_are_not() {
    let e = event_for(
        &update("provision", "provision", Health::Healthy, vec![]),
        None,
    )
    .unwrap();
    assert_eq!((e.warning, e.reason), (false, reasons::PROVISIONED));
    assert_eq!(e.note, "plugin aws provisioned (rds)");
    assert!(event_for(
        &update("provision", "refresh", Health::Healthy, vec![]),
        None
    )
    .is_some());
    assert!(event_for(
        &update("provision", "status_poll", Health::Healthy, vec![]),
        None
    )
    .is_none());
    assert!(event_for(&update("apply", "reconcile", Health::Healthy, vec![]), None).is_none());
}

#[test]
fn plugin_wide_outcomes_have_no_workload() {
    let mut load = update("fetch", "load", Health::Failing, vec![]);
    load.key = StatusKey::plugin_wide("aws");
    assert!(event_for(&load, Some(Transition::Failing)).is_none());
}

#[test]
fn notes_and_actions_fit_the_events_api() {
    let long = report(EventSeverity::Error, &"x".repeat(300), &"y".repeat(5000));
    let e = event_for(
        &update("apply", "reconcile", Health::Failing, vec![long]),
        None,
    )
    .unwrap();
    assert!(e.note.len() <= MAX_NOTE_BYTES);
    assert!(e.action.len() <= 128);
}

// ── Publishing ───────────────────────────────────────────────────────────────

fn failing() -> KubeEvent {
    event_for(
        &update(
            "apply",
            "reconcile",
            Health::Failing,
            vec![report(EventSeverity::Error, codes::TRAP, "trapped")],
        ),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn an_event_is_published_on_the_deployment_with_its_uid() {
    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let sink = KubeEventSink::new(api.kube().client());
    sink.publish(KubeEventRequest {
        application_id: "shop/shop".into(),
        targets: vec![WorkloadRef::deployment("shop", "web", None)],
        event: failing(),
    })
    .await;

    let events = api.events();
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e["type"], "Warning");
    assert_eq!(e["reason"], "PluginApplyFailed");
    assert_eq!(e["action"], "Apply plugin.trap");
    assert_eq!(e["note"], "plugin aws: plugin.trap: trapped");
    assert_eq!(e["reportingController"], "deckwatch");
    assert_eq!(e["regarding"]["kind"], "Deployment");
    assert_eq!(e["regarding"]["name"], "web");
    assert_eq!(
        e["regarding"]["uid"], "uid-web",
        "looked up for kubectl describe"
    );
    assert_eq!(e["metadata"]["namespace"], "shop");
}

/// Repeats are folded: within the republish interval nothing is sent; after
/// it the Recorder PATCHes the same Event's series instead of creating one.
#[tokio::test]
async fn repeats_are_folded_into_one_event_with_a_series() {
    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let kube = api.kube();
    let request = || KubeEventRequest {
        application_id: "shop/shop".into(),
        targets: vec![WorkloadRef::deployment(
            "shop",
            "web",
            Some("uid-web".into()),
        )],
        event: failing(),
    };

    let throttled = KubeEventSink::new(kube.client());
    for _ in 0..5 {
        throttled.publish(request()).await;
    }
    let writes = |kube: &crate::plugin_fixture::fake_kube::FakeKube| {
        kube.requests()
            .into_iter()
            .filter(|r| r.path.contains("events.k8s.io"))
            .map(|r| r.method)
            .collect::<Vec<_>>()
    };
    assert_eq!(writes(&kube), vec!["POST"], "four repeats folded here");

    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let kube = api.kube();
    let eager = KubeEventSink::with_interval(kube.client(), Duration::ZERO);
    for _ in 0..3 {
        eager.publish(request()).await;
    }
    assert_eq!(writes(&kube), vec!["POST", "PATCH", "PATCH"]);
    let events = api.events();
    assert_eq!(events.len(), 1, "one Event");
    assert_eq!(events[0]["series"]["count"], 3);

    // A different failure is a different Event.
    let mut other = request();
    other.event.action = action("Apply", Some(codes::TIMEOUT));
    eager.publish(other).await;
    assert_eq!(api.events().len(), 2);
}

#[tokio::test]
async fn with_no_targets_every_deployment_of_the_application_gets_it() {
    let api = FakeEventsApi::with_deployments(&[
        ("shop", "web", "shop"),
        ("shop", "worker", "shop"),
        ("shop", "other", "blog"),
    ]);
    let sink = KubeEventSink::new(api.kube().client());
    sink.publish(KubeEventRequest {
        application_id: "shop/shop".into(),
        targets: vec![],
        event: failing(),
    })
    .await;
    let mut names: Vec<String> = api
        .events()
        .iter()
        .map(|e| e["regarding"]["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["web", "worker"]);
}

#[tokio::test]
async fn a_rejected_event_is_logged_and_never_fails_the_caller() {
    // No Deployment and no Events API: every request 404s.
    let kube = crate::plugin_fixture::fake_kube::FakeKube::new(|_, _, _| None);
    let sink = KubeEventSink::new(kube.client());
    sink.publish(KubeEventRequest {
        application_id: "shop/shop".into(),
        targets: vec![WorkloadRef::deployment("shop", "web", Some("u".into()))],
        event: failing(),
    })
    .await;
    assert_eq!(kube.requests().len(), 1);
}

// ── Through the status worker ────────────────────────────────────────────────

#[tokio::test]
async fn the_status_worker_publishes_events_for_outcomes() {
    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let dir = tempfile::tempdir().unwrap();
    let kube = api.kube();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], Some(kube.client())).await;
    crate::plugin_status::spawn_worker_with(&state, KubeEventSink::new(kube.client()));

    let mut failing = update(
        "apply",
        "reconcile",
        Health::Failing,
        vec![report(EventSeverity::Error, codes::TRAP, "trapped")],
    );
    failing.key = StatusKey::apply("shop/shop", "aws");
    failing.targets = vec![WorkloadRef::deployment(
        "shop",
        "web",
        Some("uid-web".into()),
    )];
    state.plugin_telemetry.report(failing.clone());
    let mut healthy = failing.clone();
    healthy.health = Health::Healthy;
    healthy.events = vec![];
    state.plugin_telemetry.report(healthy);
    state.plugin_telemetry.flush().await;

    let reasons: Vec<String> = api
        .events()
        .iter()
        .map(|e| e["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(reasons, vec!["PluginApplyFailed", "PluginRecovered"]);
}
