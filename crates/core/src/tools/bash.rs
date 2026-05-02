//! `bash` tool — runs a shell command, returns combined stdout+stderr.
//!
//! Week 3 shape per the corpus survey: simpler than Codex's argv-vector
//! `shell` (Crush's single-command shape), with Codex's truncation
//! discipline (cap at 30KB, middle-elide with line-count header).
//!
//! Sandboxing is **stubbed** for week 3 — we run via `bash -lc <command>`
//! with no isolation. Week 7 wraps the same surface with seatbelt
//! (macOS) and landlock+seccomp (Linux), per `PLAN.md` §10.1.
//!
//! ## Banned commands
//!
//! Pre-week-7 safety net: a static reject list lifted from
//! `harnesses/crush/internal/agent/tools/bash.go:71-128`. Network
//! tools, package managers, system-mutating tools. The model gets a
//! `RespondToModel` error explaining what's banned and why.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolHandler, ToolInvocation, ToolOutput};

/// Default timeout: 10s. Codex uses the same. Long-running commands fail
/// cleanly rather than hanging the agent.
const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Cap on captured combined stdout+stderr, in bytes. Anything beyond is
/// middle-elided. Crush uses 30KB; Codex uses 1MiB. We split the
/// difference — 64KB lets the agent see a full file's worth of output
/// without paying for it in token cost. Tunable later.
const OUTPUT_CAP_BYTES: usize = 64 * 1024;

/// Banned argv-0s. The check is a token-aware first-word match against
/// the supplied command, after stripping leading whitespace. Doesn't
/// catch `which curl && curl ...` — that's what week 7's sandbox is for.
const BANNED_COMMANDS: &[&str] = &[
    // Network / download
    "curl", "wget", "scp", "ssh", "telnet", "nc", "ncat", "axel", "aria2c",
    "httpie", "http", "xh",
    // Browsers
    "chrome", "firefox", "safari", "lynx", "w3m", "links",
    // Privilege escalation
    "sudo", "doas", "su",
    // Shell aliasing — fragile across runs
    "alias", "unalias",
    // Package managers (system-wide mutation)
    "apt", "apt-get", "apt-cache", "dpkg", "yum", "dnf", "rpm", "pacman",
    "yay", "paru", "apk", "zypper", "emerge", "portage", "pkg", "pkg_add",
    "pkg_delete", "opkg", "home-manager", "makepkg",
    // System / disk modification
    "fdisk", "parted", "mkfs", "mount", "umount", "crontab", "at", "batch",
    "chkconfig",
];

/// Args the model passes to `bash`. JSON-shaped per [`schema`].
#[derive(Debug, Deserialize)]
struct BashArgs {
    /// Command to execute. Run via `bash -lc`.
    command: String,
    /// Working directory. Defaults to the inherited cwd.
    #[serde(default)]
    workdir: Option<String>,
    /// Override the default 10s timeout. Capped at 5min internally.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// The `bash` tool.
pub struct BashTool;

#[async_trait]
impl ToolHandler for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".into(),
            description: BASH_DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run via `bash -lc`. Required."
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Working directory. Defaults to the awidat process's cwd."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds. Default 10000, max 300000."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Bash is always potentially-mutating. Even read-only commands
        // (`ls`, `cat`) write to terminal state if the agent tries to
        // pipe them — we don't statically analyze.
        true
    }

    async fn handle(
        &self,
        inv: ToolInvocation,
        _ctx: crate::ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: BashArgs = serde_json::from_value(inv.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "bash: invalid args ({e}). Required: {{ \"command\": <string> }}."
            ))
        })?;

        if let Some(banned) = first_banned_token(&args.command) {
            warn!(banned, "bash: rejecting banned command");
            return Err(FunctionCallError::RespondToModel(format!(
                "bash: command '{banned}' is on the safety reject-list (no network, no package \
                 managers, no privilege escalation, no shell aliasing). The agent's sandbox \
                 lands in week 7; until then this is a static reject. Pick a different command."
            )));
        }

        let timeout_ms = args
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(300_000);
        let to = Duration::from_millis(timeout_ms);

        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(&args.command);
        if let Some(wd) = &args.workdir {
            cmd.current_dir(wd);
        }

        let output_fut = cmd.output();
        let output = match timeout(to, output_fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "bash: failed to launch ({e}). Check the command and workdir."
                )));
            }
            Err(_) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "bash: timed out after {timeout_ms}ms running `{}`. Re-run with a larger \
                     `timeout_ms` if this command is expected to be slow.",
                    args.command
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);
        let formatted = format_output(exit_code, &stdout, &stderr);
        Ok(ToolOutput::text(formatted))
    }
}

/// Description shown to the model in the tool list.
const BASH_DESCRIPTION: &str = "\
Run a shell command. Returns combined stdout + stderr + exit code, capped at 64KB \
(middle-elided beyond that). Default timeout: 10s. Use this to inspect the project, \
run quick scripts, query system info. Some commands are statically rejected for \
safety (network tools, package managers, sudo, alias). Sandboxing lands in week 7; \
until then, prefer narrow read-only operations.\
";

/// Return the first banned token if `command`'s first word matches one,
/// after stripping leading whitespace and any pipeline-leading wrappers.
fn first_banned_token(command: &str) -> Option<&'static str> {
    let trimmed = command.trim_start();
    let first = trimmed.split_whitespace().next()?;
    BANNED_COMMANDS.iter().find(|b| **b == first).copied()
}

