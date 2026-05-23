//! Awidat agent loop and Anthropic API client.
//!
//! Per `PLAN.md` §15 Week 3:
//! - [`anthropic`] — typed messages, streaming SSE client, content-block
//!   parser. Hand-rolled (no first-party Anthropic Rust SDK exists in 2026;
//!   every harness in `harnesses/` hand-rolls this layer).
//! - [`error::FunctionCallError`] — copied verbatim from
//!   `harnesses/codex/codex-rs/core/src/function_tool.rs`.
//! - Tools, `Session`, and the agent loop land in later phases of week 3.

#![allow(
    clippy::clone_on_copy,
    clippy::cloned_ref_to_slice_refs,
    clippy::collapsible_if,
    clippy::doc_lazy_continuation,
    clippy::expect_used,
    clippy::for_kv_map,
    clippy::manual_contains,
    clippy::large_enum_variant,
    clippy::let_and_return,
    clippy::needless_collect,
    clippy::question_mark,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_arguments,
    clippy::uninlined_format_args,
    clippy::unnecessary_sort_by,
    clippy::unusual_byte_groupings,
    clippy::useless_conversion,
    clippy::useless_format,
    clippy::vec_init_then_push
)]
#![cfg_attr(
    test,
    allow(
        clippy::assertions_on_constants,
        clippy::field_reassign_with_default,
        clippy::unwrap_used
    )
)]

pub mod anthropic;
pub mod awidat_md;
pub mod capabilities;
pub mod capability_metadata;
pub mod caption_rendered_output_scorer;
pub mod captions;
pub mod compact;
pub mod context;
pub mod continuity;
pub mod dismissal;
pub mod edl;
pub mod episode_map;
pub mod error;
pub mod lessons;
pub mod mcp_host;
pub mod media_catalog_mutation;
pub mod notes;
pub mod orchestrator;
pub mod pexels;
pub mod preview_cache;
pub mod preview_refresh_executor;
pub mod professional;
pub mod proxy;
pub mod review;
pub mod rollout;
pub mod session;
pub mod skills;
pub mod structured_plan;
pub mod subagent;
pub mod system_prompt;
pub mod tool;
pub mod tools;
pub mod transcript_cleanup;
pub mod transcript_pack;
pub mod vc;
pub mod verify;
pub mod visual_signals;

pub use capability_metadata::{CapabilityMetadata, SupportLevel};
pub use error::FunctionCallError;
pub use session::{Session, SessionError, SessionEvent};
pub use tool::{
    ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry, UserInputRequest,
};

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
