//! `awidat tui` subcommand. Opens a Ratatui chat against a project,
//! with the full 12-tool registry mounted and the approval gate wired
//! to a modal.
//!
//! This is the developer-facing surface per the competitive landscape
//! (PDF: "the human-facing wedge has to be the GUI viewer + the chat
//! surface, with the CLI as the engine, not the product"). For week 5
//! we ship the chat surface; the GUI viewer lands later.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::subagent::SubAgentRegistry;
use awidat_core::tools::{
    analyze_sync::AnalyzeSyncTool, apply_edl::ApplyEdlTool,
    assess_continuity::AssessContinuityTool, assess_edit_quality::AssessEditQualityTool,
    bash::BashTool, broll_candidates::BrollCandidatesTool, clip_search::ClipSearchTool,
    color_scopes::ColorScopesTool, create_stringout::CreateStringoutTool, delegate::DelegateTool,
    delegate_all::DelegateAllTool, diagnose_project_media::DiagnoseProjectMediaTool,
    download_yt_clip::DownloadYtClipTool, export_package::ExportPackageTool,
    find_audio_asset::FindAudioAssetTool, find_beat::FindBeatTool,
    find_black_frames::FindBlackFramesTool, find_broll_opportunities::FindBrollOpportunitiesTool,
    find_dead_air::FindDeadAirTool, find_episode_start::FindEpisodeStartTool,
    find_eye_contact::FindEyeContactTool, find_false_starts::FindFalseStartsTool,
    find_filler_words::FindFillerWordsTool,
    find_generated_broll_opportunities::FindGeneratedBrollOpportunitiesTool,
    find_moment::FindMomentTool, find_speaker_oncam::FindSpeakerOncamTool,
    granular_timeline::register_granular_timeline_tools, import_media::ImportLocalTool,
    import_media::ImportUrlTool, inspect_clip::InspectClipTool, inspect_moment::InspectMomentTool,
    list_assets::ListAssetsTool, list_bins::ListBinsTool, list_looks::ListLooksTool,
    list_markers::ListMarkersTool, list_stringouts::ListStringoutsTool, load_skill::LoadSkillTool,
    local_review_package::LocalReviewPackageTool, manage_assets::CreateBinTool,
    manage_assets::MarkSelectTool, manage_assets::MoveToBinTool, manage_assets::RateAssetTool,
    manage_assets::RenameAssetTool, manage_assets::TagAssetTool, plan_emphasis::PlanEmphasisTool,
    plan_generated_media::PlanGeneratedMediaTool, plan_look_regions::PlanLookRegionsTool,
    plan_look_regions::ReviewLookRegionsTool, plan_look_regions::StartLookRegionPassTool,
    plan_motion_scene::PlanMotionSceneTool, plan_multicam::PlanMulticamTool,
    plan_reframe::PlanReframeTool, plan_scene_aware_short_form::PlanSceneAwareShortFormTool,
    plan_short_form_review::PlanShortFormReviewTool, plan_transition::PlanTransitionTool,
    plan_visual_support::PlanVisualSupportTool,
    podcast_apply_accepted_edits::PodcastApplyAcceptedEditsTool,
    podcast_audio_polish::PodcastAudioPolishTool,
    podcast_cleanup_candidates::PodcastCleanupCandidatesTool,
    podcast_edit_proposal::PodcastEditProposalTool, podcast_episode_spans::PodcastEpisodeSpansTool,
    podcast_post_draft_check::PodcastPostDraftCheckTool, podcast_qc_report::PodcastQcReportTool,
    podcast_smooth_cut_boundaries::PodcastSmoothCutBoundariesTool,
    podcast_story_map::PodcastStoryMapTool, podcast_visual_polish::PodcastVisualPolishTool,
    poll_generated_media_job::PollGeneratedMediaJobTool, poll_render::PollRenderTool,
    preview_cache::PreviewCacheStatusTool, proxy_media::GenerateProxyTool,
    proxy_media::ProxyStatusTool, read_broll_recommendations::ReadBrollRecommendationsTool,
    read_index::ReadIndexTool, read_media_intelligence::ReadMediaIntelligenceTool,
    read_media_readiness::ReadMediaReadinessTool, read_understanding::ReadUnderstandingTool,
    relink_media::RelinkMediaTool, render_preflight::RenderPreflightTool,
    request_user_input::RequestUserInputTool, search_broll::SearchBrollTool,
    shot_summary::ShotSummaryTool, start_generated_media_job::StartGeneratedMediaJobTool,
    start_indexing::StartIndexingTool, start_render::StartRenderTool,
    stream_remux::StreamRemuxTool, transcript_pack::TranscriptPackTool,
    transcript_search::TranscriptSearchTool, transition_context::TransitionContextTool,
    update_plan::UpdatePlanTool, use_broll::UseBrollTool,
    use_generated_media::UseGeneratedMediaTool,
    validate_transition_choice::ValidateTransitionChoiceTool, vedit_blame::VeditBlameTool,
    vedit_branch::VeditBranchTool, vedit_changed_clip_ids::VeditChangedClipIdsTool,
    vedit_checkout::VeditCheckoutTool, vedit_commit::VeditCommitTool, vedit_diff::VeditDiffTool,
    vedit_log::VeditLogTool, vedit_merge_preflight::VeditMergePreflightTool,
    vedit_revert::VeditRevertTool, vedit_show::VeditShowTool, vedit_tag::VeditTagTool,
    verify_render::VerifyRenderTool, view_episode::ViewEpisodeTool, view_frame::ViewFrameTool,
    view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, ToolRegistry};
