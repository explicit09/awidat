use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use codex_arg0::arg0_dispatch_or_else;
use codex_exec::Cli as ExecCli;
use codex_exec::run_main;
use codex_login::default_client::set_default_originator;
use codex_utils_cli::CliConfigOverrides;

#[derive(Parser, Debug)]
#[command(
    name = "montage-agent",
    version,
    about = "Codex-backed Montage terminal agent runner.",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Open a non-interactive Codex turn against an Montage project.
    Chat {
        path: PathBuf,
        prompt: Option<String>,
        #[arg(long, short = 'm')]
        model: Option<String>,
        #[arg(long = "yolo", default_value_t = false)]
        yolo: bool,
        #[arg(long = "config", short = 'c', value_name = "key=value")]
        config_overrides: Vec<String>,
    },
    /// Open the Codex TUI against an Montage project.
    Tui {
        path: PathBuf,
        prompt: Option<String>,
        #[arg(long, short = 'm')]
        model: Option<String>,
        #[arg(long = "yolo", default_value_t = false)]
        yolo: bool,
        #[arg(long = "config", short = 'c', value_name = "key=value")]
        config_overrides: Vec<String>,
    },
    /// Resume a previous Codex session.
    Resume {
        selector: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Run a prepared prompt through Codex.
    RunPrepared {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        prompt: String,
        #[arg(long, default_value = "montage agent")]
        display_name: String,
        #[arg(long)]
        model: Option<String>,
    },
}

/// Montage's default model when the caller (CLI flag, config, or codex's own
/// `-c model=...`) hasn't chosen one. gpt-5.6-terra is the refreshed model
/// catalog's "balanced agentic coding model for everyday work" — see
/// vendor/codex-rs/models-manager/models.json — and replaces gpt-5.4, which
/// the catalog no longer lists (`visibility: "hide"`). Montage pins this
/// explicitly rather than relying on codex's own catalog-order default
/// (currently gpt-5.6-sol, the first `visibility: "list"` entry), since that
/// ordering is upstream's to change on any future refresh.
const MONTAGE_DEFAULT_MODEL: &str = "gpt-5.6-terra";

/// Reasoning effort to pair with [`MONTAGE_DEFAULT_MODEL`] when the caller
/// hasn't set one via `-c model_reasoning_effort=...`. Matches
/// gpt-5.6-terra's own catalog `default_reasoning_level` ("medium").
const MONTAGE_DEFAULT_REASONING_EFFORT: &str = "medium";

struct CodexRunOptions {
    prompt: Option<String>,
    dangerously_bypass: bool,
    model: Option<String>,
    config_overrides: Vec<String>,
    project_root: Option<PathBuf>,
    ephemeral: bool,
    skip_git_repo_check: bool,
    display_name: String,
}

impl CodexRunOptions {
    fn execute(self) -> ExitCode {
        if let Err(err) = set_default_originator("codex_cli_rs".to_string()) {
            tracing::warn!(?err, "failed to set montage originator override");
        }

        let abs_project_root = self.project_root.as_deref().map(absolute_project_root);
        if let Some(root) = &abs_project_root {
            // SAFETY: codex_exec's runtime starts after this point.
            unsafe {
                std::env::set_var("MONTAGE_PROJECT_ROOT", root);
            }
        }

        let mut raw_overrides = montage_mcp_overrides(abs_project_root.as_deref());
        raw_overrides.extend(self.config_overrides);
        apply_default_model_overrides(&mut raw_overrides, self.model.is_some());

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

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Chat {
            path,
            prompt,
            model,
            yolo,
            config_overrides,
        } => CodexRunOptions {
            prompt,
            dangerously_bypass: yolo,
            model,
            config_overrides,
            project_root: Some(path),
            ephemeral: false,
            skip_git_repo_check: true,
            display_name: "montage chat".into(),
        }
        .execute(),
        Command::Tui {
            path,
            prompt,
            model,
            yolo,
            config_overrides,
        } => CodexRunOptions {
            prompt,
            dangerously_bypass: yolo,
            model,
            config_overrides,
            project_root: Some(path),
            ephemeral: false,
            skip_git_repo_check: true,
            display_name: "montage tui".into(),
        }
        .execute(),
        Command::Resume { selector, model } => CodexRunOptions {
            prompt: selector,
            dangerously_bypass: false,
            model,
            config_overrides: Vec::new(),
            project_root: std::env::current_dir().ok(),
            ephemeral: false,
            skip_git_repo_check: true,
            display_name: "montage resume".into(),
        }
        .execute(),
        Command::RunPrepared {
            project,
            prompt,
            display_name,
            model,
        } => CodexRunOptions {
            prompt: Some(prompt),
            dangerously_bypass: false,
            model,
            config_overrides: Vec::new(),
            project_root: Some(project),
            ephemeral: false,
            skip_git_repo_check: true,
            display_name,
        }
        .execute(),
    }
}

