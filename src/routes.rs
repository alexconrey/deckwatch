use axum::extract::FromRequest;
use axum::routing::{get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::audit;
use crate::auth::{self, AuthConfig};
use crate::handlers::registry::RegistryStore;
use crate::handlers::{
    addons, admission, ai_fix, applications, autoscaling, configmaps_ui, cronjobs, deployments,
    deployments_ux, diagnostics, docs, events, exec, git, gitops, health, ingresses, license, logs,
    mcp, monitoring, namespaces, nodes, pods, portforward, prometheus_query, promote, registry,
    registry_ui, resource_metrics, secrets, settings, templates, tracing_handler, webhooks,
};
use crate::metrics;
use crate::state::AppState;

pub fn build_router(
    state: AppState,
    registry_store: Option<RegistryStore>,
    frontend_dir: &str,
    book_dir: &str,
    auth_config: AuthConfig,
) -> Router {
    // Shared layer instance so both `private_api` and `registry_api` enforce
    // the same auth surface. `/v2/*` OCI paths stay unlayered because docker
    // clients speak the OCI token flow (`WWW-Authenticate: Bearer realm=...`)
    // and would break if we intercepted them with a JWT check.
    let auth_layer = axum::middleware::from_fn_with_state(auth_config, auth::require_auth);

    // Public API routes (health, docs, settings-read for auth bootstrap,
    // frontend-metrics ingestion). These must remain reachable without a
    // bearer token so the SPA can decide whether to redirect to Entra
    // *before* it has one.
    //
    // Everything else in `private_api` is layered with `require_auth`,
    // which is a no-op when `auth_config.enabled` is false.
    let public_api = Router::new()
        .route("/api/healthz", get(health::healthz))
        .route("/api/features", get(health::features))
        .route("/api/readyz", get(health::readyz))
        .route(
            "/api/frontend-metrics",
            post(metrics::ingest_frontend_metrics),
        )
        .route("/api/settings", get(settings::get_settings))
        .route("/api/openapi.yaml", get(docs::openapi_yaml))
        .route("/api/docs", get(docs::swagger_ui))
        .route("/api/docs/pages", get(docs::list_pages))
        .route("/api/docs/pages/{slug}", get(docs::get_page))
        .route("/api/license", get(license::get_license))
        .route("/api/webhooks/git", post(webhooks::receive))
        // Kubernetes ValidatingAdmissionWebhook endpoint. K8s sends
        // AdmissionReview requests directly, so this must be public (no auth
        // layer). The webhook config uses `failurePolicy: Ignore`, so the
        // handler is fail-open by design.
        .route("/api/admission/validate", post(admission::validate))
        // MCP (Model Context Protocol) server endpoint. Public so MCP
        // clients (e.g. Claude Code) can connect without a bearer token.
        .route("/mcp", post(mcp::handle_mcp))
        .with_state(state.clone());

    let private_api = Router::new()
        .route(
            "/api/namespaces",
            get(namespaces::list_namespaces).post(namespaces::create_namespace),
        )
        .route(
            "/api/namespaces/{ns}/deployments",
            get(deployments::list).post(deployments::create),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}",
            get(deployments::get)
                .put(deployments::update)
                .delete(deployments::delete),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/yaml",
            get(deployments::get_yaml).put(deployments::update_yaml),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/restart",
            post(deployments::restart),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/scale",
            post(deployments::scale),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/probes",
            axum::routing::patch(deployments::update_probes),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/containers",
            post(deployments::add_container),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/containers/{container_name}",
            axum::routing::delete(deployments::remove_container),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/pods",
            get(pods::list_for_deployment),
        )
        .route("/api/ingressclasses", get(ingresses::list_classes))
        .route(
            "/api/namespaces/{ns}/ingresses",
            get(ingresses::list).post(ingresses::create),
        )
        .route(
            "/api/namespaces/{ns}/ingresses/{name}",
            get(ingresses::get)
                .put(ingresses::update)
                .delete(ingresses::delete),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/gitops",
            get(gitops::get_config)
                .put(gitops::set_config)
                .delete(gitops::delete_config),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/gitops/trigger",
            post(gitops::trigger_build),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/gitops/builds",
            get(gitops::list_builds),
        )
        .route("/api/namespaces/{ns}/pods/{pod_name}", get(pods::get))
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/logs",
            get(logs::stream_logs),
        )
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/logs/history",
            get(logs::get_logs),
        )
        .route("/api/settings", axum::routing::put(settings::put_settings))
        .route("/api/namespaces/{ns}/cronjobs", get(cronjobs::list))
        .route("/api/namespaces/{ns}/cronjobs/{name}", get(cronjobs::get))
        .route("/api/nodes", get(nodes::list_nodes))
        .route("/api/addons", get(addons::list))
        .route(
            "/api/namespaces/{ns}/deployments/{name}/addons/{addon_id}",
            post(addons::attach)
                .patch(addons::update)
                .delete(addons::detach),
        )
        .route(
            "/api/templates",
            get(templates::list).put(templates::update),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/history",
            get(deployments_ux::history),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/rollback",
            post(deployments_ux::rollback),
        )
        .route(
            "/api/namespaces/{ns}/deployments/validate",
            post(deployments_ux::validate),
        )
        .route(
            "/api/namespaces/{ns}/deployments/{name}/clone",
            post(deployments_ux::clone),
        )
        .route(
            "/api/namespaces/{ns}/diagnostics",
            get(diagnostics::list_diagnostics).post(diagnostics::create_diagnostic),
        )
        .route(
            "/api/namespaces/{ns}/diagnostics/{job_name}",
            get(diagnostics::get_diagnostic_status),
        )
        .route(
            "/api/namespaces/{ns}/diagnostics/{job_name}/result",
            get(diagnostics::get_diagnostic_result),
        )
        .route(
            "/api/namespaces/{ns}/diagnostics/{job_name}/stream",
            get(diagnostics::stream_diagnostic_output),
        )
        .route(
            "/api/namespaces/{ns}/applications",
            get(applications::list).post(applications::create),
        )
        .route(
            "/api/namespaces/{ns}/applications/{name}",
            get(applications::get)
                .put(applications::update)
                .delete(applications::delete),
        )
        .route(
            "/api/namespaces/{ns}/applications/{name}/members",
            post(applications::add_member),
        )
        .route(
            "/api/namespaces/{ns}/applications/{name}/members/{kind}/{resource_name}",
            axum::routing::delete(applications::remove_member),
        )
        .route("/api/git/branches", get(git::list_branches))
        .route("/api/namespaces/{ns}/events", get(events::list_namespaced))
        .route("/api/events", get(events::list_cluster))
        .route(
            "/api/namespaces/{ns}/pods/metrics",
            get(resource_metrics::list_pod_metrics),
        )
        .route(
            "/api/nodes/metrics",
            get(resource_metrics::list_node_metrics),
        )
        .route(
            "/api/namespaces/{ns}/secrets",
            get(secrets::list).post(secrets::create),
        )
        .route(
            "/api/namespaces/{ns}/secrets/{name}",
            get(secrets::get)
                .put(secrets::update)
                .delete(secrets::delete),
        )
        .route(
            "/api/namespaces/{ns}/configmaps",
            get(configmaps_ui::list).post(configmaps_ui::create),
        )
        .route(
            "/api/namespaces/{ns}/configmaps/{name}",
            get(configmaps_ui::get)
                .put(configmaps_ui::update)
                .delete(configmaps_ui::delete),
        )
        // Job pods (for build logs)
        .route(
            "/api/namespaces/{ns}/jobs/{job_name}/pods",
            get(gitops::list_job_pods),
        )
        // Revision YAML
        .route(
            "/api/namespaces/{ns}/deployments/{name}/history/{revision}/yaml",
            get(deployments_ux::revision_yaml),
        )
        // Validate update (dry-run for edit-existing flow)
        .route(
            "/api/namespaces/{ns}/deployments/{name}/validate",
            post(deployments_ux::validate_update),
        )
        // Auto-rollback toggle
        .route(
            "/api/namespaces/{ns}/deployments/{name}/auto-rollback",
            post(deployments_ux::set_auto_rollback),
        )
        // AI Fix
        .route(
            "/api/namespaces/{ns}/applications/{name}/ai-fix",
            post(ai_fix::create_ai_fix),
        )
        // Prometheus Monitoring (runtime-gated via settings ConfigMap)
        .route(
            "/api/namespaces/{ns}/deployments/{name}/monitor",
            get(monitoring::get)
                .put(monitoring::upsert)
                .delete(monitoring::delete),
        )
        // Prometheus range-query proxy (PromQL via curated query catalog)
        .route(
            "/api/prometheus/query_range",
            get(prometheus_query::query_range),
        )
        // HPA Autoscaling
        .route(
            "/api/namespaces/{ns}/deployments/{name}/hpa",
            get(autoscaling::get)
                .put(autoscaling::upsert)
                .delete(autoscaling::delete),
        )
        // Port forward
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/portforward",
            get(portforward::portforward_ws),
        )
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/proxy/{port}",
            axum::routing::any(portforward::portforward_http_root),
        )
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/proxy/{port}/{*rest}",
            axum::routing::any(portforward::portforward_http),
        )
        // Container exec
        .route(
            "/api/namespaces/{ns}/pods/{pod_name}/exec",
            get(exec::exec_ws),
        )
        // Encrypted credential management (API keys stored in DB)
        .route("/api/settings/credentials", post(settings::set_credentials))
        // Notifications
        .route("/api/notifications/test", post(settings::test_notification))
        // Cross-namespace deployment promotion
        .route(
            "/api/namespaces/{ns}/deployments/{name}/promote",
            post(promote::promote),
        )
        // Distributed tracing query proxy (Tempo / Jaeger)
        .route(
            "/api/namespaces/{ns}/deployments/{name}/traces",
            get(tracing_handler::list_traces),
        )
        // Audit log
        .route("/api/audit", get(audit::list_audit_logs))
        .with_state(state)
        .layer(auth_layer.clone());

    // UI-side API for the registry page. Uses `Option<RegistryStore>` state
    // so handlers return a structured "registry_disabled" response instead
    // of the route silently 404ing — the frontend needs to distinguish
    // "not enabled" from "not implemented".
    // Repo names can contain `/` (nested repos), so we use a single
    // wildcard capture and let the handler split on the request path.
    let registry_api = Router::new()
        .route("/api/registry/enabled", get(registry_ui::enabled))
        .route("/api/registry/repositories", get(registry_ui::list_repos))
        .route(
            "/api/registry/repositories/{*rest}",
            get(registry_ui_dispatch_get).delete(registry_ui_dispatch_delete),
        )
        .with_state(registry_store.clone())
        .layer(auth_layer);

    // OCI Distribution Spec surface. Only wired when a store is
    // configured — a disabled registry leaves clients hitting the SPA
    // fallback, so they get an HTML 200 back for /v2/ and abort quickly.
    // Axum can't disambiguate multi-segment `{name}` from the trailing
    // `/manifests/{ref}` etc., and can't mix `/v2/_catalog` + `/v2/{*rest}`
    // in the same router without a conflict, so we mount ONE catchall
    // that owns everything under `/v2/`.
    let mut router = Router::new()
        .route("/metrics", get(metrics::metrics_handler))
        .merge(public_api)
        .merge(private_api)
        .merge(registry_api);

    if let Some(store) = registry_store {
        let oci = Router::new()
            .route("/v2/", get(oci_root).head(oci_root))
            .route(
                "/v2/{*rest}",
                get(oci_dispatch_get)
                    .head(oci_dispatch_head)
                    .post(oci_dispatch_post)
                    .patch(oci_dispatch_patch)
                    .put(oci_dispatch_put)
                    .delete(oci_dispatch_delete),
            )
            .with_state(store);
        router = router.merge(oci);
    }

    // Serve the rendered mdBook manual at `/docs/book/` when the build
    // output is on disk. Nested as its own service so a missing page under
    // the book returns 404 from ServeDir rather than the SPA fallback
    // (which would render the Vue shell and confuse deep-linkers).
    // `append_index_html_on_directories(true)` makes `/docs/book/` serve
    // `index.html` instead of a directory listing.
    if std::path::Path::new(book_dir).is_dir() {
        let book = ServeDir::new(book_dir).append_index_html_on_directories(true);
        router = router.nest_service("/docs/book", book);
    } else {
        tracing::warn!(
            book_dir,
            "mdBook output directory not found; /docs/book/ will not be served. \
             Run scripts/build-docs.sh to generate it."
        );
    }

    let index_file = format!("{frontend_dir}/index.html");
    let spa = ServeDir::new(frontend_dir).not_found_service(ServeFile::new(index_file));

    router
        .fallback_service(spa)
        .layer(axum::middleware::from_fn(metrics::track_http))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn oci_root(
    axum::extract::State(_): axum::extract::State<RegistryStore>,
) -> axum::response::Response {
    registry::v2_root().await
}

