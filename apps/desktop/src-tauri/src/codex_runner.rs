//! Spawn the `codex-exec` binary (the JSONL-emitting flavor of codex
//! exec) as a child process, pipe its `--json` event stream into the
//! desktop's Tauri event bus, and translate each codex `ThreadEvent`
//! into the desktop protocol's [`Item`] shape.
//!
//! # Why a subprocess
//!
//! Codex's `run_main` owns its own tokio runtime and grabs the
//! process-wide `arg0_dispatch_or_else` hook to wire up codex-arg0's
//! re-exec dance for MCP child binaries. We can't run that twice in
//! the same process (the Tauri app already owns a tokio runtime), so
//! instead each `start_turn` shells out to a fresh `codex-exec ...`
//! invocation. Process isolation is a nice side-benefit — `cancel_turn`
//! simply kills the child.
//!
//! # Why `codex-exec` and not `awidat chat`
//!
//! `awidat chat` is the human-facing entry point that prints
//! human-readable output. We need codex's machine-readable JSONL
//! stream, which lives on `codex-exec --json`. `codex-exec` ships as
//! a sibling binary in the same target/ dir as `awidat` (it's in the
//! workspace via `vendor/codex-rs/exec`). We re-implement the
//! awidat-mcp-server auto-registration here that
//! `crates/cli/src/chat_codex_cmd.rs` does at the in-process layer.
//!
//! # Mapping the event stream
//!
//! Codex emits a JSONL stream defined in
//! `vendor/codex-rs/exec/src/exec_events.rs::ThreadEvent`. We deserialize
//! each line and translate into [`Item`] using best-effort rules
//! ([`map_thread_event`]). Things we can't map cleanly (collab tool
//! calls, web searches, etc) get logged and dropped — the frontend
//! tolerates missing items.
//!
//! # Status (step 8)
//!
//! - Text agent messages → `Item::Text`
//! - Reasoning summaries → `Item::Text` (rendered the same way, with
//!   a `reasoning:` id prefix so the frontend could differentiate later)
//! - Command execution → `Item::ToolCall { name: "bash", ... }`
//! - MCP tool calls → `Item::ToolCall { name: "<server>.<tool>", ... }`
//! - File changes → `Item::ToolCall { name: "apply_patch", ... }`
//! - TodoList items → `Item::Plan`
//! - Errors → `Item::Error`
//! - Approval flow is not wired (step 8b).

use std::path::PathBuf;
use std::process::Stdio;

use awidat_desktop_protocol::{Id, Item, ItemLifecycle, PlanStep};
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::events::{TURN_END_EVENT, TurnEndEvent, emit_item};

/// Handle on a running codex subprocess. Owned by
/// [`crate::state::TurnHandle`]. Dropping the token kills the child.
pub struct CodexHandle {
    /// Cancellation token watched by the reader task. Flipping it
    /// causes the reader to send SIGKILL to the child via
    /// [`tokio::process::Child::start_kill`].
    pub cancel: CancellationToken,
}

/// Resolve the absolute path to the `codex-exec` binary that should
/// drive the chat turn. Strategy:
///
/// 1. `AWIDAT_DESKTOP_CODEX_BIN` env override (used by integration tests).
/// 2. Sibling of `current_exe()` named `codex-exec` (dev build:
///    `target/debug/codex-exec`; production bundle: alongside the
///    Tauri inner binary).
fn resolve_codex_exec_binary() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("AWIDAT_DESKTOP_CODEX_BIN") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
    }
    let self_exe = std::env::current_exe().map_err(|e| format!("current_exe(): {e}"))?;
    if let Some(parent) = self_exe.parent() {
        let sibling = parent.join("codex-exec");
        if sibling.is_file() {
            return Ok(sibling);
        }
        // Some bundle layouts put binaries one level up.
        if let Some(grand) = parent.parent() {
            let alt = grand.join("codex-exec");
            if alt.is_file() {
                return Ok(alt);
            }
        }
    }
    Err(format!(
        "codex-exec binary not found near {}",
        self_exe.display()
    ))
}

/// Resolve the `awidat-mcp-server` binary so we can register it with
/// codex via `-c mcp_servers.awidat.command=...`. Mirrors the lookup
/// in `crates/cli/src/chat_codex_cmd.rs::awidat_mcp_overrides`. Returns
/// `None` (with a log) when the binary is missing — codex will then
/// run without Awidat tools, the same fallback the CLI uses.
fn resolve_mcp_server_binary() -> Option<PathBuf> {
    let self_exe = std::env::current_exe().ok()?;
    let parent = self_exe.parent()?;
    let candidate = parent.join("awidat-mcp-server");
    if candidate.is_file() {
        return Some(candidate);
    }
    if let Some(grand) = parent.parent() {
        let alt = grand.join("awidat-mcp-server");
        if alt.is_file() {
            return Some(alt);
        }
    }
    warn!(
        "awidat-mcp-server sibling binary missing; codex will run without Awidat tools"
    );
    None
}

