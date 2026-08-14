//! Streamable HTTP MCP (Model Context Protocol) server endpoint.
//!
//! Exposes deckwatch-specific tools (applications, GitOps, addons, templates)
//! alongside 160+ generic Kubernetes tools from the mcp-k8s upstream library.
//!
//! Wire up: `POST /mcp` in the public API router.

use std::time::Instant;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::entities::{agent_feedback, builds, gitops_configs};
use crate::handlers::applications;
use crate::handlers::{addons, gitops, ingresses, monitoring, settings, templates};
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
    let start = Instant::now();
    let method = request.method.clone();

    if request.method == "notifications/initialized" {
        crate::metrics::record_mcp_request(&method, "ok", start.elapsed().as_secs_f64());
        return StatusCode::OK.into_response();
    }

    let response = match request.method.as_str() {
        "initialize" => handle_initialize(&state, &request).await,
        "ping" => success_response(&request, serde_json::json!({})),
        "tools/list" => handle_tools_list(&state, &request).await,
        "tools/call" => handle_tool_call(&state, &request).await,
        "prompts/list" => handle_prompts_list(&request),
        "prompts/get" => handle_prompts_get(&request),
        _ => method_not_found(&request),
    };

    let status = if response.error.is_some() {
        "error"
    } else {
        "ok"
    };
    crate::metrics::record_mcp_request(&method, status, start.elapsed().as_secs_f64());

    ([(header::CONTENT_TYPE, "application/json")], Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

async fn handle_initialize(state: &AppState, request: &JsonRpcRequest) -> JsonRpcResponse {
    let loaded = settings::load_settings_from_db(state).await;
    let tuning = loaded.mcp_tuning;
    let agent_feedback_enabled = loaded.agent_feedback_enabled;

    let mut result = serde_json::json!({
        "protocolVersion": "2025-11-25",
        "capabilities": { "tools": {}, "prompts": {} },
        "serverInfo": { "name": "deckwatch", "version": env!("CARGO_PKG_VERSION") }
    });

    let mut instructions_parts: Vec<String> = Vec::new();

    if let Some(global) = tuning.global.filter(|s| !s.is_empty()) {
        instructions_parts.push(global);
    }

    if agent_feedback_enabled {
        instructions_parts.push(
            "Agent feedback is enabled. If at any point you encounter missing tooling, \
             a suboptimal workflow, or a situation where better guidance would have led to a \
             better outcome — record it using `submit_agent_feedback`. Be specific: what were \
             you trying to do, what was missing, and what would have helped."
                .to_string(),
        );
    }

    if !instructions_parts.is_empty() {
        result["instructions"] = serde_json::Value::String(instructions_parts.join("\n\n"));
    }

    success_response(request, result)
}

// ---------------------------------------------------------------------------
// tools/list — upstream mcp-k8s tools + deckwatch-specific tools
// ---------------------------------------------------------------------------

async fn handle_tools_list(state: &AppState, request: &JsonRpcRequest) -> JsonRpcResponse {
    let loaded = settings::load_settings_from_db(state).await;
    let tuning = loaded.mcp_tuning;
    let agent_feedback_enabled = loaded.agent_feedback_enabled;

    let perms = mcp_k8s::permissions::ActionPermissions::default();
    let mut tools = mcp_k8s::mcp::tool_definitions(&perms);
    tools.extend(mcp_k8s::resources::all_tool_definitions());
    tools.extend(deckwatch_tool_definitions());

    if agent_feedback_enabled {
        tools.push(agent_feedback_tool_definition());
    }

    let tools = tools
        .into_iter()
        .map(|t| inject_mcp_hint(t, &tuning))
        .collect::<Vec<_>>();
    success_response(request, serde_json::json!({ "tools": tools }))
}

/// Map a tool name to its resource group for hint injection.
fn mcp_resource_group(name: &str) -> Option<&'static str> {
    // Check specific deckwatch tools first to avoid ambiguous pattern matches.
    if matches!(
        name,
        "create_application" | "list_addons" | "attach_addon" | "detach_addon" | "list_templates"
    ) {
        return Some("applications");
    }
    if name.contains("gitops") || name.contains("build") || name == "trigger_build" {
        return Some("gitops");
    }
    if name.contains("plugin") {
        return Some("plugins");
    }
    if name == "enable_monitoring" || name == "disable_monitoring" || name.contains("monitor") {
        return Some("deployments");
    }
    // Pattern-based groups — ordered from most to least specific.
    if name.contains("ingress") {
        return Some("ingresses");
    }
    if name.contains("namespace") {
        return Some("namespaces");
    }
    if name.contains("deployment")
        || matches!(
            name,
            "scale_deployment" | "restart_deployment" | "rollback_deployment"
        )
    {
        return Some("deployments");
    }
    if name.contains("pod") {
        return Some("pods");
    }
    if name.contains("secret") {
        return Some("secrets");
    }
    if name.contains("node") {
        return Some("nodes");
    }
    if name.contains("pvc") || name.contains("storageclass") || name.contains("storage") {
        return Some("storage");
    }
    None
}

