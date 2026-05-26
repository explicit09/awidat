//! In-process Awidat MCP server.
//!
//! Codex consumes this server like any other stdio MCP server (see the
//! `[mcp_servers.awidat]` entry passed via `-c` in
//! `crates/cli/src/chat_codex_cmd.rs`). Codex spawns our binary, talks
//! JSON-RPC over stdin/stdout, lists our tools, and routes model tool
//! calls into them.
//!
//! Why MCP instead of a deeper codex fork (step 2 design): codex's
//! "native" tool surface is JSON descriptors fed to the model, not a
//! Rust trait we can implement. The actual Rust execution code for a
//! tool has to live somewhere off the model's call path; MCP is the
//! well-trodden hook and codex already handles connection lifecycle,
//! schema negotiation, and approval routing for MCP servers. Putting
//! our tools here keeps all Awidat changes in `crates/`, not
//! `vendor/codex-rs/`, so future fork refreshes don't touch our tool
//! surface.
//!
//! Step 5 of the migration ports the ~100 real video tools onto this
//! server. Each tool's pure logic lives in
//! [`crate::awidat_mcp::tools`]; the rmcp-facing wrappers (with
//! `#[tool(...)]` and `annotations(...)`) are right here in
//! [`AwidatMcpServer`].
//!
//! ## Adding a tool
//!
//! 1. Write a `pub fn run(args, ctx) -> Result<String, String>` (or
//!    equivalent) in `crates/core/src/awidat_mcp/tools/<name>.rs`,
//!    keeping the logic free of `ToolHandler`/`ToolContext`.
//! 2. Register it as a `pub mod` in `tools/mod.rs`.
//! 3. Add a `#[tool(description = "...", annotations(...))]` method
//!    here that calls into the new module.
//!
//! Every tool must annotate ONE of:
//!   - `read_only_hint = true`  — tool reads project state only.
//!   - `destructive_hint = true` — tool mutates state (EDL writes,
//!     render jobs, asset imports, bash).
//! Codex respects these via `requires_mcp_tool_approval`
//! (`vendor/codex-rs/core/src/mcp_tool_call.rs`) and fires a
//! pre-execution approval prompt for destructive tools when
//! `approval_policy != "never"`. Tools missing both annotations are
//! conservatively treated as mutating (fail-closed).

pub mod context;
pub mod tools;

use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;

use crate::awidat_mcp::context::McpToolCtx;
use crate::awidat_mcp::tools::list_looks::{self, ListLooksArgs};
use crate::awidat_mcp::tools::list_markers::{self, ListMarkersArgs};
use crate::awidat_mcp::tools::list_stringouts::{self, ListStringoutsArgs};
use crate::awidat_mcp::tools::read_index::{self, ReadIndexArgs};
use crate::awidat_mcp::tools::read_understanding::{self, ReadUnderstandingArgs};
use crate::awidat_mcp::tools::view_episode::{self, ViewEpisodeArgs};
use crate::awidat_mcp::tools::view_timeline::{self, ViewTimelineArgs};

/// The Awidat MCP server. One short-lived struct per child-process
/// invocation. Holds a `ToolRouter` populated by the `#[tool_router]`
/// macro on `impl AwidatMcpServer`.
#[derive(Debug, Clone)]
pub struct AwidatMcpServer {
    tool_router: ToolRouter<Self>,
}

