//! Deckwatch: Kubernetes deployment lifecycle manager.
//!
//! The binary (`src/main.rs`) only calls [`run`]; everything else lives in
//! this library so the integration tests in `tests/` can drive real code
//! paths (see `tests/outbound_tls.rs`). [`outbound`] and [`anthropic`] are
//! the public outbound HTTP API, `audit` stays public as it was in the
//! binary, and [`testing`] re-exports what the contract tests need.

mod ai_provider;
pub mod anthropic;
pub mod audit;
mod auth;
mod auto_rollback;
mod config;
mod cross_origin;
mod crypto;
mod db;
mod entities;
mod error;
mod handlers;
mod kube_ext;
mod license;
mod license_middleware;
mod log_sanitize;
mod logging;
mod metrics;
mod migrations;
mod notifications;
pub mod outbound;
mod plugin_events;
#[cfg(test)]
mod plugin_fixture;
mod plugin_k8s_events;
mod plugin_status;
mod plugins;
mod rate_limit;
mod routes;
mod state;
mod telemetry;
mod watcher;
mod webhook_registration;
mod webhook_tls;

use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::util::SubscriberInitExt as _;

use config::Config;
use handlers::registry::RegistryStore;
use handlers::s3_backend::{S3Backend, S3Config};
use rate_limit::RateLimiter;
use state::AppState;

/// Run the deckwatch server (the body of `main`).
pub async fn run() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // JSON logs on stdout, plus OpenTelemetry export of deckwatch's own
    // telemetry when an OTEL_EXPORTER_OTLP_*ENDPOINT is set (src/telemetry.rs).
    let telemetry = telemetry::Telemetry::from_env();
    telemetry::subscriber(
        logging::env_filter(),
        telemetry.telemetry(),
        std::io::stdout,
    )
    .init();
    telemetry.log();
    // Plugins get the trace context of their call (`trace_context`), so
    // their own API calls join the trace, only while traces are exported.
    if telemetry.telemetry().is_some_and(|t| t.signals().traces) {
        plugin_events::install_trace_context_provider(plugin_events::otel_traceparent);
    }

    metrics::init();

    let config = Config::parse();
    let allowed = config.allowed_namespaces();

    tracing::info!(
        namespaces = ?allowed,
        port = config.port,
        frontend_dir = %config.frontend_dir,
        book_dir = %config.book_dir,
        registry_enabled = config.registry_enabled,
        registry_storage = %config.registry_storage,
        "starting deckwatch"
    );

    // Outbound HTTP clients (timeouts, user agent, extra trusted CAs). A
    // broken DECKWATCH_EXTRA_CA_FILE stops startup here, naming the file,
    // rather than surfacing later as TLS errors on every outbound call.
    match outbound::OutboundHttp::from_env() {
        Ok(http) => {
            match http.extra_ca() {
                Some(bundle) => tracing::info!(
                    file = %bundle.path().display(),
                    certificates = bundle.len(),
                    "trusting extra CA certificates in addition to the system roots"
                ),
                None => tracing::info!("outbound TLS trusts the system roots only"),
            }
            if http.policy().allow_insecure {
                tracing::warn!(
                    "{}=true: http:// endpoints are accepted for any host",
                    outbound::ALLOW_INSECURE_ENDPOINTS_ENV
                );
            }
            outbound::install_global(http);
        }
        Err(e) => {
            tracing::error!(error = %e, "invalid outbound HTTP configuration; refusing to start");
            eprintln!("deckwatch: {e}");
            std::process::exit(1);
        }
    }

    // --- Database ---
    let db_backend = db::backend_name(&config.database_url);
    tracing::info!(
        backend = db_backend,
        url = %config.database_url,
        "connecting to database"
    );
    let db = db::connect(&config.database_url)
        .await
        .expect("Failed to connect to database and run migrations");
    tracing::info!(backend = db_backend, "database ready (migrations applied)");

    let kube_client = kube::Client::try_default()
        .await
        .expect("Failed to create Kubernetes client");

    let registry_store: Option<RegistryStore> = if config.registry_enabled {
        build_registry_store(&config).await
    } else {
        None
    };

    // Rate limiter is retained in AppState but no longer enforced in handlers.
    // It can be re-enabled as a future business feature.
    let ai_rate_limiter = RateLimiter::default();

    let state = AppState {
        kube_client,
        allowed_namespaces: allowed,
        settings_namespace: config.settings_namespace.clone().unwrap_or_else(|| {
            std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "deckwatch".to_string())
        }),
        settings_configmap_name: config.settings_configmap_name.clone(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: if config.registry_public_url.is_empty() {
            None
        } else {
            Some(config.registry_public_url.clone())
        },
        registry_internal_url: if config.registry_internal_url.is_empty() {
            if config.registry_public_url.is_empty() {
                None
            } else {
                Some(config.registry_public_url.clone())
            }
        } else {
            Some(config.registry_internal_url.clone())
        },
        registry_enabled: config.registry_enabled,
        ai_rate_limiter,
        db,
        encryption_key: config.encryption_key.clone(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        plugin_telemetry: plugin_events::PluginTelemetry::new(config.encryption_key.clone()),
    };

    migrate_configmap_apps_to_db(&state).await;
    handlers::settings::bind_legacy_credentials_at_startup(&state).await;
    // Records plugin call outcomes in the plugin_status table.
    plugin_status::spawn_worker(&state);

    // Fetch plugins declared in persisted settings. Failures per-plugin are
    // logged and skipped so a bad plugin URL never blocks startup.
    {
        let settings = handlers::settings::load_settings_from_db(&state).await;
        state
            .plugin_telemetry
            .keep_plugins(plugins::enabled_plugin_names(&settings));
        if !settings.plugins.is_empty() {
            tracing::info!(count = settings.plugins.len(), "fetching plugins");
            let loaded = plugins::fetch_plugins(&settings, &state).await;
            *state.plugins.write().await = loaded;
        }
    }

    let watcher_state = state.clone();
    tokio::spawn(async move {
        tracing::info!("starting git watcher");
        watcher::run_poller(watcher_state).await;
    });

    // Snapshot the persisted auth settings once at startup so the middleware
    // has a stable config for the process lifetime. Toggling auth on/off in
    // the Settings UI therefore requires a pod restart — the trade-off is
    // that the middleware doesn't have to consult k8s on every request.
    // Failure to read the ConfigMap (e.g. fresh install) falls back to a
    // disabled config so the server still comes up.
    let auth_config = load_auth_config(&state).await;
    if auth_config.enabled {
        tracing::info!(
            tenant_id = %auth_config.tenant_id,
            client_id = %auth_config.client_id,
            "Entra authentication enforcement is ON"
        );
    } else {
        tracing::info!("Entra authentication enforcement is OFF (settings not enabled)");
    }

    let app = routes::build_router(
        state,
        registry_store,
        &config.frontend_dir,
        &config.book_dir,
        auth_config,
    );

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind address");

    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await.expect("server error");
}

