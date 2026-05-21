//! `awidat chat` subcommand. Aider-shaped REPL: prompt, run-turn, render
//! streamed events, repeat.
//!
//! Per the corpus survey, week-3 stops short of mid-stream user input
//! (Codex's `interrupt_rx` channel pattern). EOF / `:quit` ends the loop.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tools::{
    analyze_sync::AnalyzeSyncTool, apply_edl::ApplyEdlTool,
    assess_continuity::AssessContinuityTool, assess_edit_quality::AssessEditQualityTool,
    bash::BashTool, broll_candidates::BrollCandidatesTool, clip_search::ClipSearchTool,
    color_scopes::ColorScopesTool, diagnose_project_media::DiagnoseProjectMediaTool,
    download_yt_clip::DownloadYtClipTool, export_package::ExportPackageTool,
    find_beat::FindBeatTool, find_black_frames::FindBlackFramesTool,
    find_broll_opportunities::FindBrollOpportunitiesTool, find_dead_air::FindDeadAirTool,
    find_episode_start::FindEpisodeStartTool, find_eye_contact::FindEyeContactTool,
    find_false_starts::FindFalseStartsTool, find_filler_words::FindFillerWordsTool,
    find_moment::FindMomentTool, find_speaker_oncam::FindSpeakerOncamTool,
    inspect_clip::InspectClipTool, inspect_moment::InspectMomentTool, list_assets::ListAssetsTool,
    list_looks::ListLooksTool, load_skill::LoadSkillTool, plan_emphasis::PlanEmphasisTool,
    plan_look_regions::PlanLookRegionsTool, plan_look_regions::ReviewLookRegionsTool,
    plan_look_regions::StartLookRegionPassTool, plan_multicam::PlanMulticamTool,
    plan_reframe::PlanReframeTool, plan_transition::PlanTransitionTool,
    poll_render::PollRenderTool, read_index::ReadIndexTool,
    request_user_input::RequestUserInputTool, search_broll::SearchBrollTool,
    shot_summary::ShotSummaryTool, start_indexing::StartIndexingTool,
    start_render::StartRenderTool, transition_context::TransitionContextTool,
    update_plan::UpdatePlanTool, use_broll::UseBrollTool,
    validate_transition_choice::ValidateTransitionChoiceTool, vedit_commit::VeditCommitTool,
    vedit_diff::VeditDiffTool, vedit_log::VeditLogTool, vedit_revert::VeditRevertTool,
    view_episode::ViewEpisodeTool, view_frame::ViewFrameTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, SessionEvent, ToolRegistry};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

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
includes which vision indexers have run), view_timeline, list_assets, \
find_episode_start (publishable podcast/interview start; rejects \
pre-roll and rehearsed intros).\
\n  - **Editorial index**: find_beat (typed editorial moments — \
hooks, punchlines, CTAs, etc.), inspect_moment (drill into one beat \
with surrounding transcript + dependencies). Prefer these over \
find_moment when the user asks for editorial intent.\
\n  - **Vision** (only useful when view_episode shows the matching \
indexer ran): clip_search (free-text frame search), shot_summary, \
broll_candidates, find_speaker_oncam, find_eye_contact, plan_reframe \
(static vertical/social crop fragment from subject-center evidence).\
\n  - **Raw lookup**: find_moment (transcript substring), read_index, \
inspect_clip, view_frame, color_scopes (histogram/waveform/parade/vectorscope \
evidence for one frame).\
\n  - **Edit quality**: assess_edit_quality before risky trims/splits; it recommends hard cut, recut, J/L split edit, b-roll, or motivated transition. transition_context assembles handles, transcript, frames, and continuity context before choosing a visible transition; plan_transition turns that packet into a hard-cut or visible-transition proposal. assess_continuity is the lower-level rule breakdown.\
\n  - **Editing**: apply_edl (Trim, Untrim, Delete, Split, Insert, Insert PiP). \
For `@@ anchor: clip_uuid=...`, use the clip anchor shown by \
view_timeline, usually the clip name like `clip-0`; never use the \
asset filename, proxy stem, or raw media basename as clip_uuid. \
Times are source-media seconds. view_timeline shows current \
`source=[start..end]`; to trim the first N seconds of the visible \
clip, set `start` to source start + N, and to trim the last N \
seconds, set `end` to source end - N.\
\n  - **Indexing**: start_indexing — run the configured indexers \
(whisper, scenes, audio energy, beats, etc.) over assets in raw/. \
Sha-keyed so re-runs on already-indexed assets are fast no-ops. \
Call when view_episode shows missing sidecars and the user has \
asked for an editorial operation that needs them. Don't proactively \
re-index already-indexed projects.\
\n  - **Render**: start_render, poll_render. Use scope='timeline' to \
render the edited timeline; scope='preview' renders the raw asset.\
\n  - **Finishing**: start_look_region_pass runs the LUT plan -> \
apply_edl -> timeline render sequence; plan_look_regions drafts only \
the generated LUT EDL; review_look_regions turns a completed render \
into contact-sheet/JSON/Markdown proof.\
\n  - **Plan / collab**: update_plan, request_user_input, bash.\
\n  - **Skills**: load_skill — load a named editorial workflow's \
full playbook (style + step-by-step). When the user's request maps \
to a skill in the per-turn skills catalog, call \
load_skill(name=...) BEFORE the work.\
\n\n\
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
    registry.register(Arc::new(AnalyzeSyncTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(DiagnoseProjectMediaTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(FindBlackFramesTool));
    registry.register(Arc::new(FindDeadAirTool));
    registry.register(Arc::new(FindEpisodeStartTool));
    registry.register(Arc::new(FindFillerWordsTool));
    registry.register(Arc::new(FindFalseStartsTool));
    registry.register(Arc::new(AssessContinuityTool));
    registry.register(Arc::new(AssessEditQualityTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(ListLooksTool));
    registry.register(Arc::new(ColorScopesTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(ExportPackageTool));
    registry.register(Arc::new(StartLookRegionPassTool));
    registry.register(Arc::new(PlanLookRegionsTool));
    registry.register(Arc::new(ReviewLookRegionsTool));
    registry.register(Arc::new(PlanEmphasisTool));
    registry.register(Arc::new(PlanMulticamTool));
    registry.register(Arc::new(PlanReframeTool));
    registry.register(Arc::new(PlanTransitionTool));
    registry.register(Arc::new(StartIndexingTool));
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

    let session = Arc::new(Session::new(
        client,
        registry,
        model.clone(),
        Some(SYSTEM_PROMPT.into()),
        project_root,
    ));

    println!(
        "awidat chat — model={model}, tools={}",
        session.tool_count()
    );
    println!("Type a prompt. EOF (Ctrl-D) or :quit to exit. Ctrl-C cancels the in-flight turn.\n");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        print!("> ");
        std::io::stdout().flush().ok();

        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => {
                println!();
                return Ok(());
            }
            Err(e) => return Err(anyhow!("stdin error: {e}")),
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, ":quit" | ":q" | ":exit") {
            return Ok(());
        }

        let mut events = session.subscribe();
        let cancel = CancellationToken::new();

        let session_clone = session.clone();
        let cancel_clone = cancel.clone();
        let user_input = trimmed.to_string();
        let turn_handle =
            tokio::spawn(async move { session_clone.run_turn(user_input, cancel_clone).await });

        // Ctrl-C cancels the in-flight turn but should not exit the REPL.
        let cancel_for_signal = cancel.clone();
        let signal_handle = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel_for_signal.cancel();
            }
        });

        loop {
            match events.recv().await {
                Ok(ev) => {
                    if render_event(&ev) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    println!("\n[lagged {n} events; continuing]");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        if let Err(e) = turn_handle.await? {
            eprintln!("\nturn error: {e}");
        }
        signal_handle.abort();
    }
}

/// Render one event to stdout. Returns true if this event ends the turn.
fn render_event(ev: &SessionEvent) -> bool {
    match ev {
        SessionEvent::TurnStart => {}
        SessionEvent::MessageStart { .. } => {}
        SessionEvent::TextDelta(t) => {
            print!("{t}");
            std::io::stdout().flush().ok();
        }
        SessionEvent::ToolCallStart { name, .. } => {
            println!("\n▶ {name}(...)");
        }
        SessionEvent::ToolCallArgs { args, .. } => {
            let preview = args.to_string();
            let preview = if preview.len() > 200 {
                format!("{}…", &preview[..200])
            } else {
                preview
            };
            println!("  args: {preview}");
        }
        SessionEvent::ToolResult { result, .. } => match result {
            Ok(out) => {
                let preview = if out.len() > 1000 {
                    format!("{}…[+{} chars]", &out[..1000], out.len() - 1000)
                } else {
                    out.clone()
                };
                for line in preview.lines() {
                    println!("  │ {line}");
                }
            }
            Err(err) => {
                println!("  ✗ {err}");
            }
        },
        SessionEvent::SamplingComplete { .. } => {}
        SessionEvent::EditPlanUpdate { items, note } => {
            println!("\n📝 plan updated:");
            for it in items {
                let glyph = match it.status.as_str() {
                    "completed" => "✓",
                    "in_progress" => "▶",
                    _ => " ",
                };
                println!("  [{glyph}] {}", it.step);
            }
            if let Some(n) = note {
                println!("  note: {n}");
            }
        }
        SessionEvent::AwaitingUserInput {
            question, options, ..
        } => {
            println!("\n❓ {question}");
            if let Some(opts) = options {
                for (i, o) in opts.iter().enumerate() {
                    println!("   [{i}] {o}");
                }
            }
            // The REPL has no input channel for tool-driven prompts;
            // the tool eventually times out or the user cancels.
        }
        SessionEvent::TurnEnd => {
            println!();
            return true;
        }
        SessionEvent::Error(msg) => {
            println!("\n[error] {msg}");
            return true;
        }
    }
    false
}
