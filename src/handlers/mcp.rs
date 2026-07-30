//! Streamable HTTP MCP (Model Context Protocol) server endpoint.
//!
//! Exposes deckwatch-specific tools (applications, GitOps, addons, templates)
//! alongside 160+ generic Kubernetes tools from the mcp-k8s upstream library.
//!
//! Wire up: `POST /mcp` in the public API router.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::entities::gitops_configs;
use crate::handlers::applications;
use crate::handlers::{addons, gitops, ingresses, settings, templates};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Main handler
// ---------------------------------------------------------------------------

pub async fn handle_mcp(
    State(state): State<AppState>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match request.method.as_str() {
        "initialize" => handle_initialize(&request),
        "notifications/initialized" => return StatusCode::OK.into_response(),
        "ping" => success_response(&request, serde_json::json!({})),
        "tools/list" => handle_tools_list(&request),
        "tools/call" => handle_tool_call(&state, &request).await,
        _ => method_not_found(&request),
    };

    ([(header::CONTENT_TYPE, "application/json")], Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

fn handle_initialize(request: &JsonRpcRequest) -> JsonRpcResponse {
    success_response(
        request,
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "deckwatch", "version": "0.3.2" }
        }),
    )
}

// ---------------------------------------------------------------------------
// tools/list — upstream mcp-k8s tools + deckwatch-specific tools
// ---------------------------------------------------------------------------

fn handle_tools_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    let perms = mcp_k8s::permissions::ActionPermissions::default();
    let mut tools = mcp_k8s::mcp::tool_definitions(&perms);
    tools.extend(mcp_k8s::resources::all_tool_definitions());
    tools.extend(deckwatch_tool_definitions());
    success_response(request, serde_json::json!({ "tools": tools }))
}

fn deckwatch_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "create_application",
            "description": "Create a new deckwatch application in a namespace. Optionally seeds a starter deployment from a template.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string", "description": "Application name (lowercase alphanumeric or '-', max 53 chars)" },
                    "description": { "type": "string" },
                    "template_id": { "type": "string", "enum": ["web-app", "worker", "cron-job", "static-site"] },
                    "create_deployment": { "type": "boolean", "description": "Seed a starter deployment (default: true)" }
                },
                "required": ["namespace", "name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "list_addons",
            "description": "List available deployment addons (Redis, PostgreSQL, Memcached, etc.).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        serde_json::json!({
            "name": "list_templates",
            "description": "List available deployment templates with pre-filled payloads.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        serde_json::json!({
            "name": "configure_gitops",
            "description": "Enable GitOps for a deployment — poll a git repo, build with Kaniko, auto-deploy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "repo_url": { "type": "string" },
                    "branch": { "type": "string" },
                    "dockerfile_path": { "type": "string" },
                    "docker_context": { "type": "string" },
                    "oci_repository": { "type": "string", "description": "Defaults to internal registry if available" },
                    "token_secret": { "type": "string" },
                    "git_auth_user": { "type": "string", "description": "Auto-detected: oauth2 for GitLab, x-access-token for GitHub" },
                    "poll_interval_seconds": { "type": "integer" }
                },
                "required": ["namespace", "deployment_name", "repo_url"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "get_gitops_status",
            "description": "Get GitOps configuration and last build status for a deployment (reads from deckwatch database).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["namespace", "name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "trigger_gitops_build",
            "description": "Trigger a GitOps build for a deployment. Clones the repo, builds a container image with Kaniko, and deploys it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string", "description": "Deployment name" }
                },
                "required": ["namespace", "name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "create_ingress",
            "description": "Create a Kubernetes Ingress resource. Automatically creates a backing ClusterIP Service if one doesn't exist. Supports ingress templates for pre-configured annotations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string", "description": "Ingress name" },
                    "host": { "type": "string", "description": "Hostname for the ingress rule (e.g. myapp.example.com)" },
                    "service_name": { "type": "string", "description": "Backend service name" },
                    "service_port": { "type": "integer", "description": "Backend service port (default: 80)" },
                    "path": { "type": "string", "description": "URL path (default: /)" },
                    "path_type": { "type": "string", "description": "Path matching type (default: Prefix)" },
                    "ingress_class": { "type": "string", "description": "IngressClass name" },
                    "template": { "type": "string", "description": "Ingress template name from Settings. Applies default annotations and ingress class." },
                    "annotations": { "type": "object", "description": "Additional annotations (merged with template, request wins)", "additionalProperties": { "type": "string" } }
                },
                "required": ["namespace", "name", "service_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "update_ingress",
            "description": "Update an existing Kubernetes Ingress resource (host, paths, annotations, TLS).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string" },
                    "host": { "type": "string" },
                    "service_name": { "type": "string" },
                    "service_port": { "type": "integer" },
                    "path": { "type": "string" },
                    "path_type": { "type": "string" },
                    "ingress_class": { "type": "string" },
                    "annotations": { "type": "object", "additionalProperties": { "type": "string" } }
                },
                "required": ["namespace", "name", "service_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "list_ingress_templates",
            "description": "List available ingress templates configured in deckwatch Settings. Templates provide pre-configured annotations and ingress class for creating ingresses.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
    ]
}

