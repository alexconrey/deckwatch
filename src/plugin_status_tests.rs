use super::*;
use crate::plugins::{EventSeverity, PluginEvent};
use sea_orm::EntityTrait;

fn t(secs: i64) -> DateTimeUtc {
    DateTimeUtc::from_timestamp(1_800_000_000 + secs, 0).unwrap()
}

fn report(message: &str, severity: EventSeverity) -> PluginEventReport {
    PluginEventReport {
        plugin: "aws".into(),
        export: "provision".into(),
        event: PluginEvent {
            severity,
            code: "aws.x".into(),
            message: message.into(),
            ..Default::default()
        },
        trace_id: None,
    }
}

fn update(health: Health, events: Vec<PluginEventReport>) -> StatusUpdate {
    StatusUpdate {
        key: StatusKey::resource("shop/web", "aws", "rds"),
        export: "provision".into(),
        health,
        events,
        trace_id: None,
        merge: false,
        ..Default::default()
    }
}

/// The row `next` would be stored as, at `at`.
fn stored(next: &NextRow, at: DateTimeUtc) -> plugin_status::Model {
    plugin_status::Model {
        id: "id".into(),
        application_id: "shop/web".into(),
        plugin_name: "aws".into(),
        resource_id: "rds".into(),
        health: next.health.as_str().into(),
        last_export: next.last_export.clone(),
        last_events: serde_json::to_string(&next.last_events).unwrap(),
        trace_id: next.trace_id.clone(),
        first_failure_at: next.first_failure_at,
        consecutive_failures: next.consecutive_failures,
        last_success_at: next.last_success_at,
        last_changed_at: next.last_changed_at,
        updated_at: at,
    }
}

// ── Transition rules ─────────────────────────────────────────────────────────

#[test]
fn a_first_failure_is_a_transition_and_starts_the_count() {
    let (next, transition) = next_row(
        None,
        &update(Health::Failing, vec![report("boom", EventSeverity::Error)]),
        t(0),
    );
    assert_eq!(transition, Some(Transition::Failing));
    assert_eq!(next.consecutive_failures, 1);
    assert_eq!(next.first_failure_at, Some(t(0)));
    assert_eq!(next.last_success_at, None);
    assert_eq!(next.last_changed_at, t(0));
}

#[test]
fn a_first_healthy_call_is_not_a_transition() {
    let (next, transition) = next_row(None, &update(Health::Healthy, vec![]), t(0));
    assert_eq!(transition, None);
    assert_eq!(next.consecutive_failures, 0);
    assert_eq!(next.last_success_at, Some(t(0)));
    assert_eq!(next.first_failure_at, None);
}

#[test]
fn repeated_failures_count_up_without_another_transition() {
    let (first, _) = next_row(None, &update(Health::Failing, vec![]), t(0));
    let row = stored(&first, t(0));
    let (second, transition) = next_row(Some(&row), &update(Health::Failing, vec![]), t(10));
    assert_eq!(transition, None);
    assert_eq!(second.consecutive_failures, 2);
    assert_eq!(second.first_failure_at, Some(t(0)), "the run started at t0");
    assert_eq!(second.last_changed_at, t(0));
}

#[test]
fn recovering_is_a_transition_that_resets_the_failure_run() {
    let (failing, _) = next_row(None, &update(Health::Failing, vec![]), t(0));
    let row = stored(&failing, t(0));
    let (next, transition) = next_row(
        Some(&row),
        &update(
            Health::Degraded,
            vec![report("slow", EventSeverity::Warning)],
        ),
        t(20),
    );
    assert_eq!(transition, Some(Transition::Recovered));
    assert_eq!(next.consecutive_failures, 0);
    assert_eq!(next.first_failure_at, None);
    assert_eq!(next.last_success_at, Some(t(20)));
    assert_eq!(next.last_changed_at, t(20));
}

#[test]
fn degrading_is_a_change_but_not_a_transition() {
    let (healthy, _) = next_row(None, &update(Health::Healthy, vec![]), t(0));
    let row = stored(&healthy, t(0));
    let (next, transition) = next_row(Some(&row), &update(Health::Degraded, vec![]), t(5));
    assert_eq!(transition, None);
    assert_eq!(next.health, Health::Degraded);
    assert_eq!(next.last_changed_at, t(5));
}

