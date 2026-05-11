//! Session construction: builds the `awidat-core` `ToolRegistry` and
//! the `Session` itself. Factored out so multiple commands (and any
//! future re-builders) can call into the same recipe.

use std::path::PathBuf;
use std::sync::Arc;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tool::{ApprovalRequest, UserInputRequest};
use awidat_core::tools::{
    analyze_sync::AnalyzeSyncTool, apply_edl::ApplyEdlTool,
    assess_continuity::AssessContinuityTool, bash::BashTool, broll_candidates::BrollCandidatesTool,
    clip_search::ClipSearchTool, download_yt_clip::DownloadYtClipTool,
    export_package::ExportPackageTool, find_beat::FindBeatTool,
    find_broll_opportunities::FindBrollOpportunitiesTool, find_dead_air::FindDeadAirTool,
    find_episode_start::FindEpisodeStartTool, find_eye_contact::FindEyeContactTool,
    find_false_starts::FindFalseStartsTool, find_filler_words::FindFillerWordsTool,
    find_moment::FindMomentTool, find_speaker_oncam::FindSpeakerOncamTool,
    inspect_clip::InspectClipTool, inspect_moment::InspectMomentTool, list_assets::ListAssetsTool,
    load_skill::LoadSkillTool, plan_multicam::PlanMulticamTool, poll_render::PollRenderTool,
    read_index::ReadIndexTool, request_user_input::RequestUserInputTool,
    search_broll::SearchBrollTool, shot_summary::ShotSummaryTool,
    start_indexing::StartIndexingTool, start_render::StartRenderTool, update_plan::UpdatePlanTool,
    use_broll::UseBrollTool, vedit_commit::VeditCommitTool, vedit_diff::VeditDiffTool,
    vedit_log::VeditLogTool, vedit_revert::VeditRevertTool, view_episode::ViewEpisodeTool,
    view_frame::ViewFrameTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, ToolRegistry};
use tokio::sync::mpsc;

/// Build the full editorial-tool registry. Mirrors the TUI's set.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(AnalyzeSyncTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(FindDeadAirTool));
    registry.register(Arc::new(FindEpisodeStartTool));
    registry.register(Arc::new(FindFillerWordsTool));
    registry.register(Arc::new(FindFalseStartsTool));
    registry.register(Arc::new(AssessContinuityTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(ExportPackageTool));
    registry.register(Arc::new(PlanMulticamTool));
    registry.register(Arc::new(StartIndexingTool));
    registry.register(Arc::new(UpdatePlanTool));
    registry.register(Arc::new(FindBeatTool));
    registry.register(Arc::new(InspectMomentTool));
    registry.register(Arc::new(ViewEpisodeTool));
    registry.register(Arc::new(ViewFrameTool));
    registry.register(Arc::new(ViewTimelineTool));
    registry.register(Arc::new(BrollCandidatesTool));
    registry.register(Arc::new(FindBrollOpportunitiesTool));
    registry.register(Arc::new(SearchBrollTool));
    registry.register(Arc::new(UseBrollTool));
    registry.register(Arc::new(DownloadYtClipTool));
    registry.register(Arc::new(VeditCommitTool));
    registry.register(Arc::new(VeditDiffTool));
    registry.register(Arc::new(VeditLogTool));
    registry.register(Arc::new(VeditRevertTool));
    registry.register(Arc::new(ClipSearchTool));
    registry.register(Arc::new(FindEyeContactTool));
    registry.register(Arc::new(FindSpeakerOncamTool));
    registry.register(Arc::new(ShotSummaryTool));
    registry.register(Arc::new(LoadSkillTool));
    registry
}

/// Build a fresh `Session` rooted at `project_root`.
///
/// The system prompt is assembled per-project by
/// `awidat_core::system_prompt::assemble_for_project`, which folds in
/// the project type's editorial defaults (podcast / shorts / tutorial
/// / other) and the active permission mode read from
/// `.awidat/permission_mode`. Both reads fall back to Other + Manual
/// on missing/corrupt state.
pub async fn build_session(
    project_root: PathBuf,
    approval_tx: mpsc::Sender<ApprovalRequest>,
    user_input_tx: mpsc::Sender<UserInputRequest>,
) -> Result<Arc<Session>, String> {
    let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
        format!(
            "Anthropic client: {e}. Set ANTHROPIC_API_KEY or store in OS keychain \
             (service 'awidat', account 'anthropic_api_key')."
        )
    })?;

    let prompt = awidat_core::system_prompt::assemble_for_project(&project_root);

    let mut session = Session::new(
        client,
        build_registry(),
        models::SONNET.to_string(),
        Some(prompt),
        project_root,
    )
    .with_approval_channel(approval_tx)
    .with_user_input_channel(user_input_tx);

    if std::env::var("AWIDAT_NO_ROLLOUT").ok().as_deref() != Some("1")
        && let Some(state_root) = awidat_config::defaults::state_root()
    {
        session = session.with_recorder(&state_root);
    }

    Ok(Arc::new(session))
}

/// Resume a previous rollout and wire it into the desktop approval /
/// user-input bridges. A resumed run writes to a fresh JSONL file with
/// lineage back to `log_path`, matching the CLI resume semantics.
pub async fn resume_session(
    log_path: PathBuf,
    approval_tx: mpsc::Sender<ApprovalRequest>,
    user_input_tx: mpsc::Sender<UserInputRequest>,
) -> Result<Arc<Session>, String> {
    if std::env::var("AWIDAT_NO_ROLLOUT").ok().as_deref() == Some("1") {
        return Err("session resume is disabled by AWIDAT_NO_ROLLOUT=1".into());
    }

    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| "could not resolve awidat state root".to_string())?;
    let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
        format!(
            "Anthropic client: {e}. Set ANTHROPIC_API_KEY or store in OS keychain \
             (service 'awidat', account 'anthropic_api_key')."
        )
    })?;

    let session = Session::resume(client, build_registry(), &log_path, &state_root)
        .await
        .map_err(|e| e.to_string())?
        .with_approval_channel(approval_tx)
        .with_user_input_channel(user_input_tx);

    Ok(Arc::new(session))
}
