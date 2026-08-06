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
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::entities::{builds, gitops_configs};
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
        "prompts/list" => handle_prompts_list(&request),
        "prompts/get" => handle_prompts_get(&request),
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
            "capabilities": { "tools": {}, "prompts": {} },
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
            "description": "Create a new deckwatch application in a namespace. Optionally seeds a starter deployment from a template and creates an ingress.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string", "description": "Application name (lowercase alphanumeric or '-', max 53 chars)" },
                    "description": { "type": "string" },
                    "template_id": { "type": "string", "enum": ["web-app", "worker", "cron-job", "static-site"] },
                    "create_deployment": { "type": "boolean", "description": "Seed a starter deployment (default: true)" },
                    "ingress_host": { "type": "string", "description": "If set, creates an ingress with this hostname using the default ingress template" },
                    "ingress_template": { "type": "string", "description": "Ingress template name to use (from Settings). Uses default template if not specified." }
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
            "name": "attach_addon",
            "description": "Attach a sidecar addon to a deployment (e.g. postgres, redis). For postgres, creates a PVC for persistent storage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "addon_id": { "type": "string", "description": "Addon ID from list_addons (e.g. postgres, redis, memcached)" },
                    "storage": { "type": "string", "description": "PVC size for postgres addon (default: 1Gi)" },
                    "storage_class": { "type": "string", "description": "StorageClass for PVC (defaults to settings default)" }
                },
                "required": ["namespace", "deployment_name", "addon_id"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "detach_addon",
            "description": "Detach a sidecar addon from a deployment. Removes the container and cleans up PVCs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "addon_id": { "type": "string" }
                },
                "required": ["namespace", "deployment_name", "addon_id"],
                "additionalProperties": false
            }
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
                    "token_secret": { "type": "string", "description": "Name of a shared token from Settings (looked up by display name)" },
                    "token": { "type": "string", "description": "Per-app git token (encrypted and stored on this gitops config). Use instead of token_secret for project-scoped tokens." },
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
        serde_json::json!({
            "name": "create_ingress_template",
            "description": "Create a new ingress template in deckwatch Settings. Templates define default annotations and ingress class applied when creating ingresses.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Template name (e.g. 'internal-alb', 'public-nginx')" },
                    "ingress_class": { "type": "string", "description": "IngressClass name (e.g. 'alb', 'nginx')" },
                    "annotations": { "type": "object", "description": "Default annotations as key-value pairs", "additionalProperties": { "type": "string" } },
                    "is_default": { "type": "boolean", "description": "Set as the default template for new ingresses (default: false)" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "update_ingress_template",
            "description": "Update an existing ingress template. Only modifies template-owned annotations on ingresses that use it — user-added annotations are preserved.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Template name to update" },
                    "ingress_class": { "type": "string" },
                    "annotations": { "type": "object", "description": "New annotations (replaces template annotations, user annotations preserved)", "additionalProperties": { "type": "string" } },
                    "is_default": { "type": "boolean" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "delete_ingress_template",
            "description": "Delete an ingress template from deckwatch Settings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Template name to delete" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "list_builds",
            "description": "List recent GitOps builds for a deployment with status and logs. Returns the last 20 builds from the database, including captured build logs that persist after Job cleanup.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" }
                },
                "required": ["namespace", "deployment_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "get_build_log",
            "description": "Fetch a single build's log by job name from the database. Useful for diagnosing a specific failed build after its Kubernetes Job has been cleaned up.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "job_name": { "type": "string", "description": "Build group job name (e.g. myapp-build-abc1234)" }
                },
                "required": ["namespace", "deployment_name", "job_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "generate_local_build",
            "description": "Generate a local docker run command that reproduces a deployment's Kaniko build locally. Useful for diagnosing build failures without pushing commits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "platform": { "type": "string", "description": "Target platform (default: linux/amd64)" },
                    "local_context": { "type": "string", "description": "Local path to the repo/build context (default: '.')" },
                    "no_push": { "type": "boolean", "description": "Add --no-push flag (default: true)" }
                },
                "required": ["namespace", "deployment_name"],
                "additionalProperties": false
            }
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
        "attach_addon" => Some(tool_attach_addon(state, args).await),
        "detach_addon" => Some(tool_detach_addon(state, args).await),
        "list_templates" => Some(tool_list_templates(state).await),
        "configure_gitops" => Some(tool_configure_gitops(state, args).await),
        "get_gitops_status" => Some(tool_get_gitops_status(state, args).await),
        "trigger_gitops_build" => Some(tool_trigger_gitops_build(state, args).await),
        "create_ingress" => Some(tool_create_ingress(state, args).await),
        "update_ingress" => Some(tool_update_ingress(state, args).await),
        "list_ingress_templates" => Some(tool_list_ingress_templates(state).await),
        "create_ingress_template" => Some(tool_create_ingress_template(state, args).await),
        "update_ingress_template" => Some(tool_update_ingress_template(state, args).await),
        "delete_ingress_template" => Some(tool_delete_ingress_template(state, args).await),
        "list_builds" => Some(tool_list_builds(state, args).await),
        "get_build_log" => Some(tool_get_build_log(state, args).await),
        "generate_local_build" => Some(tool_generate_local_build(state, args).await),
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
            let last_build_log = if let Some(ref job) = r.last_build_job {
                builds::Entity::find()
                    .filter(builds::Column::JobName.eq(job))
                    .one(&state.db)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|b| b.build_log)
            } else {
                None
            };

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
                "last_build_log": last_build_log,
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

