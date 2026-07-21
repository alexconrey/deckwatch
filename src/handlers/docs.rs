//! In-process documentation surface.
//!
//! - `GET /api/openapi.yaml`      — the embedded OpenAPI 3.0 spec
//! - `GET /api/docs`              — Swagger UI HTML that loads the spec above
//! - `GET /api/docs/pages`        — index of markdown docs bundled with the binary
//! - `GET /api/docs/pages/{slug}` — one bundled markdown doc (raw text)
//!
//! The spec and markdown are embedded at build time (`include_str!`) so a
//! container image doesn't need to ship the `docs/` tree separately and
//! there is no filesystem lookup at request time. The frontend renders the
//! markdown; keeping the API text-only means the same endpoint can back
//! both a Vue page and an `mdbook` build off the same source tree.

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

const OPENAPI_YAML: &str = include_str!("../../openapi/openapi.yaml");

/// One entry per file in `docs/`. Order matches the intended reading order
/// so the frontend can render the list directly without re-sorting.
struct DocPage {
    slug: &'static str,
    title: &'static str,
    body: &'static str,
}

macro_rules! doc_page {
    ($slug:expr, $title:expr, $path:expr) => {
        DocPage {
            slug: $slug,
            title: $title,
            body: include_str!($path),
        }
    };
}

const PAGES: &[DocPage] = &[
    // User guides
    doc_page!(
        "getting-started",
        "Getting Started",
        "../../docs/GETTING_STARTED.md"
    ),
    doc_page!("deployments", "Deployments", "../../docs/DEPLOYMENTS.md"),
    doc_page!("applications", "Applications", "../../docs/APPLICATIONS.md"),
    doc_page!("ingress", "Ingress", "../../docs/INGRESS.md"),
    doc_page!(
        "logs-and-debugging",
        "Logs and Debugging",
        "../../docs/LOGS_AND_DEBUGGING.md"
    ),
    doc_page!(
        "gitops-user-guide",
        "GitOps for Developers",
        "../../docs/GITOPS_USER_GUIDE.md"
    ),
    // Operator / reference docs
    doc_page!("architecture", "Architecture", "../../docs/ARCHITECTURE.md"),
    doc_page!("auth", "Authentication", "../../docs/AUTH.md"),
    doc_page!("gitops", "GitOps", "../../docs/GITOPS.md"),
    doc_page!("registry", "OCI Registry", "../../docs/REGISTRY.md"),
    doc_page!(
        "templates",
        "Deployment Templates",
        "../../docs/TEMPLATES.md"
    ),
    doc_page!("rollback", "Rollback", "../../docs/ROLLBACK.md"),
    doc_page!("settings", "Settings", "../../docs/SETTINGS.md"),
    doc_page!("metrics", "Metrics", "../../docs/METRICS.md"),
    doc_page!("tracing", "Tracing", "../../docs/TRACING.md"),
    doc_page!("mcp", "MCP Integration", "../../docs/MCP.md"),
    doc_page!(
        "ai-diagnostics",
        "AI Diagnostics",
        "../../docs/AI_DIAGNOSTICS.md"
    ),
    doc_page!("testing", "Testing", "../../docs/TESTING.md"),
];

#[derive(Serialize)]
pub struct DocsIndexEntry {
    pub slug: &'static str,
    pub title: &'static str,
}

#[derive(Serialize)]
pub struct DocsIndexResponse {
    pub pages: Vec<DocsIndexEntry>,
}

pub async fn openapi_yaml() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        OPENAPI_YAML,
    )
        .into_response()
}

pub async fn swagger_ui() -> Response {
    const HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width,initial-scale=1"/>
    <title>Deckwatch API</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.17.14/swagger-ui.css"/>
    <style>body{margin:0;background:#fafafa}</style>
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.17.14/swagger-ui-bundle.js" crossorigin></script>
    <script>
      window.addEventListener('load', () => {
        window.ui = SwaggerUIBundle({
          url: '/api/openapi.yaml',
          dom_id: '#swagger-ui',
          deepLinking: true,
          presets: [SwaggerUIBundle.presets.apis],
          layout: 'BaseLayout',
        });
      });
    </script>
  </body>
</html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        HTML,
    )
        .into_response()
}

pub async fn list_pages() -> Json<DocsIndexResponse> {
    Json(DocsIndexResponse {
        pages: PAGES
            .iter()
            .map(|p| DocsIndexEntry {
                slug: p.slug,
                title: p.title,
            })
            .collect(),
    })
}

pub async fn get_page(Path(slug): Path<String>) -> Response {
    match PAGES.iter().find(|p| p.slug == slug) {
        Some(page) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            page.body,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"not_found","slug":slug})),
        )
            .into_response(),
    }
}
