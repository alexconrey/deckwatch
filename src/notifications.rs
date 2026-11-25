#![allow(dead_code, unused_imports)]
use serde::Serialize;

use crate::handlers::settings::{DeckwatchSettings, NotificationSettings};
use crate::state::AppState;

const SETTINGS_KEY: &str = "settings";

/// One kind of notifiable event. Frontend maps each variant to a checkbox in
/// the Notifications tab; the client filters on the wire before firing so we
/// don't spam the webhook when a namespace only cares about, say, build
/// failures.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum NotificationEvent {
    BuildCompleted {
        namespace: String,
        deployment: String,
        image: String,
        commit_sha: String,
    },
    BuildFailed {
        namespace: String,
        deployment: String,
        commit_sha: String,
        error: String,
    },
    DeploymentCreated {
        namespace: String,
        deployment: String,
        image: String,
    },
    DeploymentDeleted {
        namespace: String,
        deployment: String,
    },
    DeploymentScaled {
        namespace: String,
        deployment: String,
        replicas: i32,
    },
    PodCrashLoop {
        namespace: String,
        pod: String,
        container: String,
        restart_count: i32,
    },
    ApplicationCreated {
        namespace: String,
        application: String,
    },
    ApplicationDeleted {
        namespace: String,
        application: String,
    },
    /// A plugin target started failing (see `plugin_status`). Sent once per
    /// run of failures, never on every reconcile. `namespace` and
    /// `application` are empty for a plugin that failed to load.
    PluginFailing {
        namespace: String,
        application: String,
        plugin: String,
        /// The plugin resource id, or empty for `apply()` and load.
        resource_id: String,
        export: String,
        code: String,
        /// Sanitized plugin text.
        message: String,
    },
    /// A failing plugin target works again.
    PluginRecovered {
        namespace: String,
        application: String,
        plugin: String,
        resource_id: String,
        export: String,
    },
    /// Fired from the "Test" button on the settings page.
    Test { source: String },
}

impl NotificationEvent {
    fn namespace(&self) -> Option<&str> {
        match self {
            Self::BuildCompleted { namespace, .. }
            | Self::BuildFailed { namespace, .. }
            | Self::DeploymentCreated { namespace, .. }
            | Self::DeploymentDeleted { namespace, .. }
            | Self::DeploymentScaled { namespace, .. }
            | Self::PodCrashLoop { namespace, .. }
            | Self::ApplicationCreated { namespace, .. }
            | Self::ApplicationDeleted { namespace, .. } => Some(namespace.as_str()),
            // A plugin that failed to load concerns every namespace.
            Self::PluginFailing { namespace, .. } | Self::PluginRecovered { namespace, .. } => {
                Some(namespace.as_str()).filter(|ns| !ns.is_empty())
            }
            Self::Test { .. } => None,
        }
    }