/// A Kubernetes object the call returned failed to apply afterwards: the
/// failure joins the call's row instead of replacing it.
#[test]
fn a_merge_adds_events_and_can_only_make_health_worse() {
    let (degraded, _) = next_row(
        None,
        &update(
            Health::Degraded,
            vec![report("slow", EventSeverity::Warning)],
        ),
        t(0),
    );
    let row = stored(&degraded, t(0));
    let mut merge = update(
        Health::Failing,
        vec![report("apply failed", EventSeverity::Error)],
    );
    merge.merge = true;
    merge.export = "apply".into();
    let (next, transition) = next_row(Some(&row), &merge, t(1));
    assert_eq!(transition, Some(Transition::Failing));
    assert_eq!(next.health, Health::Failing);
    assert_eq!(next.last_export, "provision", "still the call's row");
    let messages: Vec<_> = next
        .last_events
        .iter()
        .map(|r| r.event.message.as_str())
        .collect();
    assert_eq!(messages, vec!["slow", "apply failed"]);
    assert_eq!(next.consecutive_failures, 1);

    // Merging into an already failing row doesn't count another call.
    let row = stored(&next, t(1));
    let (again, transition) = next_row(Some(&row), &merge, t(2));
    assert_eq!(transition, None);
    assert_eq!(again.consecutive_failures, 1);

    // Merging something healthy can't improve the row.
    let mut ok = update(Health::Healthy, vec![]);
    ok.merge = true;
    let (still, _) = next_row(Some(&row), &ok, t(3));
    assert_eq!(still.health, Health::Failing);
}

#[test]
fn a_row_keeps_at_most_ten_events() {
    let events = (0..15)
        .map(|i| report(&format!("e{i}"), EventSeverity::Warning))
        .collect();
    let (next, _) = next_row(None, &update(Health::Degraded, events), t(0));
    assert_eq!(next.last_events.len(), MAX_STORED_EVENTS);
}

#[test]
fn unchanged_healthy_rows_are_rewritten_only_every_few_minutes() {
    let (healthy, _) = next_row(None, &update(Health::Healthy, vec![]), t(0));
    let row = stored(&healthy, t(0));
    let (same, _) = next_row(Some(&row), &update(Health::Healthy, vec![]), t(10));
    assert!(!needs_write(Some(&row), &same, t(10)));
    assert!(needs_write(Some(&row), &same, t(UNCHANGED_REWRITE_SECS)));

    let (changed, _) = next_row(
        Some(&row),
        &update(Health::Healthy, vec![report("note", EventSeverity::Info)]),
        t(10),
    );
    assert!(needs_write(Some(&row), &changed, t(10)), "new events");

    let (failing, _) = next_row(None, &update(Health::Failing, vec![]), t(0));
    let row = stored(&failing, t(0));
    let (again, _) = next_row(Some(&row), &update(Health::Failing, vec![]), t(10));
    assert!(needs_write(Some(&row), &again, t(10)), "the count moved");
    assert!(needs_write(None, &again, t(10)), "new row");
}

// ── Aggregation ──────────────────────────────────────────────────────────────

#[test]
fn reconcile_outcomes_are_combined_per_plugin_with_the_worst_health() {
    use crate::plugin_k8s_events::WorkloadRef;
    let dep = |name: &str| Some(WorkloadRef::deployment("shop", name, None));
    let mut agg = ApplyAggregate::default();
    let warn = report("slow", EventSeverity::Warning);
    agg.add("shop/web", "aws", dep("a"), Health::Healthy, vec![], None);
    agg.add(
        "shop/web",
        "aws",
        dep("b"),
        Health::Failing,
        vec![warn.clone()],
        Some("t1".into()),
    );
    agg.add(
        "shop/web",
        "aws",
        dep("c"),
        Health::Degraded,
        vec![warn.clone()],
        Some("t2".into()),
    );
    agg.add("shop/web", "vault", dep("a"), Health::Healthy, vec![], None);
    agg.add("shop/web", "vault", dep("b"), Health::Healthy, vec![], None);
    let updates: Vec<_> = agg.into_updates().collect();
    assert_eq!(updates.len(), 2);
    let aws = &updates[0];
    assert_eq!(aws.key, StatusKey::apply("shop/web", "aws"));
    assert_eq!(aws.health, Health::Failing);
    assert_eq!(aws.events, vec![warn], "each event once");
    assert_eq!(aws.trace_id.as_deref(), Some("t1"));
    assert_eq!(aws.trigger, "reconcile");
    let names = |u: &StatusUpdate| u.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(aws), vec!["b"], "only where it failed");
    assert_eq!(updates[1].health, Health::Healthy);
    assert_eq!(names(&updates[1]), vec!["a", "b"], "healthy everywhere");
}

#[test]
fn kubernetes_apply_failures_become_merge_updates_per_plugin() {
    let mut a = report("Job x failed", EventSeverity::Error);
    a.plugin = "aws".into();
    let mut b = report("Secret y failed", EventSeverity::Error);
    b.plugin = "vault".into();
    let updates = failures_by_plugin("shop/web", vec![a.clone(), b]);
    assert_eq!(updates.len(), 2);
    assert!(updates
        .iter()
        .all(|u| u.merge && u.health == Health::Failing));
    assert_eq!(updates[0].key, StatusKey::apply("shop/web", "aws"));
    assert_eq!(updates[0].events, vec![a]);
}

// ── Database and worker ──────────────────────────────────────────────────────

