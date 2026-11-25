use super::*;

use crate::otel_inject::tests::{
    patches, set_endpoints, set_injection, shop_workloads, test_state, FakeCluster,
};
use crate::otel_inject::{InjectionState, Sweep};

const COLLECTOR: &str = "http://otel-collector.observability:4318";
const PROMETHEUS: &str = "http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics";

fn otel(enabled: bool, service_name: Option<&str>, attributes: &[(&str, &str)]) -> OtelInjection {
    OtelInjection {
        enabled,
        service_name: service_name.map(str::to_string),
        resource_attributes: attributes
            .iter()
            .map(|(k, v)| ResourceAttribute {
                key: k.to_string(),
                value: v.to_string(),
            })
            .collect(),
    }
}

async fn audit_rows(state: &AppState) -> Vec<crate::entities::audit_log::Model> {
    crate::entities::audit_log::Entity::find()
        .all(&state.db)
        .await
        .unwrap()
}

async fn shop_state(cluster: &FakeCluster) -> AppState {
    let state = test_state(cluster).await;
    set_endpoints(&state, COLLECTOR, PROMETHEUS).await;
    crate::db::ensure_application(&state.db, "team", "shop")
        .await
        .unwrap();
    state
}

async fn put_otel(
    state: &AppState,
    config: OtelInjection,
) -> Result<ObservabilityResponse, AppError> {
    put(
        State(state.clone()),
        Path(("team".to_string(), "shop".to_string())),
        Json(ObservabilityUpdate {
            opentelemetry: Some(config),
        }),
    )
    .await
    .map(|Json(r)| r)
}

#[test]
fn the_audit_log_says_what_changed_with_attribute_keys_only() {
    let off = otel(false, None, &[]);
    let on = otel(
        true,
        Some("storefront"),
        &[("team", "payments"), ("tier", "web")],
    );
    assert_eq!(
        describe_change("shop", &off, &on),
        "turned OpenTelemetry injection on for application 'shop' (service name \
         \"storefront\"; extra resource attributes team, tier)"
    );
    assert!(!describe_change("shop", &off, &on).contains("payments"));
    assert_eq!(
        describe_change("shop", &on, &off),
        "turned OpenTelemetry injection off for application 'shop' (each workload's name as \
         the service name; no extra resource attributes)"
    );
    assert!(describe_change("shop", &on, &otel(true, None, &[])).starts_with("changed"));
}

#[tokio::test]
async fn saving_validates_normalizes_stores_and_audits() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = shop_state(&cluster).await;

    let error = put_otel(
        &state,
        otel(
            true,
            Some("a\u{7}b"),
            &[("k8s.namespace.name", "x"), ("team", "two words")],
        ),
    )
    .await
    .unwrap_err();
    let AppError::BadRequest(message) = error else {
        panic!("expected a 400, got {error:?}");
    };
    assert!(message.contains("service_name"), "{message}");
    assert!(message.contains("k8s.namespace.name"), "{message}");
    assert!(message.contains("value of team"), "{message}");
    assert!(audit_rows(&state).await.is_empty());

    let saved = put_otel(
        &state,
        otel(true, Some("  storefront "), &[(" team ", "payments")]),
    )
    .await
    .unwrap();
    let otel_status = &saved.opentelemetry;
    assert!(otel_status.enabled);
    assert_eq!(otel_status.service_name.as_deref(), Some("storefront"));
    assert_eq!(otel_status.resource_attributes[0].key, "team");
    assert_eq!(otel_status.otlp_endpoint.as_deref(), Some(COLLECTOR));
    assert_eq!(
        otel_status.otlp_metrics_endpoint.as_deref(),
        Some(PROMETHEUS)
    );
    let traces = &otel_status.signals[0];
    assert_eq!(traces.signal, Signal::Traces);
    assert_eq!(
        traces.endpoint.as_deref(),
        Some("http://otel-collector.observability:4318/v1/traces")
    );
    // Saving only stores: the reconciler hasn't run, so every workload waits.
    assert_eq!(otel_status.workloads.len(), 3);
    assert!(otel_status
        .workloads
        .iter()
        .all(|w| w.state == InjectionState::Pending));
    // Nothing was patched by the request itself.
    assert!(patches(&cluster.take_requests()).is_empty());

    let row = applications::Entity::find_by_id("team/shop".to_string())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let stored = ApplicationObservability::from_column(row.observability.as_deref()).unwrap();
    assert_eq!(
        stored.opentelemetry,
        otel(true, Some("storefront"), &[("team", "payments")])
    );
    let audit = audit_rows(&state).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].resource_type, "application-observability");
    assert_eq!(audit[0].resource_name, "shop");
    assert_eq!(audit[0].namespace, "team");

    // The same settings again: nothing to store or audit.
    put_otel(
        &state,
        otel(true, Some("storefront"), &[("team", "payments")]),
    )
    .await
    .unwrap();
    assert_eq!(audit_rows(&state).await.len(), 1);

    // A body without opentelemetry keeps them.
    let Json(kept) = put(
        State(state.clone()),
        Path(("team".to_string(), "shop".to_string())),
        Json(ObservabilityUpdate {
            opentelemetry: None,
        }),
    )
    .await
    .unwrap();
    assert!(kept.opentelemetry.enabled);
}

