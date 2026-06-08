//! Pure mapping functions from Codex app-server JSON notifications to
//! [`montage_desktop_protocol::Item`] values.

use std::collections::HashMap;

use montage_desktop_protocol::Id;
use montage_desktop_protocol::Item;
use montage_desktop_protocol::ItemLifecycle;
use montage_desktop_protocol::PlanStep;
use serde_json::json;

use crate::wire::ServerNotification;

const AGENT_MESSAGE_PREFIX: &str = "codex-msg";
const REASONING_PREFIX: &str = "codex-reason";

pub fn map_notification(
    notification: &ServerNotification,
    text_buffers: &mut HashMap<String, String>,
) -> Vec<Item> {
    match notification.method.as_str() {
        "item/agentMessage/delta" => {
            let item_id = string_at(&notification.params, "itemId");
            let delta = string_at(&notification.params, "delta");
            let buffer = text_buffers.entry(item_id.clone()).or_default();
            buffer.push_str(&delta);
            vec![Item::Text {
                id: Id::new(format!("{AGENT_MESSAGE_PREFIX}-{item_id}")),
                phase: ItemLifecycle::Delta,
                text: buffer.clone(),
            }]
        }
        "item/reasoning/textDelta" => {
            let item_id = string_at(&notification.params, "itemId");
            let content_index = i64_at(&notification.params, "contentIndex");
            let delta = string_at(&notification.params, "delta");
            let key = reasoning_buffer_key(&item_id, content_index);
            let buffer = text_buffers.entry(key).or_default();
            buffer.push_str(&delta);
            vec![Item::Text {
                id: Id::new(format!("{REASONING_PREFIX}-{item_id}-{content_index}")),
                phase: ItemLifecycle::Delta,
                text: buffer.clone(),
            }]
        }
        "item/reasoning/summaryTextDelta" => {
            let item_id = string_at(&notification.params, "itemId");
            let summary_index = i64_at(&notification.params, "summaryIndex");
            let delta = string_at(&notification.params, "delta");
            let key = reasoning_summary_buffer_key(&item_id, summary_index);
            let buffer = text_buffers.entry(key).or_default();
            buffer.push_str(&delta);
            vec![Item::Text {
                id: Id::new(format!(
                    "{REASONING_PREFIX}-summary-{item_id}-{summary_index}"
                )),
                phase: ItemLifecycle::Delta,
                text: buffer.clone(),
            }]
        }
        "item/started" => map_thread_item(&notification.params["item"], Phase::Started),
        "item/completed" => {
            if let Some(item_id) = notification.params["item"]["id"].as_str() {
                text_buffers.remove(item_id);
            }
            map_thread_item(&notification.params["item"], Phase::Completed)
        }
        "turn/plan/updated" => {
            let items = notification.params["plan"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|step| PlanStep {
                    step: string_at(step, "step"),
                    status: string_at(step, "status"),
                })
                .collect::<Vec<_>>();
            vec![Item::Plan {
                id: Id::new("codex-plan"),
                phase: ItemLifecycle::Delta,
                items,
                note: notification.params["explanation"]
                    .as_str()
                    .map(ToString::to_string),
            }]
        }
        "error" => vec![Item::Error {
            id: Id::new(format!(
                "codex-err-{}",
                string_at(&notification.params, "threadId")
            )),
            message: string_at(&notification.params["error"], "message"),
        }],
        _ => Vec::new(),
    }
}

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

