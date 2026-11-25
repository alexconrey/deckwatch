use std::time::{Duration, Instant};

use super::*;
use crate::plugin_events::{codes, Health};
use crate::plugin_fixture::{Behavior, FixturePlugin};
use crate::plugins::{PluginCall, PluginContext, Trigger};
use crate::test_tracing::capture;

/// #301: deckwatch built its plugin telemetry without a call timeout, so no
/// plugin call had a time limit and `plugin.timeout` could never happen.
#[test]
fn deckwatch_gives_plugin_calls_a_time_limit_by_default() {
    let config = Config::try_parse_from(["deckwatch"]).unwrap();
    assert_eq!(
        plugin_telemetry(&config).call_timeout(),
        Some(Duration::from_secs(60))
    );
}

/// A plugin call that runs past the configured limit is stopped by extism
/// and fails with `plugin.timeout`, well before deckwatch would stop waiting
/// for it (the limit plus 5 s), whose `plugin.timeout` reads differently.
#[tokio::test]
async fn a_plugin_call_past_the_configured_time_limit_is_plugin_timeout() {
    let config = Config::try_parse_from(["deckwatch", "--plugin-call-timeout", "1"]).unwrap();
    let telemetry = plugin_telemetry(&config);
    let spinner = FixturePlugin::new()
        .export("apply", Behavior::Spin)
        .loaded("spinner");
    let call = PluginCall::new(&telemetry, Trigger::Reconcile)
        .namespace("shop")
        .deployment("web");

    let started = Instant::now();
    let outcome = plugins::run_plugin(&spinner, &PluginContext::default(), &call).await;
    let took = started.elapsed();

    assert!(outcome.result.is_none());
    assert_eq!(outcome.health(), Health::Failing);
    assert_eq!(outcome.events.len(), 1, "{:?}", outcome.events);
    assert_eq!(
        outcome.failure_summary(),
        format!(
            "{}: apply() was stopped after running past its time limit",
            codes::TIMEOUT
        )
    );
    assert!(took >= Duration::from_secs(1), "stopped after {took:?}");
}

#[tokio::test]
async fn the_database_password_is_never_logged() {
    let ((), captured) = capture(async {
        log_connecting_to_database(
            "postgres://deckwatch:userinfo-pw@db.example:5432/deckwatch?password=query-pw",
        );
    })
    .await;

    let events = captured.events_on("deckwatch");
    let event = events
        .iter()
        .find(|e| e.message == "connecting to database")
        .expect("the connecting line is logged");
    assert_eq!(event.fields["backend"], "postgres");
    assert_eq!(
        event.fields["url"],
        "postgres://***@db.example:5432/deckwatch?password=***"
    );
    let everything = captured.haystack();
    assert!(!everything.contains("userinfo-pw"), "{everything}");
    assert!(!everything.contains("query-pw"), "{everything}");
}
