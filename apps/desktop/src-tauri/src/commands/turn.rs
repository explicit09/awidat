//! Turn lifecycle: `start_turn`, `cancel_turn`,
//! `respond_approval`, `respond_user_input`.
//!
//! Step 8: this module now drives the codex engine via a subprocess
//! (`codex_runner::spawn_codex_turn`). The legacy `awidat-core`
//! Session loop is no longer exercised from the desktop. The
//! `respond_approval` / `respond_user_input` commands remain as
//! `#[tauri::command]` shims so the React frontend's IPC handles
//! resolve, but they return an error directing the user to the TUI
//! until step 8b wires codex's approval channel back into the desktop.

use awidat_desktop_protocol::{Id, Item};
use tauri::{AppHandle, State};

use crate::codex_runner::spawn_codex_turn;
use crate::events::emit_item;
use crate::state::{AwidatState, TurnHandle};

/// Drive one turn end-to-end. Spawns a `codex-exec --json` child
/// process; the reader task in [`crate::codex_runner`] translates the
/// JSONL stream into protocol [`Item`]s and emits them via the Tauri
/// event bus. Returns immediately — work runs in background tasks.
#[tauri::command]
pub async fn start_turn(
    app: AppHandle,
    state: State<'_, AwidatState>,
    input: String,
) -> Result<(), String> {
    if input.trim().is_empty() {
        return Err("empty input".into());
    }
    if state.turn.lock().await.is_some() {
        return Err("a turn is already running — cancel it first".into());
    }

    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded — open or create one first".to_string())?;

    let turn_id = format!(
        "turn-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    let user_item_id = format!(
        "user-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    emit_item(
        &app,
        Item::UserInput {
            id: Id::new(&user_item_id),
            text: input.clone(),
        },
    );

    // Prefix view-state context onto the input the model actually
    // sees. If the user is scrubbed to 0:23 and asks "what's
    // happening here?", the agent now has the answer to "here."
    // No-op when no media is loaded.
    let model_input = match state.view_state.lock().await.as_ref() {
        Some(v) => format!(
            "{}\n\n{}",
            crate::commands::view::format_view_context(v),
            input
        ),
        None => input.clone(),
    };

    let handle = spawn_codex_turn(app.clone(), project_root, model_input, None)?;
    *state.turn.lock().await = Some(TurnHandle {
        id: turn_id,
        cancel: handle.cancel,
    });

    Ok(())
}

/// Cancel the in-flight turn. No-op if no turn is running. The
/// reader task watches the cancellation token and sends SIGKILL to
/// the codex child before emitting `awidat://turn-end`.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AwidatState>) -> Result<(), String> {
    if let Some(handle) = state.turn.lock().await.take() {
        handle.cancel.cancel();
    }
    Ok(())
}

/// Respond to a pending approval request.
///
/// Step 8 stub: codex's approval channel hasn't been bridged into the
/// Tauri event bus yet, so the desktop never raises an
/// `Item::ApprovalRequest` for the user to respond to. This command
/// returns an error so the frontend can surface a sensible message
/// instead of silently dropping the call. Real wiring lands in step
/// 8b.
#[tauri::command]
pub async fn respond_approval(
    _state: State<'_, AwidatState>,
    _call_id: String,
    _decision: String,
) -> Result<(), String> {
    Err("approvals not yet wired in step 8b; \
         run `awidat tui` from terminal for interactive approval flow"
        .into())
}

/// Respond to a pending `request_user_input` tool call.
///
/// Step 8 stub: same rationale as [`respond_approval`].
#[tauri::command]
pub async fn respond_user_input(
    _state: State<'_, AwidatState>,
    _call_id: String,
    _reply: String,
) -> Result<(), String> {
    Err("user-input requests not yet wired in step 8b; \
         run `awidat tui` from terminal for interactive flow"
        .into())
}