pub fn map_thread_item(item: &serde_json::Value, phase: Phase) -> Vec<Item> {
    let life: ItemLifecycle = phase.into();
    match string_at(item, "type").as_str() {
        "agentMessage" => {
            let text = string_at(item, "text");
            if text.trim().is_empty() {
                return Vec::new();
            }
            vec![Item::Text {
                id: Id::new(format!(
                    "{}-{}",
                    AGENT_MESSAGE_PREFIX,
                    string_at(item, "id")
                )),
                phase: life,
                text,
            }]
        }
        "reasoning" => {
            let mut combined = String::new();
            for field in ["summary", "content"] {
                for value in item[field].as_array().into_iter().flatten() {
                    let Some(text) = value.as_str() else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    if !combined.is_empty() {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(text);
                }
            }
            if combined.is_empty() {
                return Vec::new();
            }
            vec![Item::Text {
                id: Id::new(format!("{}-{}", REASONING_PREFIX, string_at(item, "id"))),
                phase: life,
                text: combined,
            }]
        }
        "commandExecution" => {
            let output = item["aggregatedOutput"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let result = match phase {
                Phase::Started => None,
                Phase::Completed => Some(command_execution_result(
                    item["status"].as_str().unwrap_or_default(),
                    item["exitCode"].as_i64(),
                    output,
                )),
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-cmd-{}", string_at(item, "id"))),
                phase: life,
                name: "bash".into(),
                args: json!({ "command": string_at(item, "command") }),
                result,
            }]
        }
        "fileChange" => {
            let result = match phase {
                Phase::Started => None,
                Phase::Completed => Some(file_change_result(
                    item["status"].as_str().unwrap_or_default(),
                )),
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-patch-{}", string_at(item, "id"))),
                phase: life,
                name: "apply_patch".into(),
                args: json!({ "changes": item["changes"].clone() }),
                result,
            }]
        }
        "mcpToolCall" => {
            let server = string_at(item, "server");
            let tool = string_at(item, "tool");
            let display_name = if server == "montage" {
                tool
            } else {
                format!("{server}.{tool}")
            };
            let result_value = match phase {
                Phase::Started => None,
                Phase::Completed => Some(mcp_tool_result(item)),
            };
            vec![Item::ToolCall {
                id: Id::new(format!("codex-mcp-{}", string_at(item, "id"))),
                phase: life,
                name: display_name,
                args: item["arguments"].clone(),
                result: result_value,
            }]
        }
        "plan" => vec![Item::Plan {
            id: Id::new(format!("codex-plan-{}", string_at(item, "id"))),
            phase: life,
            items: vec![PlanStep {
                step: string_at(item, "text"),
                status: "pending".to_string(),
            }],
            note: None,
        }],
        _ => Vec::new(),
    }
}

pub fn extract_reasoning(arguments: Option<&serde_json::Value>) -> Option<String> {
    let value = arguments?.get("reasoning")?.as_str()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn build_capability_metadata_for_exec(command: &str) -> serde_json::Value {
    let mutates = command_looks_destructive(command);
    json!({
        "graph_mutates": mutates,
        "preview_supported": false,
        "side_effects": ["shell_exec"],
    })
}

pub fn build_capability_metadata_for_file_change(reason: Option<&str>) -> serde_json::Value {
    json!({
        "graph_mutates": true,
        "preview_supported": false,
        "side_effects": ["filesystem_write"],
        "reason": reason,
    })
}

pub fn is_project_mutating_completion(notification: &ServerNotification) -> bool {
    if notification.method != "item/completed" {
        return false;
    }
    let item = &notification.params["item"];
    string_at(item, "type") == "mcpToolCall"
        && string_at(item, "server") == "montage"
        && item["error"].is_null()
        && item["status"].as_str() == Some("completed")
}

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
    status: &str,
    exit_code: Option<i64>,
    output: String,
) -> Result<String, String> {
    match status {
        "completed" if exit_code.unwrap_or(0) == 0 => Ok(output),
        "completed" => Err(format!(
            "exit_code={} output={output}",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into())
        )),
        "failed" => Err(format!("failed: {output}")),
        "inProgress" => Err("still in progress".to_string()),
        "declined" => Err("declined by user".to_string()),
        _ => Err(format!("unknown command status: {status}")),
    }
}

fn file_change_result(status: &str) -> Result<String, String> {
    match status {
        "completed" => Ok("applied".into()),
        "failed" => Err("apply_patch failed".into()),
        "inProgress" => Err("apply_patch still in progress".into()),
        "declined" => Err("apply_patch declined by user".into()),
        _ => Err(format!("unknown file-change status: {status}")),
    }
}

