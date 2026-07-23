//! Streamable HTTP MCP (Model Context Protocol) server endpoint.
//!
//! Implements the MCP 2025-11-25 spec over JSON-RPC 2.0. Exposes deckwatch
//! Kubernetes management capabilities as MCP tools so Claude Code (and other
//! MCP clients) can query cluster state — pod logs, events, deployment status,
//! GitOps config, build history — alongside local filesystem access.
//!
//! Wire up: `POST /mcp` in the public API router (no auth layer so MCP
//! clients can connect without a bearer token, same as healthz).

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use k8s_openapi::api::apps::v1::ReplicaSet;
use kube::api::ListParams;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::entities::gitops_configs;
use crate::handlers::applications;
use crate::handlers::{addons, gitops, templates};
use crate::kube_ext;
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
            "serverInfo": { "name": "deckwatch", "version": "0.1.0" }
        }),
    )
}

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

fn handle_tools_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    let tools = serde_json::json!({
        "tools": [
            {
                "name": "get_namespaces",
                "description": "List all Kubernetes namespaces visible to deckwatch.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "list_deployments",
                "description": "List deployments in a namespace with replica counts and status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" }
                    },
                    "required": ["namespace"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_deployment",
                "description": "Get detailed info for a single deployment including pods and ingresses.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "name": { "type": "string", "description": "Deployment name" }
                    },
                    "required": ["namespace", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_pod_logs",
                "description": "Fetch logs from a pod. Optionally scope to a container and limit line count.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "pod_name": { "type": "string", "description": "Pod name" },
                        "tail_lines": { "type": "integer", "description": "Number of recent lines to return" },
                        "container": { "type": "string", "description": "Container name (optional, defaults to first)" }
                    },
                    "required": ["namespace", "pod_name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_events",
                "description": "List Kubernetes events in a namespace, optionally filtered by resource name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "resource_name": { "type": "string", "description": "Filter to events involving this resource name" }
                    },
                    "required": ["namespace"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_deployment_history",
                "description": "List revision history (ReplicaSets) for a deployment.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "name": { "type": "string", "description": "Deployment name" }
                    },
                    "required": ["namespace", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_gitops_status",
                "description": "Get GitOps configuration and last build status for a deployment.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "name": { "type": "string", "description": "Deployment name" }
                    },
                    "required": ["namespace", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_build_logs",
                "description": "Fetch logs from a Kubernetes Job's pod (e.g. a GitOps build job).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "job_name": { "type": "string", "description": "Job name" }
                    },
                    "required": ["namespace", "job_name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "list_ingresses",
                "description": "List ingresses in a namespace with hosts, classes, and addresses.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" }
                    },
                    "required": ["namespace"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_metrics",
                "description": "Get pod resource usage metrics (CPU/memory) from metrics-server.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "label_selector": { "type": "string", "description": "Label selector to scope pods (e.g. app=foo)" }
                    },
                    "required": ["namespace"],
                    "additionalProperties": false
                }
            },
            {
                "name": "create_application",
                "description": "Create a new deckwatch application in a namespace. Optionally seeds a starter deployment from a template (web-app, worker, cron-job, static-site).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "name": { "type": "string", "description": "Application name (lowercase alphanumeric or '-', max 53 chars)" },
                        "description": { "type": "string", "description": "Human-readable description of the application" },
                        "template_id": { "type": "string", "description": "Deployment template: web-app (default), worker, cron-job, static-site", "enum": ["web-app", "worker", "cron-job", "static-site"] },
                        "create_deployment": { "type": "boolean", "description": "Seed a starter deployment from the template (default: true)" }
                    },
                    "required": ["namespace", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "list_addons",
                "description": "List available deployment addons (sidecar containers like Redis, PostgreSQL, Memcached, etc.) with their default configuration.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "list_templates",
                "description": "List available deployment templates (web-app, worker, cron-job, static-site, plus custom templates) with their pre-filled payloads.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "configure_gitops",
                "description": "Enable GitOps for a deployment. Configures deckwatch to poll a git repo, build container images with Kaniko on new commits, and auto-deploy them.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string", "description": "Kubernetes namespace" },
                        "deployment_name": { "type": "string", "description": "Name of the deployment to configure GitOps for" },
                        "repo_url": { "type": "string", "description": "Git repository URL (e.g. https://github.com/org/repo)" },
                        "branch": { "type": "string", "description": "Branch to watch (default: main)" },
                        "dockerfile_path": { "type": "string", "description": "Path to Dockerfile relative to repo root (default: Dockerfile)" },
                        "docker_context": { "type": "string", "description": "Docker build context path (default: .)" },
                        "oci_repository": { "type": "string", "description": "OCI registry destination for built images (e.g. ghcr.io/org/app). Defaults to the internal deckwatch registry if available." },
                        "token_secret": { "type": "string", "description": "Name of K8s Secret containing git credentials (for private repos)" },
                        "git_auth_user": { "type": "string", "description": "HTTP Basic auth username for git (auto-detected: oauth2 for GitLab, x-access-token for GitHub)" },
                        "poll_interval_seconds": { "type": "integer", "description": "How often to poll for new commits (default: 30)" }
                    },
                    "required": ["namespace", "deployment_name", "repo_url"],
                    "additionalProperties": false
                }
            }
        ]
    });

    success_response(request, tools)
}

