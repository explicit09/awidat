//! Montage editorial core — tool implementations, project state, and the
//! supporting types the codex-driven harness in `vendor/codex-rs/` calls
//! into.
//!
//! Step 8e/W: the legacy in-process agent loop (`session`, `orchestrator`,
//! `compact`, the hand-rolled `anthropic` client, the `rollout` recorder,
//! and the `montage_md` system-prompt assembler) was deleted in this step.
//! The codex subprocess in `vendor/codex-rs/` is now the only agent loop.
//! What remains here:
//! - [`error::FunctionCallError`] — tool dispatch result shape, copied
//!   verbatim from `vendor/codex-rs/core/src/function_tool.rs`.
//! - [`tools`] — in-process tool implementations the `montage_mcp` MCP
//!   wrappers delegate into.
//! - [`montage_mcp`] — MCP-server tool definitions codex invokes.
//! - [`events`] — `SessionEvent` / `SessionError` shape kept for forward
//!   compatibility while consumers migrate to codex's event stream.

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

pub mod broll_recommendations;
pub mod capabilities;
pub mod capability_metadata;
pub mod caption;
pub mod caption_rendered_output_scorer;
pub mod captions;
pub mod clip_candidates;
pub mod color_analysis;
pub mod context;
pub mod continuity;
pub mod dismissal;
pub mod editorial_skills;
pub mod editorial_tags;
pub mod edl;
pub mod episode_map;
pub mod error;
pub mod events;
pub mod generated_media;
pub mod lessons;
pub mod mcp_host;
pub mod media_catalog_mutation;
pub mod media_intelligence;
pub mod montage_mcp;
pub mod motion_scene;
pub mod motion_templates;
pub mod notes;
pub mod pexels;
pub mod picture_lock;
pub mod podcast_analysis;
pub mod podcast_cleanup_scan;
pub mod preview_cache;
pub mod preview_refresh_executor;
pub mod professional;
pub mod proxy;
pub mod render_preflight_core;
pub mod review;
pub mod scene_aware_short_form;
pub mod short_form_intelligence;
pub mod short_form_review;
pub mod short_form_review_context;
pub mod skill_session;
pub mod skills;
pub mod subagent;
pub mod system_prompt;
pub mod tool;
pub mod tool_schema;
pub mod tools;
pub mod transcript_alignment;
pub mod transcript_cleanup;
pub mod transcript_pack;
pub mod understanding;
pub mod vc;
pub mod verify;
pub mod visual_qa;
pub mod visual_signals;
pub mod x_trends;

pub use capability_metadata::{CapabilityMetadata, SupportLevel};
pub use error::FunctionCallError;
pub use events::{SessionError, SessionEvent};
pub use tool::{
    ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry, UserInputRequest,
};

/// Returns the version of the agent core.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Test-only synchronization for tests that touch process-global env.
#[cfg(test)]
pub(crate) mod test_env {
    /// Serializes every test that SETS, REMOVES, or READS the
    /// process-global `MONTAGE_SKILLS_ROOT` env var (via
    /// `montage_config::defaults::skills_root()` or directly). Under
    /// the parallel test harness, a mutator swapping the var to its
    /// tempdir races readers in other modules — e.g.
    /// `plan_look_regions` resolving bundled color-corrector scripts
    /// against a root that lacks them ("look_region_plan.py was not
    /// found"). Hold this for the full mutation/read window.
    pub(crate) static SKILLS_ROOT_ENV_LOCK: tokio::sync::Mutex<()> =
        tokio::sync::Mutex::const_new(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_pkg_version() {
        assert!(!version().is_empty());
    }
}