/// Spawn `codex-exec --json --skip-git-repo-check ... <prompt>` and
/// route its JSONL output into the Tauri event bus.
///
/// Returns a [`CodexHandle`] holding the cancellation token for the
/// child. Emits `awidat://turn-end` exactly once (on EOF, error, or
/// cancel).
pub fn spawn_codex_turn(
    app: AppHandle,
    project_root: PathBuf,
    prompt: String,
    model: Option<String>,
) -> Result<CodexHandle, String> {
    let bin = resolve_codex_exec_binary()?;
    let cancel = CancellationToken::new();

    let mut cmd = Command::new(&bin);
    cmd.arg("--json")
        .arg("--skip-git-repo-check")
        .arg("--dangerously-bypass-approvals-and-sandbox");
    if let Some(m) = model.as_deref() {
        cmd.arg("--model").arg(m);
    }
    // Register the awidat MCP server. Codex's `-c` parser expects
    // TOML on the RHS, so the command string has to be a TOML-quoted
    // string. Use `format!("{:?}", ...)` which gives us a `"..."`
    // literal with backslashes escaped, matching the CLI's recipe.
    if let Some(mcp) = resolve_mcp_server_binary() {
        let quoted_cmd = format!("{:?}", mcp.display().to_string());
        cmd.arg("-c")
            .arg(format!("mcp_servers.awidat.command={quoted_cmd}"));
        // Codex calls `.env_clear()` on MCP-server spawn (see
        // vendor/codex-rs/rmcp-client/src/stdio_server_launcher.rs:259),
        // so a parent `cmd.env(...)` doesn't reach the grandchild MCP
        // server. Forward project_root via per-server env config so
        // the MCP server's McpToolCtx::resolve() finds the right path.
        let quoted_root = format!("{:?}", project_root.display().to_string());
        cmd.arg("-c").arg(format!(
            "mcp_servers.awidat.env.AWIDAT_PROJECT_ROOT={quoted_root}"
        ));
    }
    // Final positional arg is the prompt.
    cmd.arg(&prompt);

    // Also export to the codex-exec child itself — the TUI panel and
    // any in-process consumers read from env. The MCP server gets it
    // via the per-server config override above (not this env).
    cmd.env("AWIDAT_PROJECT_ROOT", &project_root);
    // Run codex from the project dir so any cwd-relative behavior
    // sees what the user is editing.
    cmd.current_dir(&project_root);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Tauri's Drop semantics on the child are fine, but flipping
        // kill_on_drop gives us a free safety net if the reader task
        // panics before sending the kill signal explicitly.
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "child stdout unavailable".to_string())?;
    let stderr = child.stderr.take();

    // Spawn the reader/dispatcher. It owns the Child handle so it can
    // wait for exit and decide what to emit on the turn-end event.
    let cancel_for_task = cancel.clone();
    let app_for_task = app.clone();
    tokio::spawn(async move {
        run_event_loop(app_for_task, child, stdout, stderr, cancel_for_task).await;
    });

    Ok(CodexHandle { cancel })
}

/// Drive the JSONL stream from the codex subprocess until EOF or
/// cancellation, then emit `awidat://turn-end` once.
async fn run_event_loop(
    app: AppHandle,
    mut child: Child,
    stdout: tokio::process::ChildStdout,
    stderr: Option<tokio::process::ChildStderr>,
    cancel: CancellationToken,
) {
    // Drain stderr concurrently so the child can't deadlock writing
    // to a full pipe. We don't surface stderr to the frontend
    // directly (codex prints its bootstrap logs there) but we log it
    // so dev-runs can correlate failures.
    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "awidat::codex::stderr", "{line}");
            }
        });
    }

    let mut lines = BufReader::new(stdout).lines();
    let mut text_state: Option<TextStreamer> = None;
    let mut had_error: Option<String> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                if let Err(e) = child.start_kill() {
                    warn!(error = %e, "failed to send kill to codex child");
                }
                had_error = Some("cancelled".into());
                break;
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Err(e) = handle_jsonl_line(&app, &line, &mut text_state) {
                            warn!(error = %e, line = %line, "drop unparseable codex event");
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        had_error = Some(format!("codex stdout read error: {e}"));
                        break;
                    }
                }
            }
        }
    }

    // Reap the child so we don't leak a zombie. Errors here are
    // surfaced only via logs — the user-facing turn-end carries
    // whatever message we've already collected.
    let status = match child.wait().await {
        Ok(s) => Some(s),
        Err(e) => {
            warn!(error = %e, "wait() on codex child failed");
            None
        }
    };

    // Synthesize a final message if the child exited non-zero and we
    // haven't already captured an error.
    if had_error.is_none()
        && let Some(s) = status
        && !s.success()
    {
        had_error = Some(match s.code() {
            Some(code) => format!("codex exited with status {code}"),
            None => "codex exited abnormally".into(),
        });
    }

    let payload = TurnEndEvent {
        error: had_error.clone(),
    };
    if let Err(e) = tauri::Emitter::emit(&app, TURN_END_EVENT, payload) {
        warn!(error = %e, "failed to emit turn-end");
    }

    // Clear the turn handle in state if it still matches.
    use crate::state::AwidatState;
    use tauri::Manager;
    let st = app.state::<AwidatState>();
    *st.turn.lock().await = None;
}