// ---------------------------------------------------------------------------
// tools/call dispatch
// ---------------------------------------------------------------------------

async fn handle_tool_call(state: &AppState, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params = &request.params;
    let tool_name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];

    let result = match tool_name {
        "create_application" => Some(tool_create_application(state, args).await),
        "list_addons" => Some(tool_list_addons().await),
        "list_templates" => Some(tool_list_templates(state).await),
        "configure_gitops" => Some(tool_configure_gitops(state, args).await),
        "get_gitops_status" => Some(tool_get_gitops_status(state, args).await),
        "trigger_gitops_build" => Some(tool_trigger_gitops_build(state, args).await),
        "create_ingress" => Some(tool_create_ingress(state, args).await),
        "update_ingress" => Some(tool_update_ingress(state, args).await),
        "list_ingress_templates" => Some(tool_list_ingress_templates(state).await),
        _ => None,
    };

    let result = match result {
        Some(r) => r,
        None => {
            let k8s_client = mcp_k8s::K8sClient::new(
                state.kube_client.clone(),
                state.allowed_namespaces.clone(),
                mcp_k8s::permissions::ActionPermissions::default(),
            );
            if let Some(r) = mcp_k8s::mcp::handle_tool(&k8s_client, tool_name, args).await {
                r
            } else if let Some(r) =
                mcp_k8s::resources::handle_tool(&k8s_client, tool_name, args).await
            {
                r
            } else {
                Err(format!("Unknown tool: {tool_name}"))
            }
        }
    };

    match result {
        Ok(text) => success_response(
            request,
            serde_json::json!({ "content": [{ "type": "text", "text": text }] }),
        ),
        Err(e) => error_response(request, -32000, &e),
    }
}

// ---------------------------------------------------------------------------
// Deckwatch-specific tool implementations
// ---------------------------------------------------------------------------

async fn tool_get_gitops_status(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;

    let _ = state.deployments_api(ns).map_err(|e| e.to_string())?;

    let app_id = format!("{ns}/{name}");
    let row = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await
        .map_err(|e| format!("db error: {e}"))?;

    match row {
        Some(r) => {
            let result = serde_json::json!({
                "enabled": true,
                "repo_url": r.repo_url,
                "branch": r.branch,
                "dockerfile_path": r.dockerfile_path,
                "docker_context": r.docker_context,
                "oci_repository": r.oci_repository,
                "poll_interval_seconds": r.poll_interval_seconds,
                "webhook_enabled": r.webhook_enabled,
                "last_commit_sha": r.last_commit_sha,
                "last_build_status": r.last_build_status,
                "last_build_job": r.last_build_job,
                "last_build_time": r.last_build_time.map(|t| t.to_string()),
                "last_build_error": r.last_build_error,
            });
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        }
        None => Ok(format!("GitOps is not configured for {ns}/{name}.")),
    }
}

async fn tool_list_addons() -> Result<String, String> {
    let Json(response) = addons::list().await;
    serde_json::to_string_pretty(&response).map_err(|e| e.to_string())
}

async fn tool_list_templates(state: &AppState) -> Result<String, String> {
    let response = templates::list(State(state.clone()))
        .await
        .map_err(|e| format!("{e}"))?;
    serde_json::to_string_pretty(&response.0).map_err(|e| e.to_string())
}