/// Append the relevant org hint to a tool's description, if one is configured.
fn inject_mcp_hint(mut tool: serde_json::Value, tuning: &settings::McpTuning) -> serde_json::Value {
    let name = tool
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let hint = match mcp_resource_group(&name) {
        Some("namespaces") => tuning.namespaces.as_deref(),
        Some("deployments") => tuning.deployments.as_deref(),
        Some("applications") => tuning.applications.as_deref(),
        Some("gitops") => tuning.gitops.as_deref(),
        Some("ingresses") => tuning.ingresses.as_deref(),
        Some("pods") => tuning.pods.as_deref(),
        Some("secrets") => tuning.secrets.as_deref(),
        Some("nodes") => tuning.nodes.as_deref(),
        Some("storage") => tuning.storage.as_deref(),
        Some("plugins") => tuning.plugins.as_deref(),
        _ => None,
    };
    if let Some(hint) = hint.filter(|h| !h.is_empty()) {
        if let Some(desc) = tool.get("description").and_then(|d| d.as_str()) {
            let new_desc = format!("{desc}\n\n[Org guidance: {hint}]");
            tool["description"] = serde_json::Value::String(new_desc);
        }
    }
    tool
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
            "name": "set_gitops",
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
            "name": "get_gitops",
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
            "name": "trigger_build",
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
            "name": "watch_build",
            "description": "Poll a GitOps build until it completes (success or failed) and return the final status and build log. Blocks until the build finishes or 600 seconds elapse. Defaults to the most recent build job if job_name is not specified.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "job_name": { "type": "string", "description": "Build job name to watch (e.g. myapp-build-abc1234). Defaults to the most recent build job." }
                },
                "required": ["namespace", "deployment_name"],
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
        serde_json::json!({
            "name": "get_monitoring",
            "description": "Get Prometheus monitoring configuration for a deployment. Returns whether a PodMonitor is enabled, the scrape port, path, interval, and matching pod count.",
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
            "name": "enable_monitoring",
            "description": "Enable Prometheus metrics scraping for a deployment by creating a PodMonitor. Configures the scrape port, path, and interval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "deployment_name": { "type": "string" },
                    "port": { "type": "string", "description": "Port name or number to scrape (default: auto-detected from deployment)" },
                    "path": { "type": "string", "description": "Metrics endpoint path (default: /metrics)" },
                    "interval": { "type": "string", "description": "Scrape interval (default: 30s)" }
                },
                "required": ["namespace", "deployment_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "disable_monitoring",
            "description": "Disable Prometheus metrics scraping for a deployment by removing its PodMonitor.",
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
        // ── Plugin management ──────────────────────────────────────────────
        serde_json::json!({
            "name": "list_plugins",
            "description": "List all configured plugins — name, enabled state, source, and whether each is currently loaded in memory and ready to run.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "enable_plugin",
            "description": "Enable a configured plugin by name. Updates settings immediately and triggers a background refetch of the WASM binary so subsequent deployments use it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Plugin name as configured in settings" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "disable_plugin",
            "description": "Disable a configured plugin by name. The plugin is removed from the in-memory set immediately — subsequent deployments will not run it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Plugin name as configured in settings" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "update_plugin_config",
            "description": "Update configuration fields of a named plugin without replacing the entire plugin definition. All fields are optional — only supplied fields are applied. Triggers a background WASM refetch when config changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Plugin name as configured in settings" },
                    "config": {
                        "type": "object",
                        "description": "Static key-value pairs injected into the plugin's extism config (credentials, endpoints, etc.). Merges with existing config — use null values to remove keys.",
                        "additionalProperties": { "type": "string" }
                    },
                    "inherit_env_keys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Env var names to read from the deckwatch pod environment and inject into plugin config at invocation time. Replaces the existing list."
                    },
                    "inherit_env_file_keys": {
                        "type": "object",
                        "description": "Map of config_key → env_var_holding_file_path. Deckwatch reads each file and injects its content as the config key. Used for workload identity tokens (e.g. IRSA). Replaces the existing map.",
                        "additionalProperties": { "type": "string" }
                    },
                    "allowed_hosts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Hosts the plugin can reach via extism's HTTP host function. Supports globs. Replaces the existing list."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "associate_plugin",
            "description": "Associate a loaded plugin with an application. Once associated, the plugin reconciler will re-run the plugin against every deployment in the application every 30 seconds — regardless of whether the change came from deckwatch, kubectl, ArgoCD, or mcp-k8s. Idempotent: re-associating the same plugin is a no-op.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "application_name": { "type": "string", "description": "Application name (as created via create_application or the deckwatch UI)" },
                    "plugin_name": { "type": "string", "description": "Plugin name as configured in Settings (must be currently loaded)" }
                },
                "required": ["namespace", "application_name", "plugin_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "disassociate_plugin",
            "description": "Remove a plugin association from an application. The plugin will no longer be reconciled against this application's deployments.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "application_name": { "type": "string" },
                    "plugin_name": { "type": "string" }
                },
                "required": ["namespace", "application_name", "plugin_name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "get_plugin_config",
            "description": "Get the current configuration of a loaded plugin, including all config field values (secrets masked). Use this to audit plugin settings or understand what is configured before making changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Plugin name as configured in settings" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }),
        serde_json::json!({
            "name": "validate_plugin",
            "description": "Validate a plugin before adding it to settings. Fetches the WASM binary from the given source, confirms it loads and exports an `apply` function, then dry-runs it with a configurable test context and reports exactly what env vars, sidecars, and Kubernetes resources it would inject. Does not modify any state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {
                        "type": "object",
                        "description": "Plugin source — same schema as settings plugins[].source",
                        "properties": {
                            "type": { "type": "string", "enum": ["github", "url"] },
                            "repo": { "type": "string", "description": "GitHub owner/repo (type=github)" },
                            "ref":  { "type": "string", "description": "Git tag, branch, or SHA (type=github)" },
                            "path": { "type": "string", "description": "Path to the .wasm file within the repo or release" },
                            "use_release": { "type": "boolean", "description": "Fetch from GitHub Releases instead of raw file (type=github)" },
                            "url": { "type": "string", "description": "Full HTTPS URL (type=url)" }
                        },
                        "required": ["type"]
                    },
                    "token_secret": {
                        "type": "string",
                        "description": "Name of a git_token_secrets entry for authenticated fetches (private repos)"
                    },
                    "allowed_hosts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Hosts the plugin can reach via HTTP (e.g. [\"*.amazonaws.com\"]). Mirrors the saved plugin config."
                    },
                    "config": {
                        "type": "object",
                        "description": "Key-value config injected into the plugin (credentials, endpoints, etc.). Plugin reads via extism_pdk::config::get().",
                        "additionalProperties": { "type": "string" }
                    },
                    "test_namespace": {
                        "type": "string",
                        "description": "Namespace passed to the plugin's apply() function (default: default)"
                    },
                    "test_deployment": {
                        "type": "string",
                        "description": "Deployment name passed to apply() (default: test-deployment)"
                    },
                    "test_annotations": {
                        "type": "object",
                        "description": "Annotations passed to apply() — use these to trigger the plugin's opt-in checks",
                        "additionalProperties": { "type": "string" }
                    },
                    "test_labels": {
                        "type": "object",
                        "description": "Pod template labels passed to apply()",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["source"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Tool definition for `submit_agent_feedback`. Only included in `tools/list`
/// when `agent_feedback_enabled == true` in settings.
fn agent_feedback_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "submit_agent_feedback",
        "description": "Record feedback about missing tooling, suboptimal workflows, or situations where better guidance would have led to a better outcome. Use this whenever you encounter a gap in the available tools or documentation.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "enum": ["missing_tool", "mcp_tuning", "workflow", "documentation", "other"],
                    "description": "Category of the feedback"
                },
                "summary": {
                    "type": "string",
                    "description": "Short one-line description of the issue"
                },
                "detail": {
                    "type": "string",
                    "description": "Full description: what you were trying to do, what was missing, and what would have helped"
                },
                "suggested_tool_name": {
                    "type": "string",
                    "description": "If category is missing_tool: what the tool could be named"
                },
                "suggested_prompt": {
                    "type": "string",
                    "description": "If applicable: a suggested system prompt or guidance text that would have improved the outcome"
                }
            },
            "required": ["category", "summary", "detail"],
            "additionalProperties": false
        }
    })
}

