//! Pure mapping functions from codex protocol payloads to
//! [`montage_desktop_protocol::Item`] values.
//!
//! These are kept side-effect-free (apart from mutating the caller's
//! `text_buffers` cache) so the bridge can unit-test the translation
//! without spinning up a real app-server. The pump module passes its
//! cumulative-text buffer into [`map_notification`] for the streaming
//! agent-message / reasoning-delta paths.

use std::collections::HashMap;

use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::McpToolCallError;
use codex_app_server_protocol::McpToolCallResult;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::TurnPlanStepStatus;
use montage_desktop_protocol::Id;
use montage_desktop_protocol::Item;
use montage_desktop_protocol::ItemLifecycle;
use montage_desktop_protocol::PlanStep;
use serde_json::json;

/// Item-id prefix for `Item::Text` synthesized from
/// `ServerNotification::AgentMessageDelta`.
const AGENT_MESSAGE_PREFIX: &str = "codex-msg";
/// Item-id prefix for `Item::Text` synthesized from
/// `ServerNotification::ReasoningTextDelta` / `ReasoningSummaryTextDelta`.
const REASONING_PREFIX: &str = "codex-reason";

/// Translate one `ServerNotification` into zero or more `Item`s
/// for the desktop renderer.
///
/// `text_buffers` is a per-pump cache of the cumulative text we have
/// already emitted for a given codex item id. The Montage protocol wants
/// each `Text` Item to carry the *full* text so the renderer doesn't
/// have to splice deltas, so we keep the running concatenation here.
///
/// Unknown / unhandled notifications return an empty `Vec` — the
/// frontend tolerates missing items.
pub fn map_notification(
    notification: &ServerNotification,
    text_buffers: &mut HashMap<String, String>,
) -> Vec<Item> {
    match notification {
        ServerNotification::AgentMessageDelta(n) => {
            let buffer = text_buffers.entry(n.item_id.clone()).or_default();
            buffer.push_str(&n.delta);
            let cumulative = buffer.clone();
            vec![Item::Text {
                id: Id::new(format!("{AGENT_MESSAGE_PREFIX}-{}", n.item_id)),
                phase: ItemLifecycle::Delta,
                text: cumulative,
            }]
        }
        ServerNotification::ReasoningTextDelta(n) => {
            let key = reasoning_buffer_key(&n.item_id, n.content_index);
            let buffer = text_buffers.entry(key).or_default();
            buffer.push_str(&n.delta);
            let cumulative = buffer.clone();
            vec![Item::Text {
                id: Id::new(format!(
                    "{REASONING_PREFIX}-{}-{}",
                    n.item_id, n.content_index
                )),
                phase: ItemLifecycle::Delta,
                text: cumulative,
            }]
        }
        ServerNotification::ReasoningSummaryTextDelta(n) => {
            let key = reasoning_summary_buffer_key(&n.item_id, n.summary_index);
            let buffer = text_buffers.entry(key).or_default();
            buffer.push_str(&n.delta);
            let cumulative = buffer.clone();
            vec![Item::Text {
                id: Id::new(format!(
                    "{REASONING_PREFIX}-summary-{}-{}",
                    n.item_id, n.summary_index
                )),
                phase: ItemLifecycle::Delta,
                text: cumulative,
            }]
        }
        ServerNotification::ItemStarted(n) => map_thread_item(&n.item, Phase::Started),
        ServerNotification::ItemCompleted(n) => {
            // Drop any per-item text-delta buffer keyed by this item id
            // — the Completed mapping carries the canonical text.
            text_buffers.remove(n.item.id());
            map_thread_item(&n.item, Phase::Completed)
        }
        ServerNotification::TurnPlanUpdated(n) => {
            let items = n
                .plan
                .iter()
                .map(|step| PlanStep {
                    step: step.step.clone(),
                    status: turn_plan_status_str(step.status).to_string(),
                })
                .collect::<Vec<_>>();
            vec![Item::Plan {
                id: Id::new("codex-plan"),
                phase: ItemLifecycle::Delta,
                items,
                note: n.explanation.clone(),
            }]
        }
        // Turn end is forwarded as a turn-end emit by the caller, not
        // as an Item. The Error notification surfaces as an Item::Error
        // here so the chat shows the error text inline.
        ServerNotification::Error(n) => {
            vec![Item::Error {
                id: Id::new(format!("codex-err-{}", n.thread_id)),
                message: n.error.message.clone(),
            }]
        }
        // Everything else is best-effort — silent today.
        _ => Vec::new(),
    }
}

