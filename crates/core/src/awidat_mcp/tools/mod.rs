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

pub mod list_markers;
