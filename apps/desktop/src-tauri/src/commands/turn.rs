//! Turn lifecycle: `start_turn`, `cancel_turn`,
//! `respond_approval`, `respond_user_input`.

use std::collections::HashMap;

use awidat_core::SessionEvent;
use awidat_core::podcast_workflow::{
    format_intake_report_for_agent, podcast_production_intake_plan,
    should_run_podcast_production_intake,
};
use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, PlanStep};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::bridges::{spawn_approval_bridge, spawn_user_input_bridge};
use crate::events::{ItemEvent, TURN_END_EVENT, TurnEndEvent, emit_item};
use crate::session::{build_session, resume_session};
use crate::state::{AwidatState, TurnHandle};

/// Drive one turn end-to-end. Builds the `Session` if needed,
/// subscribes to its broadcast, maps events to protocol [`Item`]s,
/// and emits them. Returns immediately — the actual work runs on
/// background tasks.
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

    let session = {
        let mut slot = state.active.lock().await;
        match slot.session.as_ref() {
            Some(s) => s.clone(),
            None => {
                let (approval_tx, approval_rx) = mpsc::channel::<ApprovalRequest>(16);
                let (input_tx, input_rx) = mpsc::channel::<UserInputRequest>(16);

                let resume_log_path = slot.resume_log_path.clone();
                let session = match resume_log_path {
                    Some(path) if path.is_file() => {
                        resume_session(path, approval_tx, input_tx).await?
                    }
                    _ => build_session(project_root, approval_tx, input_tx).await?,
                };
                slot.session = Some(session.clone());

                spawn_approval_bridge(app.clone(), approval_rx);
                spawn_user_input_bridge(app.clone(), input_rx);

                session
            }
        }
    };

    let turn_id = format!(
        "turn-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let cancel = CancellationToken::new();
    *state.turn.lock().await = Some(TurnHandle {
        id: turn_id.clone(),
        cancel: cancel.clone(),
    });

    let user_item_id = format!(
        "user-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    emit_item(
        &app,
        Item::UserInput {
            id: Id::new(&user_item_id),
            // The chat shows the user's plain text — the view-state
            // prefix is metadata, not something the user typed.
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

    let mut events = session.subscribe();

    let run_podcast_intake = should_run_podcast_production_intake(&input);
    let session_for_turn = session.clone();
    let cancel_for_turn = cancel.clone();
    let app_for_turn = app.clone();
    let turn_id_for_turn = turn_id.clone();
    tokio::spawn(async move {
        let result = if run_podcast_intake {
            let mut plan = podcast_production_intake_plan();
            match session_for_turn
                .execute_structured_plan(&mut plan, cancel_for_turn.clone())
                .await
            {
                Ok(report) => {
                    let intake_context = format_intake_report_for_agent(&report);
                    session_for_turn
                        .run_turn(
                            format!("{intake_context}\n\nUser request:\n{model_input}"),
                            cancel_for_turn,
                        )
                        .await
                }
                Err(error) => Err(awidat_core::SessionError::Fatal(format!(
                    "podcast production intake failed: {error}"
                ))),
            }
        } else {
            session_for_turn
                .run_turn(model_input, cancel_for_turn)
                .await
        };
        let payload = TurnEndEvent {
            error: match result {
                Ok(()) => None,
                Err(e) => Some(e.to_string()),
            },
        };
        if let Err(e) = app_for_turn.emit(TURN_END_EVENT, payload) {
            warn!(error = %e, "failed to emit turn-end");
        }
        clear_turn_if_current(&app_for_turn, &turn_id_for_turn).await;
    });

    let app_for_events = app.clone();
    let cancel_for_events = cancel.clone();
    let turn_id_for_events = turn_id;
    tokio::spawn(async move {
        let mut text_streamer = TextStreamer::default();
        loop {
            tokio::select! {
                _ = cancel_for_events.cancelled() => {
                    for item in map_event(
                        &SessionEvent::Error("cancelled".into()),
                        &mut text_streamer,
                    ) {
                        if let Err(e) = app_for_events.emit(crate::events::ITEM_EVENT, ItemEvent { item }) {
                            warn!(error = %e, "emit item failed");
                        }
                    }
                    break;
                }
                ev = events.recv() => {
                    match ev {
                        Ok(ev) => {
                            for item in map_event(&ev, &mut text_streamer) {
                                if let Err(e) = app_for_events.emit(crate::events::ITEM_EVENT, ItemEvent { item }) {
                                    warn!(error = %e, "emit item failed");
                                }
                            }
                            if matches!(ev, SessionEvent::TurnEnd | SessionEvent::Error(_)) {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "broadcast lagged — events dropped");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }

        clear_turn_if_current(&app_for_events, &turn_id_for_events).await;
    });

    Ok(())
}

async fn clear_turn_if_current(app: &AppHandle, turn_id: &str) {
    let state = app.state::<AwidatState>();
    let mut active = state.turn.lock().await;
    if active.as_ref().is_some_and(|handle| handle.id == turn_id) {
        *active = None;
    }
}

/// Cancel the in-flight turn. No-op if no turn is running.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AwidatState>) -> Result<(), String> {
    if let Some(handle) = state.turn.lock().await.take() {
        handle.cancel.cancel();
    }
    Ok(())
}

/// Respond to a pending approval request. `decision` is one of
/// `"allow"`, `"allow_for_session"`, `"deny"`. Unknown values are
/// treated as `"deny"` (safer default).
#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AwidatState>,
    call_id: String,
    decision: String,
) -> Result<(), String> {
    let dec = match decision.as_str() {
        "allow" => ApprovalDecision::Allow,
        "allow_for_session" => ApprovalDecision::AllowForSession,
        _ => ApprovalDecision::Deny,
    };
    let tx = state
        .pending_approvals
        .lock()
        .await
        .remove(&call_id)
        .ok_or_else(|| format!("no pending approval for {call_id}"))?;
    let _ = tx.send(dec);
    Ok(())
}

/// Respond to a pending `request_user_input` tool call.
#[tauri::command]
pub async fn respond_user_input(
    state: State<'_, AwidatState>,
    call_id: String,
    reply: String,
) -> Result<(), String> {
    let tx = state
        .pending_inputs
        .lock()
        .await
        .remove(&call_id)
        .ok_or_else(|| format!("no pending input for {call_id}"))?;
    let _ = tx.send(reply);
    Ok(())
}

/// Per-message text-streaming state. `MessageStart` opens a new text
/// item; `TextDelta` accumulates into it; `SamplingComplete` closes it.
/// We open the text item on the first `TextDelta` (not on
/// `MessageStart`) because some messages have no text at all (pure
/// tool calls) and we don't want empty cards in those cases.
#[derive(Default)]
struct TextStreamer {
    /// Active text item id + accumulated text. None when no text item
    /// is currently open.
    active: Option<(Id, String)>,
    /// Latest tool args by call id. ToolResult events do not carry
    /// args, but the frontend's upsert replaces by id, so Completed
    /// must replay the latest args instead of `{}`.
    tool_args: HashMap<String, serde_json::Value>,
    /// Tool calls that started but have not emitted a ToolResult yet.
    pending_tools: HashMap<String, String>,
}

impl TextStreamer {
    fn next_id(&self) -> Id {
        Id::new(format!(
            "text-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ))
    }

    /// Emit a Started item for a fresh text run, or a Delta if one is
    /// already open. Returns the items to emit.
    fn on_delta(&mut self, delta: &str) -> Vec<Item> {
        match self.active.as_mut() {
            None => {
                let id = self.next_id();
                self.active = Some((id.clone(), delta.to_string()));
                vec![
                    Item::Text {
                        id: id.clone(),
                        phase: ItemLifecycle::Started,
                        text: String::new(),
                    },
                    Item::Text {
                        id,
                        phase: ItemLifecycle::Delta,
                        text: delta.to_string(),
                    },
                ]
            }
            Some((id, acc)) => {
                acc.push_str(delta);
                vec![Item::Text {
                    id: id.clone(),
                    phase: ItemLifecycle::Delta,
                    text: acc.clone(),
                }]
            }
        }
    }

    /// Close any open text item with a Completed lifecycle event.
    fn close(&mut self) -> Vec<Item> {
        match self.active.take() {
            Some((id, acc)) => vec![Item::Text {
                id,
                phase: ItemLifecycle::Completed,
                text: acc,
            }],
            None => vec![],
        }
    }

    fn start_tool(&mut self, id: &str, name: &str) {
        self.tool_args.insert(id.to_string(), serde_json::json!({}));
        self.pending_tools.insert(id.to_string(), name.to_string());
    }

    fn update_tool_args(&mut self, id: &str, args: serde_json::Value) {
        self.tool_args.insert(id.to_string(), args);
    }

    fn take_tool_args(&mut self, id: &str) -> serde_json::Value {
        self.tool_args
            .remove(id)
            .unwrap_or_else(|| serde_json::json!({}))
    }

    fn complete_tool(&mut self, id: &str) {
        self.pending_tools.remove(id);
    }

    fn close_pending_tools(&mut self, message: &str) -> Vec<Item> {
        let pending = std::mem::take(&mut self.pending_tools);
        pending
            .into_iter()
            .map(|(id, name)| Item::ToolCall {
                args: self.take_tool_args(&id),
                id: Id::new(id),
                name,
                phase: ItemLifecycle::Completed,
                result: Some(Err(message.to_string())),
            })
            .collect()
    }
}

/// Map one [`SessionEvent`] into zero or more protocol [`Item`]s.
fn map_event(ev: &SessionEvent, streamer: &mut TextStreamer) -> Vec<Item> {
    match ev {
        SessionEvent::TurnStart | SessionEvent::MessageStart { .. } => vec![],
        SessionEvent::TextDelta(t) => streamer.on_delta(t),
        SessionEvent::SamplingComplete { .. } => streamer.close(),
        SessionEvent::ToolCallStart { id, name } => {
            streamer.start_tool(id, name);
            vec![Item::ToolCall {
                id: Id::new(id),
                phase: ItemLifecycle::Started,
                name: name.clone(),
                args: serde_json::json!({}),
                result: None,
            }]
        }
        SessionEvent::ToolCallArgs { id, name, args } => {
            streamer.update_tool_args(id, args.clone());
            vec![Item::ToolCall {
                id: Id::new(id),
                phase: ItemLifecycle::Delta,
                name: name.clone(),
                args: args.clone(),
                result: None,
            }]
        }
        SessionEvent::ToolResult { id, name, result } => {
            let args = streamer.take_tool_args(id);
            streamer.complete_tool(id);
            vec![Item::ToolCall {
                id: Id::new(id),
                phase: ItemLifecycle::Completed,
                name: name.clone(),
                args,
                result: Some(match result {
                    Ok(s) => Ok(s.clone()),
                    Err(e) => Err(e.clone()),
                }),
            }]
        }
        SessionEvent::EditPlanUpdate { items, note } => vec![Item::Plan {
            id: Id::new("plan"),
            phase: ItemLifecycle::Completed,
            items: items
                .iter()
                .map(|p| PlanStep {
                    step: p.step.clone(),
                    status: p.status.clone(),
                })
                .collect(),
            note: note.clone(),
        }],
        SessionEvent::AwaitingUserInput {
            call_id,
            question,
            options,
        } => vec![Item::AwaitingUserInput {
            id: Id::new(call_id),
            phase: ItemLifecycle::Started,
            question: question.clone(),
            options: options.clone(),
        }],
        SessionEvent::TurnEnd => {
            let mut out = streamer.close();
            out.extend(streamer.close_pending_tools("turn ended before tool completed"));
            out
        }
        SessionEvent::Error(msg) => {
            let mut out = streamer.close();
            out.extend(streamer.close_pending_tools(msg));
            out.push(Item::Error {
                id: Id::new(format!(
                    "err-{}",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                )),
                message: msg.clone(),
            });
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_preserves_latest_args_for_upsert() {
        let mut streamer = TextStreamer::default();
        let start = SessionEvent::ToolCallStart {
            id: "call-1".into(),
            name: "apply_edl".into(),
        };
        let args = SessionEvent::ToolCallArgs {
            id: "call-1".into(),
            name: "apply_edl".into(),
            args: serde_json::json!({"edl": "*** Begin EDL\n*** End EDL\n"}),
        };
        let result = SessionEvent::ToolResult {
            id: "call-1".into(),
            name: "apply_edl".into(),
            result: Ok("done".into()),
        };

        let _ = map_event(&start, &mut streamer);
        let _ = map_event(&args, &mut streamer);
        let completed = map_event(&result, &mut streamer);

        let [Item::ToolCall { args, .. }] = completed.as_slice() else {
            panic!("expected one completed tool call");
        };
        assert_eq!(args["edl"], "*** Begin EDL\n*** End EDL\n");
    }

    #[test]
    fn error_completes_pending_tool_call() {
        let mut streamer = TextStreamer::default();
        let start = SessionEvent::ToolCallStart {
            id: "call-1".into(),
            name: "find_dead_air".into(),
        };
        let error = SessionEvent::Error("idle timeout after 90s".into());

        let _ = map_event(&start, &mut streamer);
        let completed = map_event(&error, &mut streamer);

        assert!(
            completed.iter().any(|item| matches!(
                item,
                Item::ToolCall {
                    phase: ItemLifecycle::Completed,
                    name,
                    result: Some(Err(_)),
                    ..
                } if name == "find_dead_air"
            )),
            "expected pending tool call to complete on stream error, got {completed:?}"
        );
    }
}
