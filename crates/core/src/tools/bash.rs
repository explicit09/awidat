//! `bash` tool — runs a shell command, returns combined stdout+stderr.
//!
//! Shape per the corpus survey: simpler than Codex's argv-vector
//! `shell` (Crush's single-command shape), with Codex's truncation
//! discipline (cap at 30KB, middle-elide with line-count header).
//!
//! ## Sandboxing (week 7 — landed)
//!
//! On macOS the command runs under `sandbox-exec` via the
//! `awidat-sandboxing` crate. The policy denies file writes outside
//! the project root + system tempdirs, and denies network egress
//! by default. On Linux the sandbox is stubbed; we fall back to
//! direct execution with the static reject-list (below) as the
//! safety net.
//!
//! ## Banned commands
//!
//! Defense-in-depth alongside the sandbox: a static reject list
//! lifted from
//! `harnesses/crush/internal/agent/tools/bash.go:71-128`. Network
//! tools, package managers, system-mutating tools. The reject-list
//! catches "the agent ran `curl`" before the sandbox even gets a
//! chance to deny the syscall — fewer denial-then-escalation
//! roundtrips for obvious-no's.

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
            ..Default::default()
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
        ctx: crate::ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: BashArgs = serde_json::from_value(inv.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "bash: invalid args ({e}). Required: {{ \"command\": <string> }}."
            ))
        })?;

        if let Some(banned) = first_banned_token(&args.command) {
            warn!(banned, "bash: rejecting banned command");
            return Err(FunctionCallError::RespondToModel(format!(
                "bash: command '{banned}' is on the reject-list (no network, no package \
                 managers, no privilege escalation, no shell aliasing). Pick a different \
                 command. If you need network or package install, the user has to do it \
                 outside the agent loop."
            )));
        }

        let timeout_ms = args
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(300_000);
        let to = Duration::from_millis(timeout_ms);

        // Sandboxed execution path. The sandbox is on by default;
        // the AWIDAT_BASH_NO_SANDBOX env var disables it for
        // diagnostics + bootstrapping (e.g. running awidat from
        // a CI runner where seatbelt isn't available). Linux
        // currently returns LinuxNotImplemented — we fall back to
        // direct exec there with the reject-list as the safety net.
        let workdir: std::path::PathBuf = args
            .workdir
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ctx.project_root.clone());
        if std::env::var_os("AWIDAT_BASH_NO_SANDBOX").is_none()
            && awidat_sandboxing::is_available()
        {
            return run_sandboxed(&args.command, &workdir, &ctx.project_root, to).await;
        }

        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(&args.command);
        cmd.current_dir(&workdir);

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

/// Run `command` under the platform sandbox. Spawns the sandbox in
/// a blocking task because `awidat_sandboxing::Sandbox::run` is
/// synchronous (it calls `Command::output`, not the tokio variant).
/// We wrap with `tokio::task::spawn_blocking` so the agent loop
/// stays responsive.
async fn run_sandboxed(
    command: &str,
    workdir: &std::path::Path,
    project_root: &std::path::Path,
    deadline: Duration,
) -> Result<ToolOutput, FunctionCallError> {
    use awidat_sandboxing::{Policy, Sandbox, SandboxErr};
    let command = command.to_string();
    let workdir = workdir.to_path_buf();
    let project_root = project_root.to_path_buf();
    let blocking = tokio::task::spawn_blocking(move || {
        let policy = Policy::for_project(&project_root);
        let sb = Sandbox;
        // We pass the command via `bash -lc <command>` so user-supplied
        // shell syntax (pipes, redirects) works. The bash binary
        // itself is read-allowed by the seatbelt profile.
        // We can't change-cwd inside sandbox-exec's argv, so we cd in
        // the inline shell.
        let cmd = format!(
            "cd {} && {}",
            shell_quote(workdir.to_str().unwrap_or(".")),
            command
        );
        sb.run(&["/bin/bash", "-lc", &cmd], &policy)
    });
    let result = match timeout(deadline, blocking).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "bash: sandbox task panicked ({e}). This is a bug — file an issue."
            )));
        }
        Err(_) => {
            return Err(FunctionCallError::RespondToModel(format!(
                "bash: sandboxed command timed out. Re-run with a larger `timeout_ms`."
            )));
        }
    };
    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let exit_code = out.status.code().unwrap_or(-1);
            let formatted = format_output(exit_code, &stdout, &stderr);
            Ok(ToolOutput::text(formatted))
        }
        Err(SandboxErr::Denied { reason }) => {
            Err(FunctionCallError::RespondToModel(format!(
                "bash: sandbox denied the command ({reason}). The sandbox \
                 only permits writes inside the project root + system \
                 tempdirs, and denies network egress by default. Either \
                 rewrite the command to stay within those bounds, or tell \
                 the user this needs unsandboxed execution and let them \
                 run it outside the agent loop."
            )))
        }
        Err(SandboxErr::LinuxNotImplemented) => {
            // Should never reach here — `is_available()` returns
            // false on Linux, so the caller picked the unsandboxed
            // path. Belt-and-suspenders error message in case.
            Err(FunctionCallError::RespondToModel(
                "bash: Linux sandbox not yet wired. Set AWIDAT_BASH_NO_SANDBOX=1 \
                 to bypass on Linux until the landlock backend lands."
                    .into(),
            ))
        }
        Err(other) => Err(FunctionCallError::RespondToModel(format!(
            "bash: sandbox error ({other}). Check `awidat doctor` for setup help."
        ))),
    }
}

/// Shell-quote a path for safe interpolation into the inline `cd`
/// that we splice ahead of the user's command. Paranoid: assumes
/// any character could be metacharacter-significant. This isn't a
/// security boundary (the sandbox is) — it's "the paths with
/// spaces should still work."
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            // End the single-quoted string, escape a literal
            // single-quote, restart.
            out.push_str(r"'\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
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

            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
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
        // Both the sandboxed and unsandboxed paths emit a clear
        // "timed out" message — we only assert that, not the exact
        // format. The exact-format checks live in the sandboxing
        // crate's tests, which exercise both paths directly.
        let err = tool
            .handle(invoke(serde_json::json!({
                "command": "sleep 5",
                "timeout_ms": 200,
            })), ctx())
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(
                    msg.contains("timed out") || msg.contains("timeout"),
                    "expected a timeout message, got {msg:?}"
                );
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
                assert!(msg.contains("reject-list"));
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
