//! Awidat Ratatui terminal UI.
//!
//! Single-pane chat for v1 (per `PLAN.md` §15 Week 5 + the competitive
//! survey reframe — the TUI is the developer-facing surface, not the
//! long-term human wedge; that's the future GUI viewer). The structure
//! mirrors the Codex TUI but keeps an order of magnitude less code:
//!
//! - [`app`] — the event loop + state + render dispatch.
//! - [`event`] — terminal events + the `AppEvent` enum the loop drains.
//! - [`chat`] — chat-pane state: streaming model deltas, tool spinners,
//!   tool results, approval prompts.
//! - [`composer`] — the input box at the bottom.
//! - [`approval`] — the modal overlay for mutating-tool approvals.
//!
//! The agent-side plumbing (`Session`, `SessionEvent`, `ApprovalRequest`,
//! `UserInputRequest`) lives in `awidat-core`; this crate only owns the
//! presentation layer.

// Crate-wide unsafe is denied by default. The `terminal_probe` module
// individually allows it (with SAFETY: comments) for narrow libc
// syscalls; nothing else in the crate should follow that lead. See
// crates/tui/Cargo.toml for the rationale.
#![deny(unsafe_code)]
#![warn(missing_docs)]
#![allow(
    clippy::doc_lazy_continuation,
    clippy::expect_used,
    clippy::if_same_then_else,
    clippy::let_underscore_future,
    clippy::manual_clamp,
    clippy::never_loop,
    clippy::redundant_closure_for_method_calls,
    clippy::uninlined_format_args,
    clippy::unwrap_used
)]

pub mod app;
pub mod approval;
pub mod chat;
pub mod composer;
pub mod custom_terminal;
pub mod event;
pub mod insert_history;
pub mod project_insights;
pub mod terminal_probe;
pub mod timeline;

pub use app::{App, AppConfig};
pub use event::AppEvent;

/// Crate version (used by `awidat tui --version`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_pkg_version() {
        assert!(!version().is_empty());
    }
}
