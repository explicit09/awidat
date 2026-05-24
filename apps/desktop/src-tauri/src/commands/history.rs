//! Chat-history/session commands for the desktop app.
//!
//! The core recorder already persists rollouts as JSONL. These commands
//! expose that store to the React shell and keep the next `start_turn`
//! pointed at the selected log so history replay and model resume stay
//! in sync.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use awidat_core::anthropic::{ContentBlock, Message, Role};
use awidat_core::rollout::{Recorder, RolloutItem, SessionMeta};
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

/// Persist a user-supplied title for a chat session. Pass an empty
/// `new_title` to clear the override and fall back to the derived
/// title (first user message).
#[tauri::command]
pub async fn rename_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
    new_title: String,
) -> Result<(), String> {
    let path = PathBuf::from(&log_path);
    // Belt-and-suspenders: confirm the path is under the configured
    // state root before touching the registry, so a bogus `log_path`
    // from the renderer can't rewrite an unrelated session row.
    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| "state root unavailable".to_string())?;
    if !path.starts_with(&state_root) {
        return Err(format!(
            "log path {} is not under state root {}",
            path.display(),
            state_root.display()
        ));
    }
    let registry = awidat_core::session_registry::SessionRegistry::open(&state_root)
        .map_err(|e| format!("open session registry: {e}"))?;
    let row = registry
        .get_by_log_path(&path)
        .map_err(|e| format!("lookup session: {e}"))?
        .ok_or_else(|| format!("no session registered for {}", path.display()))?;
    let trimmed = new_title.trim();
    let next = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    registry
        .set_custom_title(&row.id, next)
        .map_err(|e| format!("set custom title: {e}"))?;
    // Force-refresh any in-memory active-session state so the next
    // list/load picks up the new title. We don't touch `state.active`
    // directly here — `list_chat_sessions` reads from the registry.
    let _ = state;
    Ok(())
}

/// Delete a chat session entirely. Removes the JSONL log on disk and
/// the registry row. If the deleted chat was the active one, also
/// clears the active session so the next turn starts a fresh chat.
#[tauri::command]
pub async fn delete_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&log_path);
    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| "state root unavailable".to_string())?;
    if !path.starts_with(&state_root) {
        return Err(format!(
            "log path {} is not under state root {}",
            path.display(),
            state_root.display()
        ));
    }
    let registry = awidat_core::session_registry::SessionRegistry::open(&state_root)
        .map_err(|e| format!("open session registry: {e}"))?;
    let row = registry
        .get_by_log_path(&path)
        .map_err(|e| format!("lookup session: {e}"))?
        .ok_or_else(|| format!("no session registered for {}", path.display()))?;
    // If this chat is currently active, swap it out first so the
    // recorder isn't still writing to a file we're about to delete.
    {
        let mut active = state.active.lock().await;
        // `active.path()` would tell us, but its API is internal — the
        // simpler safe path is to always replace(None) before delete.
        // This is a no-op when nothing was active.
        active.replace(None).await;
    }
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("remove log file {}: {e}", path.display()));
        }
    }
    registry
        .delete_session(&row.id)
        .map_err(|e| format!("delete session row: {e}"))?;
    Ok(())
}

async fn load_history_from_path(
    state: &State<'_, AwidatState>,
    path: PathBuf,
    meta: SessionMeta,
) -> Result<ChatHistory, String> {
    let Some(state_root) = awidat_config::defaults::state_root() else {
        return Err("state root unavailable".into());
    };
    let entries = project_session_entries(&state_root, &meta.project_root)?;
    let messages = load_lineage_messages(&path, &entries)?;
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
                let _ =
                    registry.record_activity(&meta.id, messages.len() as i64, chrono::Utc::now());
            }
        }
    }

    let entries = project_session_entries(&state_root, project_root)?;
    let leaves = logical_leaf_entries(&entries);
    let mut out = Vec::with_capacity(leaves.len());
    for (path, meta) in leaves {
        // Title preference: user-supplied custom title wins. Otherwise
        // derive from the first user message in the whole logical
        // parent -> child chain. Falls back to the leaf timestamp.
        let custom_title = registry
            .get_by_log_path(&path)
            .ok()
            .flatten()
            .and_then(|row| row.custom_title);
        let messages = load_lineage_messages(&path, &entries).unwrap_or_default();
        let title = custom_title.unwrap_or_else(|| generated_session_title(&meta, &messages));
        out.push(summarize_session(&path, &meta, messages.len(), title));
    }
    Ok(out)
}