use awidat_tui::{App, AppConfig};
use tokio::sync::mpsc;

const SYSTEM_PROMPT: &str = "\
You are awidat, a terminal-first agent for editing long-form spoken \
video.\
\n\n**Discover before acting.** Never guess asset paths or filenames. \
On the first turn of any session that touches assets, call \
view_episode (or list_assets) to learn the actual filenames. \
Asset paths in this project may be UUID-style (copy_F65206FA-…MOV), \
not human-readable like 'cast.mp4'. Guessing wastes tool calls and \
surfaces avoidable errors to the user. The single discovery call is \
cheap and makes everything after it correct.\
\n\nYou have the full editorial toolset, organized by purpose:\
\n  - **Discovery / map**: view_episode (compact map of the project — \
includes which vision indexers have run), view_timeline, list_markers, list_assets, \
find_episode_start (publishable podcast/interview start; rejects \
pre-roll and rehearsed intros).\
\n  - **Editorial index**: find_beat (typed editorial moments — \
hooks, punchlines, CTAs, etc.), inspect_moment (drill into one beat \
with surrounding transcript + dependencies). Prefer these over \
find_moment when the user asks for editorial intent ('find the \
funny part', 'what's the strongest hook') — find_beat surfaces \
typed editorial decisions, not just text matches.\
\n  - **Vision** (only useful when view_episode shows the matching \
indexer ran): clip_search (free-text frame search — \"person \
holding a phone\", \"dark room\"), shot_summary (visual structure \
overview), broll_candidates (cutaway/B-roll hunting — wide/no-face \
+ steady + sharp), find_speaker_oncam (when is speaker X's face \
visible — needs whisper diarization + face), find_eye_contact \
(direct-address moments), plan_reframe (static vertical/social crop \
fragment from subject-center evidence).\
\n  - **Raw lookup**: read_understanding (fused scene/moment understanding and clip candidates), read_broll_recommendations (scored B-roll recommendations with category, strategy, insertion plan, and evidence), read_media_intelligence (progressive source/proxy/waveform/transcript/speakers/scenes/topics/moments/clips/b-roll state), read_media_readiness (source/proxy/cache/index readiness), \
find_moment (transcript substring), read_index \
(any indexer channel), inspect_clip (one clip's metadata), \
view_frame (extract a frame at a timestamp), color_scopes \
(histogram/waveform/parade/vectorscope evidence for one frame).\
\n  - **Edit quality**: assess_edit_quality before risky trims/splits; it recommends hard cut, recut, J/L split edit, b-roll, or motivated transition. transition_context assembles handles, transcript, frames, and continuity context before choosing a visible transition; plan_transition turns that packet into a hard-cut or visible-transition proposal. assess_continuity is the lower-level rule breakdown.\
\n  - **Editing**: apply_edl (commit edits — Trim, Untrim, Delete, \
Split, Insert, Insert PiP). For `@@ anchor: clip_uuid=...`, use the clip \
anchor shown by view_timeline, usually the clip name like `clip-0`; \
never use the asset filename, proxy stem, or raw media basename as \
clip_uuid. Times are source-media seconds. view_timeline shows current \
`source=[start..end]`; to trim the first N seconds of the visible clip, \
set `start` to source start + N, and to trim the last N seconds, set \
`end` to source end - N.\
\n  - **Indexing**: start_indexing — run the configured indexers \
(whisper, scenes, audio energy, beats, etc.) over assets in raw/. \
Sha-keyed so re-runs on already-indexed assets are fast no-ops. \
Call when view_episode shows missing sidecars and the user has \
asked for an editorial operation that needs them. Don't proactively \
re-index already-indexed projects.\
\n  - **Render**: start_render, poll_render. To render the *edited \
timeline* (what the user sees as 'the edit'), use scope='timeline' — \
that walks the OTIO and concats every clip's source_range with \
re-encode at boundaries. Use scope='preview'/'segment'/'full' only \
when the user wants a raw asset dumped, not the edit.\
\n  - **Finishing**: start_look_region_pass runs the LUT plan -> \
apply_edl -> timeline render sequence; plan_look_regions drafts only \
the generated LUT EDL; review_look_regions turns a completed render \
into contact-sheet/JSON/Markdown proof.\
\n  - **Plan / collab**: update_plan, request_user_input, bash. \
\n  - **Skills**: load_skill — load a named editorial workflow's full \
playbook into the current turn. Check the per-turn skills catalog for \
skill names + one-line descriptions; if the user's request maps to one \
(e.g. \"tighten this \
interview\" → interview-tightener), call load_skill(name=...) BEFORE \
starting the work. The skill body has the editorial style + \
step-by-step playbook; following it produces better cuts than \
improvising.\
\n  - **Delegate**: delegate — spawn a single read-only research \
sub-agent (episode-explorer / cut-scout / b-roll-hunter) for a \
focused question. delegate_all — spawn MULTIPLE persona reviewers \
(editor / fact-checker / brand-voice) IN PARALLEL on the same task \
and get a consolidated review back. Use delegate_all before shipping \
a draft cut to get cross-cutting feedback in one pass; use delegate \
for single-lens research. Sub-agents and personas are read-only — \
for edits, do the work yourself.\
\n\n\
Mutating tools may be approval-gated depending on the active runtime. \
If the UI asks for approval and the user denies, you'll see \
is_error=true; route around it rather than retrying the same call.\
\n\n\
EDL format (freeform, NOT JSON-escaped):\n\
*** Begin EDL\n\
*** Trim Clip|Untrim Clip|Delete Clip|Split Clip|Insert Clip|Insert BRoll|Insert PiP|Move Clip|Insert Transition\n\
@@ anchor: transcript_snippet=\"...\" or clip_uuid=...\n\
+ key: value\n\
*** End EDL\n\
\n\
For `clip_uuid=...`, use the clip anchor shown by view_timeline \
(usually the clip name, e.g. `clip-0`), NOT the asset filename. \
Call view_timeline first if you don't know the clip names.\n\
\n\
**Time semantics.** All time values in the EDL — `start`, `end`, \
`at_s` — are in **seconds into the clip's source media**, NOT into \
the timeline. For an untrimmed clip those numbers match. For a \
clip that has already been trimmed, source-media seconds run from \
0 at the *original* media start, NOT from the clip's current \
trimmed-in point. Read view_timeline output carefully: it shows \
the timeline position and current source range.\
\n\n\
**Tool-call budget.** Each turn has a hard cap of 64 sampling \
iterations. After ~52 you'll see a runtime warning; commit any \
pending edit then. Don't waste iterations on speculative bash \
exploration — use the editorial tools first; bash is the escape \
hatch.\
\n\n\
**Trim is one-way; widen with Untrim Clip.** `Trim Clip` only \
NARROWS a clip's source range. If you trim too aggressively and \
need the cut content back, use `Untrim Clip` with new start/end. \
But: Untrim can only widen back to the *original media bounds* — \
content that was never on the timeline (because the project was \
seeded with a narrower source range) cannot be brought in by \
Untrim alone; in that case, commit what you have and report the \
limitation honestly to the user instead of looping.\
\n\n\
Be concise. Commit edits via apply_edl directly when you're confident.\
";