// ---------------------------------------------------------------------------
// tools/call dispatch
// ---------------------------------------------------------------------------

async fn handle_tool_call(state: &AppState, request: &JsonRpcRequest) -> JsonRpcResponse {
    let params = &request.params;
    let tool_name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    let start = Instant::now();

    let deckwatch_result = match tool_name {
        "create_application" => Some(tool_create_application(state, args).await),
        "list_addons" => Some(tool_list_addons().await),
        "attach_addon" => Some(tool_attach_addon(state, args).await),
        "detach_addon" => Some(tool_detach_addon(state, args).await),
        "list_templates" => Some(tool_list_templates(state).await),
        "set_gitops" => Some(tool_configure_gitops(state, args).await),
        "get_gitops" => Some(tool_get_gitops_status(state, args).await),
        "trigger_build" => Some(tool_trigger_gitops_build(state, args).await),
        "create_ingress" => Some(tool_create_ingress(state, args).await),
        "update_ingress" => Some(tool_update_ingress(state, args).await),
        "list_ingress_templates" => Some(tool_list_ingress_templates(state).await),
        "create_ingress_template" => Some(tool_create_ingress_template(state, args).await),
        "update_ingress_template" => Some(tool_update_ingress_template(state, args).await),
        "delete_ingress_template" => Some(tool_delete_ingress_template(state, args).await),
        "list_builds" => Some(tool_list_builds(state, args).await),
        "get_build_log" => Some(tool_get_build_log(state, args).await),
        "watch_build" => Some(tool_watch_build(state, args).await),
        "generate_local_build" => Some(tool_generate_local_build(state, args).await),
        "get_monitoring" => Some(tool_get_monitoring(state, args).await),
        "enable_monitoring" => Some(tool_enable_monitoring(state, args).await),
        "disable_monitoring" => Some(tool_disable_monitoring(state, args).await),
        "list_plugins" => Some(tool_list_plugins(state).await),
        "enable_plugin" => Some(tool_enable_plugin(state, args).await),
        "disable_plugin" => Some(tool_disable_plugin(state, args).await),
        "update_plugin_config" => Some(tool_update_plugin_config(state, args).await),
        "get_plugin_config" => Some(tool_get_plugin_config(state, args).await),
        "validate_plugin" => Some(tool_validate_plugin(state, args).await),
        "associate_plugin" => Some(tool_associate_plugin(state, args).await),
        "disassociate_plugin" => Some(tool_disassociate_plugin(state, args).await),
        "submit_agent_feedback" => Some(tool_submit_agent_feedback(state, args).await),
        _ => None,
    };

    let result = if let Some(r) = deckwatch_result {
        let status = if r.is_ok() { "ok" } else { "error" };
        crate::metrics::record_mcp_tool_call(tool_name, status, start.elapsed().as_secs_f64());
        r
    } else {
        // mcp-k8s library instruments its own tools via mcp_k8s_tool_calls_total;
        // no duplicate recording needed here.
        let k8s_client = mcp_k8s::K8sClient::new(
            state.kube_client.clone(),
            state.allowed_namespaces.clone(),
            mcp_k8s::permissions::ActionPermissions::default(),
        );
        mcp_k8s::mcp::handle_tool(&k8s_client, tool_name, args)
            .await
            .unwrap_or_else(|| Err(format!("Unknown tool: {tool_name}")))
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

async fn tool_watch_build(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;

    let _ = state.deployments_api(ns).map_err(|e| e.to_string())?;

    let app_id = format!("{ns}/{name}");

    // Resolve the job_name to watch: use the caller-supplied value, or fall
    // back to the most recent build job recorded on the gitops_configs row.
    let job_name: String = if let Some(j) = args["job_name"].as_str() {
        j.to_string()
    } else {
        let config_row = gitops_configs::Entity::find()
            .filter(gitops_configs::Column::ApplicationId.eq(&app_id))
            .one(&state.db)
            .await
            .map_err(|e| format!("db error: {e}"))?;

        match config_row {
            Some(row) => row
                .last_build_job
                .ok_or_else(|| format!("no build has been triggered for {ns}/{name}"))?,
            None => return Err(format!("GitOps is not configured for {ns}/{name}")),
        }
    };

    const TIMEOUT_SECS: u64 = 600;
    const POLL_INTERVAL_SECS: u64 = 4;

    let start = tokio::time::Instant::now();
    let timeout = tokio::time::Duration::from_secs(TIMEOUT_SECS);
    let poll_interval = tokio::time::Duration::from_secs(POLL_INTERVAL_SECS);

    loop {
        let build_row = builds::Entity::find()
            .filter(builds::Column::ApplicationId.eq(&app_id))
            .filter(builds::Column::JobName.eq(&job_name))
            .one(&state.db)
            .await
            .map_err(|e| format!("db error: {e}"))?;

        if let Some(row) = build_row {
            if row.status == "success" || row.status == "failed" {
                let result = serde_json::json!({
                    "job_name": row.job_name,
                    "commit_sha": row.commit_sha,
                    "status": row.status,
                    "started_at": row.started_at.map(|t| t.to_string()),
                    "completed_at": row.completed_at.map(|t| t.to_string()),
                    "error_message": row.error_message,
                    "build_log": row.build_log,
                });
                return serde_json::to_string_pretty(&result).map_err(|e| e.to_string());
            }
        }

        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out after {TIMEOUT_SECS}s waiting for build '{job_name}' to complete"
            ));
        }

        tokio::time::sleep(poll_interval).await;
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
// Prometheus monitoring tools
// ---------------------------------------------------------------------------

async fn tool_get_monitoring(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;

    let response = monitoring::get(
        axum::extract::State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&response.1 .0).map_err(|e| e.to_string())
}

async fn tool_enable_monitoring(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;

    let req = monitoring::MonitorConfigRequest {
        enabled: true,
        port: args["port"].as_str().map(|s| s.to_string()),
        path: args["path"].as_str().map(|s| s.to_string()),
        interval: args["interval"].as_str().map(|s| s.to_string()),
    };

    let response = monitoring::upsert(
        axum::extract::State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
        axum::Json(req),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_string_pretty(&response.1 .0).map_err(|e| e.to_string())
}

async fn tool_disable_monitoring(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("namespace is required")?;
    let name = args["deployment_name"]
        .as_str()
        .ok_or("deployment_name is required")?;

    monitoring::delete(
        axum::extract::State(state.clone()),
        axum::extract::Path((ns.to_string(), name.to_string())),
    )
    .await
    .map_err(|e| format!("{e}"))?;

    Ok(serde_json::json!({
        "enabled": false,
        "namespace": ns,
        "deployment_name": name,
        "status": "monitoring disabled"
    })
    .to_string())
}

// ---------------------------------------------------------------------------
// Plugin management tools
// ---------------------------------------------------------------------------

async fn tool_list_plugins(state: &AppState) -> Result<String, String> {
    let s = settings::load_settings_from_db(state).await;
    let loaded = state.plugins.read().await;
    let loaded_names: std::collections::HashSet<&str> =
        loaded.iter().map(|p| p.name.as_str()).collect();

    let list: Vec<serde_json::Value> = s
        .plugins
        .iter()
        .map(|cfg| {
            let source_summary = match &cfg.source {
                settings::PluginSource::Github {
                    repo,
                    git_ref,
                    path,
                    use_release,
                } => {
                    if *use_release {
                        format!("github release: {repo}@{git_ref} → {path}")
                    } else {
                        format!("github raw: {repo}@{git_ref} → {path}")
                    }
                }
                settings::PluginSource::Url { url } => format!("url: {url}"),
                settings::PluginSource::Upload { filename } => {
                    format!("upload: {filename}")
                }
            };
            serde_json::json!({
                "name": cfg.name,
                "enabled": cfg.enabled,
                "loaded": loaded_names.contains(cfg.name.as_str()),
                "source": source_summary,
                "token_secret": cfg.token_secret,
            })
        })
        .collect();

    let summary = serde_json::json!({
        "configured": s.plugins.len(),
        "loaded": loaded.len(),
        "plugins": list,
    });
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}

async fn tool_enable_plugin(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let mut s = settings::load_settings_from_db(state).await;
    let cfg = s
        .plugins
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("plugin '{name}' not found in settings"))?;

    if cfg.enabled {
        return Ok(format!("plugin '{name}' is already enabled"));
    }
    cfg.enabled = true;

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("failed to save settings: {e}"))?;

    // Trigger background refetch so the plugin is live immediately.
    let state_clone = state.clone();
    let plugins_cfg = s.plugins.clone();
    tokio::spawn(async move {
        let git_token_secrets = settings::load_settings_from_db(&state_clone)
            .await
            .git_token_secrets;
        let snap = settings::DeckwatchSettings {
            plugins: plugins_cfg,
            git_token_secrets,
            ..Default::default()
        };
        let loaded = crate::plugins::fetch_plugins(&snap, &state_clone).await;
        tracing::info!(count = loaded.len(), "plugins reloaded after enable");
        *state_clone.plugins.write().await = loaded;
    });

    Ok(serde_json::json!({
        "name": name,
        "enabled": true,
        "status": "enabled — WASM binary fetch triggered in background"
    })
    .to_string())
}

