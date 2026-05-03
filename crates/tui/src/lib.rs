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

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod approval;
pub mod chat;
pub mod composer;
pub mod event;
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
