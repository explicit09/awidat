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
use crate::awidat_mcp::tools::analyze_sync::{self, AnalyzeSyncArgs};
use crate::awidat_mcp::tools::apply_edl::{self, ApplyEdlArgs};
use crate::awidat_mcp::tools::apply_episode_spans::{self, ApplyEpisodeSpansArgs};
use crate::awidat_mcp::tools::assess_continuity::{self, AssessContinuityArgs};
use crate::awidat_mcp::tools::assess_edit_quality::{self, AssessEditQualityArgs};
use crate::awidat_mcp::tools::attempt_completion::{self, AttemptCompletionArgs};
use crate::awidat_mcp::tools::broll_candidates::{self, BrollCandidatesArgs};
use crate::awidat_mcp::tools::clip_search::{self, ClipSearchArgs};
use crate::awidat_mcp::tools::color_scopes::{self, ColorScopesArgs};
use crate::awidat_mcp::tools::create_stringout::{self, CreateStringoutArgs};
use crate::awidat_mcp::tools::diagnose_project_media::{self, DiagnoseProjectMediaArgs};
use crate::awidat_mcp::tools::download_yt_clip::{self, DownloadYtClipArgs};
use crate::awidat_mcp::tools::export_package::{self, ExportPackageArgs};
use crate::awidat_mcp::tools::find_audio_asset::{self, FindAudioAssetArgs};
use crate::awidat_mcp::tools::find_beat::{self, FindBeatArgs};
use crate::awidat_mcp::tools::find_black_frames::{self, FindBlackFramesArgs};
use crate::awidat_mcp::tools::find_broll_opportunities::{self, FindBrollOpportunitiesArgs};
use crate::awidat_mcp::tools::find_dead_air::{self, FindDeadAirArgs};
use crate::awidat_mcp::tools::find_episode_start::{self, FindEpisodeStartArgs};
use crate::awidat_mcp::tools::find_eye_contact::{self, FindEyeContactArgs};
use crate::awidat_mcp::tools::find_false_starts::{self, FindFalseStartsArgs};
use crate::awidat_mcp::tools::find_filler_words::{self, FindFillerWordsArgs};
use crate::awidat_mcp::tools::find_generated_broll_opportunities::{
    self, FindGeneratedBrollOpportunitiesArgs,
};
use crate::awidat_mcp::tools::find_moment::{self, FindMomentArgs};
use crate::awidat_mcp::tools::find_speaker_oncam::{self, FindSpeakerOncamArgs};
use crate::awidat_mcp::tools::granular_timeline::{
    self, DeleteClipsArgs, MoveClipArgs, RippleTrimArgs, RollTrimArgs, SetClipPropertyArgs,
    SetMarkerArgs, SetTrackPropertyArgs, SlipClipArgs, SplitClipArgs, TrimClipArgs,
};
use crate::awidat_mcp::tools::import_media::{self, ImportLocalArgs, ImportUrlArgs};
use crate::awidat_mcp::tools::inspect_clip::{self, InspectClipArgs};
use crate::awidat_mcp::tools::inspect_moment::{self, InspectMomentArgs};
use crate::awidat_mcp::tools::list_assets::{self, ListAssetsArgs};
use crate::awidat_mcp::tools::list_bins::{self, ListBinsArgs};
use crate::awidat_mcp::tools::list_episodes::{self, ListEpisodesArgs};
use crate::awidat_mcp::tools::list_looks::{self, ListLooksArgs};
use crate::awidat_mcp::tools::list_markers::{self, ListMarkersArgs};
use crate::awidat_mcp::tools::list_stringouts::{self, ListStringoutsArgs};
use crate::awidat_mcp::tools::load_skill::{self, LoadSkillArgs};
use crate::awidat_mcp::tools::local_review_package::{self, LocalReviewPackageArgs};
use crate::awidat_mcp::tools::manage_assets::{
    self, CreateBinArgs, MarkSelectArgs, MoveToBinArgs, RateAssetArgs, RenameAssetArgs,
    TagAssetArgs,
};
use crate::awidat_mcp::tools::plan_captions::{self, PlanCaptionsArgs};
use crate::awidat_mcp::tools::plan_color_grade::{self, PlanColorGradeArgs};
use crate::awidat_mcp::tools::plan_emphasis::{self, PlanEmphasisArgs};
use crate::awidat_mcp::tools::plan_generated_media::{self, PlanGeneratedMediaArgs};
use crate::awidat_mcp::tools::plan_look_regions::{self, PlanLookRegionsArgs};
use crate::awidat_mcp::tools::plan_motion_scene::{self, PlanMotionSceneArgs};
use crate::awidat_mcp::tools::plan_multicam::{self, PlanMulticamArgs};
use crate::awidat_mcp::tools::plan_reframe::{self, PlanReframeArgs};
use crate::awidat_mcp::tools::plan_scene_aware_short_form::{self, PlanSceneAwareShortFormArgs};
use crate::awidat_mcp::tools::plan_short_form_review::{self, PlanShortFormReviewArgs};
use crate::awidat_mcp::tools::plan_speed_ramp::{self, PlanSpeedRampArgs};
use crate::awidat_mcp::tools::plan_transition::{self, PlanTransitionArgs};
use crate::awidat_mcp::tools::plan_visual_support::{self, PlanVisualSupportArgs};
use crate::awidat_mcp::tools::plan_visual_support_proposals::{
    self, PlanVisualSupportProposalArgs, ReviseVisualSupportProposalArgs,
    SaveVisualSupportDefaultsArgs, VerifyVisualSupportArtifactArgs,
};
use crate::awidat_mcp::tools::podcast_apply_accepted_edits::{self, PodcastApplyAcceptedEditsArgs};
use crate::awidat_mcp::tools::podcast_audio_polish::{self, PodcastAudioPolishArgs};
use crate::awidat_mcp::tools::podcast_cleanup_candidates::{self, PodcastCleanupCandidatesArgs};
use crate::awidat_mcp::tools::podcast_edit_proposal::{self, PodcastEditProposalArgs};
use crate::awidat_mcp::tools::podcast_editorial_review_pack::{
    self, PodcastEditorialReviewPackArgs,
};
use crate::awidat_mcp::tools::podcast_episode_spans::{self, PodcastEpisodeSpansArgs};
use crate::awidat_mcp::tools::podcast_post_draft_check::{self, PodcastPostDraftCheckArgs};
use crate::awidat_mcp::tools::podcast_qc_report::{self, PodcastQcReportArgs};
use crate::awidat_mcp::tools::podcast_smooth_cut_boundaries::{
    self, PodcastSmoothCutBoundariesArgs,
};
use crate::awidat_mcp::tools::podcast_story_map::{self, PodcastStoryMapArgs};
use crate::awidat_mcp::tools::podcast_visual_polish::{self, PodcastVisualPolishArgs};
use crate::awidat_mcp::tools::poll_generated_media_job::{self, PollGeneratedMediaJobArgs};
use crate::awidat_mcp::tools::poll_render::{self, PollRenderArgs};
use crate::awidat_mcp::tools::preview_cache::{self, PreviewCacheArgs};
use crate::awidat_mcp::tools::proxy_media::{self, GenerateProxyArgs};
use crate::awidat_mcp::tools::read_broll_recommendations::{self, ReadBrollRecommendationsArgs};
use crate::awidat_mcp::tools::read_index::{self, ReadIndexArgs};
use crate::awidat_mcp::tools::read_media_intelligence::{self, ReadMediaIntelligenceArgs};
use crate::awidat_mcp::tools::read_media_readiness::{self, ReadMediaReadinessArgs};
use crate::awidat_mcp::tools::read_understanding::{self, ReadUnderstandingArgs};
use crate::awidat_mcp::tools::relink_media::{self, RelinkMediaArgs};
use crate::awidat_mcp::tools::render_preflight::{self, RenderPreflightArgs};
use crate::awidat_mcp::tools::request_user_input::{self, RequestUserInputArgs};
use crate::awidat_mcp::tools::run_preview_cache_refresh::{self, RunPreviewCacheRefreshArgs};
use crate::awidat_mcp::tools::search_broll::{self, SearchBrollArgs};
use crate::awidat_mcp::tools::shot_summary::{self, ShotSummaryArgs};
use crate::awidat_mcp::tools::start_generated_media_job::{self, StartGeneratedMediaJobArgs};
use crate::awidat_mcp::tools::start_indexing::{self, StartIndexingArgs};
use crate::awidat_mcp::tools::start_render::{self, StartRenderArgs};
use crate::awidat_mcp::tools::stream_remux::{self, StreamRemuxArgs};
use crate::awidat_mcp::tools::transcript_pack::{self, TranscriptPackArgs};
use crate::awidat_mcp::tools::transcript_search::{self, TranscriptSearchArgs};
use crate::awidat_mcp::tools::transition_context::{self, TransitionContextArgs};
use crate::awidat_mcp::tools::update_plan::{self, UpdatePlanArgs};
use crate::awidat_mcp::tools::use_broll::{self, UseBrollArgs};
use crate::awidat_mcp::tools::use_generated_media::{self, UseGeneratedMediaArgs};
use crate::awidat_mcp::tools::validate_transition_choice::{self, ValidateTransitionChoiceArgs};
use crate::awidat_mcp::tools::vedit_blame::{self, VeditBlameArgs};
use crate::awidat_mcp::tools::vedit_branch::{self, VeditBranchArgs};
use crate::awidat_mcp::tools::vedit_changed_clip_ids::{self, VeditChangedClipIdsArgs};
use crate::awidat_mcp::tools::vedit_checkout::{self, VeditCheckoutArgs};
use crate::awidat_mcp::tools::vedit_commit::{self, VeditCommitArgs};
use crate::awidat_mcp::tools::vedit_diff::{self, VeditDiffArgs};
use crate::awidat_mcp::tools::vedit_log::{self, VeditLogArgs};
use crate::awidat_mcp::tools::vedit_merge::{self, VeditMergeArgs};
use crate::awidat_mcp::tools::vedit_merge_preflight::{self, VeditMergePreflightArgs};
use crate::awidat_mcp::tools::vedit_revert::{self, VeditRevertArgs};
use crate::awidat_mcp::tools::vedit_show::{self, VeditShowArgs};
use crate::awidat_mcp::tools::vedit_tag::{self, VeditTagArgs};
use crate::awidat_mcp::tools::verify_render::{self, VerifyRenderArgs};
use crate::awidat_mcp::tools::view_episode::{self, ViewEpisodeArgs};
use crate::awidat_mcp::tools::view_frame::{self, ViewFrameArgs};
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

    /// `list_episodes` — read durable episode span metadata.
    #[tool(
        description = "\
List durable episode spans stored in Timeline.metadata.awidat.episodes as \
JSON. Each episode includes id, optional name/order, source asset id, source \
start/end/duration, confidence, review status, and evidence. Use this after \
podcast_episode_spans/apply_episode_spans to inspect accepted, review-needed, \
and rejected spans without mutating the project.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_episodes(
        &self,
        args: Parameters<ListEpisodesArgs>,
    ) -> Result<String, ErrorData> {
        list_episodes::run(args.0, McpToolCtx::resolve())
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
        annotations(destructive_hint = true, read_only_hint = false)
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
        annotations(destructive_hint = true, read_only_hint = false)
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
        annotations(destructive_hint = true, read_only_hint = false)
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
        annotations(destructive_hint = true, read_only_hint = false)
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

    /// `vedit_blame` — project vedit history onto one clip.
    #[tool(
        description = "\
Project vedit history onto one clip. Walks first-parent history from \
HEAD (or start_ref), computes each commit's semantic diff, and returns \
commits whose changes touch the supplied clip name or media reference. \
This is attribution, not a branch checkout or merge operation.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_blame(&self, args: Parameters<VeditBlameArgs>) -> Result<String, ErrorData> {
        vedit_blame::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_merge_preflight` — check branch merge overlap without
    /// merging.
    #[tool(
        description = "\
Check whether a source ref can be safely merged into a target ref under \
Awidat's proposed bounded merge rule: both sides must have changed \
non-overlapping clip/media identifiers since their common ancestor. \
This tool is read-only; it does not checkout, merge, resolve conflicts, \
or modify refs.",
        annotations(read_only_hint = true)
    )]
    pub async fn vedit_merge_preflight(
        &self,
        args: Parameters<VeditMergePreflightArgs>,
    ) -> Result<String, ErrorData> {
        vedit_merge_preflight::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_tag` — create or list named local vedit checkpoints.
    #[tool(
        description = "\
Create or list local named vedit checkpoints. Tags are stable labels \
under `.vedit/refs/tags/` that point at commits. They do not switch \
HEAD, create branches, or merge anything. Use this for names like \
`client-review-v1` or `before-tightening-pass`.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn vedit_tag(&self, args: Parameters<VeditTagArgs>) -> Result<String, ErrorData> {
        vedit_tag::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `transcript_pack` — compact transcript evidence for planning.
    #[tool(
        description = "\
Build a compact transcript pack from whisper sidecars for speech-led \
cut, caption, and cleanup planning. Defaults to assets visible in the \
current timeline; set include_all_assets=true to pack every whisper \
sidecar instead. Read-only.",
        annotations(read_only_hint = true)
    )]
    pub async fn transcript_pack(
        &self,
        args: Parameters<TranscriptPackArgs>,
    ) -> Result<String, ErrorData> {
        transcript_pack::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_audio_asset` — ranked matches from the bundled audio
    /// starter pack.
    #[tool(
        description = "\
Search the bundled audio starter pack for a sound to drop on the \
timeline. Returns a ranked list of matching audio files (sfx, music, \
or ambience) with absolute paths suitable for apply_edl / ffmpeg. \
Args: kind (required, one of sfx/music/ambience), mood (optional \
free-text tag like 'hype' or 'tension'), max_duration_s (optional \
upper bound), max_results (default 8, hard cap 32). Results are \
ranked by mood-tag overlap first, then by duration ascending. When \
the pack is empty or absent, returns an empty results list (not an \
error).",
        annotations(read_only_hint = true)
    )]
    pub async fn find_audio_asset(
        &self,
        args: Parameters<FindAudioAssetArgs>,
    ) -> Result<String, ErrorData> {
        find_audio_asset::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_beat` — query the editorial-moments sidecar by kind /
    /// speaker / score.
    #[tool(
        description = "\
Query the editorial-moments index by beat kind, speaker, and minimum \
editorial score. Returns beats sorted by score (highest first) with \
moment_id, time range, kind, speaker, energy, b-roll need, \
dependencies, and the model's note for why each is editorially \
interesting. Kinds: hook, story, punchline, setup, question, answer, \
cta, emotional_peak, dead_air, tangent, explanation. The \
editorial-moments index is produced by `awidat index --indexer \
editorial-moments`; if find_beat returns empty, the index hasn't run \
yet.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_beat(&self, args: Parameters<FindBeatArgs>) -> Result<String, ErrorData> {
        find_beat::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_filler_words` — scan whisper transcripts for verbal
    /// fillers visible on the timeline.
    #[tool(
        description = "\
Scan the project's whisper transcripts for filler words (\"um\", \
\"uh\", etc) that the agent could suggest cutting. Each finding is a \
single word's span: { asset_id, text, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s }. Default filler list: \
um/uh/uhh/umm/ah/ahh/er/err. Pass `aggressive: true` to also include \
discourse markers (like / so / just / yeah / basically / you know / \
i mean). Pass `fillers: [...]` to override entirely. Findings are \
intersected with the timeline's clip ranges. Default max_results=30, \
hard cap 200. Returns an empty findings list when whisper sidecars \
haven't landed yet.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_filler_words(
        &self,
        args: Parameters<FindFillerWordsArgs>,
    ) -> Result<String, ErrorData> {
        find_filler_words::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_generated_broll_opportunities` — transcript moments
    /// suited for AI-generated B-roll.
    #[tool(
        description = "\
Find transcript moments where AI-generated podcast B-roll would help: \
visual concepts, explanations, abstract-to-concrete moments, emotional \
spikes, story reconstruction, and statistics. This is read-only and \
does not call a generation provider. Returns scored candidates with \
OpenRouter/Seedance-ready prompts so the agent can review the moment \
before calling `start_generated_media_job`.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_generated_broll_opportunities(
        &self,
        args: Parameters<FindGeneratedBrollOpportunitiesArgs>,
    ) -> Result<String, ErrorData> {
        find_generated_broll_opportunities::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `list_assets` — paginated listing of project assets.
    #[tool(
        description = "\
List source assets and renders in the project. Returns a numbered, \
paginated list with file sizes. Scope: 'raw' (source media), 'renders' \
(engine outputs), 'all' (both, default). 1-indexed offset, default \
limit 25, hard cap 100. Use this to discover what's available before \
calling `inspect_clip` or `read_index` on a specific asset.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_assets(&self, args: Parameters<ListAssetsArgs>) -> Result<String, ErrorData> {
        list_assets::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `list_bins` — enumerate available asset bins for the project.
    #[tool(
        description = "\
List the available asset bins in this project. Returns built-in role \
buckets (kind=role, ids like 'role:video', 'role:audio') plus any \
user/agent-defined bins (kind=user). Pass any returned id as the \
`bin` argument to `list_assets` to filter that surface.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_bins(&self, args: Parameters<ListBinsArgs>) -> Result<String, ErrorData> {
        list_bins::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `inspect_clip` — one-page asset metadata from sidecars.
    #[tool(
        description = "\
Get a one-page metadata summary for an asset: duration, frame_rate, audio \
sample rate, loudness, language, speakers, shot count, segment count. \
Aggregates from whichever indexer sidecars exist (whisper / scenedetect / \
audio-energy). Use this before deciding which sidecar to read in detail \
via `read_index`.",
        annotations(read_only_hint = true)
    )]
    pub async fn inspect_clip(
        &self,
        args: Parameters<InspectClipArgs>,
    ) -> Result<String, ErrorData> {
        inspect_clip::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `inspect_moment` — drill into one editorial beat.
    #[tool(
        description = "\
Drill into one editorial beat from the editorial-moments index. \
Returns: the full moment record, ±N seconds of surrounding transcript \
(default 10s), and any `dependencies` moments expanded inline so you \
can read their notes without a second tool call. Use after find_beat \
narrows to a candidate. The transcript window gives you the actual \
phrases to anchor against in apply_edl. The dependencies tell you \
what setup must stay intact when cutting this beat standalone.",
        annotations(read_only_hint = true)
    )]
    pub async fn inspect_moment(
        &self,
        args: Parameters<InspectMomentArgs>,
    ) -> Result<String, ErrorData> {
        inspect_moment::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_emphasis` — read-only single-clip emphasis-motion planner.
    #[tool(
        description = "\
Read-only emphasis planner. Given a clip id, optional beat times, visual \
context, and an LLM-friendly style word, returns the strongest executable \
single-clip parameter animation plus a parseable Set Parameter Animation EDL \
fragment. Use it when the job is emphasis inside a clip, not a cut boundary.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_emphasis(
        &self,
        args: Parameters<PlanEmphasisArgs>,
    ) -> Result<String, ErrorData> {
        plan_emphasis::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_motion_scene` — read-only native MotionScene planner.
    #[tool(
        description = "\
Read-only planner for native procedural MotionScene documents. Use after \
plan_visual_support chooses the motion_scene lane. It returns a valid \
MotionScene plus a Set Motion Scene EDL snippet for apply_edl. Text layers \
text, rectangle/solid, and project-asset image layers are preview/render \
supported; video/media layers are stored with explicit limitations and \
footage should use B-roll/PiP.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_motion_scene(
        &self,
        args: Parameters<PlanMotionSceneArgs>,
    ) -> Result<String, ErrorData> {
        plan_motion_scene::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_reframe` — read-only static reframe planner.
    #[tool(
        description = "\
Plan a static subject-aware reframe for vertical, square, or social \
delivery. Returns an EDL fragment that sets the `awidat.reframe` clip \
effect; the agent must pass that fragment to `apply_edl` to mutate the \
timeline. Use this after `find_speaker_oncam`, gaze/face evidence, or \
manual subject-center evidence identifies where the important subject sits \
in frame.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_reframe(
        &self,
        args: Parameters<PlanReframeArgs>,
    ) -> Result<String, ErrorData> {
        plan_reframe::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_transition` — read-only transition planner.
    #[tool(
        description = "\
Read-only transition planner. Pass the JSON object returned by \
transition_context. The tool recommends either a hard cut with Set Cut \
Intent metadata or one supported visible transition with a named job, safe \
duration, reason, alternate, and EDL fragment. It never applies the edit.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_transition(
        &self,
        args: Parameters<PlanTransitionArgs>,
    ) -> Result<String, ErrorData> {
        plan_transition::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_broll_opportunities` — transcript moments suited for stock
    /// (Pexels) B-roll cutaways.
    #[tool(
        description = "\
Surface moments where stock B-roll (from Pexels) would land well, by \
scanning whisper transcripts for trigger phrases (\"look at\", \"imagine \
a\", \"picture this\") followed by a concrete visual noun (\"skyline\", \
\"office\", \"graph\"). Returns findings with asset_id, source_start_s, \
source_end_s, timeline_start_s, timeline_end_s, reason, pexels_query, \
and transcript_excerpt. Distinct from `broll_candidates` (in-footage \
cutaways). Default max_results=12, hard cap 40. Empty findings means \
whisper hasn't landed yet OR the transcript has no demonstrative \
phrases — neither is an error.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_broll_opportunities(
        &self,
        args: Parameters<FindBrollOpportunitiesArgs>,
    ) -> Result<String, ErrorData> {
        find_broll_opportunities::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_episode_start` — publishable start of a podcast/interview
    /// episode, rejecting pre-roll and rehearsal intros.
    #[tool(
        description = "\
Find the actual publishable start of a podcast/interview episode by \
scanning the whisper transcript for clean host-intro cues and rejecting \
pre-roll, off-camera setup, and rehearsal intros. Use this before \
trimming the top of a podcast or answering 'what time does the episode \
start?'. It is safer than reading transcript offset 0 because raw \
recordings often begin with real but unpublished chatter. Returns a \
recommended start time, confidence, evidence transcript, and rejected \
candidates. Requires the whisper transcript index.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_episode_start(
        &self,
        args: Parameters<FindEpisodeStartArgs>,
    ) -> Result<String, ErrorData> {
        find_episode_start::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_eye_contact` — time ranges where a face is looking at the
    /// camera (direct address).
    #[tool(
        description = "\
Find time ranges where at least one face on screen is looking at the \
camera (gaze score within the at-camera threshold). Reads the gaze \
sidecar; optionally filters by speaker via the face sidecar's \
speaker→face mapping. Use this for moments of direct address — host \
breaking the fourth wall, guest delivering a punchline to camera, the \
closing pitch. Args: asset_id (optional restrict), min_duration_s \
(default 1.0), speaker (optional), limit (default 50, hard cap 200). \
Requires the gaze indexer; the speaker filter further requires the face \
indexer + whisper diarization.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_eye_contact(
        &self,
        args: Parameters<FindEyeContactArgs>,
    ) -> Result<String, ErrorData> {
        find_eye_contact::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_moment` — BM25-ranked search over whisper transcript
    /// segments.
    #[tool(
        description = "\
BM25-ranked search across whisper transcript segments. Tokenizes the \
query and ranks every segment by relevance — exact-substring queries \
still match (they rank highest), and semantically-related queries \
that share no substring (e.g. `battery problems` → `Note 7 battery \
exploded`) now find their target. Returns asset_id + start/end \
timestamps + speaker_id + a 200-char snippet + a relevance score per \
match. Returns paths/ranges only, no audio or video — follow up with \
`view_frame` for visuals or `read_index` for fuller context. Default \
limit 25, hard cap 100. Results are ordered by score (best first). \
English stopwords are filtered before ranking — use content words.",
        annotations(read_only_hint = true)
    )]
    pub async fn find_moment(&self, args: Parameters<FindMomentArgs>) -> Result<String, ErrorData> {
        find_moment::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `assess_continuity` — read-only continuity assessor.
    #[tool(
        description = "\
Evaluate whether a proposed cut/trim/split would jar the viewer. Reads \
whisper / silence / motion / scenedetect sidecars and runs five rules: \
mid-sentence, breath-beat preservation, mid-motion, speaker-turn \
boundary, rhythm preservation. Returns a per-rule breakdown plus an \
aggregate verdict (clean / risky / dirty / abstain). Args: at_s \
(timeline-time seconds), kind (cut / trim_in / trim_out), asset_id \
(optional — auto-resolved from the timeline). Read-only; applying the \
edit is a separate apply_edl step.",
        annotations(read_only_hint = true)
    )]
    pub async fn assess_continuity(
        &self,
        args: Parameters<AssessContinuityArgs>,
    ) -> Result<String, ErrorData> {
        assess_continuity::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `assess_edit_quality` — editorial-grammar recommendation above
    /// the continuity verdict.
    #[tool(
        description = "\
Assess the editorial quality of a candidate cut/trim/split. Wraps \
assess_continuity and returns a recommendation using editing grammar: \
hard cut, recut, cut on action, J-cut, L-cut, b-roll cover, or a \
motivated transition. Read-only; applying the edit is a separate \
apply_edl step. Use this before fixing dirty cuts so cross dissolves \
are not the default repair.",
        annotations(read_only_hint = true)
    )]
    pub async fn assess_edit_quality(
        &self,
        args: Parameters<AssessEditQualityArgs>,
    ) -> Result<String, ErrorData> {
        assess_edit_quality::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `find_speaker_oncam` — time ranges where a speaker's face is on
    /// screen.
    #[tool(
        description = "\
Find time ranges where a given speaker's face is on screen. Reads the \
face indexer's per_frame data and the speaker→face_id mapping that \
gets populated when whisper diarization ran before face-mcp. Use this \
for editorial decisions about reaction shots, B-roll overlay timing, \
and direct-address sequences. If the speaker→face mapping isn't \
populated, the tool returns a hint explaining why (usually whisper \
diarization didn't run).",
        annotations(read_only_hint = true)
    )]
    pub async fn find_speaker_oncam(
        &self,
        args: Parameters<FindSpeakerOncamArgs>,
    ) -> Result<String, ErrorData> {
        find_speaker_oncam::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `transcript_search` — substring search over whisper sidecars.
    #[tool(
        description = "\
Search whisper transcript segments across project media, optionally \
filtering by asset or speaker. Returns matching segments ranked by \
match count, with start/end times, speaker, and a truncated text \
preview. Read-only.",
        annotations(read_only_hint = true)
    )]
    pub async fn transcript_search(
        &self,
        args: Parameters<TranscriptSearchArgs>,
    ) -> Result<String, ErrorData> {
        transcript_search::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_generated_media` — read-only generated-media prompt planner.
    #[tool(
        description = "\
Read-only planner for generated b-roll. Returns deterministic prompt \
candidates and provider/mode options without writing the registry.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_generated_media(
        &self,
        args: Parameters<PlanGeneratedMediaArgs>,
    ) -> Result<String, ErrorData> {
        plan_generated_media::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_multicam` — read-only flattened multicam program planner.
    #[tool(
        description = "\
Create a reviewable N-camera podcast direction plan. The tool uses \
diarized transcript segments plus face speaker mapping, shot type, and \
frame quality sidecars when present. It returns flattened Program Video \
decisions with source_asset and reason metadata; it does not create OTIO \
multicam stacks.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_multicam(
        &self,
        args: Parameters<PlanMulticamArgs>,
    ) -> Result<String, ErrorData> {
        plan_multicam::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_scene_aware_short_form` — read-only scene-aware short-form
    /// planner.
    #[tool(
        description = "\
Build a read-only scene-aware short-form edit plan for one candidate clip. \
The tool uses existing Awidat evidence sidecars when available: transcript, \
word timings, topics, editorial moments, audio energy, face/gaze, scene and \
shot detection, frame quality, composition, and CLIP metadata. It analyzes \
shot layout, caption safety, negative space, motion intensity, weak visuals, \
and semantic support-visual opportunities, then returns structured \
recommendations with transcript, visual, pacing, safety, and confidence \
reasons plus an EDL fragment. The returned EDL is reviewable and should be \
applied separately with apply_edl only after inspection.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_scene_aware_short_form(
        &self,
        args: Parameters<PlanSceneAwareShortFormArgs>,
    ) -> Result<String, ErrorData> {
        plan_scene_aware_short_form::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_captions` — format-aware, read-only caption planner (long_form|accessibility).
    #[tool(
        description = "\
Build a read-only, format-aware caption plan for one clip from its transcript \
index. Supports long_form and accessibility formats only (use \
plan_scene_aware_short_form for vertical short-form). Segments transcript words \
to a <=17 CPS reading ceiling with per-format characters-per-line targets, \
applies a (format, mood) style, and returns caption recommendations, a \
readability lint, and a reviewable Insert Caption EDL fragment. Pass the \
optional `preset` field (values: clean_white | word_pop | boxed) to override \
the (format, mood) style with a named preset. Note: accessibility uses \
whole-cue reveal regardless of mood. Apply with apply_edl after inspection. \
Never burns captions into the picture.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_captions(
        &self,
        args: Parameters<PlanCaptionsArgs>,
    ) -> Result<String, ErrorData> {
        plan_captions::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_short_form_review` — read-only long-form to short-form
    /// review planner.
    #[tool(
        description = "\
Build a read-only long-form to short-form review plan for one asset. \
The tool ranks complete standalone candidate moments, allows extended \
short-form up to five minutes when the idea earns it, recommends B-roll \
by default when support visuals clarify the idea, plans speaker-aware \
9:16 layouts for wide long-form sources, and returns reviewable draft EDL \
packages plus title, caption, platform, confidence, and human review \
actions. It does not apply edits; use apply_edl separately after review so \
autopilot/co-pilot/manual approval behavior remains in control.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_short_form_review(
        &self,
        args: Parameters<PlanShortFormReviewArgs>,
    ) -> Result<String, ErrorData> {
        plan_short_form_review::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `shot_summary` — compact roll-up of an asset's visual
    /// structure.
    #[tool(
        description = "\
Compact descriptive summary of an episode's visual structure: shot \
count, mean shot length, histograms over shot type \
(close-up/medium/wide/no-face) and motion (static/slow-pan/handheld/\
fast-cut). Reads the `shot` indexer's sidecar. Use this to orient \
yourself to a new asset — the answer to 'what does this video look \
like, structurally?' before deciding whether to call \
broll_candidates, find_speaker_oncam, or just inspect a few clips.",
        annotations(read_only_hint = true)
    )]
    pub async fn shot_summary(
        &self,
        args: Parameters<ShotSummaryArgs>,
    ) -> Result<String, ErrorData> {
        shot_summary::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `transition_context` — read-only transition boundary context
    /// packet for one adjacent timeline boundary.
    #[tool(
        description = "\
Build a read-only transition decision context packet for one adjacent \
timeline boundary. Returns adjacent clip metadata, timeline/source ranges, \
transition handle availability, continuity verdict, transcript snippets, \
suggested frame timestamps, per-side motion magnitudes and screen \
directions (outgoing/incoming), motion-match classification \
(aligned/opposed/orthogonal/unknown), and missing-signal names. This tool \
does not choose or apply a transition; use it before deciding whether a \
hard cut, semantic cut, split edit, b-roll cover, or visible transition \
is warranted.",
        annotations(read_only_hint = true)
    )]
    pub async fn transition_context(
        &self,
        args: Parameters<TransitionContextArgs>,
    ) -> Result<String, ErrorData> {
        transition_context::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `validate_transition_choice` — post-application motion
    /// validator for motion-sensitive transitions.
    #[tool(
        description = "\
After applying a motion-sensitive transition (whip_pan_*, pass_by_*, \
motion_blur, slide_*, zoom_in, etc.), call this to verify the chosen \
direction matches the source clips' measured motion. Returns the \
transition's predicted direction, the measured directions from each \
side's shot sidecar, a boolean motion_match, and an editorial verdict \
('acceptable' / 'wrong_direction' / 'no_signal'). This is the \
closed-loop check that lets future plan_transition calls weight \
direction predictions more carefully.",
        annotations(read_only_hint = true)
    )]
    pub async fn validate_transition_choice(
        &self,
        args: Parameters<ValidateTransitionChoiceArgs>,
    ) -> Result<String, ErrorData> {
        validate_transition_choice::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `view_frame` — extract one frame from an asset and return it
    /// as a JSON payload with a base64-encoded image.
    #[tool(
        description = "\
Extract a single frame from a video asset at time `t_s` and return it \
as a JSON payload carrying base64-encoded image bytes plus a textual \
summary. Use this to *see* a moment — for example, to confirm a cut \
lands on the right shot, or to read text on screen. detail='preview' \
(default, <=768px longest edge) keeps the image cheap; \
detail='original' returns source resolution. format='png' (default) | \
'jpeg'. Frames are cached under .awidat/cache/frames/ keyed by \
(asset, time, format, dim, grade).",
        annotations(read_only_hint = true)
    )]
    pub async fn view_frame(&self, args: Parameters<ViewFrameArgs>) -> Result<String, ErrorData> {
        view_frame::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_qc_report` — pre-render podcast QC gate.
    #[tool(
        description = "\
Run pre-render podcast QC for gaps, missing media, captions, audio readiness, \
and suspicious timeline structure. Returns a status (ready / needs_review / \
blocked), an `issues` list with severity-tagged findings (missing_media, \
inactive_clip, timeline_gap, primary_av_duration_mismatch, caption_warning, \
audio_metering_missing, cut_intent_missing), plus a caption summary and audio \
meter count. Read-only; the gate is informational and does not block the \
render by itself.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_qc_report(
        &self,
        args: Parameters<PodcastQcReportArgs>,
    ) -> Result<String, ErrorData> {
        podcast_qc_report::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_editorial_review_pack` — compact AI editorial evidence
    /// packets for podcast cleanup and episode-shape decisions.
    #[tool(
        description = "\
Build a compact AI editorial review pack for podcast cleanup and \
episode-shape decisions. The tool collects timeline-visible recall \
signals such as false starts, production/coaching asides, and optional \
silence ranges, then adds before/during/after transcript context and \
an explicit classification schema. It is read-only and does not label \
anything as a final cut. The active agent must classify each packet \
as cut/keep/review before calling proposal or mutation tools.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_editorial_review_pack(
        &self,
        args: Parameters<PodcastEditorialReviewPackArgs>,
    ) -> Result<String, ErrorData> {
        podcast_editorial_review_pack::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_episode_spans` — plan candidate episode spans from
    /// existing transcript/audio/topic evidence.
    #[tool(
        description = "\
Plan candidate episode spans from existing transcript/audio/topic evidence. \
Wraps the bundled auto-cutter episode_span_plan.py script; returns \
recommended start/end spans and whether multiple high-confidence episodes \
require the user to choose before extraction or cleanup.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_episode_spans(
        &self,
        args: Parameters<PodcastEpisodeSpansArgs>,
    ) -> Result<String, ErrorData> {
        podcast_episode_spans::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_story_map` — tolerant first-watch story pass that
    /// surfaces strong beats, low-value spans, and production chatter.
    #[tool(
        description = "\
Build a first-watch podcast story map from indexed editorial moments and \
transcript sidecars. Returns hook/story/emotional/punchline/CTA candidates, \
tangent/dead-air candidates, and transcript spans that look like production \
or meta-direction chatter inside the interview. This tool is tolerant of \
missing indexes: it reports missing evidence instead of failing the workflow.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_story_map(
        &self,
        args: Parameters<PodcastStoryMapArgs>,
    ) -> Result<String, ErrorData> {
        podcast_story_map::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_audio_polish` — podcast audio finishing readiness gate.
    #[tool(
        description = "\
Check podcast audio finishing readiness: loudness, clipping, noise, buses, \
and recommended mix processors. Returns a status (ready / needs_review / \
needs_fix), issues by severity, the derived audio finishing state, and \
recommended mix-chain steps. Read-only; reports against current timeline \
metadata.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_audio_polish(
        &self,
        args: Parameters<PodcastAudioPolishArgs>,
    ) -> Result<String, ErrorData> {
        podcast_audio_polish::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_cleanup_candidates` — aggregate existing cleanup
    /// evidence into safe/review/risky candidate buckets.
    #[tool(
        description = "\
Aggregate existing podcast cleanup evidence into safe/review/risky \
candidate buckets. Uses current dead-air, filler-word, and false-start \
scanners; it does not mutate the timeline and does not require a new \
audio-analysis indexer.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_cleanup_candidates(
        &self,
        args: Parameters<PodcastCleanupCandidatesArgs>,
    ) -> Result<String, ErrorData> {
        podcast_cleanup_candidates::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_post_draft_check` — verify draft episode boundaries
    /// for leftover pre-show, retake, or tail chatter.
    #[tool(
        description = "\
Check the current draft timeline's opening and ending transcript windows for \
leftover pre-show, retake, or post-show chatter before render. Run after the \
agent has made an extraction/cleanup draft and before final render. The tool \
does not mutate the timeline.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_post_draft_check(
        &self,
        args: Parameters<PodcastPostDraftCheckArgs>,
    ) -> Result<String, ErrorData> {
        podcast_post_draft_check::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_smooth_cut_boundaries` — inspect cut boundaries created
    /// by accepted podcast edits and emit the follow-up tool calls
    /// needed to smooth them.
    #[tool(
        description = "\
Locate adjacent clip boundaries created by accepted podcast cuts and return \
the required smoothing checks. This tool is read-only. Run it after \
podcast_apply_accepted_edits -> apply_edl -> view_timeline -> vedit_diff, \
then run assess_edit_quality for each returned boundary. Use transition_context \
and plan_transition only when the quality assessment indicates a visual repair \
or motivated transition may be needed.",
        annotations(read_only_hint = true)
    )]
    pub async fn podcast_smooth_cut_boundaries(
        &self,
        args: Parameters<PodcastSmoothCutBoundariesArgs>,
    ) -> Result<String, ErrorData> {
        podcast_smooth_cut_boundaries::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `analyze_sync` — waveform-align external mics and cameras.
    #[tool(
        description = "\
Analyze waveform sync between a reference asset and candidate camera/mic \
assets. Uses FFmpeg waveform extraction and cross-correlation, reports \
offset_s, confidence, optional speed_factor for small stable drift, and \
an EDL Set Sync Group snippet. Low-confidence results are surfaced as \
manual offset proposals instead of silently committing.",
        annotations(read_only_hint = true)
    )]
    pub async fn analyze_sync(
        &self,
        args: Parameters<AnalyzeSyncArgs>,
    ) -> Result<String, ErrorData> {
        analyze_sync::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_visual_support` — read-only router for visual-support
    /// requests.
    #[tool(
        description = "\
Read-only visual-support router. Given a user request or agent-detected \
visual need, choose the smallest useful lane: timeline edit, b-roll, \
motion scene, title/annotation, or effects/finishing. Use before choosing \
between b-roll, generated video, freeform motion graphics, and direct \
FFmpeg/Rust render primitives.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_visual_support(
        &self,
        args: Parameters<PlanVisualSupportArgs>,
    ) -> Result<String, ErrorData> {
        plan_visual_support::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_visual_support_proposals` — return reviewable visual artifact
    /// proposals for a selected transcript/topic/timeline region.
    #[tool(
        description = "\
Read-only Proposal-to-Visual-Support planner. Given selected transcript/topic \
text and an editor request, returns visual artifact proposals with evidence, \
missing-information prompts, apply_edl payloads, preview expectations, and \
render-verification steps. Use before apply_edl for quote highlights, animated \
lists, title cards, search bars, counters, maps, and B-roll packages.",
        annotations(read_only_hint = true)
    )]
    pub async fn plan_visual_support_proposals(
        &self,
        args: Parameters<PlanVisualSupportProposalArgs>,
    ) -> Result<String, ErrorData> {
        plan_visual_support_proposals::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `revise_visual_support_proposal` — revise a pending visual-support
    /// proposal from a natural-language instruction and return a diff.
    #[tool(
        description = "\
Read-only Proposal-to-Visual-Support revision tool. Given one proposal returned \
by plan_visual_support_proposals and a natural-language instruction, returns a \
revised proposal plus a compact diff for review before apply_edl. Use for changes \
such as shorter, faster, transparent background, or alpha intent.",
        annotations(read_only_hint = true)
    )]
    pub async fn revise_visual_support_proposal(
        &self,
        args: Parameters<ReviseVisualSupportProposalArgs>,
    ) -> Result<String, ErrorData> {
        plan_visual_support_proposals::run_revision(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `verify_visual_support_artifact` — run proposal-level
    /// artifact-specific verification before or after render verification.
    #[tool(
        description = "\
Read-only artifact-specific verifier for proposals returned by \
plan_visual_support_proposals. Checks quote highlights, animated lists, and \
B-roll packages against transcript evidence, MotionScene/EDL payloads, and \
optionally project-local B-roll asset existence. Pair this with verify_render \
after rendering.",
        annotations(read_only_hint = true)
    )]
    pub async fn verify_visual_support_artifact(
        &self,
        args: Parameters<VerifyVisualSupportArtifactArgs>,
    ) -> Result<String, ErrorData> {
        plan_visual_support_proposals::run_artifact_verification(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `save_visual_support_defaults` — persist project style/export defaults
    /// learned from visual-support clarification answers.
    #[tool(
        description = "\
Persist project-local visual-support defaults learned from editor clarification \
answers. Saves preferred aspect ratio, platform, alpha/transparent-background \
intent, and reusable reference assets to .awidat/visual_support_defaults.json. \
Later plan_visual_support_proposals calls reuse these values when arguments omit \
them.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn save_visual_support_defaults(
        &self,
        args: Parameters<SaveVisualSupportDefaultsArgs>,
    ) -> Result<String, ErrorData> {
        plan_visual_support_proposals::save_visual_support_defaults(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_look_regions` — generate a look-region/LUT plan and
    /// project-local LUTs from color sidecars.
    ///
    /// Marked destructive because the run writes `renders/<stem>.edl`,
    /// `renders/<stem>.json`, `renders/<stem>.md`, and generated
    /// `.cube` LUTs under `luts/generated/`. The original
    /// `ToolHandler` had `is_mutating = true` for the same reason.
    #[tool(
        description = "\
Create a graph-native look-region/LUT plan from the current timeline and \
color-analysis indexes. This does not edit project.otio.json directly. \
It writes renders/<stem>.edl, renders/<stem>.json, renders/<stem>.md, and \
generated .cube LUTs under luts/generated/. After this tool, call \
apply_edl with the returned edl_path, inspect vedit_diff, render the \
timeline, then call review_look_regions on the render.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn plan_look_regions(
        &self,
        args: Parameters<PlanLookRegionsArgs>,
    ) -> Result<String, ErrorData> {
        plan_look_regions::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_color_grade` — sample a clip's frames, derive a correction,
    /// optionally add a creative look LUT, and emit a validated EDL.
    ///
    /// Marked destructive because the run writes `renders/<stem>.edl` and
    /// `renders/<stem>.json`.
    #[tool(
        description = "\
Plan a render-ready color grade for one clip: sample its frames, measure \
exposure/contrast/white-balance, and emit a validated EDL that corrects \
first and then (optionally, only with an existing project .cube) applies a \
creative look at reduced strength. Writes renders/<stem>.edl and \
renders/<stem>.json. After this tool, call apply_edl with the returned \
`edl` text, inspect vedit_diff, then render the timeline.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn plan_color_grade(
        &self,
        args: Parameters<PlanColorGradeArgs>,
    ) -> Result<String, ErrorData> {
        plan_color_grade::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `plan_speed_ramp` — turn a ramp intent + clip into a validated
    /// time-remap EDL.
    ///
    /// Marked destructive because the run writes `renders/<stem>.edl` and
    /// `renders/<stem>.json`.
    #[tool(
        description = "\
Plan a render-ready speed ramp for one clip: turn an intent word \
(slow_mo_reveal | ramp_to_beat | punch_then | hold_freeze) plus optional \
factor/hold/beat controls into an eased, validated Set Time Remap curve \
(source→timeline, monotonic, starts at 0/0). Writes renders/<stem>.edl and \
renders/<stem>.json and surfaces a motion-blur recommendation. After this \
tool, call apply_edl with the returned edl_path, inspect vedit_diff, then \
render the timeline.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn plan_speed_ramp(
        &self,
        args: Parameters<PlanSpeedRampArgs>,
    ) -> Result<String, ErrorData> {
        plan_speed_ramp::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `diagnose_project_media` — surface repair diagnostics for
    /// timeline media.
    #[tool(
        description = "\
Inspect the current project for missing or unsafe timeline media references. \
Returns structured repair diagnostics, including timeline paths, missing target \
paths, safe relink candidates found in the project, and non-mutating repair \
actions an agent can propose before rendering.",
        annotations(read_only_hint = true)
    )]
    pub async fn diagnose_project_media(
        &self,
        args: Parameters<DiagnoseProjectMediaArgs>,
    ) -> Result<String, ErrorData> {
        diagnose_project_media::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `render_preflight` — inspect render backend selection without
    /// starting a render job.
    #[tool(
        description = "\
Inspect render backend selection, capability metadata, and limitations without \
starting a render job.",
        annotations(read_only_hint = true)
    )]
    pub async fn render_preflight(
        &self,
        args: Parameters<RenderPreflightArgs>,
    ) -> Result<String, ErrorData> {
        render_preflight::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `verify_render` — verify an already-rendered output against the
    /// current project timeline.
    #[tool(
        description = "\
Verify an existing rendered MP4 against the current Awidat timeline. \
Checks duration, audio/video stream presence, missing media, long black \
segments, long unexpected silence, source-range manifest consistency, and \
edited-boundary probes, caption evidence, and adjacent render manifests. \
This tool does not start, poll, or change render jobs; it writes a \
verify-render evidence report and updates the adjacent render manifest when \
one exists. Call start_render/poll_render separately, then pass the finished \
output_path here.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn verify_render(
        &self,
        args: Parameters<VerifyRenderArgs>,
    ) -> Result<String, ErrorData> {
        verify_render::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `broll_candidates` — find shots usable as B-roll cutaways.
    #[tool(
        description = "\
Find shots usable as B-roll: no main face on screen, steady camera, sharp \
frames. Reads `shot` (mandatory) and `frame-quality` (optional). Returns \
shots ranked by duration descending. Defaults filter to types ['no-face', \
'wide'] + motions ['static', 'slow-pan'] + sharp_fraction >= 0.5. Override \
per call: e.g. broll_candidates(types=['wide'], min_duration_s=3) for \
sustained wide cutaways only. Use this when the user asks for cutaways, \
B-roll, transition material, or 'something to cut to' — i.e. anytime you \
need a frame that isn't a talking head.",
        annotations(read_only_hint = true)
    )]
    pub async fn broll_candidates(
        &self,
        args: Parameters<BrollCandidatesArgs>,
    ) -> Result<String, ErrorData> {
        broll_candidates::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `search_broll` — query Pexels for stock B-roll matching a
    /// free-form query.
    #[tool(
        description = "\
Search Pexels for stock B-roll videos matching a free-form query. Returns \
{ query, per_page, total_results, results: [{ pexels_id, duration_s, \
width, height, preview_thumbnail, pexels_page, attribution, renditions, \
frame_previews }] }. Use this to scout candidate cutaways for a moment \
surfaced by `find_broll_opportunities` (or any other moment you think \
wants b-roll). Tell the user the previews; they pick. Then call \
`use_broll(pexels_id, ...)` to download + place. Be specific in your \
query: \"empty city street at dawn\" beats \"loneliness\". Default \
per_page=5; cap 30. Requires PEXELS_API_KEY in env or the OS keychain.",
        annotations(read_only_hint = true)
    )]
    pub async fn search_broll(
        &self,
        args: Parameters<SearchBrollArgs>,
    ) -> Result<String, ErrorData> {
        search_broll::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `import_local` — copy or symlink a local media file into the
    /// project's `raw/` directory and record it in the asset catalog.
    ///
    /// Mutating: writes under `raw/` and rewrites `project.otio.json`.
    #[tool(
        description = "\
Import a local media file into the project's raw/ directory and record \
it in the durable asset catalog. Pass an absolute `source_path`, an \
optional `destination_name` (a single safe file name under raw/), and \
optional `link: true` to create a symlink instead of copying.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn import_local(
        &self,
        args: Parameters<ImportLocalArgs>,
    ) -> Result<String, ErrorData> {
        import_media::run_local(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `import_url` — download a URL into `raw/` via yt-dlp and record
    /// it in the asset catalog.
    ///
    /// Mutating: writes under `raw/` and rewrites `project.otio.json`.
    #[tool(
        description = "\
Download a URL into the project's raw/ directory via yt-dlp and record \
it in the durable asset catalog. Pass an http(s) `url` and an optional \
`destination_name` (a single safe file name under raw/). Requires \
`yt-dlp` on PATH.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn import_url(&self, args: Parameters<ImportUrlArgs>) -> Result<String, ErrorData> {
        import_media::run_url(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `download_yt_clip` — fetch a YouTube/Vimeo clip into
    /// `raw/broll/` via yt-dlp and return an EDL fragment.
    ///
    /// Mutating: writes a downloaded file under `raw/broll/` and a
    /// caveat record under `.awidat/yt_caveats.json`.
    #[tool(
        description = "\
Download a YouTube or Vimeo clip via yt-dlp into raw/broll/ and return \
a ready-to-paste *** Insert BRoll EDL fragment. Gated on \
`acknowledged: true` — the agent must explain the third-party copyright \
caveat to the user and get explicit confirmation before setting it. \
Allowed hosts: youtube.com, m.youtube.com, youtu.be, vimeo.com. \
Idempotent: the same URL maps to the same on-disk path (sha-keyed) and \
skips the re-download. Optional `source_start_s` + `source_end_s` trim \
a sub-window via yt-dlp --download-sections. Per-session cap: 10 \
downloads. Does NOT apply the edit — hand the edl_fragment to \
apply_edl.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn download_yt_clip(
        &self,
        args: Parameters<DownloadYtClipArgs>,
    ) -> Result<String, ErrorData> {
        download_yt_clip::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `relink_media` — apply a safe relink candidate to timeline
    /// media references.
    ///
    /// Mutating: rewrites `project.otio.json` after substituting
    /// matching ExternalReference target URLs.
    #[tool(
        description = "\
Apply a safe relink candidate to timeline media references. Use \
`diagnose_project_media` first to find missing/unsafe references and \
candidate project-relative replacements. Provide old_target_url and/or \
clip_id plus a new_target_url that already exists under the project \
root.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn relink_media(
        &self,
        args: Parameters<RelinkMediaArgs>,
    ) -> Result<String, ErrorData> {
        relink_media::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `generate_proxy` — generate or refresh the .awidat proxy for
    /// one raw asset.
    ///
    /// Mutating: writes a transcoded proxy file under
    /// `.awidat/proxies/`.
    #[tool(
        description = "\
Generate or refresh the .awidat proxy for one raw asset. The proxy is \
the cached low-bitrate playback file under `<project>/.awidat/proxies/`. \
Pass `asset_id` as the project-relative path of a raw asset (for \
example `raw/take-1.mov`); pass `force: true` to regenerate even when \
an up-to-date proxy already exists. Skips ffmpeg when the proxy is \
already fresh and `force` is false.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn generate_proxy(
        &self,
        args: Parameters<GenerateProxyArgs>,
    ) -> Result<String, ErrorData> {
        proxy_media::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `apply_edl` — the load-bearing EDL mutation tool.
    #[tool(
        description = "\
Commit an Edit Decision List (EDL) to the project timeline — this \
WRITES project.otio.json. The EDL is a freeform envelope (NOT \
JSON-escaped multi-line content — pass the raw text). Begins with \
`*** Begin EDL` and ends with `*** End EDL`. This is the graph-native \
editing path. Use it for timeline changes instead of rewriting \
project.otio.json by hand or producing edited media directly with \
bash/FFmpeg. Operations include Trim/Untrim/Delete/Split/Insert Clip, \
Insert/Delete Transition, Move Clip, Insert BRoll/PiP, Set Volume / \
Speed / Effect / Time Remap / Freeze / Color Correction / Output \
Format / Loudness Target / Package Metadata / Broadcast Overlay, \
Apply LUT / Remove LUT, Insert/Set Title, Insert Caption. Anchors are \
content-based (transcript_snippet, clip_uuid, scene_change_index). \
Time fields are seconds into source media. By default this commits; \
set dry_run=true to validate the parse without writing. Pass \
`reasoning` for the auto-commit body so future log reads have audit \
trail.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn apply_edl(&self, args: Parameters<ApplyEdlArgs>) -> Result<String, ErrorData> {
        apply_edl::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `create_stringout` — append a new named stringout.
    #[tool(
        description = "\
Create a new named stringout (ordered select-collection) in the \
project. Projects support multiple parallel stringouts (per arc, \
alt-cut, cold-open) — calling this never replaces an existing one. \
'id' is required; 'name' is an optional display label; 'items' is an \
ordered list of select ids the stringout points at. Returns an error \
if a stringout with that id already exists.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn create_stringout(
        &self,
        args: Parameters<CreateStringoutArgs>,
    ) -> Result<String, ErrorData> {
        create_stringout::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `apply_episode_spans` — persist reviewed episode spans.
    #[tool(
        description = "\
Persist reviewed episode spans into Timeline.metadata.awidat.episodes. Use \
this after podcast_episode_spans or transcript review to make episodes \
first-class project metadata. With replace=true, replaces all stored episodes; \
with replace=false, upserts by id. Each episode requires id, asset_id, \
source_start_s, source_end_s, and status one of review_needed, accepted, or \
rejected. Set create_stringouts=true to create/update source selects and an \
ordered stringout for accepted episodes.",
        annotations(destructive_hint = true)
    )]
    pub async fn apply_episode_spans(
        &self,
        args: Parameters<ApplyEpisodeSpansArgs>,
    ) -> Result<String, ErrorData> {
        apply_episode_spans::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    // ---- manage_assets sub-tools ----

    /// `create_bin` — create a durable asset bin.
    #[tool(
        description = "Create a durable user/agent asset bin in the project catalog.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn create_bin(&self, args: Parameters<CreateBinArgs>) -> Result<String, ErrorData> {
        manage_assets::run_create_bin(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `move_to_bin` — move an asset to a durable bin or clear its bin.
    #[tool(
        description = "Move a catalog asset into a durable bin, or clear its bin.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn move_to_bin(&self, args: Parameters<MoveToBinArgs>) -> Result<String, ErrorData> {
        manage_assets::run_move_to_bin(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `rename_asset` — set an asset display label.
    #[tool(
        description = "Set the human-readable label for a catalog asset without renaming the source file.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn rename_asset(
        &self,
        args: Parameters<RenameAssetArgs>,
    ) -> Result<String, ErrorData> {
        manage_assets::run_rename_asset(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `tag_asset` — add or remove asset tags.
    #[tool(
        description = "Add or remove durable tags on a catalog asset.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn tag_asset(&self, args: Parameters<TagAssetArgs>) -> Result<String, ErrorData> {
        manage_assets::run_tag_asset(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `rate_asset` — set an asset rating.
    #[tool(
        description = "Set an asset rating from 0 to 5.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn rate_asset(&self, args: Parameters<RateAssetArgs>) -> Result<String, ErrorData> {
        manage_assets::run_rate_asset(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `mark_select` — create or update a durable source select.
    #[tool(
        description = "Create or update a durable source select for an asset range.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn mark_select(&self, args: Parameters<MarkSelectArgs>) -> Result<String, ErrorData> {
        manage_assets::run_mark_select(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    // ---- granular_timeline sub-tools ----

    /// `split_clip` — granular timeline split.
    #[tool(
        description = "Split a clip by lowering to apply_edl's graph-native Split Clip operation; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn split_clip(&self, args: Parameters<SplitClipArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_split_clip(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `trim_clip` — granular timeline trim.
    #[tool(
        description = "Trim a clip by lowering to apply_edl's graph-native Trim Clip operation; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn trim_clip(&self, args: Parameters<TrimClipArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_trim_clip(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `move_clip` — granular timeline move.
    #[tool(
        description = "Move a clip by lowering to apply_edl's Move Clip operation; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn move_clip(&self, args: Parameters<MoveClipArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_move_clip(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `delete_clips` — granular timeline batch delete.
    #[tool(
        description = "Delete one or more clips in a single apply_edl envelope; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn delete_clips(
        &self,
        args: Parameters<DeleteClipsArgs>,
    ) -> Result<String, ErrorData> {
        granular_timeline::run_delete_clips(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `roll_trim` — granular timeline roll edit.
    #[tool(
        description = "Roll an edit point by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn roll_trim(&self, args: Parameters<RollTrimArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_roll_trim(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `slip_clip` — granular timeline slip.
    #[tool(
        description = "Slip a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn slip_clip(&self, args: Parameters<SlipClipArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_slip_clip(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `ripple_trim` — granular timeline ripple trim.
    #[tool(
        description = "Ripple-trim a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn ripple_trim(&self, args: Parameters<RippleTrimArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_ripple_trim(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `set_marker` — granular timeline marker add/update.
    #[tool(
        description = "Add or update a timeline marker by lowering to apply_edl's Professional Timeline Edit marker operations; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn set_marker(&self, args: Parameters<SetMarkerArgs>) -> Result<String, ErrorData> {
        granular_timeline::run_set_marker(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `set_clip_property` — granular timeline clip volume/speed/effect.
    #[tool(
        description = "Set basic clip properties by lowering to existing apply_edl ops such as Set Volume, Set Speed, or Set Effect; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn set_clip_property(
        &self,
        args: Parameters<SetClipPropertyArgs>,
    ) -> Result<String, ErrorData> {
        granular_timeline::run_set_clip_property(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `set_track_property` — granular timeline track audio properties.
    #[tool(
        description = "Set basic track audio properties by lowering to apply_edl's Set Track Audio op; apply_edl remains the canonical mutation path.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn set_track_property(
        &self,
        args: Parameters<SetTrackPropertyArgs>,
    ) -> Result<String, ErrorData> {
        granular_timeline::run_set_track_property(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_merge` — execute bounded local vedit branch merges.
    ///
    /// Marked destructive because the run writes
    /// `project.otio.json`, advances the target branch, and creates a
    /// two-parent merge commit. The original `ToolHandler` had
    /// `is_mutating = true` for the same reason.
    #[tool(
        description = "\
Merge a source vedit ref into a target branch/ref using Awidat's bounded \
non-overlapping clip-id rule. The merge first runs the same preflight as \
vedit_merge_preflight. If any changed clip/media ids overlap, it refuses \
the merge and reports a conflict. On success it writes project.otio.json, \
advances the target branch, and creates a two-parent merge commit.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn vedit_merge(&self, args: Parameters<VeditMergeArgs>) -> Result<String, ErrorData> {
        vedit_merge::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `vedit_revert` — restore the working timeline to a prior vedit
    /// commit/ref, optionally recording the restore as a new commit.
    ///
    /// Marked destructive because the run writes
    /// `project.otio.json` and (by default) creates an audit commit.
    /// The original `ToolHandler` had `is_mutating = true` for the
    /// same reason.
    #[tool(
        description = "\
Restore the project's working `project.otio.json` to a prior vedit \
commit/ref. This is the safe product-level undo path for edit history: it \
reads the timeline snapshot stored at the requested ref and writes that \
snapshot back to the current project. By default, `commit=true`, so the \
restore itself is recorded as a new commit and appears in `vedit_log`. \
Set `commit=false` only when the user explicitly asks to inspect or stage \
a restore without saving it. This is not a branch checkout and not a \
merge.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn vedit_revert(
        &self,
        args: Parameters<VeditRevertArgs>,
    ) -> Result<String, ErrorData> {
        vedit_revert::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_apply_accepted_edits` — compile accepted proposal IDs
    /// to an `apply_edl` batch.
    ///
    /// Marked destructive because the run writes a proposal/accepted
    /// edits artifact representing user-approved cuts; the actual EDL
    /// application is still a separate `apply_edl` step but this tool
    /// commits the accepted-edits manifest the next step relies on.
    #[tool(
        description = "\
Compile accepted podcast proposal item IDs into one ordered apply_edl batch.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn podcast_apply_accepted_edits(
        &self,
        args: Parameters<PodcastApplyAcceptedEditsArgs>,
    ) -> Result<String, ErrorData> {
        podcast_apply_accepted_edits::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_edit_proposal` — turn cleanup evidence into a gated
    /// edit plan.
    ///
    /// Marked destructive because the run writes a proposal artifact
    /// representing the gated edit plan that downstream mutating
    /// tools (e.g. `podcast_apply_accepted_edits`, `apply_edl`)
    /// consume.
    #[tool(
        description = "\
Build a reviewable podcast edit proposal from existing cleanup evidence. \
The tool groups timeline-visible dead air, filler words, and false starts \
into safe/review/risky edits and marks review/risky candidates with the \
continuity gate they must pass before mutation. It does not change the \
timeline.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn podcast_edit_proposal(
        &self,
        args: Parameters<PodcastEditProposalArgs>,
    ) -> Result<String, ErrorData> {
        podcast_edit_proposal::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `use_broll` — download a chosen Pexels video and return an
    /// EDL fragment ready for apply_edl.
    ///
    /// Mutating: writes a downloaded file under `raw/broll/`.
    #[tool(
        description = "\
Download a Pexels video chosen from a prior `search_broll` result and \
return an EDL fragment ready to hand to `apply_edl`. Does NOT apply \
the EDL itself — that's `apply_edl`'s job. The returned \
`edl_fragment` is a `*** Begin EDL ... *** End EDL` block. The \
download lands at `raw/broll/pexels-<id>.mp4`. Idempotent: if the \
file already exists the tool returns the EDL fragment without \
re-downloading. Per-session cap: 10 downloads. Defaults: \
max_width=1920, position=overlay. Pass position=replace to cut the \
underlying clip for the duration of the b-roll. Pass insert_as=pip \
to return an Insert PiP fragment instead.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn use_broll(&self, args: Parameters<UseBrollArgs>) -> Result<String, ErrorData> {
        use_broll::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `use_generated_media` — return an Insert BRoll EDL fragment
    /// for a completed generated-media asset.
    ///
    /// Marked destructive consistent with the other media-placement
    /// tools in this batch; the run reads the generated-media
    /// registry and assembles an EDL fragment the agent hands to
    /// apply_edl.
    #[tool(
        description = "\
Return a ready-to-apply Insert BRoll EDL fragment for a completed generated \
media asset. This does not call apply_edl or mutate the timeline.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn use_generated_media(
        &self,
        args: Parameters<UseGeneratedMediaArgs>,
    ) -> Result<String, ErrorData> {
        use_generated_media::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `stream_remux` — start a stream-copy/remux export job.
    ///
    /// Mutating: spawns an ffmpeg job that writes to the project's
    /// renders directory and persists a render manifest.
    #[tool(
        description = "\
Start a first-class stream-copy/remux export. Use this for simple container \
changes, stream extraction/reordering, subtitle/audio passthrough, or other \
packet-preserving jobs that do not need timeline effects, overlays, transitions, \
retiming, or re-encoding. Streams are explicit so the manifest records the exact \
mapping and can be replayed deterministically.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn stream_remux(
        &self,
        args: Parameters<StreamRemuxArgs>,
    ) -> Result<String, ErrorData> {
        stream_remux::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `podcast_visual_polish` — forced visual/multicam planning
    /// pass.
    ///
    /// Marked destructive consistent with the other podcast
    /// finishing tools in this batch; the run reads project state
    /// and produces a polish report that downstream mutating tools
    /// consume.
    #[tool(
        description = "\
Check podcast visual polish readiness: multicam evidence, b-roll planning, \
chapters, lower thirds, and captions.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn podcast_visual_polish(
        &self,
        args: Parameters<PodcastVisualPolishArgs>,
    ) -> Result<String, ErrorData> {
        podcast_visual_polish::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `preview_cache_status` — preview-cache readiness + bounded
    /// refresh plan.
    #[tool(
        description = "Report project preview-cache readiness and a bounded read-only refresh plan across proxies, thumbnails, and waveform sidecars.",
        annotations(read_only_hint = true)
    )]
    pub async fn preview_cache_status(
        &self,
        args: Parameters<PreviewCacheArgs>,
    ) -> Result<String, ErrorData> {
        preview_cache::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `run_preview_cache_refresh` — execute the persisted
    /// preview-cache refresh lifecycle using the ffmpeg-backed
    /// executor.
    ///
    /// Marked destructive because the run writes proxy/preview files
    /// under `.awidat/proxies/`, `.awidat/thumbnails/`, and
    /// `.awidat/waveforms/`, and atomically updates the lifecycle
    /// manifest at `.awidat/preview-cache/refresh-plan.json`.
    #[tool(
        description = "Run the persisted preview-cache refresh lifecycle to completion using the ffmpeg-backed executor. Resumes from any prior pending/in-progress tasks and skips already-completed tasks.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn run_preview_cache_refresh(
        &self,
        args: Parameters<RunPreviewCacheRefreshArgs>,
    ) -> Result<String, ErrorData> {
        run_preview_cache_refresh::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `export_package` — render timeline and write delivery sidecars
    /// (subtitles, chapters, metadata, preflight, recipe) under
    /// `renders/package/`.
    ///
    /// Marked destructive because the run starts a final-MP4 render
    /// job and writes a bundle of delivery sidecars (SRT, VTT,
    /// chapters, metadata JSON, thumbnail candidates, preflight,
    /// enhancement recipe) to disk.
    #[tool(
        description = "\
Export a delivery package under renders/package/: final MP4 render job, \
timeline-relative SRT, VTT, chapter text, package metadata JSON, \
thumbnail candidate JSON, or a turnover package for sound/color/VFX \
review. Burned-in Insert Caption overlays remain part of rendered \
exports; SRT/VTT are separate delivery artifacts. Rendered packages \
lower through an ExportPreset and can request hardware_acceleration \
off, auto, or require.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn export_package(
        &self,
        args: Parameters<ExportPackageArgs>,
    ) -> Result<String, ErrorData> {
        export_package::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `load_skill` — L2 progressive-disclosure tool: returns the full
    /// `SKILL.md` body of a named editorial skill.
    #[tool(
        description = "\
Load the full L2 body of a named editorial skill into the current \
turn's context. Use this when the user's request maps to one of the \
skills listed in the L1 catalog. The \
returned text contains the skill's editorial style, step-by-step \
playbook, and references to bundled scripts you can run via `bash`.\
\n\n\
Examples:\
\n  load_skill(name='interview-tightener') — when the user asks to \
tighten an interview\
\n  load_skill(name='b-roll-suggester')   — when the user asks for \
visual cutaway suggestions\
\n  load_skill(name='podcast-episode-producer') — for the canonical \
end-to-end episode flow\
\n\n\
You can call multiple skills in a single turn if the request spans \
their domains (e.g. tighten THEN suggest b-roll).\
",
        annotations(read_only_hint = true)
    )]
    pub async fn load_skill(&self, args: Parameters<LoadSkillArgs>) -> Result<String, ErrorData> {
        load_skill::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `start_render` — run ffmpeg to render a preview/segment/full
    /// asset or the edited timeline. Mutating: writes a render
    /// manifest under `<project>/renders/` and an ffmpeg output file.
    ///
    /// The MCP-side port runs ffmpeg INLINE: the call awaits the
    /// subprocess to completion before returning. There is no
    /// background job id; use `verify_render` against the returned
    /// output_path.
    #[tool(
        description = "\
Run an ffmpeg render to completion and return the result. NOTE: this \
in-process MCP port runs ffmpeg inline — it does NOT return a job id; \
it awaits the render and returns once ffmpeg exits. Scopes: 'preview' = \
480p H.264 of an asset; 'segment' = trim [start_s, end_s) of an asset \
via stream-copy; 'full' = high-bitrate H.264 of an asset; 'timeline' \
= render the *edited timeline* by walking project.otio.json. Output \
lands under <project>/renders/. Long renders block the agent turn — \
for hour-long timeline exports use the desktop UI or `awidat render` \
CLI instead.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn start_render(
        &self,
        args: Parameters<StartRenderArgs>,
    ) -> Result<String, ErrorData> {
        start_render::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `poll_render` — read the latest status of a render job started
    /// by `start_render`. Currently stubbed out for the in-process MCP
    /// server because `start_render` runs ffmpeg synchronously and the
    /// server holds no cross-call job state.
    #[tool(
        description = "\
Read the latest status of a render job started by `start_render`. \
NOTE: in the in-process MCP server, start_render runs ffmpeg \
synchronously and returns only when the render terminates, so there \
is no in-flight job to poll. This tool returns an unsupported-error \
and asks the caller to inspect the output_path / call verify_render \
instead.",
        annotations(read_only_hint = true)
    )]
    pub async fn poll_render(&self, args: Parameters<PollRenderArgs>) -> Result<String, ErrorData> {
        poll_render::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `start_indexing` — run the configured indexers over every media
    /// file in the project's `raw/` directory. Mutating: writes
    /// per-asset sidecars under `<project>/.awidat/index/`.
    #[tool(
        description = "\
Run the configured indexers (whisper transcription, scene detection, \
audio energy, editorial moments, etc) over every media file in the \
project's raw/ dir. Returns the summary report inline once finished. \
The dispatcher is sha-keyed — re-running on already-indexed assets is \
a fast no-op, so it's safe to call any time you suspect sidecars might \
be stale. Pass an optional `indexers` filter (e.g. ['whisper']) to \
re-run only specific producers. WARNING: indexing a fresh asset can \
take 20+ minutes for hour-long video; only call when the user has \
asked for an editorial operation that needs the sidecars and \
view_episode shows they're missing.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn start_indexing(
        &self,
        args: Parameters<StartIndexingArgs>,
    ) -> Result<String, ErrorData> {
        start_indexing::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `start_generated_media_job` — submit a generated-media (b-roll)
    /// job and write the project-local registry. Mutating: writes
    /// under `<project>/.awidat/generated-media/registry.json` and
    /// (for `mock`) a placeholder file under `raw/generated/`.
    #[tool(
        description = "\
Start a generated-media job and write the local generated-media \
registry. Provider 'mock' creates an offline completed placeholder \
record suitable for tests. Provider 'openrouter' submits an \
asynchronous OpenRouter video generation job using OPENROUTER_API_KEY. \
Provider 'seedance' is not direct; use OpenRouter or a future \
dedicated adapter.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn start_generated_media_job(
        &self,
        args: Parameters<StartGeneratedMediaJobArgs>,
    ) -> Result<String, ErrorData> {
        start_generated_media_job::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `poll_generated_media_job` — read a generated-media registry
    /// record from disk and return job state + output metadata.
    /// Read-only in the MCP port; does NOT re-poll OpenRouter or
    /// download outputs (that path needs an out-of-process worker).
    #[tool(
        description = "\
Read a generated-media registry record and return job state plus output \
metadata. Read-only: reads the on-disk registry under \
`<project>/.awidat/generated-media/registry.json` and returns whatever \
state was persisted by `start_generated_media_job`. NOTE: the \
in-process MCP server does NOT poll OpenRouter or download completed \
outputs from this call — if the record is still in flight, re-run \
`start_generated_media_job` from a desktop session that has the \
out-of-process worker wired up.",
        annotations(read_only_hint = true)
    )]
    pub async fn poll_generated_media_job(
        &self,
        args: Parameters<PollGeneratedMediaJobArgs>,
    ) -> Result<String, ErrorData> {
        poll_generated_media_job::run(args.0, McpToolCtx::resolve())
            .await
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `update_plan` — record the agent's current editorial plan as an
    /// ordered checklist. The MCP port drops the legacy
    /// `SessionEvent::EditPlanUpdate` broadcast and just echoes the
    /// validated plan back as a JSON record.
    #[tool(
        description = "\
Record the current editorial plan as an ordered list of steps. Use this \
to make your reasoning visible: what you've done, what you're doing now, \
what's left. Convention: at most one step `in_progress` at a time. The \
MCP port echoes the plan back as a JSON record — there is no event \
broadcast or persisted file in this server; the plan exists in the \
conversation transcript. Call again whenever the plan changes \
meaningfully (don't spam every micro-step).",
        annotations(read_only_hint = true)
    )]
    pub async fn update_plan(&self, args: Parameters<UpdatePlanArgs>) -> Result<String, ErrorData> {
        update_plan::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `attempt_completion` — echo the final summary back as a tool
    /// result. Under MCP this does NOT gate the turn; the model's
    /// final assistant message still does.
    #[tool(
        description = "\
Record your final answer/summary for the current turn. The `result` \
string is echoed back verbatim as the tool result so it lands in the \
conversation transcript. NOTE: under MCP this tool does NOT gate or end \
the turn — the model's final assistant message is what closes the turn. \
Use this when you want the summary to appear as a structured tool \
result in addition to your free-form reply.",
        annotations(read_only_hint = true)
    )]
    pub async fn attempt_completion(
        &self,
        args: Parameters<AttemptCompletionArgs>,
    ) -> Result<String, ErrorData> {
        attempt_completion::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `request_user_input` — stub. Will become functional once the
    /// in-process server bridges MCP elicitation to the client. Until
    /// then the agent should phrase the question in chat and wait
    /// for the user's next message.
    #[tool(
        description = "\
Ask the human a question and wait for a reply. NOTE: in the in-process \
MCP server this is currently a stub — it returns an unsupported-error \
and asks the agent to phrase the question in its chat reply and wait \
for the user's next turn instead. Will become functional once the \
server bridges MCP's elicitation protocol to the client.",
        annotations(read_only_hint = true)
    )]
    pub async fn request_user_input(
        &self,
        args: Parameters<RequestUserInputArgs>,
    ) -> Result<String, ErrorData> {
        request_user_input::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }

    /// `clip_search` — stub. Needs the lazy clip-mcp indexer
    /// subprocess pool that's not yet wired into the in-process
    /// server. Use `find_moment` for transcript-based search until
    /// then.
    #[tool(
        description = "\
Find frames matching a free-text query using per-second CLIP image \
embeddings. NOTE: in the in-process MCP server this is currently a \
stub — it returns an unsupported-error because the lazy clip-mcp \
indexer subprocess pool is not yet wired in. Use `find_moment` for a \
transcript-based search in the meantime; this tool will become \
functional once the MCP indexer pool lands.",
        annotations(read_only_hint = true)
    )]
    pub async fn clip_search(&self, args: Parameters<ClipSearchArgs>) -> Result<String, ErrorData> {
        clip_search::run(args.0, McpToolCtx::resolve())
            .map_err(|msg| ErrorData::invalid_params(msg, None))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AwidatMcpServer {}