async fn tool_disable_plugin(state: &AppState, args: &serde_json::Value) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let mut s = settings::load_settings_from_db(state).await;
    let cfg = s
        .plugins
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("plugin '{name}' not found in settings"))?;

    if !cfg.enabled {
        return Ok(format!("plugin '{name}' is already disabled"));
    }
    cfg.enabled = false;

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("failed to save settings: {e}"))?;

    // Remove from the in-memory set immediately so the next deployment
    // doesn't run this plugin even before the next startup.
    {
        let mut loaded = state.plugins.write().await;
        loaded.retain(|p| p.name != name);
    }

    Ok(serde_json::json!({
        "name": name,
        "enabled": false,
        "status": "disabled and unloaded from memory"
    })
    .to_string())
}

async fn tool_update_plugin_config(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let mut s = settings::load_settings_from_db(state).await;
    let cfg = s
        .plugins
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("plugin '{name}' not found in settings"))?;

    let mut changed = false;

    if let Some(config_val) = args.get("config").filter(|v| !v.is_null()) {
        let updates: std::collections::BTreeMap<String, String> =
            serde_json::from_value(config_val.clone())
                .map_err(|e| format!("invalid config: {e}"))?;
        cfg.config.extend(updates);
        changed = true;
    }

    if let Some(keys_val) = args.get("inherit_env_keys").filter(|v| !v.is_null()) {
        cfg.inherit_env_keys = serde_json::from_value(keys_val.clone())
            .map_err(|e| format!("invalid inherit_env_keys: {e}"))?;
        changed = true;
    }

    if let Some(file_keys_val) = args.get("inherit_env_file_keys").filter(|v| !v.is_null()) {
        cfg.inherit_env_file_keys = serde_json::from_value(file_keys_val.clone())
            .map_err(|e| format!("invalid inherit_env_file_keys: {e}"))?;
        changed = true;
    }

    if let Some(hosts_val) = args.get("allowed_hosts").filter(|v| !v.is_null()) {
        cfg.allowed_hosts = serde_json::from_value(hosts_val.clone())
            .map_err(|e| format!("invalid allowed_hosts: {e}"))?;
        changed = true;
    }

    if !changed {
        return Ok(format!(
            "plugin '{name}' — no fields supplied, nothing changed"
        ));
    }

    settings::upsert_settings_to_db_pub(&state.db, &s)
        .await
        .map_err(|e| format!("failed to save settings: {e}"))?;

    // Reload the plugin with updated config in the background.
    if s.plugins.iter().any(|p| p.name == name && p.enabled) {
        let state_clone = state.clone();
        let plugins_cfg = s.plugins.clone();
        tokio::spawn(async move {
            let git_token_secrets = settings::load_settings_from_db(&state_clone)
                .await
                .git_token_secrets;
            let snap = settings::DeckwatchSettings {
                plugins: plugins_cfg,
                git_token_secrets,
                ..Default::default()
            };
            let loaded = crate::plugins::fetch_plugins(&snap, &state_clone).await;
            tracing::info!(count = loaded.len(), "plugins reloaded after config update");
            *state_clone.plugins.write().await = loaded;
        });
    }

    let updated = s.plugins.iter().find(|p| p.name == name).unwrap();
    serde_json::to_string_pretty(&serde_json::json!({
        "name": updated.name,
        "enabled": updated.enabled,
        "config_keys": updated.config.keys().collect::<Vec<_>>(),
        "inherit_env_keys": updated.inherit_env_keys,
        "inherit_env_file_keys": updated.inherit_env_file_keys,
        "allowed_hosts": updated.allowed_hosts,
        "status": "updated — plugin reloading in background"
    }))
    .map_err(|e| e.to_string())
}