#[tokio::test]
async fn the_section_shows_each_workloads_injection() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = shop_state(&cluster).await;
    set_injection(&state, "team", "shop", otel(true, None, &[])).await;
    crate::otel_inject::reconcile(&state, &mut Sweep::default()).await;

    let Json(section) = get(
        State(state.clone()),
        Path(("team".to_string(), "shop".to_string())),
    )
    .await
    .unwrap();
    let workloads = &section.opentelemetry.workloads;
    assert_eq!(
        workloads
            .iter()
            .map(|w| (w.kind.name(), w.name.as_str(), w.container.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("Deployment", "web", Some("app")),
            ("StatefulSet", "db", Some("app")),
            ("CronJob", "nightly", Some("job")),
        ]
    );
    for w in workloads {
        assert_eq!(w.state, InjectionState::Injected, "{w:?}");
        assert!(
            w.injected.contains(&"OTEL_SERVICE_NAME".to_string()),
            "{w:?}"
        );
        assert!(w.kept.is_empty());
    }
    assert_eq!(section.opentelemetry.workloads_error, None);
}

#[tokio::test]
async fn unknown_applications_and_namespaces_are_refused() {
    let cluster = FakeCluster::default();
    let mut state = shop_state(&cluster).await;
    let missing = get(
        State(state.clone()),
        Path(("team".to_string(), "blog".to_string())),
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, AppError::NotFound(_)), "{missing:?}");

    state.allowed_namespaces = vec!["other".to_string()];
    let refused = put_otel(&state, otel(true, None, &[])).await.unwrap_err();
    assert!(
        matches!(refused, AppError::NamespaceNotAllowed(_)),
        "{refused:?}"
    );
}

#[tokio::test]
async fn prometheus_says_why_it_is_unavailable() {
    let cluster = FakeCluster::with_objects(shop_workloads());
    let state = shop_state(&cluster).await;
    let prometheus_enabled = |enabled: bool| {
        let state = state.clone();
        async move {
            let mut settings = crate::handlers::settings::load_settings_from_db(&state).await;
            settings.prometheus_enabled = enabled;
            crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
                .await
                .unwrap();
        }
    };

    // On in settings, but the fake API server doesn't serve the PodMonitor
    // CRD.
    prometheus_enabled(true).await;
    let Json(section) = get(
        State(state.clone()),
        Path(("team".to_string(), "shop".to_string())),
    )
    .await
    .unwrap();
    let prometheus = &section.prometheus;
    assert!(!prometheus.available);
    assert!(prometheus
        .unavailable_reason
        .as_deref()
        .is_some_and(|r| r.contains("PodMonitor")));
    // The application's Deployments are listed with their defaults.
    assert_eq!(prometheus.deployments.len(), 1);
    let web = &prometheus.deployments[0];
    assert_eq!(web.deployment, "web");
    assert_eq!(web.name, "web-deckwatch");
    assert!(!web.enabled);

    prometheus_enabled(false).await;
    let Json(section) = get(
        State(state.clone()),
        Path(("team".to_string(), "shop".to_string())),
    )
    .await
    .unwrap();
    assert_eq!(
        section.prometheus.unavailable_reason.as_deref(),
        Some("Prometheus monitoring is disabled. Enable it in Settings.")
    );
}