async fn tool_create_application(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;
    let description = args["description"].as_str().map(|s| s.to_string());
    let template_id = args["template_id"].as_str().map(|s| s.to_string());
    let create_deployment = args["create_deployment"].as_bool().unwrap_or(true);

    let req = applications::ApplicationRequest {
        name: name.to_string(),
        description,
        git: None,
        create_deployment: Some(create_deployment),
        template_id,
    };

    let result = applications::create(
        State(state.clone()),
        axum::extract::Path(ns.to_string()),
        Json(req),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    let (_status, Json(detail)) = result;
    serde_json::to_string_pretty(&detail).map_err(|e| e.to_string())
}

async fn tool_configure_gitops(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;
    let repo_url = args["repo_url"].as_str().ok_or("repo_url is required")?;
    let oci_repository = args["oci_repository"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            state
                .registry_public_url
                .as_ref()
                .map(|base| format!("{base}/{name}"))
        })
        .ok_or("oci_repository is required (no internal registry configured)")?;

    let req = gitops::GitOpsConfigRequest {
        repo_url: repo_url.to_string(),
        branch: args["branch"].as_str().map(|s| s.to_string()),
        token_secret: args["token_secret"].as_str().map(|s| s.to_string()),
        git_auth_user: args["git_auth_user"].as_str().map(|s| s.to_string()),
        dockerfile_path: args["dockerfile_path"].as_str().map(|s| s.to_string()),
        docker_context: args["docker_context"].as_str().map(|s| s.to_string()),
        oci_repository: Some(oci_repository),
        ecr_repository: None,
        include_paths: None,
        exclude_paths: None,
        poll_interval_seconds: args["poll_interval_seconds"].as_i64(),
        webhook_enabled: None,
        webhook_secret: None,
    };

    let result = gitops::set_config(
        State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
        Json(req),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&result.0).map_err(|e| e.to_string())
}

async fn tool_trigger_gitops_build(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;

    let result = gitops::trigger_build(
        State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&result.0).map_err(|e| e.to_string())
}

async fn tool_create_ingress(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;
    let service_name = args["service_name"]
        .as_str()
        .ok_or("service_name is required")?;
    let service_port = args["service_port"].as_i64().unwrap_or(80) as i32;
    let path = args["path"].as_str().unwrap_or("/").to_string();
    let path_type = args["path_type"].as_str().map(|s| s.to_string());
    let host = args["host"].as_str().map(|s| s.to_string());
    let ingress_class = args["ingress_class"].as_str().map(|s| s.to_string());
    let template = args["template"].as_str().map(|s| s.to_string());
    let annotations: Option<std::collections::BTreeMap<String, String>> = args
        .get("annotations")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let req = ingresses::CreateIngressRequest {
        name: name.to_string(),
        host,
        paths: vec![ingresses::IngressPathInput {
            path,
            path_type,
            service_name: service_name.to_string(),
            service_port,
        }],
        ingress_class,
        annotations,
        tls: None,
        template,
    };

    let result = ingresses::create(
        State(state.clone()),
        axum::extract::Path(ns.to_string()),
        Json(req),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    let (_status, Json(detail)) = result;
    serde_json::to_string_pretty(&detail).map_err(|e| e.to_string())
}

async fn tool_update_ingress(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;
    let service_name = args["service_name"]
        .as_str()
        .ok_or("service_name is required")?;
    let service_port = args["service_port"].as_i64().unwrap_or(80) as i32;
    let path = args["path"].as_str().unwrap_or("/").to_string();
    let path_type = args["path_type"].as_str().map(|s| s.to_string());
    let host = args["host"].as_str().map(|s| s.to_string());
    let ingress_class = args["ingress_class"].as_str().map(|s| s.to_string());
    let annotations: Option<std::collections::BTreeMap<String, String>> = args
        .get("annotations")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let req = ingresses::CreateIngressRequest {
        name: name.to_string(),
        host,
        paths: vec![ingresses::IngressPathInput {
            path,
            path_type,
            service_name: service_name.to_string(),
            service_port,
        }],
        ingress_class,
        annotations,
        tls: None,
        template: None,
    };

    let result = ingresses::update(
        State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
        Json(req),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&result.0).map_err(|e| e.to_string())
}

async fn tool_list_ingress_templates(state: &AppState) -> Result<String, String> {
    let s = settings::load_settings_from_db(state).await;
    serde_json::to_string_pretty(&s.ingress_templates).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn success_response(request: &JsonRpcRequest, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: request.id.clone(),
        result: Some(result),
        error: None,
    }
}

fn error_response(request: &JsonRpcRequest, code: i32, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: request.id.clone(),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
        }),
    }
}

fn method_not_found(request: &JsonRpcRequest) -> JsonRpcResponse {
    error_response(
        request,
        -32601,
        &format!("Method not found: {}", request.method),
    )
}

#[cfg(test)]
#[path = "../handlers_mcp_tests.rs"]
mod tests;