async fn tool_get_plugin_config(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("name is required")?;

    let s = settings::load_settings_from_db(state).await;
    let cfg = s
        .plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("plugin '{name}' not found in settings"))?;

    // Check whether the plugin is currently loaded in memory.
    let loaded_guard = state.plugins.read().await;
    let loaded_plugin = loaded_guard.iter().find(|p| p.name == name);

    let status = if !cfg.enabled {
        "disabled"
    } else if loaded_plugin.is_some() {
        "loaded"
    } else {
        "not loaded"
    };

    // Build the config field list by merging the schema (from the loaded
    // plugin's metadata) with the persisted values in settings.
    let schema_fields: &[crate::plugins::ConfigField] = loaded_plugin
        .map(|lp| lp.metadata.config_schema.as_slice())
        .unwrap_or(&[]);

    let mut config_entries: Vec<serde_json::Value> = Vec::new();
    let mut schema_keys: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for field in schema_fields {
        schema_keys.insert(field.key.as_str());
        let is_secret = field.field_type == crate::plugins::ConfigFieldType::Secret;
        let raw_value = cfg.config.get(&field.key);
        let value: serde_json::Value = match (is_secret, raw_value) {
            // Secret with a value set — mask it.
            (true, Some(_)) => serde_json::Value::String("***".to_string()),
            // Secret not yet configured.
            (true, None) => serde_json::Value::Null,
            // Non-secret with a value.
            (false, Some(v)) => serde_json::Value::String(v.clone()),
            // Non-secret not yet configured.
            (false, None) => serde_json::Value::Null,
        };
        config_entries.push(serde_json::json!({
            "key": field.key,
            "label": field.label,
            "description": field.description,
            "field_type": field.field_type,
            "value": value,
            "is_secret": is_secret,
        }));
    }

    // Append any config keys that exist in settings but were not declared in
    // the schema (e.g. keys added manually before the plugin published a schema,
    // or internal keys like CURRENT_TIMESTAMP that plugins read internally).
    for (key, val) in &cfg.config {
        if !schema_keys.contains(key.as_str()) {
            config_entries.push(serde_json::json!({
                "key": key,
                "label": key,
                "description": "",
                "field_type": "string",
                "value": val,
                "is_secret": false,
            }));
        }
    }

    let result = serde_json::json!({
        "name": cfg.name,
        "enabled": cfg.enabled,
        "status": status,
        "allowed_hosts": cfg.allowed_hosts,
        "inherit_env_keys": cfg.inherit_env_keys,
        "config": config_entries,
        "config_schema_fields": schema_fields.len(),
    });

    serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
}

