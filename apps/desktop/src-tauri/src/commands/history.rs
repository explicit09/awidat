//! Chat-history/session commands for the desktop app.
//!
//! The core recorder already persists rollouts as JSONL. These commands
//! expose that store to the React shell and keep the next `start_turn`
//! pointed at the selected log so history replay and model resume stay
//! in sync.

use std::path::{Path, PathBuf};

use awidat_core::anthropic::{ContentBlock, Message, Role};
use awidat_core::rollout::{Recorder, SessionMeta};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle};
use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionSummary {
    pub id: String,
    pub title: String,
    pub project_root: String,
    pub log_path: String,
    pub started_at: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistory {
    pub session: Option<ChatSessionSummary>,
    pub items: Vec<Item>,
}

#[tauri::command]
pub async fn list_chat_sessions(
    state: State<'_, AwidatState>,
) -> Result<Vec<ChatSessionSummary>, String> {
    let Some(project_root) = state.project_root.lock().await.clone() else {
        return Ok(Vec::new());
    };
    list_project_sessions(&project_root)
}

#[tauri::command]
pub async fn load_chat_history(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    let Some(project_root) = state.project_root.lock().await.clone() else {
        *state.resume_log_path.lock().await = None;
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };

    let Some((path, meta)) = latest_project_session(&project_root)? else {
        *state.resume_log_path.lock().await = None;
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };

    load_history_from_path(&state, path, meta).await
}

#[tauri::command]
pub async fn load_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<ChatHistory, String> {
    let path = PathBuf::from(log_path);
    let (meta, _) = Recorder::resume(&path).map_err(|e| e.to_string())?;
    let Some(project_root) = state.project_root.lock().await.clone() else {
        return Err("no project loaded".into());
    };
    if !same_project_root(&meta.project_root, &project_root) {
        return Err("that chat belongs to a different project".into());
    }
    *state.session.lock().await = None;
    load_history_from_path(&state, path, meta).await
}

#[tauri::command]
pub async fn start_new_chat_session(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    *state.session.lock().await = None;
    *state.resume_log_path.lock().await = None;
    Ok(ChatHistory {
        session: None,
        items: Vec::new(),
    })
}

async fn load_history_from_path(
    state: &State<'_, AwidatState>,
    path: PathBuf,
    meta: SessionMeta,
) -> Result<ChatHistory, String> {
    let (_meta, messages) = Recorder::resume(&path).map_err(|e| e.to_string())?;
    let summary = summarize_session(&path, &meta, messages.len());
    let items = messages_to_items(&meta.id, &messages);
    *state.session.lock().await = None;
    *state.resume_log_path.lock().await = Some(path);
    Ok(ChatHistory {
        session: Some(summary),
        items,
    })
}

fn list_project_sessions(project_root: &Path) -> Result<Vec<ChatSessionSummary>, String> {
    let Some(state_root) = awidat_config::defaults::state_root() else {
        return Ok(Vec::new());
    };
    let entries = Recorder::list(&state_root).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (path, meta) in entries {
        if !same_project_root(&meta.project_root, project_root) {
            continue;
        }
        let message_count = Recorder::resume(&path)
            .map(|(_, messages)| messages.len())
            .unwrap_or(0);
        out.push(summarize_session(&path, &meta, message_count));
    }
    Ok(out)
}

fn latest_project_session(project_root: &Path) -> Result<Option<(PathBuf, SessionMeta)>, String> {
    let Some(state_root) = awidat_config::defaults::state_root() else {
        return Ok(None);
    };
    let entries = Recorder::list(&state_root).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .find(|(_, meta)| same_project_root(&meta.project_root, project_root)))
}

fn summarize_session(path: &Path, meta: &SessionMeta, message_count: usize) -> ChatSessionSummary {
    ChatSessionSummary {
        id: meta.id.clone(),
        title: format!(
            "{} · {}",
            meta.started_at.format("%b %-d, %-I:%M %p"),
            meta.model
        ),
        project_root: meta.project_root.to_string_lossy().into_owned(),
        log_path: path.to_string_lossy().into_owned(),
        started_at: meta.started_at.to_rfc3339(),
        message_count,
    }
}

fn same_project_root(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn messages_to_items(session_id: &str, messages: &[Message]) -> Vec<Item> {
    let mut out = Vec::new();
    for (message_idx, message) in messages.iter().enumerate() {
        match message.role {
            Role::User => push_user_items(session_id, message_idx, message, &mut out),
            Role::Assistant => push_assistant_items(session_id, message_idx, message, &mut out),
        }
    }
    out
}

fn push_user_items(session_id: &str, message_idx: usize, message: &Message, out: &mut Vec<Item>) {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(strip_view_context(text)),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                apply_tool_result(out, tool_use_id, content, is_error.unwrap_or(false));
                None
            }
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    if !text.trim().is_empty() {
        out.push(Item::UserInput {
            id: Id::new(format!("hist-{session_id}-user-{message_idx}")),
            text,
        });
    }
}

fn push_assistant_items(
    session_id: &str,
    message_idx: usize,
    message: &Message,
    out: &mut Vec<Item>,
) {
    for (block_idx, block) in message.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                out.push(Item::Text {
                    id: Id::new(format!("hist-{session_id}-text-{message_idx}-{block_idx}")),
                    phase: ItemLifecycle::Completed,
                    text: text.clone(),
                });
            }
            ContentBlock::ToolUse { id, name, input } => {
                out.push(Item::ToolCall {
                    id: Id::new(id.clone()),
                    phase: ItemLifecycle::Completed,
                    name: name.clone(),
                    args: input.clone(),
                    result: None,
                });
            }
            _ => {}
        }
    }
}

fn apply_tool_result(
    items: &mut [Item],
    tool_use_id: &str,
    content: &serde_json::Value,
    is_error: bool,
) {
    for item in items.iter_mut().rev() {
        let Item::ToolCall {
            id, phase, result, ..
        } = item
        else {
            continue;
        };
        if id.0 != tool_use_id {
            continue;
        }
        *phase = ItemLifecycle::Completed;
        let text = tool_result_text(content);
        *result = Some(if is_error { Err(text) } else { Ok(text) });
        break;
    }
}

fn tool_result_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n"),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn strip_view_context(text: &str) -> String {
    if !text.starts_with("[user is ") {
        return text.to_string();
    }
    match text.split_once("]\n\n") {
        Some((_, rest)) => rest.to_string(),
        None => text.to_string(),
    }
}