// ---------------------------------------------------------------- dispatch

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// Actions parsed from a `/v2/*` path suffix.
enum OciAction {
    /// `/v2/_catalog`
    Catalog,
    /// `/v2/{name}/tags/list`
    TagsList,
    /// `/v2/{name}/manifests/{reference}`
    Manifests(String),
    /// `/v2/{name}/blobs/{digest}`
    Blob(String),
    /// `/v2/{name}/blobs/uploads/` (no UUID)
    Uploads,
    /// `/v2/{name}/blobs/uploads/{uuid}`
    UploadSession(String),
}

fn split_oci_path(rest: &str) -> Option<(String, OciAction)> {
    if rest == "_catalog" {
        return Some((String::new(), OciAction::Catalog));
    }
    // Uploads look like `<name>/blobs/uploads/` or `<name>/blobs/uploads/<uuid>`.
    if let Some(idx) = rest.find("/blobs/uploads") {
        let name = &rest[..idx];
        let tail = &rest[idx + "/blobs/uploads".len()..];
        if name.is_empty() {
            return None;
        }
        if tail.is_empty() || tail == "/" {
            return Some((name.to_string(), OciAction::Uploads));
        }
        if let Some(uuid) = tail.strip_prefix('/') {
            if !uuid.is_empty() {
                return Some((name.to_string(), OciAction::UploadSession(uuid.to_string())));
            }
        }
        return None;
    }
    if let Some(idx) = rest.rfind("/blobs/") {
        let name = &rest[..idx];
        let digest = &rest[idx + "/blobs/".len()..];
        if name.is_empty() || digest.is_empty() {
            return None;
        }
        return Some((name.to_string(), OciAction::Blob(digest.to_string())));
    }
    if let Some(idx) = rest.rfind("/manifests/") {
        let name = &rest[..idx];
        let reference = &rest[idx + "/manifests/".len()..];
        if name.is_empty() || reference.is_empty() {
            return None;
        }
        return Some((
            name.to_string(),
            OciAction::Manifests(reference.to_string()),
        ));
    }
    if let Some(name) = rest.strip_suffix("/tags/list") {
        if !name.is_empty() {
            return Some((name.to_string(), OciAction::TagsList));
        }
    }
    None
}