async fn tool_validate_plugin(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    // Parse the source from args — same JSON shape as DeckwatchSettings.plugins[].source.
    let source: settings::PluginSource = serde_json::from_value(args["source"].clone())
        .map_err(|e| format!("invalid source: {e}"))?;

    let cfg = settings::PluginConfig {
        name: "__validate__".to_string(),
        enabled: true,
        source,
        token_secret: args["token_secret"].as_str().map(|s| s.to_string()),
        allowed_hosts: args
            .get("allowed_hosts")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        config: args
            .get("config")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        inherit_env_keys: args
            .get("inherit_env_keys")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        inherit_env_file_keys: args
            .get("inherit_env_file_keys")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
    };

    let test_ctx = crate::plugins::PluginContext {
        namespace: args["test_namespace"]
            .as_str()
            .unwrap_or("default")
            .to_string(),
        deployment_name: args["test_deployment"]
            .as_str()
            .unwrap_or("test-deployment")
            .to_string(),
        annotations: args
            .get("test_annotations")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        labels: args
            .get("test_labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        plugin_outputs: Default::default(),
    };

    let vr = crate::plugins::fetch_and_validate(&cfg, test_ctx, state).await;

    let result_summary = vr.result.as_ref().map(|r| {
        serde_json::json!({
            "env_vars": r.env_vars.iter().map(|e| {
                if let Some(vf) = &e.value_from {
                    if let Some(skr) = &vf.secret_key_ref {
                        serde_json::json!({ "name": e.name, "from_secret": skr.name, "key": skr.key })
                    } else if let Some(cmkr) = &vf.config_map_key_ref {
                        serde_json::json!({ "name": e.name, "from_configmap": cmkr.name, "key": cmkr.key })
                    } else {
                        serde_json::json!({ "name": e.name, "value": "(unknown source)" })
                    }
                } else {
                    serde_json::json!({ "name": e.name, "value": e.value })
                }
            }).collect::<Vec<_>>(),
            "sidecars": r.sidecars.iter().map(|s| serde_json::json!({
                "name": s.name,
                "image": s.image,
            })).collect::<Vec<_>>(),
            "kubernetes_resources": r.kubernetes_resources.iter().map(|r| serde_json::json!({
                "apiVersion": r["apiVersion"],
                "kind": r["kind"],
                "name": r["metadata"]["name"],
                "namespace": r["metadata"]["namespace"],
            })).collect::<Vec<_>>(),
        })
    });

    let output = serde_json::json!({
        "valid": vr.error.is_none(),
        "wasm_size_bytes": vr.wasm_size_bytes,
        "apply_export_found": vr.apply_export_found,
        "test_context": {
            "namespace": vr.test_context.namespace,
            "deployment_name": vr.test_context.deployment_name,
            "annotations": vr.test_context.annotations,
        },
        "result": result_summary,
        "error": vr.error,
    });

    serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
}

// ── Application plugin association tools ────────────────────────────────────

async fn tool_associate_plugin(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("missing namespace")?;
    let app_name = args["application_name"]
        .as_str()
        .ok_or("missing application_name")?;
    let plugin_name = args["plugin_name"].as_str().ok_or("missing plugin_name")?;

    let app_id = format!("{ns}/{app_name}");

    // Verify application exists.
    use crate::entities::applications;
    use sea_orm::EntityTrait;
    applications::Entity::find_by_id(&app_id)
        .one(&state.db)
        .await
        .map_err(|e| format!("database error: {e}"))?
        .ok_or_else(|| format!("application '{app_name}' not found in namespace '{ns}'"))?;

    // Verify plugin is loaded.
    {
        let plugins = state.plugins.read().await;
        if !plugins.iter().any(|p| p.name == plugin_name) {
            return Err(format!(
                "plugin '{plugin_name}' not loaded — add and enable it in Settings first"
            ));
        }
    }

    use crate::entities::application_plugins;
    use sea_orm::ActiveValue::Set;

    let now_val = {
        use std::time::SystemTime;
        let d = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock before UNIX epoch");
        sea_orm::entity::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
            .expect("timestamp out of range")
    };

    let model = application_plugins::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        application_id: Set(app_id.clone()),
        plugin_name: Set(plugin_name.to_string()),
        created_at: Set(now_val),
    };

    use sea_orm::sea_query::OnConflict;
    let _ = application_plugins::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                application_plugins::Column::ApplicationId,
                application_plugins::Column::PluginName,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec(&state.db)
        .await
        .map_err(|e| format!("database error: {e}"))?;

    crate::audit::log_action(
        &state.db,
        "create",
        "application-plugin",
        plugin_name,
        ns,
        &format!("MCP: associated plugin '{plugin_name}' with application '{app_name}'"),
    )
    .await
    .ok();

    Ok(format!(
        "Plugin '{plugin_name}' associated with application '{app_name}' in namespace '{ns}'. \
         The plugin reconciler will begin running it against all deployments within 30 seconds."
    ))
}