async fn db(dir: &std::path::Path) -> DatabaseConnection {
    crate::db::connect(&format!("sqlite://{}?mode=rwc", dir.join("s.db").display()))
        .await
        .unwrap()
}

#[tokio::test]
async fn updates_are_stored_and_report_their_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path()).await;

    let failing = update(Health::Failing, vec![report("boom", EventSeverity::Error)]);
    assert_eq!(
        apply_update(&db, &failing, t(0)).await.unwrap(),
        Some(Transition::Failing)
    );
    assert_eq!(apply_update(&db, &failing, t(10)).await.unwrap(), None);
    assert_eq!(
        apply_update(&db, &update(Health::Healthy, vec![]), t(20))
            .await
            .unwrap(),
        Some(Transition::Recovered)
    );

    let rows = plugin_status::Entity::find().all(&db).await.unwrap();
    assert_eq!(rows.len(), 1, "one row per target");
    let row = &rows[0];
    assert_eq!(row.health, "healthy");
    assert_eq!(row.consecutive_failures, 0);
    assert_eq!(row.last_success_at, Some(t(20)));
    assert_eq!(row.last_events, "[]");
}

#[tokio::test]
async fn targets_are_unique() {
    use sea_orm::ActiveModelTrait;
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path()).await;
    apply_update(&db, &update(Health::Healthy, vec![]), t(0))
        .await
        .unwrap();
    let duplicate = plugin_status::ActiveModel {
        id: Set("other".into()),
        application_id: Set("shop/web".into()),
        plugin_name: Set("aws".into()),
        resource_id: Set("rds".into()),
        health: Set("healthy".into()),
        last_export: Set("provision".into()),
        last_events: Set("[]".into()),
        trace_id: Set(None),
        first_failure_at: Set(None),
        consecutive_failures: Set(0),
        last_success_at: Set(None),
        last_changed_at: Set(t(0)),
        updated_at: Set(t(0)),
    };
    assert!(duplicate.insert(&db).await.is_err(), "unique index");
}

#[tokio::test]
async fn the_worker_applies_updates_and_removals_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    // Without a worker, reporting is a no-op.
    state
        .plugin_telemetry
        .report(update(Health::Failing, vec![]));
    state.plugin_telemetry.flush().await;
    spawn_worker(&state);

    let telemetry = &state.plugin_telemetry;
    telemetry.report(update(Health::Failing, vec![]));
    let mut other_app = update(Health::Degraded, vec![]);
    other_app.key = StatusKey::apply("blog/web", "aws");
    telemetry.report(other_app);
    let mut other_plugin = update(Health::Healthy, vec![]);
    other_plugin.key = StatusKey::plugin_wide("vault");
    telemetry.report(other_plugin);
    telemetry.flush().await;
    let rows = plugin_status::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(rows.len(), 3);

    telemetry.forget(StatusKey::resource("shop/web", "aws", "rds"));
    telemetry.flush().await;
    assert_eq!(
        plugin_status::Entity::find()
            .all(&state.db)
            .await
            .unwrap()
            .len(),
        2
    );

    telemetry.keep_plugins(vec!["aws".into()]);
    telemetry.flush().await;
    let rows = plugin_status::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].application_id, "blog/web");

    telemetry.forget_application("blog/web");
    telemetry.flush().await;
    assert!(plugin_status::Entity::find()
        .all(&state.db)
        .await
        .unwrap()
        .is_empty());
}

// ── Notifications ────────────────────────────────────────────────────────────

/// One notification per transition: repeated failures and recoveries don't
/// add any, and a target that flaps back to failing within the flap guard
/// is quiet (its recovery too).
#[tokio::test]
async fn transitions_notify_once_and_flapping_is_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::plugin_fixture::test_state(dir.path(), vec![], None).await;
    let (url, received) = crate::plugin_fixture::serve_webhook().await;
    crate::plugin_fixture::enable_notifications(&state, &url).await;
    spawn_worker(&state);

    let failing = update(
        Health::Failing,
        vec![report("db unreachable", EventSeverity::Error)],
    );
    let healthy = update(Health::Healthy, vec![]);
    let t = &state.plugin_telemetry;
    for u in [
        &failing, &failing, &failing, &healthy, &healthy, &failing, &healthy,
    ] {
        t.report(u.clone());
    }
    t.flush().await;

    let received = received.lock().unwrap();
    let kinds: Vec<&str> = received
        .iter()
        .map(|b| b["event_type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["plugin_failing", "plugin_recovered"],
        "{received:#?}"
    );
    let failing = &received[0]["event"];
    assert_eq!(failing["namespace"], "shop");
    assert_eq!(failing["application"], "web");
    assert_eq!(failing["plugin"], "aws");
    assert_eq!(failing["resource_id"], "rds");
    assert_eq!(failing["code"], "aws.x");
    assert_eq!(failing["message"], "db unreachable");
}
