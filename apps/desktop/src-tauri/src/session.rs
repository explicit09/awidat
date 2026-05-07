//! Session construction: builds the `awidat-core` `ToolRegistry` and
//! the `Session` itself. Factored out so multiple commands (and any
//! future re-builders) can call into the same recipe.

use std::path::PathBuf;
use std::sync::Arc;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tool::{ApprovalRequest, UserInputRequest};
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, bash::BashTool, broll_candidates::BrollCandidatesTool,
    clip_search::ClipSearchTool, find_beat::FindBeatTool, find_dead_air::FindDeadAirTool,
    find_eye_contact::FindEyeContactTool, find_false_starts::FindFalseStartsTool,
    find_filler_words::FindFillerWordsTool, find_moment::FindMomentTool,
    find_speaker_oncam::FindSpeakerOncamTool, inspect_clip::InspectClipTool,
    inspect_moment::InspectMomentTool, list_assets::ListAssetsTool, load_skill::LoadSkillTool,
    poll_render::PollRenderTool, read_index::ReadIndexTool,
    request_user_input::RequestUserInputTool, shot_summary::ShotSummaryTool,
    start_indexing::StartIndexingTool, start_render::StartRenderTool, update_plan::UpdatePlanTool,
    view_episode::ViewEpisodeTool, view_frame::ViewFrameTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, ToolRegistry};
use tokio::sync::mpsc;

// Step 1.9: the system prompt is no longer a static const — it's
// assembled per-project by `awidat_core::system_prompt::assemble_for
// _project`, which folds in the project type's editorial defaults
// (podcast / shorts / tutorial / other) plus the active permission
// mode. See crates/core/src/system_prompt.rs.

/// Build the full editorial-tool registry. Mirrors the TUI's set; the
/// desktop is the buyer surface and gets every tool the agent has.
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(FindDeadAirTool));
    registry.register(Arc::new(FindFillerWordsTool));
    registry.register(Arc::new(FindFalseStartsTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(StartIndexingTool));
    registry.register(Arc::new(UpdatePlanTool));
    registry.register(Arc::new(FindBeatTool));
    registry.register(Arc::new(InspectMomentTool));
    registry.register(Arc::new(ViewEpisodeTool));
    registry.register(Arc::new(ViewFrameTool));
    registry.register(Arc::new(ViewTimelineTool));
    registry.register(Arc::new(BrollCandidatesTool));
    registry.register(Arc::new(ClipSearchTool));
    registry.register(Arc::new(FindEyeContactTool));
    registry.register(Arc::new(FindSpeakerOncamTool));
    registry.register(Arc::new(ShotSummaryTool));
    registry.register(Arc::new(LoadSkillTool));
    registry
}

/// Build a fresh `Session` rooted at `project_root`. Step 1.9
/// replaced the static `SYSTEM_PROMPT` constant with a per-project
/// builder: the prompt now includes format-specific defaults
/// (podcast / shorts / tutorial / other) read from the OTIO
/// metadata, plus the active permission mode read from
/// `.awidat/permission_mode`. Both reads tolerate missing/corrupt
/// state and fall back to Other + Manual.
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

    let session = Session::new(
        client,
        build_registry(),
        models::SONNET.to_string(),
        Some(prompt),
        project_root,
    )
    .with_approval_channel(approval_tx)
    .with_user_input_channel(user_input_tx);

    Ok(Arc::new(session))
}
