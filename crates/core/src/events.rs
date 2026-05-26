//! Session event + error types — the public surface the legacy agent loop
//! emitted, and the only piece a handful of tools (and the structured-plan
//! executor) read by name.
//!
//! Lifted out of `crate::session` in step 8e: the codex-driven loop in
//! `vendor/codex-rs/` doesn't produce or consume these types directly, but
//! `update_plan`, `request_user_input`, `delegate`, `delegate_all`, and
//! `structured_plan` still emit them via the shared broadcast channel.
//! Keeping them in their own module lets the legacy `session.rs` /
//! `orchestrator.rs` modules be deleted in the final sweep without
//! disturbing the in-process tool surface.

use crate::anthropic::{ClientError, StopReason, Usage};

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

/// Errors that abort the turn loop. Per [`crate::FunctionCallError`]
/// semantics: only [`SessionError::Fatal`] kills the turn; everything else
/// feeds back to the model so it can self-correct. Most things route via
/// [`SessionEvent::Error`] within the loop.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Anthropic client error that the loop can't recover from
    /// (auth failure, malformed request).
    #[error("client error: {0}")]
    Client(#[from] ClientError),
    /// Tool dispatch error of `Fatal` kind.
    #[error("fatal: {0}")]
    Fatal(String),
    /// User cancelled the turn.
    #[error("cancelled")]
    Cancelled,
    /// User denied a tool in the orchestrator approval flow.
    #[error("{message}")]
    ToolDenied {
        /// Tool call id.
        call_id: String,
        /// Tool name.
        tool_name: String,
        /// Message to feed back to the model.
        message: String,
    },
    /// Catch-all for setup-time errors (resume failures, etc).
    #[error("{0}")]
    Other(String),
}