pub fn run(project_root: &Path, model_override: Option<&str>) -> Result<()> {
    run_with_initial_prompt(project_root, model_override, None)
}

/// Variant that pre-fills the composer with `initial_prompt`. Used
/// by `awidat skills run <name>` to stage a first-turn message
/// asking the agent to use a specific skill. The user can edit
/// the prompt or hit enter to submit it.
/// Build the full editorial-tool registry the TUI mounts. Factored out
/// so `resume_cmd` mounts the same surface when re-opening a session.
/// The `delegate` tool is added on top of the base registry, since it
/// needs a snapshot of the registry to filter against per call. `model`
/// is the default sub-agent model.
pub fn build_full_registry(model: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    register_granular_timeline_tools(&mut registry);
    registry.register(Arc::new(AnalyzeSyncTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(DiagnoseProjectMediaTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(FindBlackFramesTool));
    registry.register(Arc::new(FindAudioAssetTool));
    registry.register(Arc::new(FindDeadAirTool));
    registry.register(Arc::new(FindEpisodeStartTool));
    registry.register(Arc::new(FindFillerWordsTool));
    registry.register(Arc::new(FindFalseStartsTool));
    registry.register(Arc::new(AssessContinuityTool));
    registry.register(Arc::new(AssessEditQualityTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ImportLocalTool));
    registry.register(Arc::new(ImportUrlTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(ListBinsTool));
    registry.register(Arc::new(ListLooksTool));
    registry.register(Arc::new(ListMarkersTool));
    registry.register(Arc::new(ListStringoutsTool));
    registry.register(Arc::new(CreateStringoutTool));
    registry.register(Arc::new(CreateBinTool));
    registry.register(Arc::new(MoveToBinTool));
    registry.register(Arc::new(RenameAssetTool));
    registry.register(Arc::new(TagAssetTool));
    registry.register(Arc::new(RateAssetTool));
    registry.register(Arc::new(MarkSelectTool));
    registry.register(Arc::new(ColorScopesTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(PreviewCacheStatusTool));
    registry.register(Arc::new(RenderPreflightTool));
    registry.register(Arc::new(ProxyStatusTool));
    registry.register(Arc::new(GenerateProxyTool));
    registry.register(Arc::new(ReadMediaIntelligenceTool));
    registry.register(Arc::new(ReadUnderstandingTool));
    registry.register(Arc::new(ReadBrollRecommendationsTool));
    registry.register(Arc::new(ReadMediaReadinessTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RelinkMediaTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(TranscriptSearchTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(StreamRemuxTool));
    registry.register(Arc::new(TranscriptPackTool));
    registry.register(Arc::new(VerifyRenderTool));
    registry.register(Arc::new(ExportPackageTool));
    registry.register(Arc::new(StartLookRegionPassTool));
    registry.register(Arc::new(PlanLookRegionsTool));
    registry.register(Arc::new(ReviewLookRegionsTool));
    registry.register(Arc::new(PlanEmphasisTool));
    registry.register(Arc::new(PlanGeneratedMediaTool));
    registry.register(Arc::new(PlanMulticamTool));
    registry.register(Arc::new(PlanReframeTool));
    registry.register(Arc::new(PlanSceneAwareShortFormTool));
    registry.register(Arc::new(PlanShortFormReviewTool));
    registry.register(Arc::new(PlanTransitionTool));
    registry.register(Arc::new(PlanVisualSupportTool));
    registry.register(Arc::new(PlanMotionSceneTool));
    registry.register(Arc::new(PodcastApplyAcceptedEditsTool));
    registry.register(Arc::new(PodcastAudioPolishTool));
    registry.register(Arc::new(PodcastCleanupCandidatesTool));
    registry.register(Arc::new(PodcastEditProposalTool));
    registry.register(Arc::new(PodcastEpisodeSpansTool));
    registry.register(Arc::new(PodcastPostDraftCheckTool));
    registry.register(Arc::new(PodcastQcReportTool));
    registry.register(Arc::new(PodcastSmoothCutBoundariesTool));
    registry.register(Arc::new(PodcastStoryMapTool));
    registry.register(Arc::new(PodcastVisualPolishTool));
    registry.register(Arc::new(PollGeneratedMediaJobTool));
    registry.register(Arc::new(StartIndexingTool));
    registry.register(Arc::new(StartGeneratedMediaJobTool));
    registry.register(Arc::new(TransitionContextTool));
    registry.register(Arc::new(ValidateTransitionChoiceTool));
    registry.register(Arc::new(UpdatePlanTool));
    registry.register(Arc::new(FindBeatTool));
    registry.register(Arc::new(InspectMomentTool));
    registry.register(Arc::new(ViewEpisodeTool));
    registry.register(Arc::new(ViewFrameTool));
    registry.register(Arc::new(ViewTimelineTool));
    registry.register(Arc::new(BrollCandidatesTool));
    registry.register(Arc::new(FindBrollOpportunitiesTool));
    registry.register(Arc::new(FindGeneratedBrollOpportunitiesTool));
    registry.register(Arc::new(SearchBrollTool));
    registry.register(Arc::new(UseBrollTool));
    registry.register(Arc::new(UseGeneratedMediaTool));
    registry.register(Arc::new(DownloadYtClipTool));
    registry.register(Arc::new(VeditCommitTool));
    registry.register(Arc::new(VeditDiffTool));
    registry.register(Arc::new(VeditChangedClipIdsTool));
    registry.register(Arc::new(VeditLogTool));
    registry.register(Arc::new(VeditMergePreflightTool));
    registry.register(Arc::new(VeditRevertTool));
    registry.register(Arc::new(VeditBranchTool));
    registry.register(Arc::new(VeditCheckoutTool));
    registry.register(Arc::new(VeditTagTool));
    registry.register(Arc::new(VeditShowTool));
    registry.register(Arc::new(VeditBlameTool));
    registry.register(Arc::new(ClipSearchTool));
    registry.register(Arc::new(FindEyeContactTool));
    registry.register(Arc::new(FindSpeakerOncamTool));
    registry.register(Arc::new(ShotSummaryTool));
    registry.register(Arc::new(LoadSkillTool));
    registry.register(Arc::new(LocalReviewPackageTool));
    let subagents = Arc::new(SubAgentRegistry::defaults());
    let snapshot = registry.clone();
    registry.register(Arc::new(DelegateTool::new(
        snapshot.clone(),
        subagents.clone(),
        model.to_string(),
    )));
    registry.register(Arc::new(DelegateAllTool::new(
        snapshot,
        subagents,
        model.to_string(),
    )));
    registry
}

/// Run the TUI against a fully built `Session` — used by
/// `resume_cmd` after `Session::resume` has replayed history.
pub fn run_with_session(
    session: std::sync::Arc<Session>,
    approval_rx: mpsc::Receiver<awidat_core::tool::ApprovalRequest>,
    user_input_rx: mpsc::Receiver<awidat_core::tool::UserInputRequest>,
    initial_prompt: Option<String>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(async move {
        let cfg = AppConfig {
            session,
            approval_rx,
            user_input_rx: Some(user_input_rx),
            initial_prompt,
        };
        let app = App::new(&cfg);
        app.run(cfg).await
    })
}

pub fn run_with_initial_prompt(
    project_root: &Path,
    model_override: Option<&str>,
    initial_prompt: Option<String>,
) -> Result<()> {
    if !project_root.is_dir() {
        return Err(anyhow!(
            "project root '{}' is not a directory",
            project_root.display()
        ));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(run_async(project_root, model_override, initial_prompt))
}

async fn run_async(
    project_root: &Path,
    model_override: Option<&str>,
    initial_prompt: Option<String>,
) -> Result<()> {
    let model = model_override.unwrap_or(models::SONNET).to_string();
    let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
        anyhow!(
            "failed to build Anthropic client: {e}. Set ANTHROPIC_API_KEY env var \
             or store via your OS keychain under service 'awidat' account 'anthropic_api_key'."
        )
    })?;

    let registry = build_full_registry(&model);

    let (approval_tx, approval_rx) = mpsc::channel(8);
    let (user_input_tx, user_input_rx) = mpsc::channel(8);

    let mut session_builder = Session::new(
        client,
        registry,
        model,
        Some(SYSTEM_PROMPT.into()),
        project_root,
    )
    .with_approval_channel(approval_tx)
    .with_user_input_channel(user_input_tx);
    // AWIDAT_NO_ROLLOUT=1 disables session recording (tests, ad-hoc runs).
    if std::env::var("AWIDAT_NO_ROLLOUT").ok().as_deref() != Some("1")
        && let Some(state_root) = awidat_config::defaults::state_root()
    {
        session_builder = session_builder.with_recorder(&state_root);
    }
    let session = Arc::new(session_builder);

    let cfg = AppConfig {
        session: session.clone(),
        approval_rx,
        user_input_rx: Some(user_input_rx),
        initial_prompt,
    };
    let app = App::new(&cfg);
    app.run(cfg).await
}
