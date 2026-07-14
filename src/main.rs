mod auth;
mod config;
mod log_sanitize;
mod metrics;
mod notifications;
mod error;
mod handlers;
mod kube_ext;
mod rate_limit;
mod routes;
mod state;
mod watcher;
mod auto_rollback;
mod license;
mod license_middleware;
mod webhook_tls;
mod webhook_registration;

use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use config::Config;
use handlers::registry::RegistryStore;
use handlers::s3_backend::{S3Backend, S3Config};
use rate_limit::{RateLimiter, DEFAULT_HOURLY_LIMIT};
use state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("deckwatch=info,tower_http=info")),
        )
        .json()
        .init();

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

    let kube_client = kube::Client::try_default()
        .await
        .expect("Failed to create Kubernetes client");

    let registry_store: Option<RegistryStore> = if config.registry_enabled {
        build_registry_store(&config).await
    } else {
        None
    };

    // Rate limiter is created with the compiled-in default up front so it
    // exists before we look at any settings. `load_ai_safety_limit` below
    // reads the ConfigMap and hot-swaps the cap if the operator has one
    // configured. This ordering means a broken settings document can never
    // leave AppState without a limiter.
    let ai_rate_limiter = RateLimiter::new(DEFAULT_HOURLY_LIMIT);

    let state = AppState {
        kube_client,
        allowed_namespaces: allowed,
        settings_namespace: config.settings_namespace.clone().unwrap_or_else(|| std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "deckwatch".to_string())),
        settings_configmap_name: config.settings_configmap_name.clone(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: if config.registry_public_url.is_empty() {
            None
        } else {
            Some(config.registry_public_url.clone())
        },
        ai_rate_limiter: ai_rate_limiter.clone(),
    };

    // Seed the running limiter from persisted settings. Best-effort; a
    // missing/broken ConfigMap keeps the default.
    if let Some(limit) = load_ai_safety_limit(&state).await {
        tracing::info!(
            limit,
            "AI job rate limit set from settings ConfigMap",
        );
        ai_rate_limiter.set_limit(limit);
    } else {
        tracing::info!(
            limit = DEFAULT_HOURLY_LIMIT,
            "AI job rate limit using compiled-in default",
        );
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

/// Peek at the persisted AI safety block. Returns `None` if the setting is
/// absent or the ConfigMap is missing/malformed — caller keeps the default.
async fn load_ai_safety_limit(state: &AppState) -> Option<u32> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::Api;

    let api: Api<ConfigMap> = Api::namespaced(state.kube_client.clone(), &state.settings_namespace);
    let cm = api.get(&state.settings_configmap_name).await.ok()?;
    let data = cm.data.as_ref()?.get("settings")?;
    let parsed = serde_json::from_str::<handlers::settings::DeckwatchSettings>(data).ok()?;
    parsed.ai_safety.map(|a| a.jobs_per_namespace_per_hour)
}
