//! Awidat agent loop and Anthropic API client.
//!
//! Per `PLAN.md` §15 Week 3:
//! - [`anthropic`] — typed messages, streaming SSE client, content-block
//!   parser. Hand-rolled (no first-party Anthropic Rust SDK exists in 2026;
//!   every harness in `harnesses/` hand-rolls this layer).
//! - [`error::FunctionCallError`] — copied verbatim from
//!   `harnesses/codex/codex-rs/core/src/function_tool.rs`.
//! - Tools, `Session`, and the agent loop land in later phases of week 3.

pub mod anthropic;
pub mod awidat_md;
pub mod compact;
pub mod edl;
pub mod context;
pub mod episode_map;
pub mod error;
pub mod mcp_host;
pub mod rollout;
pub mod session;
pub mod subagent;
pub mod skills;
pub mod tool;
pub mod tools;
pub mod verify;

pub use error::FunctionCallError;
pub use session::{Session, SessionError, SessionEvent};
pub use tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry, UserInputRequest};

/// Returns the version of the agent core.
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