// ---------------------------------------------------------------------------
// tools/call dispatch
// ---------------------------------------------------------------------------

async fn handle_tool_call(state: &AppState, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params = &request.params;
    let tool_name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];

    let result = match tool_name {
        "get_namespaces" => tool_get_namespaces(state).await,
        "list_deployments" => tool_list_deployments(state, args).await,
        "get_deployment" => tool_get_deployment(state, args).await,
        "get_pod_logs" => tool_get_pod_logs(state, args).await,
        "get_events" => tool_get_events(state, args).await,
        "get_deployment_history" => tool_get_deployment_history(state, args).await,
        "get_gitops_status" => tool_get_gitops_status(state, args).await,
        "get_build_logs" => tool_get_build_logs(state, args).await,
        "list_ingresses" => tool_list_ingresses(state, args).await,
        "get_metrics" => tool_get_metrics(state, args).await,
        "create_application" => tool_create_application(state, args).await,
        "list_addons" => tool_list_addons().await,
        "list_templates" => tool_list_templates(state).await,
        "configure_gitops" => tool_configure_gitops(state, args).await,
        _ => Err(format!("Unknown tool: {tool_name}")),
    };

    match result {
        Ok(text) => success_response(
            request,
            serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            }),
        ),
        Err(e) => error_response(request, -32000, &e),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

async fn tool_get_namespaces(state: &AppState) -> Result<String, String> {
    let ns_api = state.namespaces_api();
    let list = ns_api
        .list(&ListParams::default())
        .await
        .map_err(|e| e.to_string())?;

    let names: Vec<String> = list
        .iter()
        .filter_map(|ns| ns.metadata.name.clone())
        .filter(|name| state.is_namespace_allowed(name))
        .collect();

    Ok(names.join("\n"))
}

async fn tool_list_deployments(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let api = state.deployments_api(ns).map_err(|e| e.to_string())?;
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| e.to_string())?;

    let summaries: Vec<serde_json::Value> = list
        .iter()
        .map(|dep| {
            let s = kube_ext::deployment_summary(dep);
            serde_json::to_value(s).unwrap_or_default()
        })
        .collect();

    serde_json::to_string_pretty(&summaries).map_err(|e| e.to_string())
}

