//! Chat-history/session commands for the desktop app.
//!
//! Backed by codex's rollout JSONL store at `~/.codex/sessions/YYYY/MM/DD/`.
//! Each rollout's first line is a `session_meta` record carrying the
//! `thread_id`, `cwd`, and timestamp; subsequent lines are
//! `response_item` records (and other variants we ignore for history
//! reconstruction). See
//! `vendor/codex-rs/protocol/src/protocol.rs::RolloutItem` and
//! `vendor/codex-rs/protocol/src/models.rs::ResponseItem` for the wire
//! shape.
//!
//! `rename_chat_session` / `delete_chat_session` are still stubbed
//! (out of scope for this pass; they need a desktop-owned title
//! sidecar + a destructive operation against codex's store).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use awidat_desktop_protocol::{Id, Item, ItemLifecycle};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionSummary {
    /// Codex `thread_id`. Stable across resume — `id == thread_id`.
    pub id: String,
    /// Human-friendly chat title (first user message, truncated).
    pub title: String,
    /// Project the rollout was recorded against. Always the current
    /// project for entries returned here (we filter by cwd).
    pub project_root: String,
    /// Absolute path of the rollout `.jsonl`. Acts as the stable key
    /// the frontend uses with `load_chat_session` /
    /// `resume_chat_session`.
    pub log_path: String,
    /// ISO-8601 timestamp of the rollout's last mutation. Used both
    /// for sort order (newest-first) and as the "last activity" label.
    pub started_at: String,
    /// Count of `response_item` lines whose `payload.type == "message"`.
    pub message_count: usize,
    /// True when this rollout's `thread_id` matches the bridge's live
    /// thread (i.e. the chat the next `start_turn` will continue).
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistory {
    pub session: Option<ChatSessionSummary>,
    pub items: Vec<Item>,
}

/// Walk `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, parse the
/// `session_meta` header from each file, and surface those whose
/// recorded `cwd` matches the currently-open project. Newest-first.
#[tauri::command]
pub async fn list_chat_sessions(
    state: State<'_, AwidatState>,
) -> Result<Vec<ChatSessionSummary>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let sessions_dir = match codex_sessions_dir() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }

    let live_thread_id = current_live_thread_id(&state).await;
    let mut rollouts = Vec::new();
    walk_rollouts(&sessions_dir, &mut rollouts);

    let mut summaries: Vec<ChatSessionSummary> = rollouts
        .into_iter()
        .filter_map(|path| summarize_rollout(&path, &project_root, live_thread_id.as_deref()))
        .collect();
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(summaries)
}

/// Hydrate the currently-live codex session by mapping its
/// `thread_id` to a rollout file. With the bridge running this gives
/// the React shell the messages emitted in prior turns so a
/// project re-open doesn't blank the chat.
#[tauri::command]
pub async fn load_chat_history(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => {
            return Ok(ChatHistory {
                session: None,
                items: Vec::new(),
            });
        }
    };
    let Some(thread_id) = current_live_thread_id(&state).await else {
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };
    let Some(sessions_dir) = codex_sessions_dir() else {
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };
    if !sessions_dir.is_dir() {
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    }
    let mut rollouts = Vec::new();
    walk_rollouts(&sessions_dir, &mut rollouts);
    let rollout_path = rollouts.into_iter().find(|p| match read_first_line(p) {
        Some(line) => parse_session_meta(&line)
            .map(|m| m.thread_id == thread_id)
            .unwrap_or(false),
        None => false,
    });
    let Some(path) = rollout_path else {
        return Ok(ChatHistory {
            session: None,
            items: Vec::new(),
        });
    };
    let summary = summarize_rollout(&path, &project_root, Some(&thread_id));
    let items = read_rollout_items(&path);
    Ok(ChatHistory {
        session: summary,
        items,
    })
}

/// Load a specific rollout by its absolute path. `log_path` matches
/// what `list_chat_sessions` returned.
#[tauri::command]
pub async fn load_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<ChatHistory, String> {
    let path = PathBuf::from(&log_path);
    if !path.is_file() {
        return Err(format!("rollout not found: {log_path}"));
    }
    let project_root = state.project_root.lock().await.clone();
    let live_thread_id = current_live_thread_id(&state).await;
    let summary = match project_root.as_ref() {
        Some(root) => summarize_rollout(&path, root, live_thread_id.as_deref()),
        None => None,
    };
    let items = read_rollout_items(&path);
    Ok(ChatHistory {
        session: summary,
        items,
    })
}