/// Decode one JSONL line from codex and turn it into zero or more
/// `Item` emits.
fn handle_jsonl_line(
    app: &AppHandle,
    line: &str,
    text_state: &mut Option<TextStreamer>,
) -> Result<(), String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("json parse: {e}"))?;
    let items = map_thread_event(&value, text_state);
    for item in items {
        emit_item(app, item);
    }
    Ok(())
}

/// Per-turn text-coalescing state. Codex emits whole agent messages
/// (not deltas) via `item.completed`, but reasoning summaries can
/// arrive separately and we want to render them inline. The streamer
/// pre-allocates ids and tracks the "currently open" text run.
#[derive(Default)]
pub(crate) struct TextStreamer {
    /// Counter for synthesized text-item ids when codex hasn't given
    /// us a stable one.
    seq: u64,
}

impl TextStreamer {
    fn next_text_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!(
            "{prefix}-{}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            self.seq
        )
    }
}

/// Translate one codex `ThreadEvent` JSON value into zero or more
/// desktop protocol `Item`s.
///
/// The matching is type-tag driven (`event["type"]`) so we don't need
/// to depend on codex-exec's Rust types at the desktop layer.
pub(crate) fn map_thread_event(
    event: &serde_json::Value,
    state: &mut Option<TextStreamer>,
) -> Vec<Item> {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let streamer = state.get_or_insert_with(TextStreamer::default);

    match ty {
        // Thread / turn lifecycle — surface as no-ops here. turn.started
        // and turn.completed flow through our wrapper's TurnEndEvent.
        "thread.started" | "turn.started" | "turn.completed" => Vec::new(),

        // Top-level fatal error from the codex stream itself.
        "error" => {
            let msg = event
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("codex stream error")
                .to_string();
            vec![Item::Error {
                id: Id::new(streamer.next_text_id("err")),
                message: msg,
            }]
        }

        "turn.failed" => {
            let msg = event
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("turn failed")
                .to_string();
            vec![Item::Error {
                id: Id::new(streamer.next_text_id("err")),
                message: msg,
            }]
        }

        "item.started" | "item.updated" | "item.completed" => {
            let phase = match ty {
                "item.started" => ItemLifecycle::Started,
                "item.updated" => ItemLifecycle::Delta,
                _ => ItemLifecycle::Completed,
            };
            let Some(item) = event.get("item") else {
                return Vec::new();
            };
            map_item(item, phase, streamer)
        }

        _ => {
            debug!(target: "awidat::codex", "unhandled event type {ty}");
            Vec::new()
        }
    }
}

