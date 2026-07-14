use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use kube::api::{AttachParams, TerminalSize};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ExecQuery {
    pub container: Option<String>,
    pub command: Option<String>,
}

/// WebSocket upgrade for `kubectl exec`-style container access.
///
/// Query params:
/// - `container` — target container name (required when the pod has more than one)
/// - `command` — shell to invoke (defaults to `/bin/sh`, falls back to `/bin/bash` client-side if needed)
///
/// Wire protocol (WebSocket messages):
/// - Client → server: text frames are treated as stdin bytes. Binary frames whose first byte is
///   `0x04` (resize) followed by JSON `{"cols":N,"rows":N}` resize the TTY.
/// - Server → client: text frames carry stdout+stderr bytes as UTF-8 (invalid UTF-8 is lossy-decoded).
pub async fn exec_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((ns, pod_name)): Path<(String, String)>,
    Query(query): Query<ExecQuery>,
) -> Result<Response, AppError> {
    // Validate namespace access + pod existence up front so we can return a real HTTP error
    // (WebSocket upgrade errors are opaque to browsers).
    let pods_api = state.pods_api(&ns)?;
    let _ = pods_api.get(&pod_name).await?;

    let command = query.command.unwrap_or_else(|| "/bin/sh".to_string());
    let container = query.container;

    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(e) = run_exec(socket, pods_api, pod_name.clone(), container, command).await {
            warn!(pod = %pod_name, error = %e, "exec session ended with error");
        }
    }))
}

async fn run_exec(
    socket: WebSocket,
    pods_api: kube::Api<k8s_openapi::api::core::v1::Pod>,
    pod_name: String,
    container: Option<String>,
    command: String,
) -> anyhow::Result<()> {
    let mut params = AttachParams::interactive_tty().stdin(true).stdout(true);
    if let Some(c) = container.as_deref() {
        params = params.container(c);
    }

    info!(pod = %pod_name, container = ?container, command = %command, "starting exec session");

    // Kubernetes' exec API takes an argv slice; running `/bin/sh -c /bin/sh` would be silly, so
    // pass the command as a single argv element and let PID 1 in the container handle it.
    let mut attached = pods_api.exec(&pod_name, [command.as_str()], &params).await?;

    let mut stdout = attached
        .stdout()
        .ok_or_else(|| anyhow::anyhow!("exec stdout stream unavailable"))?;
    let mut stdin = attached
        .stdin()
        .ok_or_else(|| anyhow::anyhow!("exec stdin stream unavailable"))?;
    let mut resize_tx = attached.terminal_size();

    let (mut ws_tx, mut ws_rx) = socket.split();

    // stdout -> WebSocket
    let stdout_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if ws_tx.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "exec stdout read error");
                    break;
                }
            }
        }
        // Best-effort close notification to the browser.
        let _ = ws_tx.send(Message::Close(None)).await;
    });

    // WebSocket -> stdin (+ resize control frames)
    let stdin_task = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    debug!(error = %e, "ws recv error");
                    break;
                }
            };
            match msg {
                Message::Text(t) => {
                    if stdin.write_all(t.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Message::Binary(b) if b.first() == Some(&0x04) => {
                    if let Some(ref mut tx) = resize_tx {
                        if let Ok(size) = serde_json::from_slice::<ClientTerminalSize>(&b[1..]) {
                            let _ = tx.send(TerminalSize {
                                width: size.cols,
                                height: size.rows,
                            }).await;
                        }
                    }
                }
                Message::Binary(b) => {
                    if stdin.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
        let _ = stdin.shutdown().await;
    });

    // If either side hangs up, tear down both halves so the AttachedProcess can be joined.
    tokio::select! {
        _ = stdout_task => {}
        _ = stdin_task => {}
    }

    attached.abort();
    let _ = attached.join().await;
    info!(pod = %pod_name, "exec session closed");
    Ok(())
}

#[derive(Deserialize)]
struct ClientTerminalSize {
    cols: u16,
    rows: u16,
}
