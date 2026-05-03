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
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, bash::BashTool, find_moment::FindMomentTool,
    inspect_clip::InspectClipTool, list_assets::ListAssetsTool, poll_render::PollRenderTool,
    read_index::ReadIndexTool, request_user_input::RequestUserInputTool,
    start_render::StartRenderTool, update_plan::UpdatePlanTool, view_frame::ViewFrameTool,
    view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, ToolRegistry};
use awidat_tui::{App, AppConfig};
use tokio::sync::mpsc;

const SYSTEM_PROMPT: &str = "\
You are awidat, a terminal-first agent for editing long-form spoken \
video. You have 12 tools: find_moment, view_timeline, inspect_clip, \
view_frame, list_assets, read_index, start_render, poll_render, \
update_plan, request_user_input, apply_edl, bash. \
\n\n\
Mutating tools (apply_edl, start_render, bash) require user approval — \
the UI shows a modal and the user picks Allow / Allow-for-Session / \
Deny. If the user denies, you'll see is_error=true; route around it \
rather than retrying the same call.\
\n\n\
EDL format (freeform, NOT JSON-escaped):\n\
*** Begin EDL\n\
*** Trim Clip|Delete Clip|Insert BRoll|Move Clip|Insert Transition\n\
@@ anchor: transcript_snippet=\"...\" or clip_uuid=...\n\
+ key: value\n\
*** End EDL\n\
\n\
Be concise. Commit edits via apply_edl directly when you're confident.\
";

pub fn run(project_root: &Path, model_override: Option<&str>) -> Result<()> {
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
    runtime.block_on(run_async(project_root, model_override))
}

async fn run_async(project_root: &Path, model_override: Option<&str>) -> Result<()> {
    let model = model_override.unwrap_or(models::SONNET).to_string();
    let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
        anyhow!(
            "failed to build Anthropic client: {e}. Set ANTHROPIC_API_KEY env var \
             or store via your OS keychain under service 'awidat' account 'anthropic_api_key'."
        )
    })?;

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(UpdatePlanTool));
    registry.register(Arc::new(ViewFrameTool));
    registry.register(Arc::new(ViewTimelineTool));

    let (approval_tx, approval_rx) = mpsc::channel(8);
    let (user_input_tx, user_input_rx) = mpsc::channel(8);

    let session = Arc::new(
        Session::new(
            client,
            registry,
            model,
            Some(SYSTEM_PROMPT.into()),
            project_root,
        )
        .with_approval_channel(approval_tx)
        .with_user_input_channel(user_input_tx),
    );

    let cfg = AppConfig {
        session: session.clone(),
        approval_rx,
        user_input_rx: Some(user_input_rx),
    };
    let app = App::new(&cfg);
    app.run(cfg).await
}
