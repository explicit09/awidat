//! Tool-call error type. Lifted verbatim from
//! `harnesses/codex/codex-rs/core/src/function_tool.rs` per `PLAN.md` §7.2.
//!
//! The 3-way split is load-bearing:
//!
//! - **`RespondToModel`** — recoverable. The error string becomes the tool's
//!   output (with `is_error: true`); the model sees it and self-corrects.
//!   Use for "your args were invalid", "asset not found", "anchor moved".
//! - **`MissingCallId`** — protocol violation. The model emitted a tool
//!   call without an id. Should never reach the model; bubbles up.
//! - **`Fatal`** — kills the turn. Project file corrupt, sandbox bootstrap
//!   failed, MCP server died. The agent loop returns; the user retries.
//!
//! See `_notes/codex.md` §1 ("Function tool error model") for context.

use thiserror::Error;

/// What can go wrong dispatching a tool call.
#[derive(Debug, Error, PartialEq)]
pub enum FunctionCallError {
    /// Recoverable: surface the message to the model as the tool's output
    /// (with `is_error: true`) so the model self-corrects.
    #[error("{0}")]
    RespondToModel(String),
    /// Recoverable sandbox denial. The orchestrator may request an
    /// explicit unsandboxed retry approval for sandbox-capable tools.
    #[error("sandbox denied execution: {reason}")]
    SandboxDenied {
        /// Denial reason reported by the sandbox backend.
        reason: String,
    },
    /// Protocol violation: the model emitted a tool call with no id.
    /// Should never escape — bubbles up.
    #[error("tool call missing call_id or id")]
    MissingCallId,
    /// Fatal: kill the turn.
    #[error("fatal error: {0}")]
    Fatal(String),
}
