use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, warn};

use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct PortQuery {
    pub port: u16,
}

/// WebSocket bridge to a pod's container port.
///
/// Route: `GET /api/namespaces/{ns}/pods/{pod_name}/portforward?port=<port>`
///
/// Wire protocol (WebSocket messages):
/// - Client -> server: `Binary` and `Text` frames are forwarded to the pod
///   as raw bytes. `Close` tears the tunnel down.
/// - Server -> client: pod bytes arrive as `Binary` frames. When the pod
///   closes its end of the stream, the server sends `Close` and exits.
pub async fn portforward_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((ns, pod_name)): Path<(String, String)>,
    Query(query): Query<PortQuery>,
) -> Result<Response, AppError> {
    // Validate namespace + pod up front so we can return a real HTTP error
    // (WS upgrade errors are opaque to the browser).
    let pods_api = state.pods_api(&ns)?;
    let _ = pods_api.get(&pod_name).await?;

    let port = query.port;

    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(e) = run_portforward(socket, pods_api, pod_name.clone(), port).await {
            warn!(pod = %pod_name, port, error = %e, "port-forward session ended with error");
        }
    }))
}

async fn run_portforward(
    socket: WebSocket,
    pods_api: kube::Api<k8s_openapi::api::core::v1::Pod>,
    pod_name: String,
    port: u16,
) -> anyhow::Result<()> {
    info!(pod = %pod_name, port, "starting port-forward session");

    let mut pf = pods_api.portforward(&pod_name, &[port]).await?;
    let stream = pf
        .take_stream(port)
        .ok_or_else(|| anyhow::anyhow!("port {port} not available on portforwarder"))?;
    let (mut pod_rx, mut pod_tx) = tokio::io::split(stream);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // pod -> WebSocket
    let pod_to_ws = tokio::spawn(async move {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match pod_rx.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "portforward pod read error");
                    break;
                }
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await;
    });

    // WebSocket -> pod
    let ws_to_pod = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    debug!(error = %e, "portforward ws recv error");
                    break;
                }
            };
            match msg {
                Message::Binary(b) => {
                    if pod_tx.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Message::Text(t) => {
                    if pod_tx.write_all(t.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
        let _ = pod_tx.shutdown().await;
    });

    // Tear both halves down when either side hangs up so the Portforwarder
    // can be joined cleanly.
    tokio::select! {
        _ = pod_to_ws => {}
        _ = ws_to_pod => {}
    }

    pf.abort();
    let _ = pf.join().await;
    info!(pod = %pod_name, port, "port-forward session closed");
    Ok(())
}

// ------------------------------------------------------------------ HTTP proxy

/// HTTP proxy: forwards a browser request to a pod's container port and
/// streams the response back. Route:
///   `/api/namespaces/{ns}/pods/{pod_name}/proxy/{port}/{*rest}`
///
/// Each request opens a fresh port-forward, sends the request with
/// `Connection: close`, and streams bytes back until the pod closes the
/// socket. That is enough to browse admin UIs, hit REST endpoints, etc.,
/// without dragging in a full HTTP client stack on top of the tunnel.
pub async fn portforward_http(
    State(state): State<AppState>,
    Path((ns, pod_name, port, rest)): Path<(String, String, u16, String)>,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    proxy_http(state, ns, pod_name, port, rest, req).await
}

/// Variant for the root path (`/proxy/{port}` and `/proxy/{port}/`) where
/// axum's `{*rest}` wildcard would not match an empty tail.
pub async fn portforward_http_root(
    State(state): State<AppState>,
    Path((ns, pod_name, port)): Path<(String, String, u16)>,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    proxy_http(state, ns, pod_name, port, String::new(), req).await
}

async fn proxy_http(
    state: AppState,
    ns: String,
    pod_name: String,
    port: u16,
    rest: String,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    let pods_api = state.pods_api(&ns)?;
    // Validate pod exists so we can return a real 404 rather than a stream
    // error partway through.
    let _ = pods_api.get(&pod_name).await?;

    // Reconstruct the target path + query on the pod side. axum strips the
    // matched prefix from `{*rest}`, so we prepend "/" ourselves.
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let target_path = if rest.is_empty() {
        format!("/{query}")
    } else {
        format!("/{rest}{query}")
    };

    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to read request body: {e}")))?;

    let mut pf = pods_api.portforward(&pod_name, &[port]).await?;
    let stream = pf.take_stream(port).ok_or_else(|| {
        AppError::BadRequest(format!("port {port} not available on portforwarder"))
    })?;
    let (pod_rx, mut pod_tx) = tokio::io::split(stream);

    // Build a raw HTTP/1.1 request. We rewrite Host to localhost:<port> so
    // servers that switch on the Host header see something sensible, and set
    // Connection: close so the pod closes its end at end-of-body -- giving
    // us an unambiguous EOF signal without having to parse chunked framing.
    let mut request = Vec::with_capacity(512 + body_bytes.len());
    request.extend_from_slice(method.as_str().as_bytes());
    request.push(b' ');
    request.extend_from_slice(target_path.as_bytes());
    request.extend_from_slice(b" HTTP/1.1\r\n");
    request.extend_from_slice(format!("Host: localhost:{port}\r\n").as_bytes());
    request.extend_from_slice(b"Connection: close\r\n");

    for (name, value) in headers.iter() {
        // Skip hop-by-hop / transport-managed headers we're regenerating or
        // that would confuse the origin server about framing.
        let n = name.as_str().to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "upgrade"
                | "proxy-connection"
                | "keep-alive"
                | "te"
                | "trailer"
        ) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            request.extend_from_slice(name.as_str().as_bytes());
            request.extend_from_slice(b": ");
            request.extend_from_slice(v.as_bytes());
            request.extend_from_slice(b"\r\n");
        }
    }
    request.extend_from_slice(format!("Content-Length: {}\r\n", body_bytes.len()).as_bytes());
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(&body_bytes);

    pod_tx
        .write_all(&request)
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to send request to pod: {e}")))?;

    // Parse status line + headers off the response, then stream the body.
    let (status, response_headers, initial_body, mut reader) = read_http_head(pod_rx)
        .await
        .map_err(|e| AppError::BadRequest(format!("bad response from pod: {e}")))?;

    // Body stream: emit whatever came in the initial header buffer, then
    // continue draining the socket until EOF. `Connection: close` on the
    // request means the pod's EOF is the body terminator, so we can ignore
    // chunked framing entirely and forward bytes verbatim.
    let (tx, rx) = mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        if !initial_body.is_empty() {
            let _ = tx.send(Ok(initial_body.into())).await;
        }
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx
                        .send(Ok(axum::body::Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
        pf.abort();
        let _ = pf.join().await;
    });

    let body = Body::from_stream(ReceiverStream::new(rx));
    let mut response = Response::builder().status(status);
    let response_headers_ref = response
        .headers_mut()
        .expect("response builder has no error yet");
    for (name, value) in response_headers {
        // Drop framing headers -- axum sets Content-Length / Transfer-Encoding
        // for us based on the streaming body.
        let ln = name.to_ascii_lowercase();
        if matches!(
            ln.as_str(),
            "content-length" | "transfer-encoding" | "connection"
        ) {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::try_from(&value),
        ) {
            response_headers_ref.append(hn, hv);
        }
    }
    Ok(response
        .body(body)
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()))
}