fn route_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "errors": [{
                "code": "NAME_UNKNOWN",
                "message": "unknown route",
            }]
        })),
    )
        .into_response()
}

async fn oci_dispatch_get(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
) -> Response {
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::Catalog => registry::list_catalog(State(store)).await,
        OciAction::Manifests(reference) => {
            registry::get_manifest(State(store), AxumPath((name, reference))).await
        }
        OciAction::Blob(digest) => registry::get_blob(State(store), AxumPath((name, digest))).await,
        OciAction::TagsList => registry::list_tags(State(store), AxumPath(name)).await,
        OciAction::UploadSession(uuid) => {
            registry::get_upload_status(State(store), AxumPath((name, uuid))).await
        }
        OciAction::Uploads => route_not_found(),
    }
}

async fn oci_dispatch_head(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
) -> Response {
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::Manifests(reference) => {
            registry::head_manifest(State(store), AxumPath((name, reference))).await
        }
        OciAction::Blob(digest) => {
            registry::head_blob(State(store), AxumPath((name, digest))).await
        }
        _ => route_not_found(),
    }
}

async fn oci_dispatch_post(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
    Query(q): Query<registry::UploadInitQuery>,
    body: axum::body::Bytes,
) -> Response {
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::Uploads => {
            registry::start_upload(State(store), AxumPath(name), Query(q), body).await
        }
        _ => route_not_found(),
    }
}

