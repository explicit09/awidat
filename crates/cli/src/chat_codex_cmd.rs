//! `awidat chat-codex` — hello-world spine for the codex-harness
//! migration.
//!
//! Drives the vendored codex agent loop end-to-end against an OpenAI
//! key with zero Awidat tools registered. There's no project loaded,
//! no AGENTS.md, no MCP servers — just the codex runtime answering a
//! prompt. The point is to prove the integration is alive before we
//! start porting the ~100 Awidat tools.
//!
//! Removed once `awidat tui` and `awidat chat` are rewired to codex in
//! step 7 of the migration plan.

use std::process::ExitCode;

use codex_arg0::arg0_dispatch_or_else;
use codex_exec::Cli as ExecCli;
use codex_exec::run_main;

/// Runtime entry point. Hands control to `codex_exec::run_main` via
/// `arg0_dispatch_or_else`, which owns the tokio runtime for the
/// subprocess. Awidat's `main` is intentionally synchronous so this
/// doesn't nest runtimes.
pub fn run(
    prompt: Option<String>,
    dangerously_bypass: bool,
    model: Option<String>,
) -> ExitCode {
    let mut cli = ExecCli {
        prompt,
        // Don't insist on a git repo for the hello-world; the user
        // might run this anywhere just to validate their OpenAI key.
        skip_git_repo_check: true,
        // Don't persist session state.
        ephemeral: true,
        ..ExecCli::default()
    };
    // `dangerously_bypass_approvals_and_sandbox` and `model` live on
    // the inner `SharedCliOptions` reached through Cli's DerefMut.
    cli.dangerously_bypass_approvals_and_sandbox = dangerously_bypass;
    if model.is_some() {
        cli.model = model;
    }

    let result = arg0_dispatch_or_else(|paths| async move { run_main(cli, paths).await });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("codex chat-codex failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}
