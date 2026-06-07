//! Session event + error types — the public surface the legacy agent loop
//! emitted, kept around for forward compatibility while the codex-rs
//! migration is in flight.
//!
//! Step 8e/W: the codex-driven loop in `vendor/codex-rs/` does not emit
//! or consume these types. The legacy in-process tool surface that did
//! (`update_plan`, `request_user_input`, `delegate`, `delegate_all`) was
//! deleted in this step. The types stay re-exported from `lib.rs` so
//! `montage_core::SessionEvent` / `SessionError` are still valid type
//! paths for any caller that imports them by name (and so the
//! `desktop-protocol` doc-comment cross-reference still resolves).
//!
//! `ClientError`, `StopReason`, and `Usage` used to live in the deleted
//! `crate::anthropic` module. They are inlined here as small standalone
//! types so `events.rs` no longer depends on the harness wire layer.

use std::time::Duration;

/// Stream parse error — placeholder inlined from the deleted
/// `crate::anthropic::sse` to keep `ClientError::Parse` valid without
/// pulling the harness back in.
#[derive(Debug, thiserror::Error)]
#[error("stream parse error")]
pub struct StreamParseError;

/// HTTP-shaped client error from the legacy Anthropic wire client.
/// Inlined here so `SessionError::Client` stays representable without
/// the deleted `crate::anthropic::Client`.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// HTTP / network failure.
    #[error("network error: {0}")]
    Network(String),
    /// Server returned a non-2xx with an error body.
    #[error("API returned {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from the server.
        message: String,
    },
    /// Stream closed before completion. Treat as retryable.
    #[error("stream closed prematurely: {0}")]
    StreamClosed(String),
    /// No event arrived within the idle window.
    #[error("idle timeout after {0:?} with no event")]
    Timeout(Duration),
    /// Parser error — malformed frame from the server.
    #[error("stream parse error: {0}")]
    Parse(#[from] StreamParseError),
    /// API key missing — neither env var nor keychain has it.
    #[error("ANTHROPIC_API_KEY not set in env or keychain")]
    MissingApiKey,
}

/// Why a model iteration stopped. Inlined here from the deleted
/// `crate::anthropic::messages` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Natural completion.
    EndTurn,
    /// Hit `max_tokens`.
    MaxTokens,
    /// A stop sequence.
    StopSequence,
    /// The model emitted a `tool_use` block; the next turn must include
    /// `tool_result`s.
    ToolUse,
    /// Refusal.
    Refusal,
    /// Pause for a tool call we've not yet implemented.
    PauseTurn,
}

/// Token usage from a model response. Inlined here from the deleted
/// `crate::anthropic::messages` module.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Input token count.
    pub input_tokens: Option<u32>,
    /// Output token count.
    pub output_tokens: Option<u32>,
    /// Cached input tokens (cache read hit).
    pub cache_read_input_tokens: Option<u32>,
    /// Cached input tokens (cache write).
    pub cache_creation_input_tokens: Option<u32>,
}

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