fn mcp_tool_result(item: &serde_json::Value) -> Result<String, String> {
    if let Some(message) = item["error"]["message"].as_str() {
        return Err(message.to_string());
    }
    match item["status"].as_str().unwrap_or_default() {
        "completed" => {
            if item["result"].is_null() {
                Ok(String::new())
            } else {
                Ok(serde_json::to_string(&item["result"]).unwrap_or_default())
            }
        }
        "failed" => Err("mcp tool failed".into()),
        "inProgress" => Err("mcp tool still in progress".into()),
        other => Err(format!("unknown mcp tool status: {other}")),
    }
}

fn string_at(value: &serde_json::Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn i64_at(value: &serde_json::Value, key: &str) -> i64 {
    value[key].as_i64().unwrap_or_default()
}

fn reasoning_buffer_key(item_id: &str, content_index: i64) -> String {
    format!("{REASONING_PREFIX}-{item_id}-{content_index}")
}

fn reasoning_summary_buffer_key(item_id: &str, summary_index: i64) -> String {
    format!("{REASONING_PREFIX}-summary-{item_id}-{summary_index}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn notification(method: &str, params: serde_json::Value) -> ServerNotification {
        ServerNotification {
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn agent_message_delta_accumulates_cumulative_text() {
        let mut buffers = HashMap::new();
        let first = map_notification(
            &notification(
                "item/agentMessage/delta",
                json!({"itemId": "a-1", "delta": "hello "}),
            ),
            &mut buffers,
        );
        let second = map_notification(
            &notification(
                "item/agentMessage/delta",
                json!({"itemId": "a-1", "delta": "world"}),
            ),
            &mut buffers,
        );
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
        map_notification(
            &notification(
                "item/agentMessage/delta",
                json!({"itemId": "a-1", "delta": "partial"}),
            ),
            &mut buffers,
        );
        let items = map_notification(
            &notification(
                "item/completed",
                json!({"item": {"type": "agentMessage", "id": "a-1", "text": "the canonical full reply"}}),
            ),
            &mut buffers,
        );
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::Text { text, phase, .. } => {
                assert_eq!(text, "the canonical full reply");
                assert_eq!(*phase, ItemLifecycle::Completed);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn command_execution_started_emits_bash_tool_call_with_no_result() {
        let item = json!({
            "type": "commandExecution",
            "id": "c-1",
            "command": "ls",
            "status": "inProgress",
            "aggregatedOutput": null,
            "exitCode": null
        });
        let items = map_thread_item(&item, Phase::Started);
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
        let item = json!({
            "type": "commandExecution",
            "id": "c-1",
            "command": "ls",
            "status": "completed",
            "aggregatedOutput": "file1\nfile2\n",
            "exitCode": 0
        });
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
        let item = json!({
            "type": "mcpToolCall",
            "id": "m-1",
            "server": "montage",
            "tool": "view_timeline",
            "status": "completed",
            "arguments": {"focus": "1.0s"},
            "result": null,
            "error": null
        });
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall { name, .. } => assert_eq!(name, "view_timeline"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn mcp_tool_other_server_keeps_namespace_prefix() {
        let item = json!({
            "type": "mcpToolCall",
            "id": "m-2",
            "server": "other",
            "tool": "x",
            "status": "completed",
            "arguments": {},
            "result": null,
            "error": null
        });
        let items = map_thread_item(&item, Phase::Completed);
        match &items[0] {
            Item::ToolCall { name, .. } => assert_eq!(name, "other.x"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn file_change_completed_emits_ok_result() {
        let item = json!({
            "type": "fileChange",
            "id": "f-1",
            "changes": [],
            "status": "completed"
        });
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
    fn capability_metadata_exec_flags_destructive_commands() {
        let safe = build_capability_metadata_for_exec("ls -l");
        let destructive = build_capability_metadata_for_exec("rm -rf /tmp/foo");
        assert_eq!(safe["graph_mutates"], json!(false));
        assert_eq!(destructive["graph_mutates"], json!(true));
    }

    #[test]
    fn mutating_completion_detects_completed_montage_tool() {
        let notification = notification(
            "item/completed",
            json!({
                "item": {
                    "type": "mcpToolCall",
                    "server": "montage",
                    "status": "completed",
                    "error": null
                }
            }),
        );
        assert!(is_project_mutating_completion(&notification));
    }
}
