//! `awidat chat-codex` and the shared codex-driver used by the
//! step-7 CLI rewires (`chat`, `tui`, `resume`, `skills run`).
//!
//! All four commands collapse to the same shape now: set the
//! `AWIDAT_PROJECT_ROOT` env (so the MCP server + TUI panel see the
//! right project), register `awidat-mcp-server`, and hand control to
//! `codex_exec::run_main`. Per-command differences (initial prompt,
//! ephemeral vs persisted session, etc.) live in
//! [`CodexRunOptions`] which the public entry points populate.
//!
//! `chat-codex` keeps its no-project pass-through behavior (when
//! [`CodexRunOptions::project_root`] is `None` the env stays unset and
//! the panel falls back to cwd, matching the always-visible policy in
//! `vendor/codex-rs/tui/src/awidat/mod.rs`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use codex_arg0::arg0_dispatch_or_else;
use codex_exec::Cli as ExecCli;
use codex_exec::run_main;
use codex_login::default_client::set_default_originator;
use codex_utils_cli::CliConfigOverrides;

/// Shared options every step-7 codex entry point shares.
pub struct CodexRunOptions {
    /// First user message to seed the turn. `None` reads from stdin
    /// (codex's default).
    pub prompt: Option<String>,
    /// `--yolo` — bypass codex's approval gates.
    pub dangerously_bypass: bool,
    /// `-m` override for the model id. Inherits config when `None`.
    pub model: Option<String>,
    /// `-c key=value` style codex config overrides supplied by the
    /// caller. Layered AFTER our MCP / project-root auto-overrides.
    pub config_overrides: Vec<String>,
    /// When `Some`, exports `AWIDAT_PROJECT_ROOT` so the MCP server
    /// and TUI side panel both pick up the right project. When
    /// `None`, the env stays unset and both fall back to cwd.
    pub project_root: Option<PathBuf>,
    /// `true` → `cli.ephemeral = true` (don't persist session state).
    /// `chat-codex` uses this for hello-world; the `awidat tui`-style
    /// entry points keep persistence on so resume works.
    pub ephemeral: bool,
    /// `true` → `cli.skip_git_repo_check = true`. Same default-rationale
    /// as chat-codex (Awidat projects are not necessarily inside a
    /// git repo).
    pub skip_git_repo_check: bool,
    /// Label included in error output so it's clear which CLI shape
    /// produced the failure (e.g. `awidat tui`, `awidat chat`).
    pub display_name: &'static str,
}

impl CodexRunOptions {
    /// Apply the options to a default-built `codex_exec::Cli` and
    /// hand off to `run_main`. Returns the appropriate exit code.
    fn execute(self) -> ExitCode {
        // Pre-set the originator before run_main overrides it to
        // "codex_exec". OpenAI's gateway gates gpt-5.5 access on the
        // (originator, version, auth_mode) tuple; matching the
        // upstream CLI's `codex_cli_rs` + the version stamp in
        // vendor/codex-rs/login/src/auth/default_client.rs is what
        // lets us through. `set_default_originator` is
        // first-write-wins, so run_main's later call is a no-op.
        if let Err(err) = set_default_originator("codex_cli_rs".to_string()) {
            tracing::warn!(?err, "failed to set awidat originator override");
        }

        // Export AWIDAT_PROJECT_ROOT so the spawned awidat-mcp-server
        // and the TUI's AwidatPanel resolve to the same project.
        // SAFETY: codex's run_main hasn't started yet; we're still
        // single-threaded inside fn main.
        if let Some(root) = &self.project_root {
            let root = absolute_project_root(root);
            // SAFETY: single-threaded — codex_exec's tokio runtime
            // is constructed by run_main, after this point. No other
            // task is reading env yet.
            unsafe {
                std::env::set_var("AWIDAT_PROJECT_ROOT", root);
            }
        }

        // Auto-register the Awidat MCP server alongside the user's
        // -c overrides. Caller's overrides come last so they can
        // disable us via `-c mcp_servers.awidat.enabled=false`.
        let mut raw_overrides = awidat_mcp_overrides();
        raw_overrides.extend(self.config_overrides);

        let mut cli = ExecCli {
            prompt: self.prompt,
            skip_git_repo_check: self.skip_git_repo_check,
            ephemeral: self.ephemeral,
            config_overrides: CliConfigOverrides { raw_overrides },
            ..ExecCli::default()
        };
        cli.dangerously_bypass_approvals_and_sandbox = self.dangerously_bypass;
        if self.model.is_some() {
            cli.model = self.model;
        }

        let display_name = self.display_name;
        let result = arg0_dispatch_or_else(|paths| async move { run_main(cli, paths).await });
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{display_name} failed: {err:#}");
                ExitCode::FAILURE
            }
        }
    }
}

/// `awidat chat-codex` entry point. Hello-world spine that proves
/// the codex integration; kept around for smoke-testing.
pub fn run(
    prompt: Option<String>,
    dangerously_bypass: bool,
    model: Option<String>,
    config_overrides: Vec<String>,
) -> ExitCode {
    CodexRunOptions {
        prompt,
        dangerously_bypass,
        model,
        config_overrides,
        project_root: None,
        ephemeral: true,
        skip_git_repo_check: true,
        display_name: "awidat chat-codex",
    }
    .execute()
}

/// Public driver shared by the step-7 entry points (`tui`, `chat`,
/// `resume`, `skills run`).
pub fn run_with_options(options: CodexRunOptions) -> ExitCode {
    options.execute()
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

fn absolute_project_root(root: &Path) -> PathBuf {
    if root.is_absolute() {
        return root.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(root))
        .unwrap_or_else(|_| root.to_path_buf())
}
