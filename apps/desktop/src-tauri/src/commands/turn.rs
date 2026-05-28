//! Turn lifecycle: `start_turn`, `cancel_turn`, `respond_approval`,
//! `respond_user_input`.
//!
//! The desktop now drives codex via an in-process app-server bridge
//! ([`awidat_codex_bridge::CodexAppServer`]). One bridge per open
//! project, lazily constructed on the first `start_turn` and rebuilt
//! when the project changes (see [`crate::commands::project`]).
//!
//! Approval round-trip is real now (no more "step 8b stub" errors):
//! the bridge raises `Item::ApprovalRequest` or `Item::AwaitingUserInput`
//! when codex needs a decision, the frontend renders the card, the
//! user clicks, and `respond_approval` / `respond_user_input` route
//! the answer back through the bridge's `pending` map.

use awidat_codex_bridge::ApprovalDecision;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::codex_session::CodexSession;
use crate::events::emit_item;
use crate::state::{AwidatState, TurnHandle, ViewState};
use awidat_desktop_protocol::{Id, Item};

/// Drive one turn end-to-end. Ensures a live `CodexSession` exists for
/// the open project (launching one on first call, tearing down + relaunching
/// on project switch), then asks the bridge to start a turn. The bridge's
/// background pump task emits `awidat://item-event` and `awidat://turn-end`
/// as codex makes progress.
#[tauri::command]
pub async fn start_turn(
    app: AppHandle,
    state: State<'_, AwidatState>,
    input: String,
    context: Option<Vec<String>>,
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

    // Echo the user message immediately. The bridge does NOT emit
    // UserInput items for us; codex sees the input as a turn prompt
    // and the frontend wants visual confirmation right away.
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

    let turn_context = context.unwrap_or_default();
    let model_input = format_model_input(
        &input,
        state.view_state.lock().await.as_ref(),
        &turn_context,
    );

    // Ensure-or-launch the bridge for this project_root. Project switch
    // is handled by `set_project_root` (it tears down the old session),
    // so by the time we get here the session is either absent or
    // already-launched-for-this-project.
    {
        let mut slot = state.codex.lock().await;
        let needs_launch = match slot.as_ref() {
            Some(s) if s.project_root == project_root => false,
            Some(_) => {
                // Defensive: if the session is for a different project,
                // tear it down here too. set_project_root should have
                // done this already; doing it again is safe.
                if let Some(old) = slot.take() {
                    if let Err(e) = old.bridge.shutdown().await {
                        tracing::warn!(error = %e, "shutting down stale codex session before relaunch");
                    }
                }
                true
            }
            None => true,
        };
        if needs_launch {
            let session = CodexSession::launch(app.clone(), project_root.clone(), None)
                .await
                .map_err(|e| format!("launch codex bridge: {e}"))?;
            *slot = Some(session);
        }
    }

    // Kick off the turn. The pump task in the bridge will emit items
    // and a turn-end signal as codex makes progress.
    let turn_id = {
        let slot = state.codex.lock().await;
        let session = slot
            .as_ref()
            .ok_or_else(|| "codex session vanished mid-launch".to_string())?;
        session
            .bridge
            .start_turn(model_input, None)
            .await
            .map_err(|e| format!("start_turn: {e}"))?
    };

    // Stash the turn id so cancel_turn can find it and so the UI can
    // correlate. cancel: CancellationToken is unused with the bridge
    // driver (we cancel via JSONRPC interrupt) but the field is still
    // on TurnHandle for now; pass a fresh token to keep the type the
    // same. Future cleanup can drop the field.
    *state.turn.lock().await = Some(TurnHandle {
        id: turn_id,
        cancel: CancellationToken::new(),
    });

    Ok(())
}

/// Cancel the in-flight turn. Asks the bridge to issue
/// `ClientRequest::TurnInterrupt`. The bridge's pump task will emit
/// `awidat://turn-end` once codex acknowledges. No-op if no turn is
/// running.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AwidatState>) -> Result<(), String> {
    let turn_id = match state.turn.lock().await.take() {
        Some(handle) => handle.id,
        None => return Ok(()),
    };
    if let Some(session) = state.codex.lock().await.as_ref() {
        if let Err(e) = session.bridge.interrupt(&turn_id).await {
            // Codex may have already finished the turn between when
            // we read state.turn and when we sent the interrupt; that's
            // benign — log and return Ok so the UI doesn't surface an
            // error for a race.
            tracing::warn!(error = %e, %turn_id, "interrupt returned error");
        }
    }
    Ok(())
}

/// Respond to a pending approval request raised by codex (e.g. before
/// executing `bash`, applying a file change, or granting a permission).
///
/// `call_id` is the `item_id` codex assigned to the in-flight tool call —
/// it lives on `Item::ApprovalRequest.id` and the bridge's pending map
/// keys on the same string.
///
/// `decision` is one of `"allow" | "allow_for_session" | "deny"` (matches
/// the React `ApprovalCard`'s strings).
#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AwidatState>,
    call_id: String,
    decision: String,
) -> Result<(), String> {
    let parsed = parse_approval_decision(&decision)?;
    let slot = state.codex.lock().await;
    let session = slot
        .as_ref()
        .ok_or_else(|| "no active codex session".to_string())?;
    session
        .bridge
        .respond_approval(&call_id, parsed)
        .await
        .map_err(|e| format!("respond_approval: {e}"))
}

/// Respond to a pending `request_user_input` tool call. v1 supports a
/// single free-text reply; the bridge wires it into codex's
/// `ToolRequestUserInputResponse` shape (first question, single answer).
#[tauri::command]
pub async fn respond_user_input(
    state: State<'_, AwidatState>,
    call_id: String,
    reply: String,
) -> Result<(), String> {
    let slot = state.codex.lock().await;
    let session = slot
        .as_ref()
        .ok_or_else(|| "no active codex session".to_string())?;
    session
        .bridge
        .respond_user_input(&call_id, reply)
        .await
        .map_err(|e| format!("respond_user_input: {e}"))
}

fn parse_approval_decision(s: &str) -> Result<ApprovalDecision, String> {
    match s {
        "allow" => Ok(ApprovalDecision::Allow),
        "allow_for_session" => Ok(ApprovalDecision::AllowForSession),
        "deny" => Ok(ApprovalDecision::Deny),
        other => Err(format!(
            "unknown approval decision \"{other}\" (expected allow | allow_for_session | deny)"
        )),
    }
}

fn format_model_input(input: &str, view: Option<&ViewState>, context: &[String]) -> String {
    let mut blocks = Vec::new();
    if let Some(v) = view {
        blocks.push(crate::commands::view::format_view_context(v));
    }

    let context_lines = context
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .take(12)
        .map(|line| format!("- {}", line.chars().take(240).collect::<String>()))
        .collect::<Vec<_>>();
    if !context_lines.is_empty() {
        blocks.push(format!("[visible context]\n{}", context_lines.join("\n")));
    }

    if blocks.is_empty() {
        input.to_string()
    } else {
        format!("{}\n\n{}", blocks.join("\n"), input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ViewState;

    #[test]
    fn format_turn_context_prefix_combines_view_state_and_visible_context() {
        let view = ViewState {
            stem: "intro".into(),
            current_time_s: 12.0,
            is_playing: false,
        };
        let context = vec![
            "project: Episode 12".to_string(),
            "media: Clip: intro".to_string(),
        ];

        assert_eq!(
            format_model_input("What is happening here?", Some(&view), &context),
            "[user is viewing intro at 0:12]\n[visible context]\n- project: Episode 12\n- media: Clip: intro\n\nWhat is happening here?"
        );
    }
}