/// Lifecycle phase for `map_thread_item`. We don't expose codex's
/// raw `ItemStarted` / `ItemCompleted` because the desktop's three-state
/// `Delta` is rarely used for thread items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Started,
    Completed,
}

impl From<Phase> for ItemLifecycle {
    fn from(p: Phase) -> Self {
        match p {
            Phase::Started => ItemLifecycle::Started,
            Phase::Completed => ItemLifecycle::Completed,
        }
    }
}

/// Translate one `v2::ThreadItem` into zero or more desktop `Item`s.
pub fn map_thread_item(item: &ThreadItem, phase: Phase) -> Vec<Item> {
    let life: ItemLifecycle = phase.into();
    match item {
        ThreadItem::AgentMessage { id, text, .. } => {
            if text.trim().is_empty() {
                return Vec::new();
            }
            vec![Item::Text {
                id: Id::new(format!("{AGENT_MESSAGE_PREFIX}-{id}")),
                phase: life,
                text: text.clone(),
            }]
        }
        ThreadItem::Reasoning {
            id,
            summary,
            content,
        } => {
            // Reasoning items aggregate multiple text fragments. Render
            // them by concatenating with double newlines so each block
            // is visually separated. Empty content/summary is skipped.
            let mut combined = String::new();
            for s in summary.iter().chain(content.iter()) {
                if !s.trim().is_empty() {
                    if !combined.is_empty() {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(s);
                }
            }
            if combined.is_empty() {
                return Vec::new();
            }
            vec![Item::Text {
                id: Id::new(format!("{REASONING_PREFIX}-{id}")),
                phase: life,
                text: combined,
            }]
        }
        ThreadItem::CommandExecution {
            id,
            command,
            status,
            aggregated_output,
            exit_code,
            ..
        } => {
            let output = aggregated_output.clone().unwrap_or_default();
            let result = match phase {
                Phase::Started => None,
                Phase::Completed => Some(command_execution_result(status, *exit_code, output)),
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-cmd-{id}")),
                phase: life,
                name: "bash".into(),
                args: json!({ "command": command }),
                result,
            }]
        }
        ThreadItem::FileChange {
            id,
            changes,
            status,
        } => {
            let result = match phase {
                Phase::Started => None,
                Phase::Completed => Some(file_change_result(status)),
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-patch-{id}")),
                phase: life,
                name: "apply_patch".into(),
                args: json!({ "changes": changes }),
                result,
            }]
        }
        ThreadItem::McpToolCall {
            id,
            server,
            tool,
            status,
            arguments,
            result,
            error,
            ..
        } => {
            // For Montage-MCP tools, surface the bare tool name so the
            // React per-tool card dispatch works ("apply_edl", not
            // "montage.apply_edl"). For other servers, namespace-prefix.
            let display_name = if server == "montage" {
                tool.clone()
            } else {
                format!("{server}.{tool}")
            };
            let result_value = match phase {
                Phase::Started => None,
                Phase::Completed => {
                    Some(mcp_tool_result(status, result.as_deref(), error.as_ref()))
                }
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-mcp-{id}")),
                phase: life,
                name: display_name,
                args: arguments.clone(),
                result: result_value,
            }]
        }
        ThreadItem::Plan { id, text } => {
            // Treat a Plan ThreadItem as a single-step plan emission;
            // the renderer collapses repeated emissions into a single
            // upserted Plan item if they share an id.
            vec![Item::Plan {
                id: Id::new(format!("codex-plan-{id}")),
                phase: life,
                items: vec![PlanStep {
                    step: text.clone(),
                    status: "pending".to_string(),
                }],
                note: None,
            }]
        }
        ThreadItem::UserMessage { .. } => {
            // Codex echoes every user input back as a UserMessage
            // ThreadItem. We DON'T render it: the desktop emits its
            // own `Item::UserInput` at start_turn time (the clean
            // user-typed text, before we prefix view-state context or
            // the skills catalog). Surfacing codex's echo would
            // double-render the input AND leak our internal context
            // prefix into the chat as a visible bubble.
            //
            // For thread-resume (history.rs is currently stubbed), we
            // will need a different path that reads the original
            // user-typed text from a separate audit log rather than
            // recovering it from codex's prefixed echo.
            Vec::new()
        }
        // Web search, image, collab — drop until/unless desktop renders them.
        _ => Vec::new(),
    }
}