/// Tear down the currently-live codex session so the next `start_turn`
/// constructs a fresh thread. The frontend treats the response as a
/// signal to clear its in-memory item list; the real `thread_id`
/// materializes when codex assigns one on the next `thread/start`.
#[tauri::command]
pub async fn start_new_chat_session(state: State<'_, AwidatState>) -> Result<ChatHistory, String> {
    if let Some(session) = state.codex.lock().await.take() {
        if let Err(e) = session.bridge.shutdown().await {
            tracing::warn!(error = %e, "codex bridge shutdown during start_new_chat_session");
        }
    }
    Ok(ChatHistory {
        session: None,
        items: Vec::new(),
    })
}

/// Tear down the current bridge and re-launch it bound to an existing
/// codex thread. Subsequent `start_turn` calls thread off the
/// rollout-backed history instead of opening a new conversation.
#[tauri::command]
pub async fn resume_chat_session(
    app: tauri::AppHandle,
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<ChatHistory, String> {
    if state.turn.lock().await.is_some() {
        return Err("cannot resume a chat while a turn is running; stop it first".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let path = PathBuf::from(&log_path);
    if !path.is_file() {
        return Err(format!("rollout not found: {log_path}"));
    }
    let first_line = read_first_line(&path).ok_or_else(|| "empty rollout".to_string())?;
    let meta = parse_session_meta(&first_line)
        .ok_or_else(|| "rollout missing session_meta header".to_string())?;
    if meta.cwd.as_path() != project_root.as_path() {
        return Err(format!(
            "rollout belongs to a different project ({} vs {})",
            meta.cwd.display(),
            project_root.display()
        ));
    }

    // Drop the current bridge so we can replace it with a resumed one.
    if let Some(session) = state.codex.lock().await.take() {
        if let Err(e) = session.bridge.shutdown().await {
            tracing::warn!(error = %e, "codex bridge shutdown during resume");
        }
    }

    let session = crate::codex_session::CodexSession::launch(
        app,
        project_root.clone(),
        Some(meta.thread_id.clone()),
    )
    .await
    .map_err(|e| format!("resume codex bridge: {e}"))?;
    *state.codex.lock().await = Some(session);

    let summary = summarize_rollout(&path, &project_root, Some(&meta.thread_id));
    let items = read_rollout_items(&path);
    Ok(ChatHistory {
        session: summary,
        items,
    })
}

/// Persist a user-supplied title for a chat session.
///
/// Stub: still requires a desktop-owned title sidecar; codex rollouts
/// are append-only and don't carry titles. Surfaces the gap clearly
/// instead of silently no-op'ing.
#[tauri::command]
pub async fn rename_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
    new_title: String,
) -> Result<(), String> {
    let _ = (state, log_path, new_title);
    Err("chat-history rename not yet supported (needs desktop title sidecar)".into())
}

/// Delete a chat session entirely.
///
/// Stub: deletion against codex's rollout store is destructive and
/// out of scope for this pass.
#[tauri::command]
pub async fn delete_chat_session(
    state: State<'_, AwidatState>,
    log_path: String,
) -> Result<(), String> {
    let _ = (state, log_path);
    Err("chat-history delete not yet supported".into())
}

// --- helpers ----------------------------------------------------------------

/// Header data we care about out of a rollout's first line.
struct RolloutMeta {
    thread_id: String,
    cwd: PathBuf,
}

fn codex_sessions_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

/// Recursively collect every `rollout-*.jsonl` under `dir`. Codex
/// stores them under `YYYY/MM/DD/` so the walk is shallow but we
/// don't hard-code the layout in case codex adds nesting.
fn walk_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rollouts(&path, out);
        } else if is_rollout_file(&path) {
            out.push(path);
        }
    }
}

fn is_rollout_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("rollout-"))
            .unwrap_or(false)
}

fn read_first_line(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let n = reader.read_line(&mut line).ok()?;
    if n == 0 {
        return None;
    }
    Some(line)
}

fn parse_session_meta(line: &str) -> Option<RolloutMeta> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = v.get("payload")?;
    let thread_id = payload.get("id")?.as_str()?.to_string();
    let cwd = PathBuf::from(payload.get("cwd")?.as_str()?);
    Some(RolloutMeta { thread_id, cwd })
}

