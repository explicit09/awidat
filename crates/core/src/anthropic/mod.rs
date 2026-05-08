//! Hand-rolled streaming client for Anthropic's Messages API.
//!
//! There is no first-party Anthropic Rust SDK in 2026 — every harness in
//! `harnesses/` (codex, goose) hand-rolls. We adopt that pattern: a thin
//! `reqwest` wrapper + `eventsource-stream` for SSE framing + a
//! content-block parser for the Anthropic-specific interleaved tool-use
//! shape.
//!
//! # Surface
//!
//! - [`Client::messages_stream`] — submit a [`MessagesRequest`], get back a
//!   stream of [`StreamEvent`]s.
//! - [`StreamEvent::TextDelta`] / `ToolCallStart` / `ToolCallEnd` — the
//!   parsed shape the agent loop consumes.
//! - [`MessagesRequest`] / [`Message`] / [`ContentBlock`] / [`Tool`] — the
//!   typed wire surface, narrow to what we actually use today.
//!
//! See `_notes/anthropic.md` (TODO) for design rationale; for now, the
//! load-bearing reference is
//! `harnesses/goose/crates/goose/src/providers/formats/anthropic.rs:557-797`
//! which is the production Rust implementation we're modeled after.

mod client;
mod messages;
mod sse;
mod stream;

pub use client::{Client, ClientConfig, ClientError};
pub use messages::{
    ContentBlock, Message, MessagesRequest, Role, StopReason, Tool, ToolChoice, Usage, tool_result,
};
pub use stream::{StreamEvent, StreamParseError};

/// API version header value. Anthropic preserves backward compat; this
/// has been stable since 2023 and is what every production client sends.
/// Spec: <https://platform.claude.com/docs/en/api/versioning>.
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Default base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Models we name for callers. Convenience constants; the API accepts any
/// string the platform recognizes.
pub mod models {
    /// Latest Opus snapshot (cited in PLAN.md as the default for awidat).
    pub const OPUS: &str = "claude-opus-4-7";
    /// Latest Sonnet snapshot — fast, cheap-ish.
    pub const SONNET: &str = "claude-sonnet-4-6";
    /// Latest Haiku snapshot — for editor-shaped subtasks (Aider-style).
    pub const HAIKU: &str = "claude-haiku-4-5-20251001";
}