async fn tool_disassociate_plugin(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let ns = args["namespace"].as_str().ok_or("missing namespace")?;
    let app_name = args["application_name"]
        .as_str()
        .ok_or("missing application_name")?;
    let plugin_name = args["plugin_name"].as_str().ok_or("missing plugin_name")?;

    let app_id = format!("{ns}/{app_name}");

    use crate::entities::application_plugins;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let deleted = application_plugins::Entity::delete_many()
        .filter(application_plugins::Column::ApplicationId.eq(&app_id))
        .filter(application_plugins::Column::PluginName.eq(plugin_name))
        .exec(&state.db)
        .await
        .map_err(|e| format!("database error: {e}"))?;

    if deleted.rows_affected == 0 {
        return Err(format!(
            "plugin '{plugin_name}' is not associated with application '{app_name}' in namespace '{ns}'"
        ));
    }

    crate::audit::log_action(
        &state.db,
        "delete",
        "application-plugin",
        plugin_name,
        ns,
        &format!("MCP: removed plugin '{plugin_name}' from application '{app_name}'"),
    )
    .await
    .ok();

    Ok(format!(
        "Plugin '{plugin_name}' disassociated from application '{app_name}' in namespace '{ns}'."
    ))
}

// ---------------------------------------------------------------------------
// Agent Feedback tool implementation
// ---------------------------------------------------------------------------

async fn tool_submit_agent_feedback(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<String, String> {
    let category = args["category"].as_str().ok_or("category is required")?;
    let summary = args["summary"].as_str().ok_or("summary is required")?;
    let detail = args["detail"].as_str().ok_or("detail is required")?;

    let valid_categories = [
        "missing_tool",
        "mcp_tuning",
        "workflow",
        "documentation",
        "other",
    ];
    if !valid_categories.contains(&category) {
        return Err(format!(
            "invalid category '{}'; must be one of: {}",
            category,
            valid_categories.join(", ")
        ));
    }

    let suggested_tool_name = args["suggested_tool_name"].as_str().map(|s| s.to_string());
    let suggested_prompt = args["suggested_prompt"].as_str().map(|s| s.to_string());

    let now = {
        use std::time::SystemTime;
        let d = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock before UNIX epoch");
        sea_orm::entity::prelude::DateTimeUtc::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
            .expect("timestamp out of range")
    };

    use sea_orm::ActiveValue::Set;
    let model = agent_feedback::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        created_at: Set(now),
        category: Set(category.to_string()),
        summary: Set(summary.to_string()),
        detail: Set(detail.to_string()),
        suggested_tool_name: Set(suggested_tool_name),
        suggested_prompt: Set(suggested_prompt),
        status: Set("pending".to_string()),
        reviewed_at: Set(None),
    };

    use sea_orm::EntityTrait as _;
    agent_feedback::Entity::insert(model)
        .exec(&state.db)
        .await
        .map_err(|e| format!("db error: {e}"))?;

    Ok(format!(
        "Feedback recorded (category: {category}): {summary}"
    ))
}

// ---------------------------------------------------------------------------
// prompts/list + prompts/get
// ---------------------------------------------------------------------------

