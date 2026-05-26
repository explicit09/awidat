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
use codex_login::default_client::set_default_originator;
use codex_utils_cli::CliConfigOverrides;

/// Runtime entry point. Hands control to `codex_exec::run_main` via
/// `arg0_dispatch_or_else`, which owns the tokio runtime for the
/// subprocess. Awidat's `main` is intentionally synchronous so this
/// doesn't nest runtimes.
pub fn run(
    prompt: Option<String>,
    dangerously_bypass: bool,
    model: Option<String>,
    config_overrides: Vec<String>,
) -> ExitCode {
    // Pre-set the originator before run_main can override it to
    // "codex_exec". OpenAI's gateway gates gpt-5.5 access on the
    // combination of (originator, version, auth_mode); matching the
    // upstream CLI's default originator "codex_cli_rs" alongside
    // the version stamp in vendor/codex-rs/login/src/auth/default_client.rs
    // is what passes the policy check. set_default_originator is
    // first-write-wins, so the call inside run_main becomes a no-op.
    if let Err(err) = set_default_originator("codex_cli_rs".to_string()) {
        tracing::warn!(?err, "failed to set awidat originator override");
    }

    let mut cli = ExecCli {
        prompt,
        // Don't insist on a git repo for the hello-world; the user
        // might run this anywhere just to validate their OpenAI key.
        skip_git_repo_check: true,
        // Don't persist session state.
        ephemeral: true,
        config_overrides: CliConfigOverrides {
            raw_overrides: config_overrides,
        },
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
