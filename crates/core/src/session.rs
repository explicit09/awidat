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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::FunctionCallError;
use crate::anthropic::{
    Client, ContentBlock, Message, MessagesRequest, Role, StopReason, StreamEvent, ToolChoice,
    Usage,
};
use crate::tool::{
    ApprovalDecision, ApprovalRequest, ToolContext, ToolInvocation, ToolRegistry, UserInputRequest,
};

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
    /// `update_plan` tool call landed. Carries the agent's full plan
    /// snapshot. The REPL prints it; the TUI will render it richly.
    EditPlanUpdate {
        /// One bullet per item.
        items: Vec<crate::tool::PlanItem>,
        /// Free-form short note from the model (one sentence).
        note: Option<String>,
    },
    /// `request_user_input` tool call landed. The runtime is now awaiting
    /// the user's reply via the user_input channel.
    AwaitingUserInput {
        /// Tool call id.
        call_id: String,
        /// Question to display.
        question: String,
        /// Optional choices.
        options: Option<Vec<String>>,
    },
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
    project_root: PathBuf,
    history: Arc<Mutex<Vec<Message>>>,
    events_tx: broadcast::Sender<SessionEvent>,
    /// Channel for surfacing `request_user_input` prompts to the REPL/TUI.
    /// `None` if the session was constructed without one — calls to
    /// `request_user_input` then return `RespondToModel`.
    user_input_tx: Option<mpsc::Sender<UserInputRequest>>,
    /// Shared render-job manager handed to every tool via `ToolContext`.
    job_manager: awidat_render::JobManager,
    /// Channel the loop uses to ask the front-end to approve mutating tool
    /// calls. `None` ⇒ loop defaults to allow (tests, batch CLI, MCP).
    approval_tx: Option<mpsc::Sender<ApprovalRequest>>,
    /// Set of tool names the user has elected to allow for the rest of
    /// the session via [`ApprovalDecision::AllowForSession`]. Future calls
    /// to a name in this set skip the approval prompt.
    approved_for_session: Arc<Mutex<HashSet<String>>>,
}