async fn tool_attach_addon(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;
    let addon_id = args["addon_id"].as_str().ok_or("addon_id is required")?;

    let mut req_body = addons::AttachAddonRequest::default();
    if let Some(s) = args["storage"].as_str() {
        req_body.storage = Some(s.to_string());
    }
    if let Some(s) = args["storage_class"].as_str() {
        req_body.storage_class = Some(s.to_string());
    }

    let result = addons::attach(
        State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string(), addon_id.to_string())),
        Some(Json(req_body)),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    let (_status, Json(detail)) = result;
    serde_json::to_string_pretty(&detail).map_err(|e| e.to_string())
}

async fn tool_detach_addon(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;
    let addon_id = args["addon_id"].as_str().ok_or("addon_id is required")?;

    let result = addons::detach(
        State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string(), addon_id.to_string())),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&result.0).map_err(|e| e.to_string())
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

    if let Some(host) = args["ingress_host"].as_str() {
        let template = args["ingress_template"].as_str().map(|s| s.to_string());
        let ingress_req = ingresses::CreateIngressRequest {
            name: name.to_string(),
            host: Some(host.to_string()),
            paths: vec![ingresses::IngressPathInput {
                path: "/".to_string(),
                path_type: Some("Prefix".to_string()),
                service_name: name.to_string(),
                service_port: 80,
            }],
            ingress_class: None,
            annotations: None,
            tls: None,
            template,
        };
        let _ = ingresses::create(
            State(state.clone()),
            axum::extract::Path(ns.to_string()),
            Json(ingress_req),
        )
        .await;
    }

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
        token: args["token"].as_str().map(|s| s.to_string()),
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

async fn tool_create_ingress_template(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;
    let ingress_class = args["ingress_class"].as_str().map(|s| s.to_string());
    let annotations: std::collections::BTreeMap<String, String> = args
        .get("annotations")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let is_default = args["is_default"].as_bool().unwrap_or(false);

    let mut s = settings::load_settings_from_db(state).await;
    if s.ingress_templates.iter().any(|t| t.name == name) {
        return Err(format!("ingress template '{name}' already exists"));
    }

    if is_default {
        for t in &mut s.ingress_templates {
            t.is_default = false;
        }
    }

    s.ingress_templates.push(settings::IngressTemplate {
        name: name.to_string(),
        ingress_class,
        annotations,
        is_default,
    });

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(s.ingress_templates.last().unwrap()).map_err(|e| e.to_string())
}

