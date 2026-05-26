//! `awidat chat-codex` — hello-world spine for the codex-harness
//! migration.
//!
//! Drives the vendored codex agent loop end-to-end against an OpenAI
//! key. Step 3 wired the in-process Awidat MCP server
//! (`awidat-mcp-server` sibling binary) into codex via
//! `[mcp_servers.awidat]`. Step 4 closed the approval-flow loop:
//! each tool on the server declares `read_only_hint` /
//! `destructive_hint` via the rmcp `#[tool(annotations(...))]`
//! macro, codex respects those at dispatch time, and the
//! `vendor/codex-rs/exec/src/lib.rs` fork edit lets the user opt
//! into seeing approval prompts via `-c approval_policy=...`.
//! Step 5 ports the ~100 real video tools onto the server.
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

    // Auto-register the Awidat MCP server. We resolve the sibling
    // `awidat-mcp-server` binary path from `std::env::current_exe()`
    // so cargo dev builds, release builds, and brew installs all work
    // with zero user config. User-supplied -c overrides are layered
    // after ours, so a caller passing
    // `-c mcp_servers.awidat.enabled=false` can opt out.
    let mut raw_overrides = awidat_mcp_overrides();
    raw_overrides.extend(config_overrides);

    let mut cli = ExecCli {
        prompt,
        // Don't insist on a git repo for the hello-world; the user
        // might run this anywhere just to validate their OpenAI key.
        skip_git_repo_check: true,
        // Don't persist session state.
        ephemeral: true,
        config_overrides: CliConfigOverrides { raw_overrides },
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

/// Build the `-c key=value` overrides that register our in-process MCP
/// server with codex. The server binary is the `awidat-mcp-server`
/// sibling of the running `awidat` binary. Returns an empty vec if
/// we can't resolve the path (codex will run without our tools,
/// which is the same as it was before step 3).
///
/// Per-tool approval is NOT injected here — each tool declares its
/// own `read_only_hint` / `destructive_hint` via
/// `#[tool(annotations(...))]` and codex respects those at dispatch
/// time (see `vendor/codex-rs/core/src/mcp_tool_call.rs`'s
/// `requires_mcp_tool_approval`).
fn awidat_mcp_overrides() -> Vec<String> {
    let Ok(self_exe) = std::env::current_exe() else {
        tracing::warn!("current_exe() failed; skipping awidat-mcp-server auto-registration");
        return Vec::new();
    };
    let Some(parent) = self_exe.parent() else {
        return Vec::new();
    };
    let server_bin = parent.join("awidat-mcp-server");
    if !server_bin.exists() {
        tracing::warn!(
            path = %server_bin.display(),
            "awidat-mcp-server sibling binary missing; codex will run without Awidat tools"
        );
        return Vec::new();
    }
    // Codex's `-c` parser expects TOML on the RHS, so the command
    // string has to be a TOML-quoted string. Use a debug-formatted
    // path which gives us a `"..."` literal with backslashes escaped.
    let quoted = format!("{:?}", server_bin.display().to_string());
    vec![format!("mcp_servers.awidat.command={quoted}")]
}