impl Session {
    /// Build a fresh session rooted at `project_root`. Tools that need a
    /// project directory (most of week 4) read it from the
    /// [`ToolContext`] handed to their `handle()`.
    pub fn new(
        client: Client,
        registry: ToolRegistry,
        model: impl Into<String>,
        system_prompt: Option<String>,
        project_root: impl Into<PathBuf>,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(128);
        Self {
            client,
            registry,
            system_prompt,
            model: model.into(),
            project_root: project_root.into(),
            history: Arc::new(Mutex::new(Vec::new())),
            events_tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            approved_for_session: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Wire an approval channel. The REPL/TUI receives one
    /// [`ApprovalRequest`] per mutating tool call (per
    /// [`crate::ToolHandler::is_mutating`]) and replies with an
    /// [`ApprovalDecision`]. Without this the loop defaults to
    /// [`ApprovalDecision::Allow`] — appropriate for tests, batch CLI,
    /// and the MCP server which run unattended.
    #[must_use]
    pub fn with_approval_channel(
        mut self,
        tx: mpsc::Sender<ApprovalRequest>,
    ) -> Self {
        self.approval_tx = Some(tx);
        self
    }

    /// Wire a user-input channel. The REPL/TUI receives one
    /// [`UserInputRequest`] per `request_user_input` tool call and replies
    /// via the embedded `oneshot`. Without this, that tool returns
    /// `RespondToModel("interactive input not available in this runtime")`.
    #[must_use]
    pub fn with_user_input_channel(
        mut self,
        tx: mpsc::Sender<UserInputRequest>,
    ) -> Self {
        self.user_input_tx = Some(tx);
        self
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

    /// Project root the session was opened against. The TUI's timeline
    /// pane reads `project.otio.json` from here on every `apply_edl`
    /// commit so what's painted stays authoritative.
    pub fn project_root(&self) -> &std::path::Path {
        &self.project_root
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
        //
        // Cap raised to 64 after first-real-video runs surfaced the
        // editorial-flow agent burning 8-12 iterations on legitimate
        // bash exploration before settling into the cut. We also emit
        // a one-shot "approaching cap" warning at 80% so the agent
        // can compact its plan before it hits the hard stop —
        // anthropics' tool-use cookbook recommends this pattern.
        const MAX_INNER_ITERATIONS: usize = 64;
        const WARN_AT_ITERATION: usize = (MAX_INNER_ITERATIONS * 4) / 5; // 80%
        let mut warned = false;
        for iter in 0..MAX_INNER_ITERATIONS {
            if cancel.is_cancelled() {
                let _ = self.events_tx.send(SessionEvent::Error("cancelled".into()));
                return Err(SessionError::Cancelled);
            }

            // 3. Build the request from current history.
            //
            // Two-tier prompt cache (mirrors aider/chat_chunks.py and
            // swe-agent/CacheControlHistoryProcessor):
            //
            //   tier-1 (static): system + tools, marked once on the
            //     last tool. Persists across the whole session.
            //   tier-2 (moving): a breakpoint on the most recent
            //     user-role message that's followed by an assistant
            //     turn — i.e. the latest "stable" history boundary.
            //     Walks forward each iteration; old breakpoints get
            //     cleared first or Anthropic rejects the request.
            //
            // Net effect: the prefix of (system + tools + all prior
            // turns) is cache-read at ~10% input price after the
            // first turn. Cache writes cost +25% on the first turn
            // through, which pays back after 1-2 reuses.
            let mut history_snapshot = self.history.lock().await.clone();
            apply_moving_cache_breakpoint(&mut history_snapshot);

            let mut req = MessagesRequest::new(self.model.clone(), history_snapshot)
                .with_max_tokens(4096);
            if let Some(sys) = &self.system_prompt {
                req = req.with_system_cached(sys.clone());
            }
            let schemas = self.registry.schemas();
            if !schemas.is_empty() {
                req = req.with_tools(schemas).with_tool_choice(ToolChoice::Auto);
                req.cache_last_tool();
            }

            // Inject a budget warning into history when we cross the
            // 80% threshold. The model sees it as a system reminder
            // and can choose to compact / commit / wrap up.
            if !warned && iter >= WARN_AT_ITERATION {
                warned = true;
                let remaining = MAX_INNER_ITERATIONS - iter;
                let mut h = self.history.lock().await;
                h.push(Message::user_text(format!(
                    "[awidat-runtime] Heads up: {remaining} sampling iterations \
                     remain in this turn before it auto-ends. If you have a \
                     pending edit, commit it now; otherwise wrap up your reply."
                )));
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
                                assistant_blocks.push(ContentBlock::text(
                                    std::mem::take(&mut current_text),
                                ));
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
            assistant_blocks.push(ContentBlock::text(current_text));
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
                content: crate::anthropic::tool_result::text(format!(
                    "tool '{}' arguments failed to parse: {err}",
                    call.name
                )),
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
                content: crate::anthropic::tool_result::text(format!(
                    "tool '{}' is not registered. Available: {:?}",
                    call.name,
                    self.registry.names().collect::<Vec<_>>()
                )),
                is_error: Some(true),
            });
        };

        let invocation = ToolInvocation {
            call_id: call.id.clone(),
            name: call.name.clone(),
            args,
        };

        // Approval gate. Mutating tools must clear an approval check
        // before dispatch when the runtime wired an approval channel.
        // No channel ⇒ allow (tests, batch CLI, MCP).
        let mutating = handler.is_mutating(&invocation);
        if mutating && let Some(tx) = self.approval_tx.clone() {
            let already_session_allowed = self
                .approved_for_session
                .lock()
                .await
                .contains(&call.name);
            if !already_session_allowed {
                let decision = self
                    .request_approval(&tx, &invocation, cancel)
                    .await?;
                match decision {
                    ApprovalDecision::Allow => {}
                    ApprovalDecision::AllowForSession => {
                        self.approved_for_session
                            .lock()
                            .await
                            .insert(call.name.clone());
                    }
                    ApprovalDecision::Deny => {
                        let msg = format!("user denied execution of '{}'", call.name);
                        let _ = self.events_tx.send(SessionEvent::ToolResult {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            result: Err(msg.clone()),
                        });
                        return Ok(ContentBlock::ToolResult {
                            tool_use_id: call.id,
                            content: crate::anthropic::tool_result::text(msg),
                            is_error: Some(true),
                        });
                    }
                }
            }
        }

        let ctx = ToolContext {
            project_root: self.project_root.clone(),
            events_tx: self.events_tx.clone(),
            user_input_tx: self.user_input_tx.clone(),
            job_manager: self.job_manager.clone(),
            approval_tx: self.approval_tx.clone(),
        };

        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SessionError::Cancelled),
            res = handler.handle(invocation, ctx) => res,
        };

        match result {
            Ok(out) => {
                // Build the wire content: plain string for text-only,
                // multi-block array when the tool attached images.
                let wire_content = if out.images.is_empty() {
                    crate::anthropic::tool_result::text(&out.content)
                } else {
                    crate::anthropic::tool_result::text_and_images(
                        &out.content, &out.images,
                    )
                };
                // The event broadcast carries just the text part; the TUI
                // renders images via the file-cache path (week 5+).
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Ok(out.content.clone()),
                });
                Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: wire_content,
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
                    content: crate::anthropic::tool_result::text(msg),
                    is_error: Some(true),
                })
            }
        }
    }

    /// Send an approval request and await the user's decision.
    ///
    /// On cancellation the loop reports `SessionError::Cancelled` so the
    /// outer driver can shut down. On a closed reply oneshot (the UI
    /// dropped without responding) we treat it as an implicit deny —
    /// dropping is meaningfully different from explicit allow.
    async fn request_approval(
        &self,
        approval_tx: &mpsc::Sender<ApprovalRequest>,
        invocation: &ToolInvocation,
        cancel: &CancellationToken,
    ) -> Result<ApprovalDecision, SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let req = ApprovalRequest {
            call_id: invocation.call_id.clone(),
            tool_name: invocation.name.clone(),
            args_summary: summarize_args(&invocation.args),
            reply: reply_tx,
        };
        // If the channel is closed (no live UI), default to allow rather
        // than hanging — the surrounding `is_some()` check already gated
        // entry; a closed channel here means the UI process exited mid-turn.
        if approval_tx.send(req).await.is_err() {
            warn!("approval channel closed mid-turn; defaulting to allow");
            return Ok(ApprovalDecision::Allow);
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(SessionError::Cancelled),
            decision = reply_rx => Ok(decision.unwrap_or(ApprovalDecision::Deny)),
        }
    }
}