async fn tool_get_deployment(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;

    let dep_api = state.deployments_api(ns).map_err(|e| e.to_string())?;
    let dep = dep_api.get(name).await.map_err(|e| e.to_string())?;
    let detail = kube_ext::deployment_detail(&dep);

    // Fetch pods for the deployment
    let pods = {
        let pods_api = state.pods_api(ns).map_err(|e| e.to_string())?;
        let selector = dep
            .spec
            .as_ref()
            .and_then(|s| s.selector.match_labels.as_ref())
            .map(|labels| {
                labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let lp = ListParams::default().labels(&selector);
        let pod_list = pods_api.list(&lp).await.map_err(|e| e.to_string())?;
        let summaries: Vec<serde_json::Value> = pod_list
            .iter()
            .map(|p| serde_json::to_value(kube_ext::pod_summary(p)).unwrap_or_default())
            .collect();
        summaries
    };

    // Fetch ingresses that reference this deployment's service
    let ingresses = {
        let ing_api = state.ingresses_api(ns).map_err(|e| e.to_string())?;
        let all = ing_api
            .list(&ListParams::default())
            .await
            .map_err(|e| e.to_string())?;
        let matching: Vec<serde_json::Value> = all
            .iter()
            .filter(|ing| {
                ing.spec
                    .as_ref()
                    .and_then(|s| s.rules.as_ref())
                    .map(|rules| {
                        rules.iter().any(|r| {
                            r.http
                                .as_ref()
                                .map(|http| {
                                    http.paths.iter().any(|p| {
                                        p.backend
                                            .service
                                            .as_ref()
                                            .map(|s| s.name == name)
                                            .unwrap_or(false)
                                    })
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
            .map(|ing| serde_json::to_value(kube_ext::ingress_summary(ing)).unwrap_or_default())
            .collect();
        matching
    };

    let result = serde_json::json!({
        "detail": serde_json::to_value(detail).unwrap_or_default(),
        "pods": pods,
        "ingresses": ingresses,
    });

    serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
}

async fn tool_get_pod_logs(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let pod = args["pod_name"].as_str().ok_or("pod_name is required")?;
    let tail = args["tail_lines"].as_i64();
    let container = args["container"].as_str().map(|s| s.to_string());

    let pods_api = state.pods_api(ns).map_err(|e| e.to_string())?;
    let params = kube::api::LogParams {
        tail_lines: tail,
        container,
        timestamps: true,
        ..Default::default()
    };
    let logs = pods_api
        .logs(pod, &params)
        .await
        .map_err(|e| e.to_string())?;
    Ok(logs)
}

async fn tool_get_events(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let resource_name = args["resource_name"].as_str();

    let api = state.events_api(ns).map_err(|e| e.to_string())?;
    let mut lp = ListParams::default();
    if let Some(name) = resource_name {
        lp = lp.fields(&format!("involvedObject.name={name}"));
    }
    let list = api.list(&lp).await.map_err(|e| e.to_string())?;

    let mut events: Vec<String> = list
        .iter()
        .map(|e| {
            let s = kube_ext::event_summary(e);
            format!(
                "[{}] {} {} {}: {}",
                s.last_timestamp
                    .as_deref()
                    .or(s.first_timestamp.as_deref())
                    .unwrap_or("?"),
                s.event_type,
                s.involved_object_kind,
                s.involved_object_name,
                s.message.as_deref().unwrap_or("(no message)"),
            )
        })
        .collect();

    // Sort newest first by the formatted timestamp prefix
    events.sort_by(|a, b| b.cmp(a));

    if events.is_empty() {
        Ok("No events found.".to_string())
    } else {
        Ok(events.join("\n"))
    }
}

async fn tool_get_deployment_history(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;

    let dep_api = state.deployments_api(ns).map_err(|e| e.to_string())?;
    let dep = dep_api.get(name).await.map_err(|e| e.to_string())?;

    let selector = dep
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();

    let rs_api: kube::Api<ReplicaSet> = kube::Api::namespaced(state.kube_client.clone(), ns);
    let lp = ListParams::default().labels(&selector);
    let rs_list = rs_api.list(&lp).await.map_err(|e| e.to_string())?;

    let mut revisions: Vec<String> = rs_list
        .items
        .iter()
        .filter_map(|rs| {
            let annotations = rs.metadata.annotations.as_ref()?;
            let revision: i64 = annotations
                .get("deployment.kubernetes.io/revision")?
                .parse()
                .ok()?;
            let image = rs
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.spec.as_ref())
                .and_then(|s| s.containers.first())
                .and_then(|c| c.image.clone())
                .unwrap_or_else(|| "<unknown>".to_string());
            let replicas = rs.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
            let ready = rs
                .status
                .as_ref()
                .and_then(|s| s.ready_replicas)
                .unwrap_or(0);
            let created = rs
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0.to_string())
                .unwrap_or_else(|| "?".to_string());
            let change_cause = annotations
                .get("kubernetes.io/change-cause")
                .cloned()
                .unwrap_or_default();

            Some(format!(
                "Rev {revision}: image={image} replicas={replicas}/{ready} created={created} cause={change_cause}"
            ))
        })
        .collect();

    revisions.sort_by(|a, b| b.cmp(a));

    if revisions.is_empty() {
        Ok("No revision history found.".to_string())
    } else {
        Ok(revisions.join("\n"))
    }
}

async fn tool_get_gitops_status(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["name"].as_str().ok_or("name is required")?;

    // Verify namespace is allowed
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

async fn tool_get_build_logs(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let job_name = args["job_name"].as_str().ok_or("job_name is required")?;

    // Find the pod for this job
    let pods_api = state.pods_api(ns).map_err(|e| e.to_string())?;
    let lp = ListParams::default().labels(&format!("job-name={job_name}"));
    let pods = pods_api.list(&lp).await.map_err(|e| e.to_string())?;

    let pod_name = pods
        .items
        .first()
        .and_then(|p| p.metadata.name.clone())
        .ok_or_else(|| format!("No pod found for job {job_name}"))?;

    let params = kube::api::LogParams {
        timestamps: true,
        ..Default::default()
    };
    let logs = pods_api
        .logs(&pod_name, &params)
        .await
        .map_err(|e| e.to_string())?;
    Ok(logs)
}

async fn tool_list_ingresses(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let api = state.ingresses_api(ns).map_err(|e| e.to_string())?;
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| e.to_string())?;

    let summaries: Vec<serde_json::Value> = list
        .iter()
        .map(|ing| serde_json::to_value(kube_ext::ingress_summary(ing)).unwrap_or_default())
        .collect();

    serde_json::to_string_pretty(&summaries).map_err(|e| e.to_string())
}

async fn tool_get_metrics(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let label_selector = args["label_selector"].as_str();

    if !state.is_namespace_allowed(ns) {
        return Err(format!("Namespace '{ns}' is not in the allowed list"));
    }

    // Build the metrics API URI
    let mut uri = format!("/apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods");
    if let Some(sel) = label_selector.filter(|s| !s.is_empty()) {
        uri.push_str("?labelSelector=");
        // Minimal URL-encoding for label selectors
        for b in sel.bytes() {
            match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'~'
                | b'='
                | b',' => uri.push(b as char),
                _ => uri.push_str(&format!("%{b:02X}")),
            }
        }
    }

    let req = axum::http::Request::builder()
        .uri(&uri)
        .body(Vec::new())
        .map_err(|e| e.to_string())?;

    // Use the same wire types as resource_metrics.rs
    #[derive(Deserialize)]
    struct MetricsList {
        items: Vec<RawPodMetrics>,
    }
    #[derive(Deserialize)]
    struct RawPodMetrics {
        metadata: MetricsMeta,
        #[allow(dead_code)]
        timestamp: String,
        containers: Vec<RawContainerMetrics>,
    }
    #[derive(Deserialize)]
    struct MetricsMeta {
        name: String,
    }
    #[derive(Deserialize)]
    struct RawContainerMetrics {
        name: String,
        usage: Usage,
    }
    #[derive(Deserialize)]
    struct Usage {
        cpu: String,
        memory: String,
    }

    let result: Result<MetricsList, kube::Error> = state.kube_client.request(req).await;

    match result {
        Ok(list) => {
            let mut lines: Vec<String> = Vec::new();
            for pod in list.items {
                for c in pod.containers {
                    lines.push(format!(
                        "pod={} container={} cpu={} memory={}",
                        pod.metadata.name, c.name, c.usage.cpu, c.usage.memory,
                    ));
                }
            }
            if lines.is_empty() {
                Ok("No pod metrics found. Is metrics-server installed?".to_string())
            } else {
                Ok(lines.join("\n"))
            }
        }
        Err(kube::Error::Api(api_err)) if api_err.code == 404 => {
            Ok("metrics-server does not appear to be installed in this cluster.".to_string())
        }
        Err(kube::Error::Api(api_err)) if api_err.code == 503 => {
            Ok("metrics-server is installed but not ready. Give it 60s after startup.".to_string())
        }
        Err(e) => Err(e.to_string()),
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
    serde_json::to_string_pretty(&response.0) .map_err(|e| e.to_string())
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
    let repo_url = args["repo_url"]
        .as_str()
        .ok_or("repo_url is required")?;
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

    serde_json::to_string_pretty(&result.0) .map_err(|e| e.to_string())
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