impl AwidatMcpServer {
    /// Build a fresh server with all registered tools.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for AwidatMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl AwidatMcpServer {
    /// `list_markers` — read-only marker inventory across clip,
    /// timeline, and guide-track scopes. Description text mirrors
    /// `list_markers::DESCRIPTION` because `#[tool(description =
    /// ...)]` only accepts a string literal.
    #[tool(
        description = "\
List markers across the project timeline as JSON. Includes clip-level OTIO \
markers, timeline-level metadata markers, and guide-track markers by default. \
Use `start_s`/`end_s` for a timeline window, `marker_id`/`label`/`category` \
for exact matching, and the include_* flags to limit scopes. The result is \
read-only and suitable for finding marker ids before UpdateMarker/DeleteMarker \
EDL operations.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_markers(
        &self,
        args: Parameters<ListMarkersArgs>,
    ) -> Result<String, ErrorData> {
        list_markers::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `view_episode` — compact textual episode map.
    #[tool(
        description = "\
Return a compact textual map of the episode: title, speaker count, \
topic list with timestamps, and the current editorial state (clip \
count, total duration, trimmed/untrimmed). The same map was injected \
into your system prompt at session start; call this tool to refresh \
it after edits or after a context compaction. No arguments. Cheap \
and read-only.",
        annotations(read_only_hint = true)
    )]
    pub async fn view_episode(
        &self,
        args: Parameters<ViewEpisodeArgs>,
    ) -> Result<String, ErrorData> {
        view_episode::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `list_looks` — agent-facing color-corrector look catalog.
    #[tool(
        description = "\
Returns the agent-facing catalog of named color-corrector looks. \
Each entry includes id, display_name, description, \
default_input_space, default_output_space, default_size, \
recommended_strength_min, recommended_strength_max, and tags. Use \
this before composing an `awidat.lut` or `awidat.color_pipeline` \
effect to pick a look that's compatible with the clip's input \
space and apply a sensible strength.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_looks(&self, args: Parameters<ListLooksArgs>) -> Result<String, ErrorData> {
        list_looks::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `list_stringouts` — enumerate named select-collections.
    #[tool(
        description = "\
List the project's named stringouts (ordered select-collections). \
Each stringout has a stable id, an optional display name, and a count \
of ordered select ids it references. Use `create_stringout` to add a \
new one without disturbing existing ones — projects support multiple \
stringouts in parallel (e.g. per arc, alt-cut, cold-open).",
        annotations(read_only_hint = true)
    )]
    pub async fn list_stringouts(
        &self,
        args: Parameters<ListStringoutsArgs>,
    ) -> Result<String, ErrorData> {
        list_stringouts::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `read_index` — read one footage-index channel for an asset.
    #[tool(
        description = "\
Read one channel of the footage index for an asset. Channels: \
'transcript' (whisper words+segments), 'scenes' (shot boundaries), \
'audio_levels' (LUFS + silences), 'beats' (tempo + beat times), 'topics' (topic segmentation), \
'editorial_moments' (typed edit beats), 'color' (per-frame color/exposure analysis), \
'clip' (CLIP embedding metadata), 'face', 'gaze', 'shot', 'composition', \
'frame_quality', 'summary' (one-line overview of all channels). Windowed channels accept \
offset+limit (default 0+50). Result is capped at 8KB; page via offset \
when truncated.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_index(&self, args: Parameters<ReadIndexArgs>) -> Result<String, ErrorData> {
        read_index::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `view_timeline` — windowed view of the project's OTIO timeline.
    #[tool(
        description = "\
Show clips in the project timeline within a time window. Each line is \
one clip/gap/transition: track-kind, timeline time range, duration, name, \
the exact `anchor=clip_uuid=<clip name>` value to use in apply_edl, \
current `source=[start..end]` bounds, and media reference. Transition \
lines show the visual range and the centered cut time (`cut=<seconds>`). For a user \
request like \"trim the first N seconds\" of an existing clip, set Trim \
Clip `start` to current source start + N; for \"trim the last N seconds\", \
set `end` to current source end - N. Default window 60s starting at 0. \
The header shows total timeline duration; the footer notes how many clips \
are out of cap. Use `start_s`/`end_s`/`lines` to navigate. Stateless across \
calls — pass `start_s` to scroll.",
        annotations(read_only_hint = true)
    )]
    pub async fn view_timeline(
        &self,
        args: Parameters<ViewTimelineArgs>,
    ) -> Result<String, ErrorData> {
        view_timeline::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `read_understanding` — fused scene/moment understanding +
    /// derived clip candidates.
    #[tool(
        description = "\
Read consolidated scene/moment understanding and derived short-form clip \
candidates without side effects. Fuses existing transcript, scene, topic, \
audio-energy, and editorial-moment sidecars, then returns reviewable clip \
candidates with scores, explanations, evidence ids, and one-click assembly \
metadata.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_understanding(
        &self,
        args: Parameters<ReadUnderstandingArgs>,
    ) -> Result<String, ErrorData> {
        read_understanding::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AwidatMcpServer {}