/// Cheap mtime → ISO-8601 conversion. Falls back to the empty string
/// rather than failing — sorting just degrades.
fn mtime_iso(path: &Path) -> String {
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let dt: DateTime<Utc> = modified.into();
    dt.to_rfc3339()
}

/// Build a summary for a rollout, returning `None` if the file's
/// `session_meta.cwd` doesn't match `project_root`. Reads the file
/// twice (once for the header, once line-by-line for the title +
/// message count); rollouts are small enough that this is cheap.
fn summarize_rollout(
    path: &Path,
    project_root: &Path,
    live_thread_id: Option<&str>,
) -> Option<ChatSessionSummary> {
    let first_line = read_first_line(path)?;
    let meta = parse_session_meta(&first_line)?;
    if meta.cwd.as_path() != project_root {
        return None;
    }

    let (title, message_count) = title_and_count(path);
    let started_at = mtime_iso(path);
    let is_active = live_thread_id == Some(meta.thread_id.as_str());

    Some(ChatSessionSummary {
        id: meta.thread_id,
        title,
        project_root: project_root.display().to_string(),
        log_path: path.display().to_string(),
        started_at,
        message_count,
        is_active,
    })
}

/// Walk a rollout once, returning the first user-message text
/// (truncated) and the total count of `payload.type == "message"`
/// entries. Robust to malformed lines — they are skipped.
fn title_and_count(path: &Path) -> (String, usize) {
    use std::io::{BufRead, BufReader};
    let Ok(file) = fs::File::open(path) else {
        return ("(empty session)".to_string(), 0);
    };
    let reader = BufReader::new(file);
    let mut title: Option<String> = None;
    let mut count: usize = 0;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = v.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        count += 1;
        if title.is_none()
            && payload.get("role").and_then(|r| r.as_str()) == Some("user")
            && let Some(text) = extract_message_text(payload)
        {
            title = Some(truncate_title(&text));
        }
    }
    (
        title.unwrap_or_else(|| "(empty session)".to_string()),
        count,
    )
}

/// Concatenate all text-bearing content items in a message payload.
/// Codex emits `input_text` for user messages and `output_text` for
/// assistant; both carry a `text` field. Returns `None` if no text
/// content was found.
fn extract_message_text(payload: &serde_json::Value) -> Option<String> {
    let content = payload.get("content")?.as_array()?;
    let mut buf = String::new();
    for item in content {
        let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind != "input_text" && kind != "output_text" {
            continue;
        }
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(text);
        }
    }
    if buf.is_empty() { None } else { Some(buf) }
}

fn truncate_title(text: &str) -> String {
    let trimmed = text.trim();
    let one_line: String = trimmed.lines().next().unwrap_or("").to_string();
    if one_line.chars().count() <= 60 {
        one_line
    } else {
        let mut out: String = one_line.chars().take(57).collect();
        out.push_str("...");
        out
    }
}

/// Read a rollout end-to-end and map each `response_item` of kind
/// `message` to a desktop-protocol `Item`. User messages become
/// `Item::UserInput`; assistant messages become a Completed
/// `Item::Text`. Other rollout records (FunctionCall, Reasoning,
/// EventMsg, …) are skipped — they're orchestrator-internal and
/// the desktop renderer doesn't have a faithful representation.
fn read_rollout_items(path: &Path) -> Vec<Item> {
    use std::io::{BufRead, BufReader};
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut items = Vec::new();
    let mut counter: u64 = 0;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = v.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let Some(text) = extract_message_text(payload) else {
            continue;
        };
        counter += 1;
        match role {
            "user" => items.push(Item::UserInput {
                id: Id::new(format!("hist-user-{counter}")),
                text,
            }),
            "assistant" => items.push(Item::Text {
                id: Id::new(format!("hist-asst-{counter}")),
                phase: ItemLifecycle::Completed,
                text,
            }),
            _ => {}
        }
    }
    items
}

/// Look up the active codex bridge's thread id, if any.
async fn current_live_thread_id(state: &State<'_, AwidatState>) -> Option<String> {
    state
        .codex
        .lock()
        .await
        .as_ref()
        .map(|s| s.bridge.thread_id().to_string())
}
