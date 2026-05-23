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
        state.active.lock().await.replace(None).await;
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };

    let Some((path, meta)) = latest_project_session(&project_root)? else {
        state.active.lock().await.replace(None).await;
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
    // No explicit clear here — `load_history_from_path` calls
    // `active.replace(Some(path))` which atomically shuts down the
    // current session (if any) before swapping the resume path in.
    load_history_from_path(&state, path, meta).await
}

#[tauri::command]
pub async fn start_new_chat_session(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    state.active.lock().await.replace(None).await;
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
    let title = generated_session_title(&meta, &messages);
    let summary = summarize_session(&path, &meta, messages.len(), title);
    let items = messages_to_items(&meta.id, &messages);
    // Atomic swap: shut down the current session (flushing its
    // recorder) before pointing at the new resume log. Prevents two
    // recorders from racing on the same JSONL file.
    state.active.lock().await.replace(Some(path)).await;
    Ok(ChatHistory {
        session: Some(summary),
        items,
    })
}

fn list_project_sessions(project_root: &Path) -> Result<Vec<ChatSessionSummary>, String> {
    let Some(state_root) = awidat_config::defaults::state_root() else {
        return Ok(Vec::new());
    };
    let registry = awidat_core::session_registry::SessionRegistry::open(&state_root)
        .map_err(|e| format!("open session registry: {e}"))?;

    // Backfill: on first boot after the registry landed, the JSONL
    // files exist but the DB has no rows. Walk the filesystem once
    // and INSERT. Subsequent calls find rows already there and skip
    // the walk.
    if registry
        .list(Some(project_root))
        .map_err(|e| format!("list registry: {e}"))?
        .is_empty()
    {
        let entries = Recorder::list(&state_root).map_err(|e| e.to_string())?;
        for (path, meta) in &entries {
            if !same_project_root(&meta.project_root, project_root) {
                continue;
            }
            // INSERT OR REPLACE — idempotent if we end up here again.
            if let Err(e) = registry.create_session(
                &meta.id,
                &meta.project_root,
                path,
                &meta.model,
                meta.started_at,
            ) {
                tracing::warn!(error = %e, path = %path.display(), "registry backfill failed");
            }
            // Mark Completed (backfilled rows are not the active
            // session — by definition the active one didn't exist
            // yet when this scan started).
            let _ = registry.set_status(
                &meta.id,
                awidat_core::session_registry::SessionStatus::Completed,
            );
            // Bump message_count by reading the JSONL once. Cheap
            // and one-shot; subsequent activity comes from the live
            // recorder's per-message updates.
            if let Ok((_, messages)) = Recorder::resume(path) {
                let _ = registry.record_activity(
                    &meta.id,
                    messages.len() as i64,
                    chrono::Utc::now(),
                );
            }
        }
    }

    let rows = registry
        .list(Some(project_root))
        .map_err(|e| format!("list registry: {e}"))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        // The registry doesn't store the session title (it's derived
        // from message content). Resume the log just for the title;
        // tolerated cost since chat history listings are interactive.
        let title = match Recorder::resume(&row.log_path) {
            Ok((meta, messages)) => generated_session_title(&meta, &messages),
            Err(_) => fallback_session_title(&fallback_meta_for_row(&row)),
        };
        out.push(ChatSessionSummary {
            id: row.id,
            title,
            project_root: row.project_root.to_string_lossy().into_owned(),
            log_path: row.log_path.to_string_lossy().into_owned(),
            started_at: row.started_at.to_rfc3339(),
            message_count: row.message_count,
        });
    }
    Ok(out)
}

/// Synthesize a minimal SessionMeta for fallback title generation
/// when the JSONL can't be parsed. We only need it so the existing
/// `fallback_session_title` helper compiles.
fn fallback_meta_for_row(row: &awidat_core::session_registry::SessionRow) -> SessionMeta {
    SessionMeta {
        id: row.id.clone(),
        project_root: row.project_root.clone(),
        model: row.model.clone(),
        started_at: row.started_at,
        awidat_version: String::new(),
        resumed_from: None,
    }
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

fn summarize_session(
    path: &Path,
    meta: &SessionMeta,
    message_count: usize,
    title: String,
) -> ChatSessionSummary {
    ChatSessionSummary {
        id: meta.id.clone(),
        title,
        project_root: meta.project_root.to_string_lossy().into_owned(),
        log_path: path.to_string_lossy().into_owned(),
        started_at: meta.started_at.to_rfc3339(),
        message_count,
    }
}

fn generated_session_title(meta: &SessionMeta, messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|message| matches!(message.role, Role::User))
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(strip_view_context(text)),
            _ => None,
        })
        .map(|text| title_from_prompt(&text))
        .find(|title| !title.is_empty())
        .unwrap_or_else(|| fallback_session_title(meta))
}

fn fallback_session_title(meta: &SessionMeta) -> String {
    format!("Chat from {}", meta.started_at.format("%b %-d, %-I:%M %p"))
}

fn title_from_prompt(prompt: &str) -> String {
    let normalized = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(prompt)
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c.is_ascii_punctuation());

    let mut words = Vec::new();
    for word in normalized.split_whitespace() {
        let cleaned = word
            .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
            .trim_matches(|c: char| c == ',' || c == '.' || c == ':' || c == ';');
        if cleaned.is_empty() {
            continue;
        }
        words.push(cleaned);
        if words.len() == 7 {
            break;
        }
    }

    let title = words.join(" ");
    if title.len() <= 64 {
        title
    } else {
        format!("{}...", title.chars().take(61).collect::<String>())
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