async fn oci_dispatch_patch(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
    body: axum::body::Body,
) -> Response {
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::UploadSession(uuid) => {
            registry::patch_upload(State(store), AxumPath((name, uuid)), body).await
        }
        _ => route_not_found(),
    }
}

async fn oci_dispatch_put(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
    req: axum::extract::Request,
) -> Response {
    let query = Query::<registry::UploadCompleteQuery>::try_from_uri(req.uri()).ok();
    let body = match axum::body::Bytes::from_request(req, &()).await {
        Ok(b) => b,
        Err(_) => return (axum::http::StatusCode::BAD_REQUEST, "invalid body").into_response(),
    };
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::Manifests(reference) => {
            registry::put_manifest(
                State(store),
                AxumPath((name, reference)),
                HeaderMap::new(),
                body,
            )
            .await
        }
        OciAction::UploadSession(uuid) => {
            let Some(q) = query else {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "errors": [{
                            "code": "DIGEST_INVALID",
                            "message": "missing digest query parameter",
                        }]
                    })),
                )
                    .into_response();
            };
            registry::put_upload(State(store), AxumPath((name, uuid)), q, body).await
        }
        _ => route_not_found(),
    }
}

async fn oci_dispatch_delete(
    State(store): State<RegistryStore>,
    AxumPath(rest): AxumPath<String>,
) -> Response {
    let Some((name, action)) = split_oci_path(&rest) else {
        return route_not_found();
    };
    match action {
        OciAction::Manifests(reference) => {
            registry::delete_manifest(State(store), AxumPath((name, reference))).await
        }
        OciAction::Blob(digest) => {
            registry::delete_blob(State(store), AxumPath((name, digest))).await
        }
        OciAction::UploadSession(uuid) => {
            registry::cancel_upload(State(store), AxumPath((name, uuid))).await
        }
        _ => route_not_found(),
    }
}