fn montage_mcp_overrides(project_root: Option<&Path>) -> Vec<String> {
    let Ok(self_exe) = std::env::current_exe() else {
        tracing::warn!("current_exe() failed; skipping montage-mcp-server auto-registration");
        return Vec::new();
    };
    let Some(parent) = self_exe.parent() else {
        return Vec::new();
    };
    let server_bin = parent.join("montage-mcp-server");
    if !server_bin.exists() {
        tracing::warn!(
            path = %server_bin.display(),
            "montage-mcp-server sibling binary missing; codex will run without Montage tools"
        );
        return Vec::new();
    }

    let quoted_cmd = format!("{:?}", server_bin.display().to_string());
    let mut overrides = vec![format!("mcp_servers.montage.command={quoted_cmd}")];
    if let Some(root) = project_root {
        let quoted_root = format!("{:?}", root.display().to_string());
        overrides.push(format!(
            "mcp_servers.montage.env.MONTAGE_PROJECT_ROOT={quoted_root}"
        ));
    }
    overrides
}

/// Appends `-c model=...` / `-c model_reasoning_effort=...` overrides for
/// [`MONTAGE_DEFAULT_MODEL`] / [`MONTAGE_DEFAULT_REASONING_EFFORT`] unless the
/// caller already chose a model (`model_explicitly_set`, e.g. via `--model`)
/// or already has a `model=`/`model_reasoning_effort=` entry in
/// `raw_overrides` (e.g. via `-c model=...` or a user's own config.toml
/// default surfaced as a raw override upstream). Mutates `raw_overrides` in
/// place so callers can push it straight into `CliConfigOverrides`.
fn apply_default_model_overrides(raw_overrides: &mut Vec<String>, model_explicitly_set: bool) {
    let user_set_model = model_explicitly_set
        || raw_overrides
            .iter()
            .any(|entry| entry.starts_with("model="));
    if !user_set_model {
        raw_overrides.push(format!("model=\"{MONTAGE_DEFAULT_MODEL}\""));
    }
    let user_set_reasoning_effort = raw_overrides
        .iter()
        .any(|entry| entry.starts_with("model_reasoning_effort="));
    if !user_set_reasoning_effort {
        raw_overrides.push(format!(
            "model_reasoning_effort=\"{MONTAGE_DEFAULT_REASONING_EFFORT}\""
        ));
    }
}

fn absolute_project_root(root: &Path) -> PathBuf {
    if root.is_absolute() {
        return root.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(root))
        .unwrap_or_else(|_| root.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_model_and_reasoning_effort_when_unset() {
        let mut overrides = vec!["mcp_servers.montage.command=\"/bin/montage-mcp-server\"".into()];
        apply_default_model_overrides(&mut overrides, /*model_explicitly_set*/ false);

        assert!(overrides.contains(&format!("model=\"{MONTAGE_DEFAULT_MODEL}\"")));
        assert!(overrides.contains(&format!(
            "model_reasoning_effort=\"{MONTAGE_DEFAULT_REASONING_EFFORT}\""
        )));
    }

    #[test]
    fn does_not_override_explicit_model_flag() {
        let mut overrides = Vec::new();
        apply_default_model_overrides(&mut overrides, /*model_explicitly_set*/ true);

        assert!(
            !overrides.iter().any(|entry| entry.starts_with("model=")),
            "explicit --model should not get a competing -c model= override: {overrides:?}"
        );
        // Reasoning effort still defaults even when the model was chosen explicitly.
        assert!(overrides.contains(&format!(
            "model_reasoning_effort=\"{MONTAGE_DEFAULT_REASONING_EFFORT}\""
        )));
    }

    #[test]
    fn does_not_override_explicit_config_overrides() {
        let mut overrides = vec![
            "model=\"gpt-5.6-sol\"".to_string(),
            "model_reasoning_effort=\"high\"".to_string(),
        ];
        apply_default_model_overrides(&mut overrides, /*model_explicitly_set*/ false);

        assert_eq!(
            overrides,
            vec![
                "model=\"gpt-5.6-sol\"".to_string(),
                "model_reasoning_effort=\"high\"".to_string(),
            ],
            "user's own -c overrides must win over Montage's defaults"
        );
    }
}
