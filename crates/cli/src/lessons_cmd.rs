//! `montage lessons {learn,show}` — STUBBED during the codex-harness migration.
//!
//! The original implementation scanned legacy Montage rollout files via
//! `montage_core::rollout::Recorder::collect_decisions` and distilled
//! `EditorialDecision` events into `learned-style.md`. Codex stores its
//! rollouts at `~/.codex/sessions/` in a different format; the
//! `Recorder` API and the legacy rollout layout are scheduled for
//! deletion in step 8e/W.
//!
//! This subcommand stays wired into the clap enum so `montage lessons …`
//! prints a clear "not yet ported" message instead of failing with a
//! linker error. The re-port onto codex's `RolloutRecorder` API is
//! tracked alongside #60 (`history.rs` re-target).
//!
//! See `crates/core/src/lessons.rs` — the pattern extraction and
//! markdown rendering helpers are still live and reusable once a
//! codex-format reader replaces `Recorder::collect_decisions`.

use anyhow::Result;

pub fn learn() -> Result<()> {
    eprintln!(
        "montage lessons learn: pending re-port onto codex rollouts \
         (~/.codex/sessions/). Legacy Montage rollout reader was retired \
         during the codex-harness migration."
    );
    Ok(())
}

pub fn show() -> Result<()> {
    eprintln!(
        "montage lessons show: pending re-port onto codex rollouts \
         (~/.codex/sessions/). Legacy Montage rollout reader was retired \
         during the codex-harness migration."
    );
    Ok(())
}
