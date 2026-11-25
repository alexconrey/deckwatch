use super::*;
use crate::plugin_fixture::{enable_notifications, serve_webhook, test_state};

fn failing(namespace: &str, message: &str) -> NotificationEvent {
    NotificationEvent::PluginFailing {
        namespace: namespace.into(),
        application: if namespace.is_empty() { "" } else { "web" }.into(),
        plugin: "aws".into(),
        resource_id: "rds".into(),
        export: "provision".into(),
        code: "plugin.trap".into(),
        message: message.into(),
    }
}

#[test]
fn plugin_notifications_have_their_own_kinds_and_colors() {
    let f = failing("shop", "boom");
    assert_eq!(f.kind(), "plugin_failing");
    assert_eq!(color_for(&f), "danger");
    let r = NotificationEvent::PluginRecovered {
        namespace: "shop".into(),
        application: "web".into(),
        plugin: "aws".into(),
        resource_id: "".into(),
        export: "apply".into(),
    };
    assert_eq!(r.kind(), "plugin_recovered");
    assert_eq!(color_for(&r), "good");
    assert_eq!(
        r.text(),
        ":white_check_mark: Plugin recovered — `aws` on `shop/web` apply() works again"
    );
}

#[test]
fn a_plugin_that_failed_to_load_concerns_every_namespace() {
    assert_eq!(failing("shop", "x").namespace(), Some("shop"));
    assert_eq!(failing("", "x").namespace(), None);
    assert_eq!(
        failing("", "fetch failed").text(),
        ":rotating_light: Plugin failing — `aws` provision() — plugin.trap: fetch failed"
    );
}

/// Plugin text can't become a Slack mention or link.
#[test]
fn plugin_text_is_escaped_for_slack() {
    let text = failing("shop", "<!channel> see <https://evil.example|here> & more").text();
    assert_eq!(
        text,
        ":rotating_light: Plugin failing — `aws` on `shop/web` (rds) provision() — plugin.trap: \
         &lt;!channel&gt; see &lt;https://evil.example|here&gt; &amp; more"
    );
    let payload = build_payload(&failing("shop", "<!here>"));
    assert_eq!(payload["event_type"], "plugin_failing");
    assert_eq!(payload["event"]["event"], "plugin_failing");
    assert_eq!(payload["event"]["code"], "plugin.trap");
    assert!(!payload["text"].as_str().unwrap().contains("<!here>"));
}

/// Settings live in the database. The client used to read the old
/// ConfigMap, so nothing was ever sent.
#[tokio::test]
async fn notifications_use_the_settings_saved_in_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;
    let (url, received) = serve_webhook().await;
    let client = NotificationClient::new(state.clone());

    // Not configured: nothing is sent, and that's not an error.
    client.send_now(failing("shop", "boom")).await.unwrap();
    assert!(received.lock().unwrap().is_empty());

    enable_notifications(&state, &url).await;
    client
        .send_now(NotificationEvent::Test {
            source: "test".into(),
        })
        .await
        .unwrap();
    client.send_now(failing("shop", "boom")).await.unwrap();
    let kinds: Vec<String> = received
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["event_type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["test", "plugin_failing"]);
}

#[tokio::test]
async fn plugin_notifications_respect_the_event_type_and_namespace_filters() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), vec![], None).await;
    let (url, received) = serve_webhook().await;
    let mut settings = crate::handlers::settings::load_settings_from_db(&state).await;
    settings.notifications = Some(NotificationSettings {
        enabled: true,
        webhook_url: url,
        event_types: vec!["plugin_failing".into()],
        namespaces: vec!["prod".into()],
    });
    crate::handlers::settings::upsert_settings_to_db_pub(&state.db, &settings)
        .await
        .unwrap();
    let client = NotificationClient::new(state);

    client
        .send_now(failing("shop", "other namespace"))
        .await
        .unwrap();
    client.send_now(failing("prod", "sent")).await.unwrap();
    client
        .send_now(failing("", "plugin-wide: sent"))
        .await
        .unwrap();
    client
        .send_now(NotificationEvent::PluginRecovered {
            namespace: "prod".into(),
            application: "web".into(),
            plugin: "aws".into(),
            resource_id: "".into(),
            export: "apply".into(),
        })
        .await
        .unwrap();
    let messages: Vec<String> = received
        .lock()
        .unwrap()
        .iter()
        .map(|b| {
            b["event"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(messages, vec!["sent", "plugin-wide: sent"]);
}

#[tokio::test]
async fn webhook_errors_leave_out_the_url() {
    // A port nothing listens on: the request fails before any response.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/services/T000/B000/hook-path-token?k=query-token");
    let err = post_webhook(&reqwest::Client::new(), &url, &serde_json::json!({}))
        .await
        .expect_err("nothing listens on the port");
    let msg = format!("{err:#}");
    assert!(msg.starts_with("webhook request failed"), "{msg}");
    assert!(!msg.contains("hook-path-token"), "{msg}");
    assert!(!msg.contains("query-token"), "{msg}");
}