/// Read an HTTP/1.1 status line + headers off `reader`. Returns the parsed
/// status, header pairs, any bytes that came after the header terminator
/// (the initial slice of the body), and the reader positioned to continue
/// draining the body.
async fn read_http_head<R>(
    mut reader: R,
) -> anyhow::Result<(StatusCode, Vec<(String, String)>, Vec<u8>, R)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    let sep_at = loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            anyhow::bail!("pod closed connection before sending response headers");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_terminator(&buf) {
            break pos;
        }
        // Guard against pathologically large header blocks; 64 KiB is plenty.
        if buf.len() > 64 * 1024 {
            anyhow::bail!("response headers exceed 64 KiB");
        }
    };

    let head = &buf[..sep_at];
    let body_start = sep_at + 4; // skip past "\r\n\r\n"
    let head_str = std::str::from_utf8(head)
        .map_err(|_| anyhow::anyhow!("response headers are not valid UTF-8"))?;
    let mut lines = head_str.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty response head"))?;
    let mut parts = status_line.splitn(3, ' ');
    let _http_version = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("malformed status line"))?;
    let code_str = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing status code"))?;
    let code: u16 = code_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid status code"))?;
    let status = StatusCode::from_u16(code)
        .map_err(|_| anyhow::anyhow!("status code out of range"))?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let initial_body = buf[body_start..].to_vec();
    Ok((status, headers, initial_body, reader))
}

fn find_header_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