    /// Stable identifier matched against `NotificationSettings.event_types`.
    /// Kept as its own function (not the serde tag) so we can rename the
    /// wire schema without breaking every operator's saved settings.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BuildCompleted { .. } => "build_completed",
            Self::BuildFailed { .. } => "build_failed",
            Self::DeploymentCreated { .. } => "deployment_created",
            Self::DeploymentDeleted { .. } => "deployment_deleted",
            Self::DeploymentScaled { .. } => "deployment_scaled",
            Self::PodCrashLoop { .. } => "pod_crash_loop",
            Self::ApplicationCreated { .. } => "application_created",
            Self::ApplicationDeleted { .. } => "application_deleted",
            Self::PluginFailing { .. } => "plugin_failing",
            Self::PluginRecovered { .. } => "plugin_recovered",
            Self::Test { .. } => "test",
        }
    }

    fn short_sha(sha: &str) -> &str {
        &sha[..7.min(sha.len())]
    }

    /// Human-readable line rendered into `text` for Slack/Teams and into the
    /// `text` field for generic webhooks. Also used as the Slack fallback for
    /// clients that can't render blocks.
    pub fn text(&self) -> String {
        match self {
            Self::BuildCompleted {
                namespace,
                deployment,
                image,
                commit_sha,
            } => format!(
                ":white_check_mark: Build succeeded — `{}/{}` -> `{}` ({})",
                namespace,
                deployment,
                image,
                Self::short_sha(commit_sha),
            ),
            Self::BuildFailed {
                namespace,
                deployment,
                commit_sha,
                error,
            } => format!(
                ":x: Build failed — `{}/{}` at {}: {}",
                namespace,
                deployment,
                Self::short_sha(commit_sha),
                error,
            ),
            Self::DeploymentCreated {
                namespace,
                deployment,
                image,
            } => format!(
                ":package: Deployment created — `{}/{}` image `{}`",
                namespace, deployment, image,
            ),
            Self::DeploymentDeleted {
                namespace,
                deployment,
            } => format!(
                ":wastebasket: Deployment deleted — `{}/{}`",
                namespace, deployment,
            ),
            Self::DeploymentScaled {
                namespace,
                deployment,
                replicas,
            } => format!(
                ":arrows_counterclockwise: Deployment scaled — `{}/{}` -> {} replicas",
                namespace, deployment, replicas,
            ),
            Self::PodCrashLoop {
                namespace,
                pod,
                container,
                restart_count,
            } => format!(
                ":rotating_light: CrashLoopBackOff — `{}/{}` container `{}` ({} restarts)",
                namespace, pod, container, restart_count,
            ),
            Self::ApplicationCreated {
                namespace,
                application,
            } => format!(
                ":sparkles: Application created — `{}/{}`",
                namespace, application,
            ),
            Self::ApplicationDeleted {
                namespace,
                application,
            } => format!(
                ":wastebasket: Application deleted — `{}/{}`",
                namespace, application,
            ),
            Self::PluginFailing {
                namespace,
                application,
                plugin,
                resource_id,
                export,
                code,
                message,
            } => format!(
                ":rotating_light: Plugin failing — `{}` {}{}() — {}: {}",
                slack_escape(plugin),
                plugin_target(namespace, application, resource_id),
                slack_escape(export),
                slack_escape(code),
                slack_escape(message),
            ),
            Self::PluginRecovered {
                namespace,
                application,
                plugin,
                resource_id,
                export,
            } => format!(
                ":white_check_mark: Plugin recovered — `{}` {}{}() works again",
                slack_escape(plugin),
                plugin_target(namespace, application, resource_id),
                slack_escape(export),
            ),
            Self::Test { source } => format!(":wave: Deckwatch notification test from {}", source,),
        }
    }
}

/// Escape text for Slack's mrkdwn, so plugin text can't turn into a
/// mention (`<!channel>`) or a link.
fn slack_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// "on `shop/web` (rds) " — empty for plugin-wide state.
fn plugin_target(namespace: &str, application: &str, resource_id: &str) -> String {
    if application.is_empty() {
        return String::new();
    }
    let resource = if resource_id.is_empty() {
        String::new()
    } else {
        format!(" ({})", slack_escape(resource_id))
    };
    format!(
        "on `{}/{}`{resource} ",
        slack_escape(namespace),
        slack_escape(application)
    )
}

/// Fire-and-forget notification dispatcher.
///
/// Cheap to clone: it holds only a shared `AppState` (already Arc-backed via
/// `kube::Client`) and a `reqwest::Client` (also internally reference-counted).
/// Spawn a task per event so handlers never block on webhook latency — a
/// stalled Slack API must not stall the CRUD path.
#[derive(Clone)]
pub struct NotificationClient {
    state: AppState,
    http: reqwest::Client,
}

impl NotificationClient {
    pub fn new(state: AppState) -> Self {
        let http = crate::outbound::global().client(crate::outbound::Purpose::Webhook);
        Self { state, http }
    }

