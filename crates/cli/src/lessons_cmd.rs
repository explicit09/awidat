//! `montage lessons {learn,show}` — distill editorial decisions into
//! learned-style.md.
//!
//! Decisions are appended by live MCP `apply_edl` commits into
//! `editorial-decisions.jsonl` (see `montage_core::lessons`).

use anyhow::{Result, bail};

pub fn learn() -> Result<()> {
    let (path, decisions, patterns) = montage_core::lessons::learn_from_disk()
        .map_err(|e| anyhow::anyhow!("montage lessons learn: {e}"))?;
    println!(
        "montage lessons learn: {decisions} decision(s) → {patterns} pattern(s) → {}",
        path.display()
    );
    Ok(())
}

pub fn show() -> Result<()> {
    let Some(path) = montage_core::lessons::default_output_path() else {
        bail!("montage lessons show: no config directory available");
    };
    match montage_core::lessons::read_learned_style(&path) {
        Some(md) => {
            println!("{md}");
            Ok(())
        }
        None => {
            println!(
                "montage lessons show: no learned-style at {} (run `montage lessons learn` first)",
                path.display()
            );
            Ok(())
        }
    }
}