fn latest_project_session(project_root: &Path) -> Result<Option<(PathBuf, SessionMeta)>, String> {
    let Some(state_root) = awidat_config::defaults::state_root() else {
        return Ok(None);
    };
    let entries = project_session_entries(&state_root, project_root)?;
    Ok(logical_leaf_entries(&entries).into_iter().next())
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

fn project_session_entries(
    state_root: &Path,
    project_root: &Path,
) -> Result<Vec<(PathBuf, SessionMeta)>, String> {
    let entries = Recorder::list(state_root).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|(_, meta)| same_project_root(&meta.project_root, project_root))
        .collect())
}

fn logical_leaf_entries(entries: &[(PathBuf, SessionMeta)]) -> Vec<(PathBuf, SessionMeta)> {
    let parent_ids = entries
        .iter()
        .filter_map(|(_, meta)| meta.resumed_from.as_deref())
        .collect::<HashSet<_>>();
    entries
        .iter()
        .filter(|(_, meta)| !parent_ids.contains(meta.id.as_str()))
        .cloned()
        .collect()
}

fn load_lineage_messages(
    leaf_path: &Path,
    entries: &[(PathBuf, SessionMeta)],
) -> Result<Vec<Message>, String> {
    let mut by_id = HashMap::new();
    for (path, meta) in entries {
        by_id.insert(meta.id.clone(), (path.clone(), meta.clone()));
    }

    let mut chain = Vec::new();
    let mut current = entries
        .iter()
        .find(|(path, _)| same_log_path(path, leaf_path))
        .cloned()
        .ok_or_else(|| {
            format!(
                "session log not found in project list: {}",
                leaf_path.display()
            )
        })?;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current.1.id.clone()) {
            return Err(format!("cycle in chat session lineage at {}", current.1.id));
        }
        chain.push(current.clone());
        let Some(parent_id) = current.1.resumed_from.as_deref() else {
            break;
        };
        let Some(parent) = by_id.get(parent_id).cloned() else {
            break;
        };
        current = parent;
    }

    chain.reverse();
    let mut messages = Vec::new();
    for (path, _) in chain {
        let mut part = display_messages_from_log(&path)?;
        messages.append(&mut part);
    }
    Ok(messages)
}

fn display_messages_from_log(path: &Path) -> Result<Vec<Message>, String> {
    let body = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(RolloutItem::Message(message)) = serde_json::from_str::<RolloutItem>(line) else {
            continue;
        };
        messages.push(message);
    }
    Ok(messages)
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

/// Derive a short ChatGPT-style chat title from the first user
/// message. Strategy mirrors what ChatGPT does without paying for a
/// summarization model call:
///
///   1. Take the first non-empty line of the prompt.
///   2. Drop common leading filler ("can you", "please", "i want to",
///      "could you", "would you", "let's", "lets").
///   3. Capitalize the first character.
///   4. Cap at ~40 characters / 5 words, ending on a word boundary.
///   5. Add an ellipsis when truncated.
///
/// Examples:
///   "delete empty track"            → "Delete empty track"
///   "can you trim the gap?"         → "Trim the gap"
///   "please cut the first 30s"      → "Cut the first 30s"
///   "make a vertical version for tiktok with captions burned in"
///                                   → "Make a vertical version for…"
fn title_from_prompt(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(prompt)
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'');

    // Drop trailing punctuation but keep internal punctuation so
    // numbers ("30s", "16:9") stay intact.
    let stripped = first_line.trim_end_matches(|c: char| c == '.' || c == '!' || c == '?');

    // Filler prefixes to drop. Match longest-first.
    const FILLER_PREFIXES: &[&str] = &[
        "can you please ",
        "could you please ",
        "would you please ",
        "i want you to ",
        "i'd like you to ",
        "i would like you to ",
        "can you ",
        "could you ",
        "would you ",
        "please ",
        "i want to ",
        "i'd like to ",
        "let's ",
        "lets ",
    ];
    let lowered = stripped.to_lowercase();
    let mut start = 0usize;
    for prefix in FILLER_PREFIXES {
        if lowered.starts_with(prefix) {
            start = prefix.len();
            break;
        }
    }
    let body = stripped[start..].trim();
    if body.is_empty() {
        return String::new();
    }

    // Take up to 5 words, ~40 chars max.
    const MAX_WORDS: usize = 5;
    const MAX_CHARS: usize = 40;
    let mut taken_words = 0usize;
    let mut taken_chars = 0usize;
    let mut truncated = false;
    let mut out = String::new();
    for word in body.split_whitespace() {
        if taken_words >= MAX_WORDS {
            truncated = true;
            break;
        }
        // +1 for the space we'd insert before this word.
        let need = if out.is_empty() {
            word.chars().count()
        } else {
            word.chars().count() + 1
        };
        if taken_chars + need > MAX_CHARS {
            truncated = true;
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        taken_chars += need;
        taken_words += 1;
    }
    if out.is_empty() {
        return String::new();
    }
    // Capitalize first char.
    let mut chars = out.chars();
    let first = chars.next().unwrap();
    let mut titled = String::with_capacity(out.len());
    for c in first.to_uppercase() {
        titled.push(c);
    }
    titled.extend(chars);
    if truncated {
        titled.push('…');
    }
    titled
}