// ---------------------------------------------- registry UI catchall dispatch

/// UI paths under `/api/registry/repositories/*` can be:
///   * `<name>/tags`           list tags in repo
///   * `<name>/tags/<tag>`     manifest detail (or DELETE)
///
/// Same split-in-code pattern as the OCI dispatcher because `name` can
/// contain `/` and axum's router can't handle nested wildcards.
async fn registry_ui_dispatch_get(
    State(store): State<Option<RegistryStore>>,
    AxumPath(rest): AxumPath<String>,
) -> Response {
    match split_ui_path(&rest) {
        Some(UiAction::Tags(name)) => registry_ui::list_tags(State(store), AxumPath(name)).await,
        Some(UiAction::Manifest(name, tag)) => {
            registry_ui::get_manifest_detail(State(store), AxumPath((name, tag))).await
        }
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(
                serde_json::json!({"error":"not_found","message":"unknown registry UI path"}),
            ),
        )
            .into_response(),
    }
}

async fn registry_ui_dispatch_delete(
    State(store): State<Option<RegistryStore>>,
    AxumPath(rest): AxumPath<String>,
) -> Response {
    match split_ui_path(&rest) {
        Some(UiAction::Manifest(name, tag)) => {
            registry_ui::delete_tag(State(store), AxumPath((name, tag))).await
        }
        _ => (
            StatusCode::METHOD_NOT_ALLOWED,
            axum::Json(serde_json::json!({"error":"method_not_allowed"})),
        )
            .into_response(),
    }
}

enum UiAction {
    Tags(String),
    Manifest(String, String),
}

fn split_ui_path(rest: &str) -> Option<UiAction> {
    if let Some(name) = rest.strip_suffix("/tags") {
        if !name.is_empty() {
            return Some(UiAction::Tags(name.to_string()));
        }
    }
    if let Some(idx) = rest.rfind("/tags/") {
        let name = &rest[..idx];
        let tag = &rest[idx + "/tags/".len()..];
        if !name.is_empty() && !tag.is_empty() && !tag.contains('/') {
            return Some(UiAction::Manifest(name.to_string(), tag.to_string()));
        }
    }
    None
}