async fn tool_update_ingress_template(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let mut s = settings::load_settings_from_db(state).await;
    let tmpl = s
        .ingress_templates
        .iter_mut()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("ingress template '{name}' not found"))?;

    if let Some(ic) = args["ingress_class"].as_str() {
        tmpl.ingress_class = Some(ic.to_string());
    }
    if let Some(anns) = args.get("annotations").and_then(|v| {
        serde_json::from_value::<std::collections::BTreeMap<String, String>>(v.clone()).ok()
    }) {
        tmpl.annotations = anns;
    }
    if let Some(d) = args["is_default"].as_bool() {
        if d {
            let target_name = name.to_string();
            for t in &mut s.ingress_templates {
                t.is_default = t.name == target_name;
            }
        } else {
            let tmpl = s
                .ingress_templates
                .iter_mut()
                .find(|t| t.name == name)
                .unwrap();
            tmpl.is_default = false;
        }
    }

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("{e}"))?;

    let updated = s.ingress_templates.iter().find(|t| t.name == name).unwrap();
    serde_json::to_string_pretty(updated).map_err(|e| e.to_string())
}

async fn tool_delete_ingress_template(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let mut s = settings::load_settings_from_db(state).await;
    let before = s.ingress_templates.len();
    s.ingress_templates.retain(|t| t.name != name);
    if s.ingress_templates.len() == before {
        return Err(format!("ingress template '{name}' not found"));
    }

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("{e}"))?;

    Ok(format!("deleted ingress template '{name}'"))
}

async fn tool_list_builds(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;

    let _ = state.deployments_api(ns).map_err(|e| e.to_string())?;

    let app_id = format!("{ns}/{name}");
    let rows = builds::Entity::find()
        .filter(builds::Column::ApplicationId.eq(&app_id))
        .order_by_desc(builds::Column::CreatedAt)
        .limit(20)
        .all(&state.db)
        .await
        .map_err(|e| format!("db error: {e}"))?;

    let build_list: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "job_name": row.job_name,
                "commit_sha": row.commit_sha,
                "image_tag": row.image_tag,
                "status": row.status,
                "started_at": row.started_at.map(|t| t.to_string()),
                "completed_at": row.completed_at.map(|t| t.to_string()),
                "error_message": row.error_message,
                "build_log": row.build_log,
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({ "builds": build_list }))
        .map_err(|e| e.to_string())
}

async fn tool_get_build_log(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;
    let job_name = args["job_name"].as_str().ok_or("job_name is required")?;

    let _ = state.deployments_api(ns).map_err(|e| e.to_string())?;

    let app_id = format!("{ns}/{name}");
    let row = builds::Entity::find()
        .filter(builds::Column::ApplicationId.eq(&app_id))
        .filter(builds::Column::JobName.eq(job_name))
        .one(&state.db)
        .await
        .map_err(|e| format!("db error: {e}"))?;

    match row {
        Some(b) => {
            let result = serde_json::json!({
                "job_name": b.job_name,
                "commit_sha": b.commit_sha,
                "status": b.status,
                "error_message": b.error_message,
                "build_log": b.build_log,
            });
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        }
        None => Err(format!(
            "no build found for job '{job_name}' in {ns}/{name}"
        )),
    }
}

