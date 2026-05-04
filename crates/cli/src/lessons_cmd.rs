//! `awidat lessons {learn,show}` — distill learned style from past
//! sessions, or print the current rendered file.
//!
//! `learn` reads every `EditorialDecision` under `state_root` (#149)
//! and writes a fresh `learned-style.md` to the user config dir
//! (#150). `show` prints whatever's there now. The `Session::new`
//! path reads that same file at session start and splices it into
//! the system prompt.
//!
//! V1 surface is intentionally narrow — no `--clear`, no `--explain`.
//! The full-rebuild semantics are the right default: every `learn`
//! overwrites with patterns derived from current data.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use awidat_core::lessons;
use awidat_core::rollout::Recorder;

pub fn learn() -> Result<()> {
    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| anyhow!("could not resolve state root"))?;
    let out_path = lessons::default_output_path()
        .ok_or_else(|| anyhow!("could not resolve user config dir for learned-style.md"))?;

    let decisions = Recorder::collect_decisions(&state_root)
        .with_context(|| format!("failed to read decisions under {}", state_root.display()))?;
    let total = decisions.len();
    if total == 0 {
        println!(
            "No editorial decisions found yet under {}.\n\
             Run `awidat tui`, accept or deny some apply_edl / start_render / bash \
             modals, and try again.",
            state_root.join("sessions").display()
        );
        return Ok(());
    }

    let patterns = lessons::extract_from_decisions(&decisions);
    let Some(body) = lessons::render_markdown(&patterns, total) else {
        println!(
            "{} decisions read, but no patterns crossed the significance threshold.\n\
             Patterns require ≥{} events with ≥{:.0}pp deviation from baseline.\n\
             Keep using awidat — patterns surface as the histogram fills in.",
            total,
            lessons::MIN_EVENTS,
            lessons::MIN_DEVIATION_PP,
        );
        return Ok(());
    };

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&out_path, &body)
        .with_context(|| format!("failed to write {}", out_path.display()))?;

    println!(
        "Wrote {} pattern{} to {}",
        patterns.len(),
        if patterns.len() == 1 { "" } else { "s" },
        out_path.display()
    );
    println!("(distilled from {total} decisions across all rollout files)");
    println!("\nPreview:\n");
    print!("{body}");
    Ok(())
}

pub fn show() -> Result<()> {
    let path = lessons::default_output_path()
        .ok_or_else(|| anyhow!("could not resolve user config dir"))?;
    match lessons::read_learned_style(&path) {
        Some(body) => {
            println!("# {}", path.display());
            println!();
            print!("{body}");
        }
        None => {
            println!(
                "No learned style at {}. Run `awidat lessons learn` to build one from \
                 your session history.",
                path.display()
            );
        }
    }
    Ok(())
}

/// Override path for tests + advanced users. Not currently surfaced
/// by the CLI but kept here so a future `--out <path>` flag is a
/// one-line wiring change.
#[allow(dead_code)]
pub fn show_at(path: &PathBuf) -> Result<()> {
    match lessons::read_learned_style(path) {
        Some(body) => {
            print!("{body}");
            Ok(())
        }
        None => Err(anyhow!("no learned style at {}", path.display())),
    }
}
