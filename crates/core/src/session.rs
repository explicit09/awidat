//! `Session` — the agent's per-conversation state, plus the two-loop
//! turn driver.
//!
//! Per the corpus survey of `harnesses/codex/codex-rs/core/src/session/turn.rs`:
//! - **Outer turn loop**: one iteration per user input. Builds a request,
//!   runs the inner sampling loop, decides if we need another iteration
//!   (yes if the model emitted tool_use blocks; no otherwise).
//! - **Inner sampling loop**: opens one streaming response, drains it
//!   into [`SessionEvent`]s. Tool calls are dispatched as they arrive
//!   and their results appended to history for the next iteration.
//!
//! Cancellation is plumbed via `tokio_util::sync::CancellationToken`
//! everywhere (Codex pattern). We use `tokio::select!` with the cancel
//! token at every await point in the loop.
//!
//! Events are broadcast via `tokio::sync::broadcast` so the week-5 TUI
//! can subscribe alongside the week-3 REPL.

use std::sync::Arc;

use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::FunctionCallError;
use crate::anthropic::{
    Client, ContentBlock, Message, MessagesRequest, Role, StopReason, StreamEvent, ToolChoice,
    Usage,
};
use crate::tool::{ToolInvocation, ToolRegistry};

/// One event emitted by the agent loop. The REPL prints these; the TUI
/// will render them more richly later.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// User input was accepted; about to start a turn.
    TurnStart,
    /// Model started a response (one per inner-loop iteration).
    MessageStart {
        /// Server-allocated id from `message_start`.
        message_id: String,
        /// Model name (echo).
        model: String,
    },
    /// Streaming text delta from the model.
    TextDelta(String),
    /// A tool call begins. Args are still streaming.
    ToolCallStart {
        /// Tool call id (echoed in the matching `tool_result`).
        id: String,
        /// Tool name.
        name: String,
    },
    /// Tool call args fully assembled and parsed.
    ToolCallArgs {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
        /// Parsed args.
        args: serde_json::Value,
    },
    /// Tool result ready. Either the tool's output (`Ok`) or an error
    /// the model will see as `is_error: true` (`Err`).
    ToolResult {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
        /// Result.
        result: Result<String, String>,
    },
    /// One inner-loop iteration finished. Carries the stop reason; the
    /// outer loop decides whether to iterate again.
    SamplingComplete {
        /// Why the model stopped this iteration.
        stop_reason: Option<StopReason>,
        /// Token usage for this iteration.
        usage: Usage,
    },
    /// Whole turn finished; control returned to the user.
    TurnEnd,
    /// A turn-fatal error. Loop stops; user can start a new turn.
    Error(String),
}

/// Errors that abort the turn loop. Per [`FunctionCallError`] semantics:
/// only [`SessionError::Fatal`] kills the turn; everything else feeds back
/// to the model so it can self-correct. Most things route via
/// [`SessionEvent::Error`] within the loop.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Anthropic client error that the loop can't recover from
    /// (auth failure, malformed request).
    #[error("client error: {0}")]
    Client(#[from] crate::anthropic::ClientError),
    /// Tool dispatch error of `Fatal` kind.
    #[error("fatal: {0}")]
    Fatal(String),
    /// User cancelled the turn.
    #[error("cancelled")]
    Cancelled,
}

/// Per-conversation state. Cheaply cloneable (`Arc`-wrapped internals).
pub struct Session {
    client: Client,
    registry: ToolRegistry,
    system_prompt: Option<String>,
    model: String,
    history: Arc<Mutex<Vec<Message>>>,
    events_tx: broadcast::Sender<SessionEvent>,
}

impl Session {
    /// Build a fresh session.
    pub fn new(
        client: Client,
        registry: ToolRegistry,
        model: impl Into<String>,
        system_prompt: Option<String>,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(128);
        Self {
            client,
            registry,
            system_prompt,
            model: model.into(),
            history: Arc::new(Mutex::new(Vec::new())),
            events_tx,
        }
    }