/// Format an output blob the model will see. Truncates middle when over
/// the cap, with an explicit line count so the model knows it's elided.
fn format_output(exit_code: i32, stdout: &str, stderr: &str) -> String {
    let mut out = String::with_capacity(stdout.len() + stderr.len() + 128);
    out.push_str(&format!("exit code: {exit_code}\n"));
    if !stdout.is_empty() {
        out.push_str("--- stdout ---\n");
        out.push_str(&truncate_middle(stdout, OUTPUT_CAP_BYTES / 2));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if !stderr.is_empty() {
        out.push_str("--- stderr ---\n");
        out.push_str(&truncate_middle(stderr, OUTPUT_CAP_BYTES / 2));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Middle-truncate `s` to at most `cap` bytes. If truncated, inserts a
/// `\n... [TRUNCATED N lines, M bytes] ...\n` marker between the head
/// and tail halves.
fn truncate_middle(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let total_lines = s.lines().count();
    let total_bytes = s.len();
    let half = cap / 2;
    // Walk forward by chars to a clean UTF-8 boundary near `half`.
    let head_end = clamp_to_char_boundary(s, half);
    let tail_start = clamp_to_char_boundary(s, s.len().saturating_sub(half));
    let head = &s[..head_end];
    let tail = &s[tail_start..];
    let elided_bytes = total_bytes - head.len() - tail.len();
    let elided_lines =
        total_lines.saturating_sub(head.lines().count() + tail.lines().count());
    format!(
        "{head}\n... [TRUNCATED ~{elided_lines} lines, ~{elided_bytes} bytes] ...\n{tail}"
    )
}

fn clamp_to_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "bash".into(),
            args,
        }
    }

    fn ctx() -> crate::ToolContext {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        crate::ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
        }
    }

    #[tokio::test]
    async fn happy_path_runs_and_returns_stdout() {
        let tool = BashTool;
        let out = tool
            .handle(invoke(serde_json::json!({"command": "echo hello-bash"})), ctx())
            .await
            .unwrap();
        assert!(out.content.contains("exit code: 0"));
        assert!(out.content.contains("hello-bash"));
        assert!(out.content.contains("--- stdout ---"));
    }

    #[tokio::test]
    async fn stderr_is_captured_separately() {
        let tool = BashTool;
        let out = tool
            .handle(invoke(serde_json::json!({"command": "echo to-err 1>&2"})), ctx())
            .await
            .unwrap();
        assert!(out.content.contains("--- stderr ---"));
        assert!(out.content.contains("to-err"));
    }

    #[tokio::test]
    async fn non_zero_exit_is_surfaced_not_an_error() {
        let tool = BashTool;
        let out = tool
            .handle(invoke(serde_json::json!({"command": "false"})), ctx())
            .await
            .unwrap();
        assert!(out.content.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn missing_command_arg_is_respond_to_model() {
        let tool = BashTool;
        let err = tool.handle(invoke(serde_json::json!({})), ctx()).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("invalid args"));
                assert!(msg.contains("command"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_is_respond_to_model() {
        let tool = BashTool;
        let err = tool
            .handle(invoke(serde_json::json!({
                "command": "sleep 5",
                "timeout_ms": 200,
            })), ctx())
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("timed out"));
                assert!(msg.contains("200ms"));
            }
            other => panic!("want RespondToModel timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn banned_command_is_rejected_with_helpful_message() {
        let tool = BashTool;
        let err = tool
            .handle(invoke(serde_json::json!({"command": "curl https://example.com"})), ctx())
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("'curl'"));
                assert!(msg.contains("safety reject-list"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn banned_check_is_first_word_only() {
        // `echo curl` is fine; the first token is `echo`.
        let tool = BashTool;
        let out = tool
            .handle(invoke(serde_json::json!({"command": "echo curl"})), ctx())
            .await
            .unwrap();
        assert!(out.content.contains("curl"));
    }

    #[tokio::test]
    async fn workdir_is_honored() {
        let tool = BashTool;
        let dir = tempfile::tempdir().unwrap();
        let out = tool
            .handle(invoke(serde_json::json!({
                "command": "pwd",
                "workdir": dir.path().to_string_lossy(),
            })), ctx())
            .await
            .unwrap();
        assert!(out.content.contains(&*dir.path().to_string_lossy()));
    }

    #[test]
    fn truncate_middle_preserves_short_strings() {
        assert_eq!(truncate_middle("hello", 100), "hello");
    }

    #[test]
    fn truncate_middle_inserts_marker_when_over_cap() {
        let s: String = "x".repeat(1000);
        let t = truncate_middle(&s, 100);
        assert!(t.contains("[TRUNCATED"));
        assert!(t.starts_with('x'));
        assert!(t.ends_with('x'));
        assert!(t.len() < s.len());
    }

    #[test]
    fn first_banned_token_finds_alone() {
        assert_eq!(first_banned_token("curl https://x"), Some("curl"));
        assert_eq!(first_banned_token("  sudo rm"), Some("sudo"));
        assert_eq!(first_banned_token("ls"), None);
        assert_eq!(first_banned_token("echo curl"), None);
        assert_eq!(first_banned_token(""), None);
    }
}
