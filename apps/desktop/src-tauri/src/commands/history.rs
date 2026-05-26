//! Chat-history/session commands for the desktop app.
//!
//! **Step 8d:** stubbed out.
//!
//! The legacy implementation read `awidat-core`'s Anthropic-shape
//! rollout JSONL files (the now-deleted `awidat_core::rollout::Recorder`
//! and `awidat_core::anthropic` Message/ContentBlock shapes) to
//! reconstruct chat history for the React shell. The desktop has
//! since cut over to driving the codex engine via a subprocess
//! (`apps/desktop/src-tauri/src/codex_runner.rs`), which writes
//! its own rollout files into `~/.codex/sessions/...` in a
//! different shape (`vendor/codex-rs/core/src/rollout/`).
//!
//! Re-targeting these commands at the codex rollout store is real
//! work — lineage walking, message→item mapping, title derivation,
//! plus a desktop registry for custom titles — and was not in scope
//! for the 8d cleanup pass. The commands are stubbed so the
//! frontend's IPC handles still resolve (it tolerates missing
//! history), and the legacy rollout files on disk are left
//! untouched.
//!
//! **Follow-up:** port these commands onto codex's
//! `RolloutRecorder::list_conversations` and the
//! `vendor/codex-rs/protocol/src/protocol.rs::RolloutItem` /
//! `ResponseItem` shapes. Track in a follow-up task referenced from
//! the commit message.

use awidat_desktop_protocol::Item;
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

/// List recent chat sessions for the current project.
///
/// Stub: codex-rollout-backed listing not implemented; returns an
/// empty list. The frontend renders this as "no past chats" which
/// matches a fresh project anyway, so users see no error.
#[tauri::command]
pub async fn list_chat_sessions(
    state: State<'_, AwidatState>,
) -> Result<Vec<ChatSessionSummary>, String> {
    let _ = state;
    Ok(Vec::new())
}

/// Load history for the current project's latest chat (called on
/// project open).
///
/// Stub: returns an empty history, which the frontend renders as a
/// blank chat pane (same UX as a fresh chat).
#[tauri::command]
pub async fn load_chat_history(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    let _ = state;
    Ok(ChatHistory {
        session: None,
        items: Vec::new(),
    })
}

/// Load a specific chat session by its on-disk log path.
///
/// Stub: codex rollouts aren't yet surfaced through this command;
/// returns an empty history. With [`list_chat_sessions`] also
/// stubbed the frontend can't reach this path through the rail UI,
/// so the empty-history fallback is just defense in depth.
#[tauri::command]
pub async fn load_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<ChatHistory, String> {
    let _ = (state, log_path);
    Ok(ChatHistory {
        session: None,
        items: Vec::new(),
    })
}

/// Start a fresh chat — clears any active session context.
///
/// Stub: nothing to clear (each turn spawns its own codex
/// subprocess), so this just returns the empty history shape the
/// frontend expects.
#[tauri::command]
pub async fn start_new_chat_session(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    let _ = state;
    Ok(ChatHistory {
        session: None,
        items: Vec::new(),
    })
}

/// Persist a user-supplied title for a chat session.
///
/// Stub: with chat-listing un-wired there's nothing to rename;
/// returns an error so accidental invocations from the UI surface
/// clearly rather than silently no-op'ing.
#[tauri::command]
pub async fn rename_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
    new_title: String,
) -> Result<(), String> {
    let _ = (state, log_path, new_title);
    Err("chat-history rename not yet wired to codex rollouts (step 8d follow-up)".into())
}

/// Delete a chat session entirely.
///
/// Stub: see [`rename_chat_session`].
#[tauri::command]
pub async fn delete_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<(), String> {
    let _ = (state, log_path);
    Err("chat-history delete not yet wired to codex rollouts (step 8d follow-up)".into())
}