/// Extract the agent's `reasoning` string from a JSON tool-args
/// payload, if any. Wave 3's Brief surface reads `rationale` on every
/// row (approvals included); `apply_edl(reasoning = …)` and other
/// tools that adopt the same contract surface their "why" through
/// this helper.
///
/// Contract: looks for a top-level `reasoning` string field. Anything
/// non-stringy (object, array, missing key) returns `None`. We trim
/// whitespace and treat empty strings as `None` so the frontend never
/// renders a hollow rationale chip.
///
/// Kept out of any specific approval path because the L1 catalog
/// commits to `reasoning` as the universal field name — every new
/// tool with a "why" arg should pipe through here, not invent its
/// own surface.
pub fn extract_reasoning(arguments: Option<&serde_json::Value>) -> Option<String> {
    let value = arguments?.get("reasoning")?.as_str()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Build the capability metadata JSON the ApprovalCard renderer
/// expects for a shell-command approval. The legacy keys
/// (`graph_mutates`, `preview_supported`, `side_effects`) are the
/// contract; codex's own approval payload doesn't carry the same
/// shape, so we synthesize a sensible default.
pub fn build_capability_metadata_for_exec(command: &str) -> serde_json::Value {
    let mutates = command_looks_destructive(command);
    json!({
        "graph_mutates": mutates,
        "preview_supported": false,
        "side_effects": ["shell_exec"],
    })
}

/// Build the capability metadata JSON for a file-change approval. File
/// changes always mutate the timeline graph.
pub fn build_capability_metadata_for_file_change(reason: Option<&str>) -> serde_json::Value {
    json!({
        "graph_mutates": true,
        "preview_supported": false,
        "side_effects": ["filesystem_write"],
        "reason": reason,
    })
}

/// Best-effort lexical heuristic for "this shell command is destructive."
/// Used only to color the approval card; not relied on for safety.
fn command_looks_destructive(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    [
        " rm ",
        "mkfs",
        " dd ",
        "git push --force",
        "sudo ",
        " mv ",
        "shutdown",
        "reboot",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || lower.starts_with("rm ")
        || lower.starts_with("mv ")
        || lower.starts_with("dd ")
}

fn command_execution_result(
    status: &CommandExecutionStatus,
    exit_code: Option<i32>,
    output: String,
) -> Result<String, String> {
    match status {
        CommandExecutionStatus::Completed if exit_code.unwrap_or(0) == 0 => Ok(output),
        CommandExecutionStatus::Completed => Err(format!(
            "exit_code={} output={output}",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into())
        )),
        CommandExecutionStatus::Failed => Err(format!("failed: {output}")),
        CommandExecutionStatus::InProgress => Err("still in progress".to_string()),
        CommandExecutionStatus::Declined => Err("declined by user".to_string()),
    }
}

fn file_change_result(status: &PatchApplyStatus) -> Result<String, String> {
    match status {
        PatchApplyStatus::Completed => Ok("applied".into()),
        PatchApplyStatus::Failed => Err("apply_patch failed".into()),
        PatchApplyStatus::InProgress => Err("apply_patch still in progress".into()),
        PatchApplyStatus::Declined => Err("apply_patch declined by user".into()),
    }
}

/// True when a `ServerNotification::ItemCompleted` carries an
/// montage-MCP tool call that just successfully mutated project state
/// on disk (most importantly `project.otio.json`). Used by the pump
/// to nudge the desktop's `montage://timeline-changed` listener so the
/// React timeline view refetches.
///
/// Inclusion rule is "any montage tool tagged `destructive_hint = true`
/// that completed successfully." False positives (e.g. `start_render`
/// queues a job but doesn't touch OTIO) just trigger an idempotent
/// refetch, which is cheap. False negatives would leave the UI stale,
/// which we already saw — biased toward over-emit.
pub fn is_project_mutating_completion(notification: &ServerNotification) -> bool {
    let ServerNotification::ItemCompleted(n) = notification else {
        return false;
    };
    let ThreadItem::McpToolCall {
        server,
        status,
        error,
        ..
    } = &n.item
    else {
        return false;
    };
    if server != "montage" {
        return false;
    }
    if error.is_some() {
        return false;
    }
    matches!(status, McpToolCallStatus::Completed)
}

fn mcp_tool_result(
    status: &McpToolCallStatus,
    result: Option<&McpToolCallResult>,
    error: Option<&McpToolCallError>,
) -> Result<String, String> {
    if let Some(err) = error {
        return Err(err.message.clone());
    }
    match status {
        McpToolCallStatus::Completed => match result {
            Some(r) => Ok(serde_json::to_string(r).unwrap_or_default()),
            None => Ok(String::new()),
        },
        McpToolCallStatus::Failed => Err("mcp tool failed".into()),
        McpToolCallStatus::InProgress => Err("mcp tool still in progress".into()),
    }
}

fn reasoning_buffer_key(item_id: &str, content_index: i64) -> String {
    format!("{REASONING_PREFIX}-{item_id}-{content_index}")
}

fn reasoning_summary_buffer_key(item_id: &str, summary_index: i64) -> String {
    format!("{REASONING_PREFIX}-summary-{item_id}-{summary_index}")
}

fn turn_plan_status_str(status: TurnPlanStepStatus) -> &'static str {
    match status {
        TurnPlanStepStatus::Pending => "pending",
        TurnPlanStepStatus::InProgress => "in_progress",
        TurnPlanStepStatus::Completed => "completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::AgentMessageDeltaNotification;
    use codex_app_server_protocol::CommandExecutionSource;
    use codex_app_server_protocol::ItemCompletedNotification;
    use codex_app_server_protocol::ItemStartedNotification;
    use pretty_assertions::assert_eq;

    fn agent_delta(item_id: &str, delta: &str) -> ServerNotification {
        ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: "th".into(),
            turn_id: "tu".into(),
            item_id: item_id.into(),
            delta: delta.into(),
        })
    }

    #[test]
    fn agent_message_delta_accumulates_cumulative_text() {
        let mut buffers = HashMap::new();
        let first = map_notification(&agent_delta("a-1", "hello "), &mut buffers);
        let second = map_notification(&agent_delta("a-1", "world"), &mut buffers);
        match (&first[0], &second[0]) {
            (
                Item::Text {
                    text: t1,
                    phase: p1,
                    ..
                },
                Item::Text {
                    text: t2,
                    phase: p2,
                    ..
                },
            ) => {
                assert_eq!(t1, "hello ");
                assert_eq!(t2, "hello world");
                assert_eq!(*p1, ItemLifecycle::Delta);
                assert_eq!(*p2, ItemLifecycle::Delta);
            }
            _ => panic!("expected Text items"),
        }
    }

    #[test]
    fn item_completed_agent_message_produces_canonical_completed_text() {
        let mut buffers = HashMap::new();
        map_notification(&agent_delta("a-1", "partial"), &mut buffers);
        let completed = ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: "th".into(),
            turn_id: "tu".into(),
            completed_at_ms: 0,
            item: ThreadItem::AgentMessage {
                id: "a-1".into(),
                text: "the canonical full reply".into(),
                phase: None,
                memory_citation: None,
            },
        });
        let items = map_notification(&completed, &mut buffers);
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::Text { text, phase, .. } => {
                assert_eq!(text, "the canonical full reply");
                assert_eq!(*phase, ItemLifecycle::Completed);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    fn make_cwd() -> codex_utils_absolute_path::AbsolutePathBuf {
        codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(std::path::PathBuf::from(
            "/tmp",
        ))
        .expect("/tmp is absolute")
    }

    #[test]
    fn command_execution_started_emits_bash_tool_call_with_no_result() {
        let item = ThreadItem::CommandExecution {
            id: "c-1".into(),
            command: "ls".into(),
            cwd: make_cwd(),
            process_id: None,
            source: CommandExecutionSource::default(),
            status: CommandExecutionStatus::InProgress,
            command_actions: Vec::new(),
            aggregated_output: None,
            exit_code: None,
            duration_ms: None,
        };
        let items = map_thread_item(&item, Phase::Started);
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::ToolCall {
                name,
                phase,
                result,
                ..
            } => {
                assert_eq!(name, "bash");
                assert_eq!(*phase, ItemLifecycle::Started);
                assert!(result.is_none());
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn command_execution_completed_zero_exit_is_ok() {
        let item = ThreadItem::CommandExecution {
            id: "c-1".into(),
            command: "ls".into(),
            cwd: make_cwd(),
            process_id: None,
            source: CommandExecutionSource::default(),
            status: CommandExecutionStatus::Completed,
            command_actions: Vec::new(),
            aggregated_output: Some("file1\nfile2\n".into()),
            exit_code: Some(0),
            duration_ms: Some(12),
        };
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall {
                phase,
                result: Some(Ok(out)),
                ..
            } => {
                assert_eq!(*phase, ItemLifecycle::Completed);
                assert!(out.contains("file1"));
            }
            other => panic!("expected completed Ok ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_montage_server_drops_namespace_prefix() {
        let item = ThreadItem::McpToolCall {
            id: "m-1".into(),
            server: "montage".into(),
            tool: "view_timeline".into(),
            status: McpToolCallStatus::Completed,
            arguments: json!({"focus": "1.0s"}),
            mcp_app_resource_uri: None,
            plugin_id: None,
            result: None,
            error: None,
            duration_ms: Some(4),
        };
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall { name, .. } => assert_eq!(name, "view_timeline"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_other_server_keeps_namespace_prefix() {
        let item = ThreadItem::McpToolCall {
            id: "m-2".into(),
            server: "other".into(),
            tool: "x".into(),
            status: McpToolCallStatus::Completed,
            arguments: json!({}),
            mcp_app_resource_uri: None,
            plugin_id: None,
            result: None,
            error: None,
            duration_ms: None,
        };
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall { name, .. } => assert_eq!(name, "other.x"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn file_change_completed_emits_ok_result() {
        let item = ThreadItem::FileChange {
            id: "f-1".into(),
            changes: Vec::new(),
            status: PatchApplyStatus::Completed,
        };
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall {
                name,
                phase,
                result: Some(Ok(out)),
                ..
            } => {
                assert_eq!(name, "apply_patch");
                assert_eq!(*phase, ItemLifecycle::Completed);
                assert_eq!(out, "applied");
            }
            other => panic!("expected applied apply_patch tool, got {other:?}"),
        }
    }

    #[test]
    fn extract_reasoning_pulls_top_level_string() {
        let args = json!({
            "ops": [{ "trim_clip": {} }],
            "reasoning": "trimmed 0.42s silence per podcast defaults",
        });
        assert_eq!(
            extract_reasoning(Some(&args)).as_deref(),
            Some("trimmed 0.42s silence per podcast defaults"),
        );
    }

    #[test]
    fn extract_reasoning_treats_missing_and_empty_as_none() {
        assert!(extract_reasoning(None).is_none());
        assert!(extract_reasoning(Some(&json!({}))).is_none());
        assert!(extract_reasoning(Some(&json!({ "reasoning": "" }))).is_none());
        assert!(extract_reasoning(Some(&json!({ "reasoning": "   " }))).is_none());
        // Non-string reasoning is ignored — we don't try to coerce
        // structured payloads into a one-liner.
        assert!(extract_reasoning(Some(&json!({ "reasoning": { "x": 1 } }))).is_none());
        assert!(extract_reasoning(Some(&json!({ "reasoning": 12 }))).is_none());
    }

    #[test]
    fn capability_metadata_exec_flags_destructive_commands() {
        let safe = build_capability_metadata_for_exec("ls -l");
        let destructive = build_capability_metadata_for_exec("rm -rf /tmp/foo");
        assert_eq!(safe["graph_mutates"], json!(false));
        assert_eq!(destructive["graph_mutates"], json!(true));
        assert_eq!(destructive["side_effects"], json!(["shell_exec"]));
    }

    #[test]
    fn item_started_command_execution_routes_through_map_thread_item() {
        let mut buffers = HashMap::new();
        let started = ServerNotification::ItemStarted(ItemStartedNotification {
            thread_id: "th".into(),
            turn_id: "tu".into(),
            started_at_ms: 0,
            item: ThreadItem::CommandExecution {
                id: "c-1".into(),
                command: "echo hi".into(),
                cwd: make_cwd(),
                process_id: None,
                source: CommandExecutionSource::default(),
                status: CommandExecutionStatus::InProgress,
                command_actions: Vec::new(),
                aggregated_output: None,
                exit_code: None,
                duration_ms: None,
            },
        });
        let items = map_notification(&started, &mut buffers);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Item::ToolCall { ref name, .. } if name == "bash"));
    }
}
