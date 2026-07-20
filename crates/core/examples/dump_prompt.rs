//! Dump the assembled developer-instructions prompt for a project root.
//!
//! Used by the prompt-layer A/B harness (see
//! `docs/decision-prompt-layer-merge-2026-07-19.md` §6) to generate the
//! config-A developer file from the *actual* production assembly path
//! instead of a hand-copied transcription that could drift.
//!
//! Usage: `cargo run -p montage-core --example dump_prompt -- <project_root>`
//!
//! The learned-style layer is pinned to `None` so the output is
//! deterministic across machines: the A/B compares prompt *architecture*,
//! and the user-scoped lessons state would otherwise leak host-specific
//! text into config A only.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(root) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: dump_prompt <project_root>");
        return ExitCode::FAILURE;
    };
    print!(
        "{}",
        montage_core::system_prompt::assemble_for_project_with_style(&root, None)
    );
    ExitCode::SUCCESS
}