/// Map one `ThreadItem` JSON shape to a protocol [`Item`]. The shape
/// is `{ id, type, ...payload }` with `type` discriminating which
/// payload fields are present.
fn map_item(
    item: &serde_json::Value,
    phase: ItemLifecycle,
    _streamer: &mut TextStreamer,
) -> Vec<Item> {
    let raw_id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match kind {
        "agent_message" => {
            // Render the whole message as a Completed text item — codex
            // doesn't ship deltas through the JSONL surface today.
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.trim().is_empty() {
                return Vec::new();
            }
            vec![Item::Text {
                id: Id::new(format!("codex-msg-{raw_id}")),
                phase,
                text,
            }]
        }

        "reasoning" => {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.trim().is_empty() {
                return Vec::new();
            }
            // Tag reasoning items so the frontend could style them
            // differently later. For now they share the Text variant
            // with the rest.
            vec![Item::Text {
                id: Id::new(format!("codex-reason-{raw_id}")),
                phase,
                text,
            }]
        }

        "command_execution" => {
            let command = item
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = item
                .get("aggregated_output")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let exit_code = item.get("exit_code").and_then(|v| v.as_i64());

            let result = match (phase, status) {
                (ItemLifecycle::Completed, "completed") if exit_code.unwrap_or(0) == 0 => {
                    Some(Ok(output))
                }
                (ItemLifecycle::Completed, _) => Some(Err(format!(
                    "exit_code={exit_code:?} status={status} output={output}"
                ))),
                _ => None,
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-cmd-{raw_id}")),
                phase,
                name: "bash".into(),
                args: serde_json::json!({ "command": command }),
                result,
            }]
        }

        "mcp_tool_call" => {
            let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("");
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = item
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");

            let result_value = item.get("result").cloned();
            let error_value = item
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let result = if matches!(phase, ItemLifecycle::Completed) {
                if let Some(err) = error_value {
                    Some(Err(err))
                } else if status == "failed" {
                    Some(Err("mcp tool failed".to_string()))
                } else {
                    Some(Ok(result_value
                        .map(|v| serde_json::to_string(&v).unwrap_or_default())
                        .unwrap_or_default()))
                }
            } else {
                None
            };

            // For awidat-mcp-server tools we want the unprefixed tool
            // name in the chat card (e.g. `apply_edl`, not
            // `awidat.apply_edl`); the frontend's per-tool card
            // dispatch keys on the bare name.
            let display_name = if server == "awidat" {
                tool.to_string()
            } else {
                format!("{server}.{tool}")
            };

            vec![Item::ToolCall {
                id: Id::new(format!("codex-mcp-{raw_id}")),
                phase,
                name: display_name,
                args: arguments,
                result,
            }]
        }

        "file_change" => {
            let changes = item
                .get("changes")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let result = match (phase, status) {
                (ItemLifecycle::Completed, "completed") => Some(Ok("applied".into())),
                (ItemLifecycle::Completed, _) => Some(Err(format!("patch status={status}"))),
                _ => None,
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-patch-{raw_id}")),
                phase,
                name: "apply_patch".into(),
                args: serde_json::json!({ "changes": changes }),
                result,
            }]
        }

        "todo_list" => {
            let items = item
                .get("items")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|todo| {
                            let step = todo
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let completed = todo
                                .get("completed")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            PlanStep {
                                step,
                                status: if completed {
                                    "completed".into()
                                } else {
                                    "pending".into()
                                },
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            vec![Item::Plan {
                id: Id::new("codex-plan"),
                phase,
                items,
                note: None,
            }]
        }

        "error" => {
            // ErrorItem-as-thread-item (non-fatal). Surface as an
            // Error card too — the frontend handles both.
            let msg = item
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .to_string();
            vec![Item::Error {
                id: Id::new(format!("codex-erri-{raw_id}")),
                message: msg,
            }]
        }

        // Web search, collab tool calls, etc — drop until/unless we
        // need them. The frontend already handles missing items.
        _ => {
            debug!(target: "awidat::codex", "unhandled item type {kind}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_agent_message_to_text_item() {
        let mut state = None;
        let event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_0",
                "type": "agent_message",
                "text": "hello world",
            }
        });
        let items = map_thread_event(&event, &mut state);
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::Text {
                text,
                phase: ItemLifecycle::Completed,
                ..
            } => assert_eq!(text, "hello world"),
            other => panic!("expected Item::Text, got {other:?}"),
        }
    }

    #[test]
    fn maps_mcp_tool_call_started_then_completed() {
        let mut state = None;
        let started = serde_json::json!({
            "type": "item.started",
            "item": {
                "id": "item_1",
                "type": "mcp_tool_call",
                "server": "awidat",
                "tool": "apply_edl",
                "arguments": {"edl": "..."},
                "status": "in_progress",
                "result": null,
                "error": null,
            }
        });
        let completed = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "mcp_tool_call",
                "server": "awidat",
                "tool": "apply_edl",
                "arguments": {"edl": "..."},
                "status": "completed",
                "result": {"content": [{"type": "text", "text": "applied"}]},
                "error": null,
            }
        });
        let mut all: Vec<Item> = Vec::new();
        all.extend(map_thread_event(&started, &mut state));
        all.extend(map_thread_event(&completed, &mut state));
        assert_eq!(all.len(), 2);
        match &all[0] {
            Item::ToolCall {
                name,
                phase: ItemLifecycle::Started,
                result: None,
                ..
            } => assert_eq!(name, "apply_edl"),
            other => panic!("expected started ToolCall, got {other:?}"),
        }
        match &all[1] {
            Item::ToolCall {
                name,
                phase: ItemLifecycle::Completed,
                result: Some(Ok(_)),
                ..
            } => assert_eq!(name, "apply_edl"),
            other => panic!("expected completed ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn maps_turn_failed_to_error_item() {
        let mut state = None;
        let event = serde_json::json!({
            "type": "turn.failed",
            "error": {"message": "rate limited"},
        });
        let items = map_thread_event(&event, &mut state);
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::Error { message, .. } => assert_eq!(message, "rate limited"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_types_are_dropped() {
        let mut state = None;
        let event = serde_json::json!({"type": "something_we_dont_know"});
        let items = map_thread_event(&event, &mut state);
        assert!(items.is_empty());
    }
}