async fn tool_generate_local_build(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;
    let platform = args["platform"].as_str().unwrap_or("linux/amd64");
    let local_context = args["local_context"].as_str().unwrap_or(".");
    let no_push = args["no_push"].as_bool().unwrap_or(true);

    let _ = state.deployments_api(ns).map_err(|e| e.to_string())?;

    let app_id = format!("{ns}/{name}");
    let config = gitops_configs::Entity::find()
        .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
        .one(&state.db)
        .await
        .map_err(|e| format!("db error: {e}"))?
        .ok_or_else(|| format!("no gitops config found for {ns}/{name}"))?;

    let settings_data = settings::load_settings_from_db(state).await;
    let bs = &settings_data.build_settings;

    let dockerfile = &config.dockerfile_path;
    let context = &config.docker_context;

    let local_build_context = if context != "." {
        format!("{local_context}/{context}")
    } else {
        local_context.to_string()
    };

    let dockerfile_basename = dockerfile.split('/').next_back().unwrap_or(dockerfile);

    let mut kaniko_args = vec![
        format!("--dockerfile=/workspace/{dockerfile_basename}"),
        "--context=dir:///workspace".to_string(),
        format!("{}={platform}", bs.platform_flag),
        format!("--snapshot-mode={}", bs.snapshot_mode),
    ];

    if bs.cache_enabled {
        kaniko_args.push("--cache=true".to_string());
    }

    if no_push {
        kaniko_args.push("--no-push".to_string());
    } else {
        let oci_repo = &config.oci_repository;
        kaniko_args.push(format!("--destination={oci_repo}:local-test"));
    }

    for extra in &bs.extra_kaniko_args {
        kaniko_args.push(extra.clone());
    }

    let kaniko_args_str = kaniko_args
        .iter()
        .map(|a| format!("  {a}"))
        .collect::<Vec<_>>()
        .join(" \\\n");

    let docker_cmd = format!(
        "docker run --rm \\\n\
         --platform {platform} \\\n\
         -v {local_build_context}:/workspace \\\n\
         {kaniko_image} \\\n\
         {kaniko_args_str}",
        kaniko_image = bs.kaniko_image,
    );

    let result = serde_json::json!({
        "command": docker_cmd,
        "kaniko_image": bs.kaniko_image,
        "platform": platform,
        "dockerfile": dockerfile,
        "docker_context": context,
        "local_context": local_context,
        "no_push": no_push,
        "notes": [
            "Run this command from your local repo root to reproduce the Kaniko build locally.",
            "The --no-push flag prevents pushing to the registry (safe for local testing).",
            format!("To test arm64: change --platform to linux/arm64"),
            format!("Kaniko image: {} (from deckwatch build settings)", bs.kaniko_image),
        ]
    });

    serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// prompts/list + prompts/get
// ---------------------------------------------------------------------------

fn handle_prompts_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    success_response(
        request,
        serde_json::json!({
            "prompts": [{
                "name": "deployment-readiness-check",
                "description": "Check if an application is ready to be deployed and managed by deckwatch",
                "arguments": [
                    {"name": "namespace", "description": "Kubernetes namespace", "required": true},
                    {"name": "deployment_name", "description": "Deployment name", "required": true}
                ]
            }]
        }),
    )
}

fn handle_prompts_get(request: &JsonRpcRequest) -> JsonRpcResponse {
    let params = &request.params;

    let name = match params["name"].as_str() {
        Some(n) => n,
        None => return error_response(request, -32602, "Missing 'name' parameter"),
    };

    match name {
        "deployment-readiness-check" => {
            let args = &params["arguments"];
            let ns = args["namespace"].as_str().unwrap_or("default");
            let dep = args["deployment_name"].as_str().unwrap_or("my-app");

            let prompt_text = format!(
                r#"You are evaluating whether the application "{dep}" in namespace "{ns}" is ready for production deployment on deckwatch. Run the following checks using the available MCP tools and report a checklist with pass/fail for each item:

1. **Deployment exists**: Call `get_deployment` for namespace "{ns}", name "{dep}"
2. **Container image**: Check the image field — flag if it uses `:latest` tag or no tag
3. **GitOps configured**: Call `get_gitops_status` — check repo_url, branch, dockerfile_path are set
4. **Database**: Check DATABASE_URL env var — flag if pointing to localhost, sqlite, or missing
5. **Resource limits**: Check resource_limits and resource_requests are set
6. **Readiness probe**: Check readiness_probe is configured
7. **Ingress**: Call `list_ingresses` in namespace "{ns}" — check an ingress exists for this service
8. **Application label**: Check deckwatch.io/application label exists on the deployment
9. **Pod health**: Call `list_pods` in namespace "{ns}" — check all pods are Running/Ready with zero restarts
10. **Image pull**: Verify no ImagePullBackOff or ErrImagePull on any pod

Format the output as a checklist with a checkmark or X for each item, with details on what needs to be fixed for any failing checks."#,
            );

            success_response(
                request,
                serde_json::json!({
                    "description": "Check if an application is ready to be deployed and managed by deckwatch",
                    "messages": [{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": prompt_text
                        }
                    }]
                }),
            )
        }
        _ => error_response(request, -32602, &format!("Unknown prompt: {name}")),
    }
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