    /// Subscribe to event broadcast. Multiple subscribers are supported
    /// (REPL + TUI in week 5+).
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }

    /// Read-only view of the current conversation history.
    pub async fn history(&self) -> Vec<Message> {
        self.history.lock().await.clone()
    }

    /// Snapshot the current registered tool count. Useful for diagnostics.
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// Run one turn: append `user_input` to history, drive the two-loop
    /// engine until the model stops (or cancellation, or fatal error).
    pub async fn run_turn(
        &self,
        user_input: impl Into<String>,
        cancel: CancellationToken,
    ) -> Result<(), SessionError> {
        let _ = self.events_tx.send(SessionEvent::TurnStart);

        // 1. Append user input to history.
        {
            let mut h = self.history.lock().await;
            h.push(Message::user_text(user_input));
        }

        // 2. Outer loop: keep sampling until the model says end_turn (or
        //    we're cancelled).
        const MAX_INNER_ITERATIONS: usize = 32;
        for _iter in 0..MAX_INNER_ITERATIONS {
            if cancel.is_cancelled() {
                let _ = self.events_tx.send(SessionEvent::Error("cancelled".into()));
                return Err(SessionError::Cancelled);
            }

            // 3. Build the request from current history.
            let history_snapshot = self.history.lock().await.clone();
            let mut req = MessagesRequest::new(self.model.clone(), history_snapshot)
                .with_max_tokens(4096);
            if let Some(sys) = &self.system_prompt {
                req = req.with_system(sys.clone());
            }
            let schemas = self.registry.schemas();
            if !schemas.is_empty() {
                req = req.with_tools(schemas).with_tool_choice(ToolChoice::Auto);
            }

            // 4. Inner sampling loop.
            let outcome = self.run_sampling(req, &cancel).await?;

            match outcome.stop_reason {
                Some(StopReason::ToolUse) => {
                    // Loop again: history now contains the assistant's
                    // tool_use message + our tool_result reply.
                    debug!("model emitted tool_use; iterating");
                    continue;
                }
                _ => {
                    // end_turn / max_tokens / stop_sequence / refusal /
                    // pause_turn — outer loop ends.
                    let _ = self.events_tx.send(SessionEvent::TurnEnd);
                    return Ok(());
                }
            }
        }
        // Iteration cap reached — defensive against runaway tool loops.
        warn!("hit MAX_INNER_ITERATIONS; ending turn");
        let _ = self.events_tx.send(SessionEvent::Error(format!(
            "turn exceeded {MAX_INNER_ITERATIONS} sampling iterations; ending"
        )));
        let _ = self.events_tx.send(SessionEvent::TurnEnd);
        Ok(())
    }

    /// One streaming sampling iteration. Returns the inner-loop outcome
    /// (stop reason + usage) after consuming the entire stream.
    async fn run_sampling(
        &self,
        req: MessagesRequest,
        cancel: &CancellationToken,
    ) -> Result<SamplingOutcome, SessionError> {
        use futures::StreamExt;

        let mut stream = Box::pin(self.client.messages_stream(req));
        // Assistant content blocks accumulated this iteration. Appended
        // to history at iteration end so the next request includes them.
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        // Pending tool calls awaiting dispatch (queued during the stream;
        // dispatched after the stream ends so we don't fight the model
        // for the SSE channel).
        let mut pending_calls: Vec<PendingCall> = Vec::new();
        let mut current_text = String::new();
        let mut stop_reason = None;
        let mut usage = Usage::default();

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = self.events_tx.send(SessionEvent::Error("cancelled".into()));
                    return Err(SessionError::Cancelled);
                }
                next = stream.next() => {
                    let Some(item) = next else { break; };
                    let event = item?;
                    match event {
                        StreamEvent::MessageStart { message_id, model } => {
                            let _ = self.events_tx.send(SessionEvent::MessageStart {
                                message_id, model,
                            });
                        }
                        StreamEvent::TextDelta(t) => {
                            current_text.push_str(&t);
                            let _ = self.events_tx.send(SessionEvent::TextDelta(t));
                        }
                        StreamEvent::ToolCallStart { id, name } => {
                            // Flush any in-progress text block before the
                            // tool block so message order matches what the
                            // model emitted.
                            if !current_text.is_empty() {
                                assistant_blocks.push(ContentBlock::Text {
                                    text: std::mem::take(&mut current_text),
                                });
                            }
                            let _ = self.events_tx.send(SessionEvent::ToolCallStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                            pending_calls.push(PendingCall {
                                id, name, args: None, args_err: None,
                            });
                        }
                        StreamEvent::ToolCallEnd { id, name, result } => {
                            let pending = pending_calls
                                .iter_mut()
                                .find(|c| c.id == id);
                            match (pending, result) {
                                (Some(p), Ok(args)) => {
                                    p.args = Some(args.clone());
                                    let _ = self.events_tx.send(SessionEvent::ToolCallArgs {
                                        id, name, args,
                                    });
                                }
                                (Some(p), Err(msg)) => {
                                    p.args_err = Some(msg.clone());
                                    let _ = self.events_tx.send(SessionEvent::ToolResult {
                                        id, name,
                                        result: Err(msg),
                                    });
                                }
                                (None, _) => {
                                    warn!(call_id = %id, "ToolCallEnd without matching Start");
                                }
                            }
                        }
                        StreamEvent::Done { stop_reason: sr, usage: u } => {
                            stop_reason = sr;
                            usage = u;
                            // Stream's done; loop exits.
                            break;
                        }
                    }
                }
            }
        }

        // Flush any final text into a block.
        if !current_text.is_empty() {
            assistant_blocks.push(ContentBlock::Text { text: current_text });
        }
        // Append the assistant's tool_use blocks (with the input we parsed)
        // so the model's next view of history is complete.
        for call in &pending_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.args.clone().unwrap_or(serde_json::json!({})),
            });
        }
        if !assistant_blocks.is_empty() {
            let mut h = self.history.lock().await;
            h.push(Message {
                role: Role::Assistant,
                content: assistant_blocks,
            });
        }

        // Dispatch tool calls and append their results as a single user
        // message (Anthropic's tool-result protocol expects all tool_results
        // for one assistant message to land in one user message).
        if !pending_calls.is_empty() {
            let mut result_blocks = Vec::with_capacity(pending_calls.len());
            for call in pending_calls {
                let block = self.dispatch_tool(call, cancel).await?;
                result_blocks.push(block);
            }
            let mut h = self.history.lock().await;
            h.push(Message {
                role: Role::User,
                content: result_blocks,
            });
        }

        let _ = self.events_tx.send(SessionEvent::SamplingComplete {
            stop_reason,
            usage: usage.clone(),
        });
        Ok(SamplingOutcome { stop_reason, usage })
    }

    /// Dispatch one tool call. Returns the `ToolResult` content block to
    /// feed back to the model. Recoverable failures (`RespondToModel`,
    /// args-parse error) become `is_error: true` results; `Fatal` aborts.
    async fn dispatch_tool(
        &self,
        call: PendingCall,
        cancel: &CancellationToken,
    ) -> Result<ContentBlock, SessionError> {
        // If args failed to parse, surface the parse error directly without
        // dispatching.
        if let Some(err) = call.args_err {
            return Ok(ContentBlock::ToolResult {
                tool_use_id: call.id,
                content: format!(
                    "tool '{}' arguments failed to parse: {err}",
                    call.name
                ),
                is_error: Some(true),
            });
        }
        let args = call.args.unwrap_or(serde_json::json!({}));
        let Some(handler) = self.registry.get(&call.name) else {
            let _ = self.events_tx.send(SessionEvent::ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                result: Err(format!("no such tool: {}", call.name)),
            });
            return Ok(ContentBlock::ToolResult {
                tool_use_id: call.id,
                content: format!(
                    "tool '{}' is not registered. Available: {:?}",
                    call.name,
                    self.registry.names().collect::<Vec<_>>()
                ),
                is_error: Some(true),
            });
        };

        let invocation = ToolInvocation {
            call_id: call.id.clone(),
            name: call.name.clone(),
            args,
        };

        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SessionError::Cancelled),
            res = handler.handle(invocation) => res,
        };

        match result {
            Ok(out) => {
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Ok(out.content.clone()),
                });
                Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: out.content,
                    is_error: None,
                })
            }
            Err(FunctionCallError::Fatal(msg)) => {
                let _ = self.events_tx.send(SessionEvent::Error(format!(
                    "fatal: {msg}"
                )));
                Err(SessionError::Fatal(msg))
            }
            Err(other) => {
                let msg = other.to_string();
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Err(msg.clone()),
                });
                Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: msg,
                    is_error: Some(true),
                })
            }
        }
    }
}

/// One in-progress tool call accumulating during the stream.
struct PendingCall {
    id: String,
    name: String,
    /// Set when args fully parse.
    args: Option<serde_json::Value>,
    /// Set when args fail to parse.
    args_err: Option<String>,
}

/// What one inner-loop iteration produced.
struct SamplingOutcome {
    stop_reason: Option<StopReason>,
    /// Currently unused but threaded through for future telemetry.
    #[allow(dead_code)]
    usage: Usage,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: Session can be constructed without panicking and reports
    /// zero registered tools by default.
    #[test]
    fn empty_session_construction() {
        // Use a bogus client (no network calls happen at construction).
        let client = Client::new(
            "test-key",
            crate::anthropic::ClientConfig::default(),
        )
        .expect("client construction without network");
        let s = Session::new(client, ToolRegistry::new(), "claude-haiku-4-5-20251001", None);
        assert_eq!(s.tool_count(), 0);
    }
}
