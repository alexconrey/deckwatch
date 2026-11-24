use super::*;
use crate::plugin_events::codes;
use crate::plugin_fixture::{test_state, Behavior, FixturePlugin};
use crate::plugin_status::{spawn_worker, StatusKey, StatusUpdate};
use crate::plugins::{EventSeverity, PluginEvent, PluginResource};

fn status(key: StatusKey, health: Health, message: &str) -> StatusUpdate {
    StatusUpdate {
        key,
        export: "apply".into(),
        health,
        events: vec![PluginEventReport {
            plugin: "aws".into(),
            export: "apply".into(),
            event: PluginEvent {
                severity: EventSeverity::Warning,
                code: "aws.x".into(),
                message: message.into(),
                ..Default::default()
            },
            trace_id: None,
        }],
        trace_id: None,
        merge: false,
        ..Default::default()
    }
}

async fn seeded(dir: &std::path::Path, allowed: Vec<String>) -> AppState {
    let mut state = test_state(dir, vec![], None).await;
    state.allowed_namespaces = allowed;
    spawn_worker(&state);
    let t = &state.plugin_telemetry;
    t.report(status(
        StatusKey::apply("shop/web", "aws"),
        Health::Degraded,
        "slow",
    ));
    t.report(status(
        StatusKey::resource("shop/web", "aws", "rds"),
        Health::Failing,
        "down",
    ));
    t.report(status(
        StatusKey::apply("blog/site", "aws"),
        Health::Healthy,
        "fine",
    ));
    t.report(status(
        StatusKey::apply("secret/app", "aws"),
        Health::Failing,
        "hidden",
    ));
    t.report(status(
        StatusKey::plugin_wide("vault"),
        Health::Failing,
        "fetch failed",
    ));
    t.flush().await;
    state
}

fn names(views: &[PluginStatusView]) -> Vec<String> {
    views
        .iter()
        .map(|v| {
            format!(
                "{}/{}:{}:{}",
                v.namespace, v.application, v.plugin, v.resource_id
            )
        })
        .collect()
}

#[tokio::test]
async fn problems_are_listed_most_severe_first() {
    let dir = tempfile::tempdir().unwrap();
    let state = seeded(dir.path(), vec![]).await;

    let Json(problems) = list(
        State(state.clone()),
        Query(StatusQuery {
            health: Some("failing,degraded".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    let healths: Vec<_> = problems.iter().map(|v| v.health).collect();
    assert_eq!(
        healths,
        vec![
            Health::Failing,
            Health::Failing,
            Health::Failing,
            Health::Degraded
        ]
    );
    assert!(!names(&problems).iter().any(|n| n.starts_with("blog/")));

    let rds = problems
        .iter()
        .find(|v| v.resource_id == "rds")
        .expect("resource row");
    assert_eq!(
        (rds.namespace.as_str(), rds.application.as_str()),
        ("shop", "web")
    );
    assert_eq!(rds.consecutive_failures, 1);
    assert!(rds.first_failure_at.is_some());
    assert_eq!(rds.last_events[0].event.message, "down");

    let wide = problems.iter().find(|v| v.plugin == "vault").unwrap();
    assert_eq!(
        (wide.namespace.as_str(), wide.application.as_str()),
        ("", "")
    );
}

#[tokio::test]
async fn rows_outside_the_namespace_allowlist_are_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let state = seeded(dir.path(), vec!["shop".into(), "blog".into()]).await;

    let Json(all) = list(State(state.clone()), Query(StatusQuery::default()))
        .await
        .unwrap();
    let all = names(&all);
    assert!(!all.iter().any(|n| n.starts_with("secret/")), "{all:?}");
    assert!(
        all.iter().any(|n| n.starts_with("/:vault")),
        "plugin-wide rows stay"
    );

    let err = list(
        State(state),
        Query(StatusQuery {
            namespace: Some("secret".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::NamespaceNotAllowed(_)));
}

#[tokio::test]
async fn filters_by_plugin_namespace_application_and_plugin_page() {
    let dir = tempfile::tempdir().unwrap();
    let state = seeded(dir.path(), vec![]).await;

    let Json(shop) = list(
        State(state.clone()),
        Query(StatusQuery {
            namespace: Some("shop".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(shop.len(), 2);

    let Json(app) = for_application(State(state.clone()), Path(("shop".into(), "web".into())))
        .await
        .unwrap();
    assert_eq!(names(&app), vec!["shop/web:aws:rds", "shop/web:aws:"]);

    let Json(vault) = for_plugin(State(state.clone()), Path("vault".into()))
        .await
        .unwrap();
    assert_eq!(names(&vault), vec!["/:vault:"]);

    let err = list(
        State(state),
        Query(StatusQuery {
            health: Some("broken".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::BadRequest(_)));
}

#[tokio::test]
async fn the_json_shape_is_what_the_ui_reads() {
    let dir = tempfile::tempdir().unwrap();
    let state = seeded(dir.path(), vec![]).await;
    let Json(app) = for_application(State(state), Path(("shop".into(), "web".into())))
        .await
        .unwrap();
    let json = serde_json::to_value(&app[0]).unwrap();
    for key in [
        "namespace",
        "application",
        "plugin",
        "resource_id",
        "health",
        "last_export",
        "last_events",
        "first_failure_at",
        "consecutive_failures",
        "last_success_at",
        "last_changed_at",
        "updated_at",
    ] {
        assert!(json.get(key).is_some(), "missing {key}");
    }
    assert_eq!(json["health"], "failing");
    assert!(
        json.get("trace_id").is_none(),
        "omitted when tracing is off"
    );
}

/// End to end: a failing provision shows up as a failing resource row, and
/// a later successful refresh recovers it.
#[tokio::test]
async fn a_failing_provision_shows_up_in_the_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = FixturePlugin::new()
        .export("provision", Behavior::Trap)
        .loaded("aws");
    plugin.metadata.resources = vec![PluginResource {
        id: "rds".into(),
        label: "RDS".into(),
        ..Default::default()
    }];
    let state = test_state(dir.path(), vec![plugin], None).await;
    crate::db::ensure_application(&state.db, "shop", "web")
        .await
        .unwrap();
    spawn_worker(&state);

    let path = || Path(("shop".into(), "web".into(), "aws".into(), "rds".into()));
    crate::handlers::application_resources::provision(State(state.clone()), path(), None)
        .await
        .unwrap_err();
    state.plugin_telemetry.flush().await;

    let Json(rows) = for_application(State(state.clone()), Path(("shop".into(), "web".into())))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].resource_id, "rds");
    assert_eq!(rows[0].health, Health::Failing);
    assert_eq!(rows[0].last_events[0].event.code, codes::TRAP);
}