fn same_project_root(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn same_log_path(a: &Path, b: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_capitalizes_first_word() {
        assert_eq!(
            title_from_prompt("delete empty track"),
            "Delete empty track"
        );
    }

    #[test]
    fn title_drops_can_you_prefix() {
        assert_eq!(title_from_prompt("can you trim the gap?"), "Trim the gap");
    }

    #[test]
    fn title_drops_please_prefix() {
        assert_eq!(
            title_from_prompt("please cut the first 30s"),
            "Cut the first 30s"
        );
    }

    #[test]
    fn title_drops_i_want_to_prefix() {
        assert_eq!(
            title_from_prompt("i want to make a vertical clip"),
            "Make a vertical clip"
        );
    }

    #[test]
    fn title_caps_at_max_words_and_appends_ellipsis() {
        let t =
            title_from_prompt("make a vertical version for tiktok with captions burned in please");
        // 5 words max → "Make a vertical version for"; ellipsis added.
        assert!(
            t.ends_with('…'),
            "expected ellipsis on truncation; got {t:?}"
        );
        assert!(t.chars().count() <= 42, "title too long: {t:?}");
    }

    #[test]
    fn title_handles_view_context_stripped_prompt() {
        // strip_view_context runs first; title sees just the user text.
        let prompt = strip_view_context(
            "[user is viewing copy_F65206FA-9AEC-4CA8-BD2F-ACD63775F7FA at 0:00]\n\ndelete empty track",
        );
        assert_eq!(title_from_prompt(&prompt), "Delete empty track");
    }

    #[test]
    fn title_empty_input_returns_empty() {
        assert_eq!(title_from_prompt(""), "");
        assert_eq!(title_from_prompt("   "), "");
    }

    #[test]
    fn title_preserves_internal_punctuation() {
        // Times, colons, ratios should survive — we only strip end punct.
        assert_eq!(title_from_prompt("cut 0:30 to 1:00"), "Cut 0:30 to 1:00");
    }

    #[test]
    fn title_strips_trailing_punctuation() {
        assert_eq!(title_from_prompt("trim it!"), "Trim it");
        assert_eq!(title_from_prompt("done?"), "Done");
    }

    #[test]
    fn title_lets_prefix_dropped() {
        assert_eq!(
            title_from_prompt("let's add some titles"),
            "Add some titles"
        );
        assert_eq!(title_from_prompt("lets add some titles"), "Add some titles");
    }

    #[test]
    fn logical_leaf_entries_hide_resumed_parent() {
        let started_at = chrono::Utc::now();
        let parent = SessionMeta {
            id: "parent".into(),
            project_root: PathBuf::from("/tmp/project"),
            model: "m".into(),
            started_at,
            awidat_version: "test".into(),
            resumed_from: None,
        };
        let child = SessionMeta {
            id: "child".into(),
            project_root: PathBuf::from("/tmp/project"),
            model: "m".into(),
            started_at,
            awidat_version: "test".into(),
            resumed_from: Some("parent".into()),
        };
        let entries = vec![
            (PathBuf::from("/tmp/parent.jsonl"), parent),
            (PathBuf::from("/tmp/child.jsonl"), child),
        ];

        let leaves = logical_leaf_entries(&entries);

        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].1.id, "child");
    }

    #[test]
    fn display_messages_include_unfinished_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let meta = SessionMeta {
            id: "s".into(),
            project_root: PathBuf::from("/tmp/project"),
            model: "m".into(),
            started_at: chrono::Utc::now(),
            awidat_version: "test".into(),
            resumed_from: None,
        };
        let body = [
            serde_json::to_string(&RolloutItem::SessionMeta(meta)).unwrap(),
            serde_json::to_string(&RolloutItem::TurnStarted {
                turn_id: "t".into(),
                started_at: chrono::Utc::now(),
            })
            .unwrap(),
            serde_json::to_string(&RolloutItem::Message(Message::user_text("partial"))).unwrap(),
        ]
        .join("\n");
        std::fs::write(&path, body).unwrap();

        let messages = display_messages_from_log(&path).unwrap();

        assert_eq!(messages.len(), 1);
    }
}