    /// Fire-and-forget. Spawns a task so the caller never awaits webhook I/O.
    /// Errors are logged; there is no retry — dropped notifications are
    /// preferable to blocked control-plane operations.
    pub fn send(&self, event: NotificationEvent) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.send_inner(event).await {
                tracing::warn!(error = %e, "notification dispatch failed");
            }
        });
    }

    /// Awaited variant used by the /api/notifications/test endpoint so the
    /// UI can surface a real success/failure to the operator.
    pub async fn send_now(&self, event: NotificationEvent) -> anyhow::Result<()> {
        self.send_inner(event).await
    }

    async fn send_inner(&self, event: NotificationEvent) -> anyhow::Result<()> {
        // Re-read settings every send so operators can change the URL
        // without a restart. The read is a single database row — cheap
        // compared to the HTTP POST it enables.
        let Some(settings) = self.load_notifications().await? else {
            return Ok(());
        };
        if !settings.enabled {
            return Ok(());
        }
        if settings.webhook_url.trim().is_empty() {
            anyhow::bail!("notifications enabled but webhook_url is empty");
        }
        if !event_type_enabled(&settings, event.kind()) {
            return Ok(());
        }
        if let Some(ns) = event.namespace() {
            if !namespace_enabled(&settings, ns) {
                return Ok(());
            }
        }

        post_webhook(&self.http, &settings.webhook_url, &build_payload(&event)).await
    }

    /// The saved notification settings. Settings live in the database (the
    /// ConfigMap is only read once, to seed it); reading the ConfigMap here
    /// meant notifications, the Test button included, silently did nothing.
    async fn load_notifications(&self) -> anyhow::Result<Option<NotificationSettings>> {
        Ok(
            crate::handlers::settings::load_settings_from_db(&self.state)
                .await
                .notifications,
        )
    }
}

fn event_type_enabled(settings: &NotificationSettings, kind: &str) -> bool {
    // Test events always fire when notifications are enabled — the "Test"
    // button in the UI must work before the operator has picked any event
    // types.
    if kind == "test" {
        return true;
    }
    // Absent/empty list = send everything (backwards compatible with the
    // old two-field NotificationSettings). A non-empty list is an allow-list.
    if settings.event_types.is_empty() {
        return true;
    }
    settings.event_types.iter().any(|k| k == kind)
}

fn namespace_enabled(settings: &NotificationSettings, ns: &str) -> bool {
    if settings.namespaces.is_empty() {
        return true;
    }
    settings.namespaces.iter().any(|n| n == ns)
}

/// POST `payload` as JSON to the webhook `url`; non-2xx is an error carrying
/// the status and response body.
pub async fn post_webhook(
    http: &reqwest::Client,
    url: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let resp = http.post(url).json(payload).send().await.map_err(|e| {
        anyhow::anyhow!(
            "webhook request failed: {}",
            crate::outbound::error_chain(&e)
        )
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("webhook returned {status}: {body}");
    }
    Ok(())
}

/// Slack-compatible payload. Slack ignores unknown top-level fields, so the
/// same JSON works for generic webhooks (which get `text`, `event_type`, and
/// the structured `event` payload) and for Microsoft Teams' incoming webhook
/// (which renders `text` as a MessageCard body).
pub fn build_payload(event: &NotificationEvent) -> serde_json::Value {
    let text = event.text();
    let structured = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "text": text,
        "event_type": event.kind(),
        "event": structured,
        "attachments": [{
            "color": color_for(event),
            "text": text,
        }],
    })
}

fn color_for(event: &NotificationEvent) -> &'static str {
    match event {
        NotificationEvent::BuildFailed { .. }
        | NotificationEvent::PodCrashLoop { .. }
        | NotificationEvent::PluginFailing { .. } => "danger",
        NotificationEvent::BuildCompleted { .. }
        | NotificationEvent::DeploymentCreated { .. }
        | NotificationEvent::ApplicationCreated { .. }
        | NotificationEvent::PluginRecovered { .. } => "good",
        NotificationEvent::DeploymentDeleted { .. }
        | NotificationEvent::ApplicationDeleted { .. } => "warning",
        _ => "#439FE0",
    }
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
