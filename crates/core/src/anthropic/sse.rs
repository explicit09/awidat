//! Minimal SSE wrapper. Lifted from
//! `harnesses/codex/codex-rs/codex-client/src/sse.rs` (~48 LOC) — that is
//! the canonical Rust SSE-for-LLM-streams shape and we don't improve on it.
//!
//! Forwards each `data:` frame to `tx` as a UTF-8 string. Idle timeouts and
//! "stream closed before completion" errors are surfaced so the agent loop
//! can distinguish a real graceful end (a `message_stop` frame) from a
//! transient network glitch.

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::client::ClientError;

/// One SSE chunk: either a `data:` frame's payload, or an error.
pub(crate) type SseItem = Result<String, ClientError>;

/// Spawn a task that reads `byte_stream` line-by-line and forwards each
/// SSE `data:` payload onto `tx`. Errors and timeouts terminate the task
/// after sending one final `Err` item.
///
/// `idle_timeout` triggers if no event arrives within the window — the
/// model is hung. Anthropic's typical inter-event gap is well under 1s
/// during streaming; a 90-second timeout is safe and matches Codex.
pub(crate) fn forward_sse<S, E>(byte_stream: S, idle_timeout: Duration, tx: mpsc::Sender<SseItem>)
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut stream = byte_stream
            .map(|res| res.map_err(|e| ClientError::Network(e.to_string())))
            .eventsource();

        loop {
            match timeout(idle_timeout, stream.next()).await {
                Ok(Some(Ok(ev))) => {
                    if tx.send(Ok(ev.data.clone())).await.is_err() {
                        return;
                    }
                }
                Ok(Some(Err(e))) => {
                    let _ = tx.send(Err(ClientError::Network(e.to_string()))).await;
                    return;
                }
                Ok(None) => {
                    let _ = tx
                        .send(Err(ClientError::StreamClosed(
                            "stream closed before message_stop".into(),
                        )))
                        .await;
                    return;
                }
                Err(_) => {
                    let _ = tx.send(Err(ClientError::Timeout(idle_timeout))).await;
                    return;
                }
            }
        }
    });
}