/// Build the registry store per the configured backend. On failure we log
/// and return None — the server still comes up, but /v2/* and /api/registry
/// return the "registry_disabled" envelope. A total startup abort would
/// black out the whole app for a misconfigured optional feature.
async fn build_registry_store(config: &Config) -> Option<RegistryStore> {
    match config.registry_storage.as_str() {
        "filesystem" | "" => {
            let store = RegistryStore::filesystem(&config.registry_root);
            match store.ensure_dirs().await {
                Ok(()) => {
                    tracing::info!(
                        root = %config.registry_root,
                        "OCI registry storage ready (filesystem)"
                    );
                    Some(store)
                }
                Err(e) => {
                    tracing::error!(
                        root = %config.registry_root,
                        error = %e,
                        "failed to prepare OCI registry storage, disabling registry",
                    );
                    None
                }
            }
        }
        "s3" => {
            let s3_cfg = S3Config {
                bucket: config.registry_s3_bucket.clone(),
                prefix: config.registry_s3_prefix.clone(),
                region: config.registry_s3_region.clone(),
                endpoint: config.registry_s3_endpoint.clone(),
                path_style: config.registry_s3_path_style,
            };
            match S3Backend::new(s3_cfg) {
                Ok(backend) => {
                    tracing::info!(
                        bucket = %config.registry_s3_bucket,
                        prefix = %config.registry_s3_prefix,
                        region = %config.registry_s3_region,
                        endpoint = %config.registry_s3_endpoint,
                        "OCI registry storage ready (s3)"
                    );
                    Some(RegistryStore::s3(backend))
                }
                Err(e) => {
                    tracing::error!(
                        bucket = %config.registry_s3_bucket,
                        error = %e,
                        "failed to build S3 registry backend, disabling registry",
                    );
                    None
                }
            }
        }
        other => {
            tracing::error!(
                storage = %other,
                "unknown registry storage backend; must be `filesystem` or `s3` — disabling registry",
            );
            None
        }
    }
}

/// Read the deckwatch settings ConfigMap directly (bypassing the HTTP handler)
/// and produce an [`AuthConfig`]. Any error — missing CM, parse failure,
/// missing tenant/client — degrades to a disabled config so a broken settings
/// document can't lock operators out of the API entirely.
async fn load_auth_config(state: &AppState) -> auth::AuthConfig {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::Api;

    let api: Api<ConfigMap> = Api::namespaced(state.kube_client.clone(), &state.settings_namespace);
    let Ok(cm) = api.get(&state.settings_configmap_name).await else {
        return auth::AuthConfig::disabled();
    };
    let Some(data) = cm.data.as_ref().and_then(|d| d.get("settings")) else {
        return auth::AuthConfig::disabled();
    };
    let Ok(parsed) = serde_json::from_str::<handlers::settings::DeckwatchSettings>(data) else {
        tracing::warn!("failed to parse settings ConfigMap; auth will be disabled");
        return auth::AuthConfig::disabled();
    };
    auth::AuthConfig::from_settings(parsed.auth.as_ref())
}