/// Build a short human-readable args summary for the approval modal.
///
/// Heuristic: 200 chars of compact JSON, with newlines and runs of
/// whitespace squashed. Tools with rich args (like `apply_edl` with a
/// multi-line EDL) get truncated; the modal can show full args on demand
/// later if we need it. Keeping this in core means every front-end
/// (REPL, TUI, future GUI) gets the same default summary.
///
/// Truncation is done by Unicode chars (not bytes) so multi-byte input
/// (emoji, accents) doesn't trigger a panic at a sub-char boundary.
fn summarize_args(args: &serde_json::Value) -> String {
    const CAP: usize = 200;
    let raw = args.to_string();
    let squashed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() > CAP {
        let truncated: String = squashed.chars().take(CAP).collect();
        format!("{truncated}…")
    } else {
        squashed
    }
}

/// Walk the history and place the *moving* tier-2 cache breakpoint.
///
/// Strategy mirrors aider's `chat_chunks.py:add_cache_control_headers`
/// and swe-agent's `CacheControlHistoryProcessor`:
///
/// 1. Clear cache marks from every prior message — Anthropic rejects
///    requests that have stale breakpoints scattered through history.
/// 2. Scan from the end backwards; mark the first User message we
///    find. That covers the entire prefix from the start of the
///    request up to and including that user turn.
///
/// On the first turn (history = `[Message::user_text(prompt)]`) the
/// loop marks the very prompt the user just sent — fine; that
/// becomes the next turn's tier-2 cache hit.
///
/// On later turns, marking the latest user message means everything
/// before the assistant's *current* turn (which we're about to
/// regenerate) is cached. The volatile bit — the new tool_result we
/// just appended, or the fresh assistant emission — sits past the
/// breakpoint and is fresh-billed. That's the right slice.
fn apply_moving_cache_breakpoint(messages: &mut [Message]) {
    for m in messages.iter_mut() {
        m.clear_cache_breakpoint();
    }
    if let Some(latest_user) = messages.iter_mut().rev().find(|m| m.role == Role::User) {
        latest_user.set_cache_breakpoint();
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
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        );
        assert_eq!(s.tool_count(), 0);
    }

    /// Wiring smoke: `with_approval_channel` populates the field. The
    /// full approval round-trip is exercised by the TUI bringup test.
    #[test]
    fn with_approval_channel_wires_the_field() {
        let client = Client::new(
            "test-key",
            crate::anthropic::ClientConfig::default(),
        )
        .expect("client");
        let (tx, _rx) = mpsc::channel(8);
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        )
        .with_approval_channel(tx);
        assert!(s.approval_tx.is_some(), "approval_tx must be set");
    }

    #[test]
    fn summarize_args_truncates_long_payloads() {
        let v = serde_json::json!({"edl": "x".repeat(500)});
        let s = summarize_args(&v);
        // 200 chars of ASCII + a multi-byte '…' (3 bytes UTF-8). Use
        // chars() not bytes for the cap.
        assert!(s.chars().count() <= 201, "200 chars + ellipsis: {}", s.chars().count());
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_args_squashes_whitespace() {
        let v = serde_json::json!({"edl": "line1\n\nline2\n   line3"});
        let s = summarize_args(&v);
        assert!(!s.contains('\n'), "newlines squashed: {s}");
    }

    #[test]
    fn moving_cache_breakpoint_marks_latest_user_message_only() {
        let mut history = vec![
            Message::user_text("first user prompt"),
            Message::assistant_text("first reply"),
            Message::user_text("second user prompt"),
            Message::assistant_text("second reply"),
        ];
        apply_moving_cache_breakpoint(&mut history);

        // Helper: count text blocks with cache_control set.
        fn marked(m: &Message) -> usize {
            m.content
                .iter()
                .filter(|b| {
                    matches!(
                        b,
                        crate::anthropic::ContentBlock::Text {
                            cache_control: Some(_),
                            ..
                        }
                    )
                })
                .count()
        }
        // Only the *last* user message should carry a breakpoint.
        assert_eq!(marked(&history[0]), 0, "first user not marked");
        assert_eq!(marked(&history[1]), 0, "first assistant not marked");
        assert_eq!(marked(&history[2]), 1, "second user marked");
        assert_eq!(marked(&history[3]), 0, "second assistant not marked");
    }

    #[test]
    fn moving_cache_breakpoint_clears_old_marks_before_remarking() {
        // Simulate a stale breakpoint left from a prior turn on an
        // earlier user message.
        let mut older = Message::user_text("old prompt");
        older.set_cache_breakpoint();
        let mut history = vec![
            older,
            Message::assistant_text("reply"),
            Message::user_text("new prompt"),
        ];
        apply_moving_cache_breakpoint(&mut history);

        // Old mark cleared.
        let stale_marked = matches!(
            &history[0].content[0],
            crate::anthropic::ContentBlock::Text {
                cache_control: Some(_),
                ..
            }
        );
        assert!(!stale_marked, "old breakpoint must be cleared");
        // New mark on the latest user message.
        let new_marked = matches!(
            &history[2].content[0],
            crate::anthropic::ContentBlock::Text {
                cache_control: Some(_),
                ..
            }
        );
        assert!(new_marked, "new breakpoint must be set");
    }
}