fn handle_prompts_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    success_response(
        request,
        serde_json::json!({
            "prompts": [
                {
                    "name": "pre-deployment-check",
                    "description": "Review the current project directory to check if the application is ready to be onboarded to deckwatch",
                    "arguments": []
                },
                {
                    "name": "deployment-readiness-check",
                    "description": "Check if a deployed application on the cluster is healthy and properly configured in deckwatch",
                    "arguments": [
                        {"name": "namespace", "description": "Kubernetes namespace", "required": true},
                        {"name": "deployment_name", "description": "Deployment name", "required": true}
                    ]
                }
            ]
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
        "pre-deployment-check" => {
            let prompt_text = r#"You are reviewing the current project directory to determine if this application is ready to be onboarded to deckwatch for Kubernetes deployment.

IMPORTANT CONTEXT: Deckwatch includes a built-in GitOps pipeline. When a git push is detected, deckwatch automatically builds a container image using Kaniko and deploys it — no external CI/CD system (GitHub Actions, Jenkins, GitLab CI, etc.) is required or expected. Do NOT recommend setting up a CI pipeline; deckwatch IS the pipeline.

Analyze the codebase and report a checklist with pass/fail for each item:

1. **Dockerfile exists**: Look for a Dockerfile (or Dockerfile.*) in the project root. Check that it builds a runnable container — verify it has a CMD or ENTRYPOINT. Deckwatch uses Kaniko to build this image on every git push.
2. **Git repository with remote**: Check if this is a git repo with a remote origin configured (`git remote -v`). Deckwatch polls this URL to detect new commits and trigger builds — this is the deployment trigger, replacing a traditional CI pipeline.
3. **Container port exposed**: Check the Dockerfile for an EXPOSE directive. Deckwatch needs to know which port the app listens on to configure the Service and readiness probe.
4. **Health endpoint**: Search the codebase for a health check endpoint (e.g. `/health`, `/healthz`, `/readyz`, `/ping`). Deckwatch configures Kubernetes readiness and liveness probes against these automatically.
5. **No secrets in code**: Scan for hardcoded passwords, API keys, tokens, or credentials in source code. These must be in Kubernetes Secrets or managed via deckwatch's plugin system — never baked into the image.
6. **Environment variables documented**: Look for `.env` files, environment variable references in code or config files. List which env vars the app needs — these will be configured on the deckwatch deployment, not in a CI system.
7. **Dependencies pinned**: Check if dependency versions are pinned (requirements.txt with ==, Cargo.lock, package-lock.json, go.sum). Unpinned dependencies cause non-reproducible Kaniko builds.
8. **Database not localhost**: Check for database connection strings. Flag if SQLite, localhost, or 127.0.0.1 is used — production workloads need an external database reachable from the cluster.
9. **Startup time**: Check for long startup sequences (migrations, cache warming). If present, a Kubernetes startup probe should be configured in addition to the readiness probe.
10. **.dockerignore exists**: Check for a .dockerignore file. Missing it causes Kaniko to send the full source tree as build context, significantly slowing builds. Flag any large directories (node_modules, .git, test fixtures) that should be excluded.

Format the output as a checklist with a checkmark or X for each item, with specific details on what needs to be fixed for any failing checks. End with a summary of whether the app is ready for deckwatch onboarding, and remind the user that no external CI pipeline is needed — deckwatch handles image builds automatically."#;

            success_response(
                request,
                serde_json::json!({
                    "description": "Review the current project directory to check if the application is ready to be onboarded to deckwatch",
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
        "deployment-readiness-check" => {
            let args = &params["arguments"];
            let ns = args["namespace"].as_str().unwrap_or("default");
            let dep = args["deployment_name"].as_str().unwrap_or("my-app");

            let prompt_text = format!(
                r#"You are evaluating whether the application "{dep}" in namespace "{ns}" is ready for production on deckwatch. Deckwatch builds and deploys images automatically via GitOps — no external CI/CD pipeline is required or expected. Run the following checks using the available MCP tools and report a checklist with pass/fail for each item:

1. **Deployment exists**: Call `get_deployment` for namespace "{ns}", name "{dep}"
2. **GitOps configured**: Call `get_gitops` — check repo_url, branch, and dockerfile_path are set. If not configured, this deployment is managed manually rather than via GitOps and won't receive automatic updates on git push.
3. **Last build status**: If GitOps is configured, check last_build_status — flag if "failed" or "building" is stuck
4. **Database**: Check DATABASE_URL env var — flag if pointing to localhost, sqlite, or 127.0.0.1
5. **Resource limits**: Check resource_limits and resource_requests are both set
6. **Readiness probe**: Check readiness_probe is configured — required for zero-downtime rollouts
7. **Ingress**: Call `list_ingresses` in namespace "{ns}" — check an ingress exists for this service
8. **Pod health**: Call `list_pods` in namespace "{ns}" — check all pods are Running/Ready with zero restarts
9. **Image pull errors**: Verify no ImagePullBackOff or ErrImagePull on any pod
10. **OOMKilled**: Check if any pod has been OOMKilled — indicates memory limits are too low

Format the output as a checklist with a checkmark or X for each item, with details on what needs to be fixed for failing checks."#,
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
#[path = "mcp_tests.rs"]
mod tests;
