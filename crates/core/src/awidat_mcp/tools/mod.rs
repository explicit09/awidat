//! Ported Awidat video tools, in MCP-native form.
//!
//! Each submodule corresponds to one tool that used to live in
//! `crates/core/src/tools/`. Step 5 of the codex-harness migration
//! ports them onto `AwidatMcpServer`. The original files in
//! `crates/core/src/tools/` stay live until step 7 deletes the old
//! Awidat agent loop; until then, both call sites use distinct
//! copies of the logic.
//!
//! Rule of thumb when porting:
//! - Copy the pure helpers (validation, collection, rendering) into
//!   the new module. They do NOT depend on `ToolContext`.
//! - Each `pub fn` here is callable from `awidat_mcp::mod`'s
//!   `#[tool(...)]` method. The method handles serde and result
//!   shaping; the helpers handle real work.
//! - Use [`crate::awidat_mcp::context::McpToolCtx::resolve`] to get
//!   the project root.

pub mod color_scopes;
pub mod find_black_frames;
pub mod find_dead_air;
pub mod find_false_starts;
pub mod list_looks;
pub mod list_markers;
pub mod list_stringouts;
pub mod local_review_package;
pub mod read_broll_recommendations;
pub mod read_index;
pub mod read_media_intelligence;
pub mod read_media_readiness;
pub mod read_understanding;
pub mod transcript_pack;
pub mod vedit_blame;
pub mod vedit_branch;
pub mod vedit_changed_clip_ids;
pub mod vedit_checkout;
pub mod vedit_commit;
pub mod vedit_diff;
pub mod vedit_log;
pub mod vedit_merge_preflight;
pub mod vedit_show;
pub mod vedit_tag;
pub mod view_episode;
pub mod view_timeline;
