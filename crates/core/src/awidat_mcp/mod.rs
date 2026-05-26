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
use crate::awidat_mcp::tools::color_scopes::{self, ColorScopesArgs};
use crate::awidat_mcp::tools::find_black_frames::{self, FindBlackFramesArgs};
use crate::awidat_mcp::tools::find_dead_air::{self, FindDeadAirArgs};
use crate::awidat_mcp::tools::find_false_starts::{self, FindFalseStartsArgs};
use crate::awidat_mcp::tools::list_looks::{self, ListLooksArgs};
use crate::awidat_mcp::tools::list_markers::{self, ListMarkersArgs};
use crate::awidat_mcp::tools::list_stringouts::{self, ListStringoutsArgs};
use crate::awidat_mcp::tools::local_review_package::{self, LocalReviewPackageArgs};
use crate::awidat_mcp::tools::read_broll_recommendations::{self, ReadBrollRecommendationsArgs};
use crate::awidat_mcp::tools::read_index::{self, ReadIndexArgs};
use crate::awidat_mcp::tools::read_media_intelligence::{self, ReadMediaIntelligenceArgs};
use crate::awidat_mcp::tools::read_media_readiness::{self, ReadMediaReadinessArgs};
use crate::awidat_mcp::tools::read_understanding::{self, ReadUnderstandingArgs};
use crate::awidat_mcp::tools::vedit_branch::{self, VeditBranchArgs};
use crate::awidat_mcp::tools::vedit_changed_clip_ids::{self, VeditChangedClipIdsArgs};
use crate::awidat_mcp::tools::vedit_checkout::{self, VeditCheckoutArgs};
use crate::awidat_mcp::tools::vedit_commit::{self, VeditCommitArgs};
use crate::awidat_mcp::tools::vedit_diff::{self, VeditDiffArgs};
use crate::awidat_mcp::tools::vedit_log::{self, VeditLogArgs};
use crate::awidat_mcp::tools::vedit_show::{self, VeditShowArgs};
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

    /// `read_broll_recommendations` — scored B-roll opportunities.
    #[tool(
        description = "\
Read scored B-roll recommendations derived from fused understanding without \
side effects. Returns category, confidence score, asset strategy, insertion \
plan, rationale, score breakdown, and source evidence ids for review.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_broll_recommendations(
        &self,
        args: Parameters<ReadBrollRecommendationsArgs>,
    ) -> Result<String, ErrorData> {
        read_broll_recommendations::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `read_media_intelligence` — progressive intelligence state.
    #[tool(
        description = "\
Read the progressive intelligence state machine for raw media assets without \
side effects. Returns independent layer readiness for source, proxy, waveform, \
transcript, speakers, scenes, topics, moments, clip candidates, and b-roll, \
plus aggregate state and next actions.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_media_intelligence(
        &self,
        args: Parameters<ReadMediaIntelligenceArgs>,
    ) -> Result<String, ErrorData> {
        read_media_intelligence::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `color_scopes` — luma/RGB histograms, waveform, RGB parade, and
    /// vectorscope evidence from one sampled video frame.
    #[tool(
        description = "\
Computes color scope evidence from one video frame: luma histogram, \
RGB histogram, luma waveform, RGB parade, and Cb/Cr vectorscope. Use \
this before or after applying color effects when you need objective \
frame-level exposure, channel balance, and chroma distribution evidence.",
        annotations(read_only_hint = true)
    )]
    pub async fn color_scopes(
        &self,
        args: Parameters<ColorScopesArgs>,
    ) -> Result<String, ErrorData> {
        color_scopes::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `author_local_review_package` — author a local review package
    /// from a rendered output path and the latest vedit commit.
    #[tool(
        description = "\
Author a local review package from a rendered output path. The package links that \
asset to the latest vedit commit, including commit header, commit hash, timeline hash, \
generated time, tags, and the commit reasoning body. The package is written as JSON \
under `<project>/.awidat/review-packages/` and returned as a JSON object.\
If you are handing off a review render to a collaborator, use this tool before \
you share the file manually; third-party review APIs are intentionally not part of \
this local-only flow.",
        annotations(read_only_hint = true)
    )]
    pub async fn author_local_review_package(
        &self,
        args: Parameters<LocalReviewPackageArgs>,
    ) -> Result<String, ErrorData> {
        local_review_package::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `read_media_readiness` — read-only media/cache/index readiness.
    #[tool(
        description = "\
Read media readiness for the current project without side effects. Returns \
source existence, playable artifact choice, proxy/thumbnail/waveform cache \
state, index sidecar availability, and recommended next actions so the agent \
knows whether it can rely on transcript, word timings, speaker labels, scenes, \
audio, visual evidence, or whether it should ask for repair/indexing first.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_media_readiness(
        &self,
        args: Parameters<ReadMediaReadinessArgs>,
    ) -> Result<String, ErrorData> {
        read_media_readiness::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_black_frames` — detect black-frame ranges in a source.
    #[tool(
        description = "\
Detect black-frame ranges in one project source asset using FFmpeg \
blackdetect. Use this as a quality/eval inspection tool after renders or \
before accepting suspicious cuts; it is read-only and returns source-time \
ranges with start_s, end_s, and duration_s.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_black_frames(
        &self,
        args: Parameters<FindBlackFramesArgs>,
    ) -> Result<String, ErrorData> {
        find_black_frames::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_dead_air` — silence findings on the timeline.
    #[tool(
        description = "\
Surface silence ranges (\"dead air\") on the project timeline as \
editorial findings. Reads the per-asset silence sidecars produced \
on import, intersects each silence range with the clip's current \
source_range on the timeline, and returns surviving silences plus \
surrounding transcript context. Each finding has: asset_id, \
source_start_s, source_end_s, timeline_start_s, timeline_end_s, \
duration_s, transcript_before, transcript_after. Default \
min_duration_s=1.5; max_results=20, hard cap 100. Returns empty \
when no silence sidecars exist.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_dead_air(
        &self,
        args: Parameters<FindDeadAirArgs>,
    ) -> Result<String, ErrorData> {
        find_dead_air::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_false_starts` — restart-marker / production-aside heuristic.
    #[tool(
        description = "\
Detect places where the speaker began a thought, abandoned it, and \
restarted, plus production/coaching asides such as \"cut\", \"one more \
time\", or \"you can just say\". v1 heuristic: scans the whisper transcript \
for restart markers and production-aside language, then surfaces the \
visible source fragment as the candidate false-start. Each finding: \
{ asset_id, marker, source_start_s, source_end_s, timeline_start_s, \
timeline_end_s, snippet }. Default max_results=20, hard cap 100. \
Treat findings as suggestions for user review — never auto-trim.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_false_starts(
        &self,
        args: Parameters<FindFalseStartsArgs>,
    ) -> Result<String, ErrorData> {
        find_false_starts::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_branch` — create or list local vedit branches.
    #[tool(
        description = "\
Create or list local vedit branches/alternates. A branch is a movable \
ref under `.vedit/refs/heads/` that can hold an alternate cut. Creating \
a branch does not switch HEAD or merge anything; use `vedit_checkout` \
to switch the working timeline to an existing branch.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_branch(
        &self,
        args: Parameters<VeditBranchArgs>,
    ) -> Result<String, ErrorData> {
        vedit_branch::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_changed_clip_ids` — clip names touched by a vedit diff.
    #[tool(
        description = "\
List the sorted clip names, media references, and clip animation targets \
touched by a vedit diff. Default: from='session-start', to='HEAD'. \
Read-only preflight for history review or future merge conflict checks; \
does not checkout, merge, or modify refs.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_changed_clip_ids(
        &self,
        args: Parameters<VeditChangedClipIdsArgs>,
    ) -> Result<String, ErrorData> {
        vedit_changed_clip_ids::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_checkout` — switch HEAD to an existing branch.
    #[tool(
        description = "\
Switch HEAD to an existing local vedit branch and restore \
`project.otio.json` to that branch's committed timeline snapshot. This \
is branch checkout for alternate cuts; it is not a merge and it does \
not create an audit commit by itself.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_checkout(
        &self,
        args: Parameters<VeditCheckoutArgs>,
    ) -> Result<String, ErrorData> {
        vedit_checkout::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_commit` — snapshot the timeline as a vedit commit.
    #[tool(
        description = "\
Snapshot the project's current `project.otio.json` as a vedit commit. \
Use this when the user asks to save this version, mark a checkpoint, or \
commit a session of work. The commit message format is canonical: a \
one-line imperative header plus an optional reasoning body. Returns \
the new commit hash + the timeline-content hash.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_commit(
        &self,
        args: Parameters<VeditCommitArgs>,
    ) -> Result<String, ErrorData> {
        vedit_commit::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_diff` — structured diff between two refs.
    #[tool(
        description = "\
Show the structured diff between two refs. Default: \
`from='session-start'`, `to='HEAD'` — i.e. everything that's changed \
since this session began. Returns: { from, to, change_count, \
structural_changes_empty, changes: [...] } as a list of structured \
operations (TrackAdded, Trimmed, Moved, Added, Removed, Replaced, \
TransitionAdded/Removed, EffectsChanged). Pass explicit refs to \
compare arbitrary points: branch names, full hashes, or short hashes \
(>= 4 hex chars).",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_diff(&self, args: Parameters<VeditDiffArgs>) -> Result<String, ErrorData> {
        vedit_diff::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_log` — recent vedit commits, newest-first.
    #[tool(
        description = "\
List recent vedit commits, newest-first. Each entry: { commit_hash, \
timeline_hash, timestamp, header, full_message, action_metadata, \
parents }. The header is the first line of the commit message; \
full_message is the body for deep dives. Default limit=30, hard cap \
200. Returns an empty entries list when the repo has no commits yet.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_log(&self, args: Parameters<VeditLogArgs>) -> Result<String, ErrorData> {
        vedit_log::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_show` — one vedit commit with diff from first parent.
    #[tool(
        description = "\
Show one vedit commit with its message, hashes, parents, and semantic \
diff from the first parent to that commit. Use this for deep-diving a \
history entry without listing the full log again.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_show(&self, args: Parameters<VeditShowArgs>) -> Result<String, ErrorData> {
        vedit_show::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AwidatMcpServer {}
