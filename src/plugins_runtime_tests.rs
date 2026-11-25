//! Plugin calls run off the async workers (#385): on Tokio's blocking pool,
//! inside the caller's span, at most `MAX_CONCURRENT_PLUGIN_CALLS` at a
//! time, and no caller waits past `call_wait`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

use super::*;
use crate::plugin_events::EventRateLimiter;
use crate::plugin_events_acceptance_tests::provisioned;
use crate::plugin_fixture::capture::capture_async;
use crate::plugin_fixture::fake_events::FakeEventsApi;
use crate::plugin_fixture::{test_state, Behavior, FixturePlugin, LogLevel};
use crate::watcher::{reconcile_application_plugins, PluginFieldSweep};

/// How long the slow plugin holds its thread: its call's time limit.
const SLOW_CALL: Duration = Duration::from_millis(1500);

/// The kubelet's probe: `GET /api/healthz` through the router, in a task of
/// its own (so it needs a runtime worker, like a real request), and how long
/// the answer took.
async fn probe_healthz(app: &axum::Router) -> Duration {
    let app = app.clone();
    let start = Instant::now();
    let response = tokio::spawn(async move {
        app.oneshot(Request::get("/api/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    start.elapsed()
}

/// The reconcile loop ran plugins on a runtime worker, and a CPU limit under
/// 2 cores gives the runtime a single worker, so nothing else ran while a
/// plugin did: the liveness probe timed out and the kubelet restarted the
/// pod. Here the loop runs a plugin that holds its thread for 1.5 s on a
/// one-worker runtime, and `/api/healthz` must answer within 500 ms
/// throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn healthz_answers_while_the_reconcile_loop_runs_a_slow_plugin() {
    let spinner = FixturePlugin::new()
        .export("apply", Behavior::Spin)
        .loaded("spinner");
    let api = FakeEventsApi::with_deployments(&[("shop", "web", "shop")]);
    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state(dir.path(), vec![spinner], Some(api.kube().client())).await;
    // The spinner runs until the time limit stops it.
    state.plugin_telemetry =
        PluginTelemetry::with_options("", Some(SLOW_CALL), EventRateLimiter::default());
    provisioned(&state, "shop").await;
    let app = crate::routes::build_router(
        state.clone(),
        None,
        "/nonexistent",
        "/nonexistent",
        crate::auth::AuthConfig::from_settings(None),
    );

    let started = Instant::now();
    let reconcile = tokio::spawn(async move {
        reconcile_application_plugins(&state, &mut PluginFieldSweep::default()).await;
    });
    // Probe until the loop is done. The pauses happen on this thread, not on
    // the runtime: with its worker stuck in a plugin, its timers would stall
    // too.
    let mut latencies = Vec::new();
    while !reconcile.is_finished() {
        latencies.push(probe_healthz(&app).await);
        std::thread::sleep(Duration::from_millis(100));
    }
    reconcile.await.unwrap();

    let took = started.elapsed();
    assert!(
        took >= SLOW_CALL,
        "the plugin should hold its thread for {SLOW_CALL:?}, but the loop took {took:?}"
    );
    let slowest = latencies.iter().max().unwrap();
    assert!(
        *slowest < Duration::from_millis(500),
        "/api/healthz took {slowest:?} while the plugin ran: {latencies:?}"
    );
    assert!(latencies.len() >= 5, "only {} probes", latencies.len());
}

/// The call runs on another thread, yet its span is a child of the caller's
/// (here a request's), and the plugin's own log lines sit inside both: they
/// stay in the caller's trace.
#[tokio::test]
async fn a_plugin_call_stays_inside_the_callers_span() {
    let plugin = FixturePlugin::new()
        .export(
            "apply",
            Behavior::logs_then_output(&[(LogLevel::Info, "hello from the plugin")], "{}"),
        )
        .loaded("echo");
    let telemetry = PluginTelemetry::default();
    let (outcome, events) = capture_async(async {
        let call = PluginCall::new(&telemetry, Trigger::DeploymentCreate)
            .namespace("shop")
            .deployment("web");
        run_plugin(&plugin, &PluginContext::default(), &call)
            .instrument(tracing::info_span!(
                "http",
                path = "/api/namespaces/shop/deployments"
            ))
            .await
    })
    .await;
    assert!(outcome.result.is_some(), "{:?}", outcome.events);

    let line = events
        .iter()
        .find(|e| e.target == "extism::pdk" && e.message == "hello from the plugin")
        .unwrap_or_else(|| panic!("plugin log line not captured: {events:#?}"));
    let spans: Vec<&str> = line.spans.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(spans, ["http", "plugin apply"]);
    assert_eq!(
        line.spans[0].fields["path"],
        "/api/namespaces/shop/deployments"
    );
    assert_eq!(line.spans[1].fields["deckwatch.plugin.name"], "echo");
}

/// Each call holds a thread and compiles its module, so a burst of them
/// (many deployment saves at once) waits for slots instead of all running.
#[tokio::test]
async fn no_more_than_the_limit_of_plugin_calls_run_at_once() {
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let calls = (0..3 * MAX_CONCURRENT_PLUGIN_CALLS).map(|_| {
        let (running, peak) = (running.clone(), peak.clone());
        run_blocking(Duration::from_secs(30), move || {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            running.fetch_sub(1, Ordering::SeqCst);
        })
    });
    for result in futures::future::join_all(calls).await {
        result.expect("no call panicked");
    }
    let peak = peak.load(Ordering::SeqCst);
    assert!(
        (1..=MAX_CONCURRENT_PLUGIN_CALLS).contains(&peak),
        "{peak} calls ran at once"
    );
}

/// No call timeout is configured by default (#301), and then a plugin HTTP
/// request that never gets an answer never returns. Calls like that must not
/// hold their slots past the wait, or a few of them would stop every later
/// plugin call.
#[tokio::test]
async fn calls_that_never_return_give_up_their_slots_when_the_wait_runs_out() {
    // Enough of them to take every slot. Each blocks until its sender is
    // dropped at the end of the test.
    let mut releases = Vec::new();
    let mut hung = Vec::new();
    for _ in 0..MAX_CONCURRENT_PLUGIN_CALLS {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        releases.push(release);
        hung.push(run_blocking(Duration::from_millis(200), move || {
            let _ = blocked.recv();
        }));
    }
    let results = tokio::time::timeout(Duration::from_secs(30), futures::future::join_all(hung))
        .await
        .expect("the callers stop waiting");
    for result in results {
        assert!(
            matches!(result, Err(Lost::Abandoned(wait)) if wait == Duration::from_millis(200)),
            "{result:?}"
        );
    }

    // The abandoned calls are still blocked, yet the next call gets a slot.
    let next = tokio::time::timeout(
        Duration::from_secs(10),
        run_blocking(Duration::from_secs(10), || "ran"),
    )
    .await
    .expect("a later call isn't held up by the abandoned ones");
    assert_eq!(next.unwrap(), "ran");

    // Let the abandoned threads finish, so the runtime can shut down.
    drop(releases);
}

#[test]
fn callers_wait_a_little_longer_than_the_call_timeout_or_two_minutes() {
    assert_eq!(call_wait(None), Duration::from_secs(120));
    assert_eq!(
        call_wait(Some(Duration::from_secs(30))),
        Duration::from_secs(35)
    );
    assert_eq!(call_wait(Some(Duration::MAX)), Duration::MAX);
}

/// When deckwatch stops waiting, the call fails as a timeout that says the
/// plugin may still be running.
#[test]
fn an_abandoned_call_is_a_timeout() {
    let plugin = FixturePlugin::new().loaded("fixture");
    let telemetry = PluginTelemetry::default();
    let call = PluginCall::new(&telemetry, Trigger::Reconcile);
    let outcome = lost_call::<PluginResult>(
        &plugin,
        Export::Apply,
        &call,
        Duration::from_secs(120),
        &Lost::Abandoned(Duration::from_secs(120)),
    );
    assert!(outcome.result.is_none());
    assert_eq!(outcome.health(), Health::Failing);
    assert_eq!(
        outcome.failure_summary(),
        format!(
            "{}: apply() didn't finish within 120s; deckwatch stopped waiting for it, \
             but it may still be running",
            codes::TIMEOUT
        )
    );
}

/// A panic while calling the plugin (in extism or the funnel) fails that
/// call; the caller still gets an outcome to report.
#[tokio::test]
async fn a_call_that_panics_is_a_failed_call() {
    let plugin = FixturePlugin::new().loaded("fixture");
    let telemetry = PluginTelemetry::default();
    let call = PluginCall::new(&telemetry, Trigger::Provision).resource("rds");
    let outcome =
        off_runtime::<ResourceProvisionResult, _>(&plugin, Export::Provision, &call, |_, _| {
            panic!("a bug in the funnel")
        })
        .await;
    assert!(outcome.result.is_none());
    assert_eq!(outcome.health(), Health::Failing);
    assert_eq!(outcome.plugin, "fixture");
    assert_eq!(outcome.export, Export::Provision);
    assert_eq!(outcome.trigger, Trigger::Provision);
    assert_eq!(
        outcome.failure_summary(),
        format!(
            "{}: provision() crashed: deckwatch panicked while calling it",
            codes::TRAP
        )
    );
}
