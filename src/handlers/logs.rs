use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::AsyncBufReadExt;
use futures::Stream;
use kube::api::LogParams;
use serde::Deserialize;

use axum::Json;

use crate::error::AppError;
use crate::metrics::{self, K8sTimer};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LogQuery {
    pub container: Option<String>,
    pub tail_lines: Option<i64>,
    pub follow: Option<bool>,
}

#[derive(serde::Serialize)]
pub struct LogHistoryResponse {
    pub lines: Vec<String>,
}

/// Stream wrapper that tracks active SSE connections. `sse_opened()` fires on
/// construction; `sse_closed()` fires when the wrapper is dropped, whether
/// the client disconnected cleanly or the underlying stream ended. Using a
/// `Drop` guard means we don't need a matching finalization branch — any
/// cancellation of the response future frees the gauge.
struct TrackedSseStream<S> {
    inner: S,
}

impl<S> TrackedSseStream<S> {
    fn new(inner: S) -> Self {
        metrics::sse_opened();
        Self { inner }
    }
}

impl<S> Drop for TrackedSseStream<S> {
    fn drop(&mut self) {
        metrics::sse_closed();
    }
}

impl<S, T> Stream for TrackedSseStream<S>
where
    S: Stream<Item = T> + Unpin,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

pub async fn get_logs(
    State(state): State<AppState>,
    Path((ns, pod_name)): Path<(String, String)>,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogHistoryResponse>, AppError> {
    let pods_api = state.pods_api(&ns)?;

    let log_params = LogParams {
        follow: false,
        tail_lines: query.tail_lines,
        container: query.container,
        timestamps: true,
        ..Default::default()
    };

    let t = K8sTimer::new("pods", "logs");
    let log_text = pods_api.logs(&pod_name, &log_params).await;
    t.finish(log_text.is_ok());
    let log_text = log_text?;
    let lines: Vec<String> = log_text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(Json(LogHistoryResponse { lines }))
}

pub async fn stream_logs(
    State(state): State<AppState>,
    Path((ns, pod_name)): Path<(String, String)>,
    Query(query): Query<LogQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let pods_api = state.pods_api(&ns)?;

    let t = K8sTimer::new("pods", "get");
    let pod = pods_api.get(&pod_name).await;
    t.finish(pod.is_ok());
    let pod = pod?;
    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown");

    if phase == "Pending" {
        return Err(AppError::BadRequest(format!(
            "Pod '{pod_name}' is in Pending phase, logs not yet available"
        )));
    }

    let log_params = LogParams {
        follow: query.follow.unwrap_or(true),
        tail_lines: query.tail_lines,
        container: query.container,
        timestamps: true,
        ..Default::default()
    };

    let t = K8sTimer::new("pods", "log_stream");
    let log_reader = pods_api.log_stream(&pod_name, &log_params).await;
    t.finish(log_reader.is_ok());
    let log_reader = log_reader?;
    let lines = log_reader.lines();

    let event_stream = futures::StreamExt::map(lines, |result| {
        let event = match result {
            Ok(line) => {
                let data = serde_json::json!({ "line": line.trim_end() });
                Event::default().event("log").data(data.to_string())
            }
            Err(e) => {
                let data = serde_json::json!({ "message": e.to_string() });
                Event::default().event("error").data(data.to_string())
            }
        };
        Ok::<_, Infallible>(event)
    });

    // Wrap so `active_sse_connections` reflects live streams. The gauge
    // increments here and decrements when axum drops the response body.
    let tracked = TrackedSseStream::new(event_stream);

    Ok(Sse::new(tracked).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
