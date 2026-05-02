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
use awidat_core::tools::bash::BashTool;
use awidat_core::{Session, SessionEvent, ToolRegistry};
use tokio_util::sync::CancellationToken;
use tokio::io::{AsyncBufReadExt, BufReader};

const SYSTEM_PROMPT: &str = "\
You are awidat, a terminal-first agent for editing long-form spoken video. \
This is week-3 — the project state is mostly stub. You have one tool: `bash`. \
Use it to inspect the project, run quick scripts, and answer the user's \
questions. Be concise; prefer one short reply over a long lecture. When you \
call `bash`, explain in one sentence what the command will do.\
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

async fn run_async(_project_root: &Path, model_override: Option<&str>) -> Result<()> {
    let model = model_override.unwrap_or(models::SONNET).to_string();
    let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
        anyhow!(
            "failed to build Anthropic client: {e}. Set ANTHROPIC_API_KEY env var \
             or store via your OS keychain under service 'awidat' account 'anthropic_api_key'."
        )
    })?;

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));

    let session = Arc::new(Session::new(
        client,
        registry,
        model.clone(),
        Some(SYSTEM_PROMPT.into()),
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
