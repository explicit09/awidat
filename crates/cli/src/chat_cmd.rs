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
    apply_edl::ApplyEdlTool, bash::BashTool, broll_candidates::BrollCandidatesTool,
    clip_search::ClipSearchTool, find_beat::FindBeatTool,
    find_eye_contact::FindEyeContactTool, find_moment::FindMomentTool,
    find_speaker_oncam::FindSpeakerOncamTool, inspect_clip::InspectClipTool,
    inspect_moment::InspectMomentTool, list_assets::ListAssetsTool,
    load_skill::LoadSkillTool, poll_render::PollRenderTool,
    read_index::ReadIndexTool, request_user_input::RequestUserInputTool,
    shot_summary::ShotSummaryTool, start_indexing::StartIndexingTool,
    start_render::StartRenderTool, update_plan::UpdatePlanTool,
    view_episode::ViewEpisodeTool,
    view_frame::ViewFrameTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, SessionEvent, ToolRegistry};
use tokio_util::sync::CancellationToken;
use tokio::io::{AsyncBufReadExt, BufReader};

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
\n\nYou have 21 tools, organized by purpose:\
\n  - **Discovery / map**: view_episode (compact map of the project — \
includes which vision indexers have run), view_timeline, list_assets.\
\n  - **Editorial index**: find_beat (typed editorial moments — \
hooks, punchlines, CTAs, etc.), inspect_moment (drill into one beat \
with surrounding transcript + dependencies). Prefer these over \
find_moment when the user asks for editorial intent.\
\n  - **Vision** (only useful when view_episode shows the matching \
indexer ran): clip_search (free-text frame search), shot_summary, \
broll_candidates, find_speaker_oncam, find_eye_contact.\
\n  - **Raw lookup**: find_moment (transcript substring), read_index, \
inspect_clip, view_frame.\
\n  - **Editing**: apply_edl (Trim, Untrim, Delete, Split, Insert).\
\n  - **Indexing**: start_indexing — run the configured indexers \
(whisper, scenes, audio energy, beats, etc.) over assets in raw/. \
Sha-keyed so re-runs on already-indexed assets are fast no-ops. \
Call when view_episode shows missing sidecars and the user has \
asked for an editorial operation that needs them. Don't proactively \
re-index already-indexed projects.\
\n  - **Render**: start_render, poll_render. Use scope='timeline' to \
render the edited timeline; scope='preview' renders the raw asset.\
\n  - **Plan / collab**: update_plan, request_user_input, bash.\
\n  - **Skills**: load_skill — load a named editorial workflow's \
full playbook (style + step-by-step). When the user's request maps \
to a skill in the catalog below the system prompt, call \
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
    // Same 20-tool registry as `awidat tui`. Chat is the
    // text-only fallback; both surfaces should expose the same
    // editorial powers.
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FindMomentTool));
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

    let session = Arc::new(Session::new(
        client,
        registry,
        model.clone(),
        Some(SYSTEM_PROMPT.into()),
        project_root,
    ));

    println!("awidat chat — model={model}, tools={}", session.tool_count());
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

        // Subscribe to events for this turn.
        let mut events = session.subscribe();
        let cancel = CancellationToken::new();

        // Drive the turn in the background; render events on the
        // foreground task.
        let session_clone = session.clone();
        let cancel_clone = cancel.clone();
        let user_input = trimmed.to_string();
        let turn_handle = tokio::spawn(async move {
            session_clone.run_turn(user_input, cancel_clone).await
        });

        // Wire Ctrl-C to cancel; restore on next prompt.
        let cancel_for_signal = cancel.clone();
        let signal_handle = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel_for_signal.cancel();
            }
        });

        // Render events until TurnEnd.
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
        // Make sure the turn task finished before next prompt; surface
        // unrecoverable errors.
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
            // One-line preview of the args; full args are echoed on the
            // ToolCallStart line in week 5+ TUI. For week 3 keep it terse.
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
        SessionEvent::AwaitingUserInput { question, options, .. } => {
            println!("\n❓ {question}");
            if let Some(opts) = options {
                for (i, o) in opts.iter().enumerate() {
                    println!("   [{i}] {o}");
                }
            }
            // The chat REPL doesn't yet drive the user_input channel —
            // wired in when we add the runtime input loop. For now this
            // event is informational; the tool will time out / cancel.
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