/// One-time startup migration: import application records from legacy
/// ConfigMaps into the database, then delete the ConfigMaps. Runs on
/// every boot but is a no-op once all ConfigMaps have been migrated.
async fn migrate_configmap_apps_to_db(state: &AppState) {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::ListParams;
    use kube::Api;
    use sea_orm::ActiveValue::Set;
    use sea_orm::EntityTrait;

    use crate::entities::applications;

    let selector = "app.kubernetes.io/component=application,app.kubernetes.io/managed-by=deckwatch";
    let cm_api: Api<ConfigMap> = Api::all(state.kube_client.clone());
    let lp = ListParams::default().labels(selector);

    let cms = match cm_api.list(&lp).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, "failed to list legacy application ConfigMaps; skipping migration");
            return;
        }
    };

    if cms.items.is_empty() {
        return;
    }

    tracing::info!(
        count = cms.items.len(),
        "found legacy application ConfigMaps to migrate"
    );

    for cm in &cms.items {
        let cm_name = match cm.metadata.name.as_deref() {
            Some(n) => n,
            None => continue,
        };
        let cm_ns = match cm.metadata.namespace.as_deref() {
            Some(n) => n,
            None => continue,
        };

        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct LegacyAppData {
            name: String,
            #[serde(default)]
            description: String,
            #[serde(default)]
            created_at: Option<String>,
            #[serde(default)]
            updated_at: Option<String>,
            #[serde(default)]
            git: Option<crate::kube_ext::ApplicationGitConfig>,
        }

        let data_str = match cm.data.as_ref().and_then(|d| d.get("application")) {
            Some(s) => s,
            None => {
                tracing::warn!(
                    cm = cm_name,
                    ns = cm_ns,
                    "ConfigMap missing 'application' key; skipping"
                );
                continue;
            }
        };
        let data: LegacyAppData = match serde_json::from_str(data_str) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(cm = cm_name, ns = cm_ns, error = %e, "failed to parse ConfigMap data; skipping");
                continue;
            }
        };

        let app_id = format!("{cm_ns}/{}", data.name);

        let existing = match applications::Entity::find_by_id(&app_id)
            .one(&state.db)
            .await
        {
            Ok(Some(_)) => {
                tracing::info!(app = %app_id, "application already in database; deleting legacy ConfigMap");
                true
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(app = %app_id, error = %e, "DB lookup failed; skipping");
                continue;
            }
        };

        if !existing {
            let now = {
                use std::time::SystemTime;
                let d = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("system clock before UNIX epoch");
                sea_orm::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
                    .expect("timestamp out of range")
            };

            let git_config_json = data
                .git
                .as_ref()
                .and_then(|g| serde_json::to_string(g).ok());

            let model = applications::ActiveModel {
                id: Set(app_id.clone()),
                name: Set(data.name.clone()),
                namespace: Set(cm_ns.to_string()),
                description: Set(data.description),
                team: Set(String::new()),
                deployment_name: Set(Some(data.name.clone())),
                git_config: Set(git_config_json),
                created_at: Set(now),
                updated_at: Set(now),
            };

            if let Err(e) = applications::Entity::insert(model).exec(&state.db).await {
                tracing::warn!(app = %app_id, error = %e, "failed to insert application into DB; skipping");
                continue;
            }
            tracing::info!(app = %app_id, "migrated application from ConfigMap to database");
        }

        let ns_api: Api<ConfigMap> = Api::namespaced(state.kube_client.clone(), cm_ns);
        if let Err(e) = ns_api.delete(cm_name, &Default::default()).await {
            tracing::warn!(cm = cm_name, ns = cm_ns, error = %e, "failed to delete legacy ConfigMap");
        } else {
            tracing::info!(
                cm = cm_name,
                ns = cm_ns,
                "deleted legacy application ConfigMap"
            );
        }
    }
}

/// Entry points for the outbound HTTP contract tests
/// (`tests/outbound_tls.rs`). Not a stable API.
#[doc(hidden)]
pub mod testing {
    pub use crate::handlers::prometheus_query::{execute_range, PodSeries};
    pub use crate::handlers::tracing_handler::{fetch_jaeger_compat, TraceSummary};
    pub use crate::notifications::{build_payload, post_webhook, NotificationEvent};
    pub use crate::watcher::{check_remote_head, github_changed_files};
}

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod live_plugin_events_tests;
#[cfg(test)]
mod plugin_events_acceptance_tests;
