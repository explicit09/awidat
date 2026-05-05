//! Turn lifecycle: `start_turn`, `cancel_turn`,
//! `respond_approval`, `respond_user_input`.

use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_core::SessionEvent;
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, PlanStep};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::bridges::{spawn_approval_bridge, spawn_user_input_bridge};
use crate::events::{ItemEvent, TURN_END_EVENT, TurnEndEvent, emit_item};
use crate::session::build_session;
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
        let mut slot = state.session.lock().await;
        match slot.as_ref() {
            Some(s) => s.clone(),
            None => {
                let (approval_tx, approval_rx) = mpsc::channel::<ApprovalRequest>(16);
                let (input_tx, input_rx) = mpsc::channel::<UserInputRequest>(16);

                let session = build_session(project_root, approval_tx, input_tx).await?;
                *slot = Some(session.clone());

                spawn_approval_bridge(app.clone(), approval_rx);
                spawn_user_input_bridge(app.clone(), input_rx);

                session
            }
        }
    };

    let cancel = CancellationToken::new();
    *state.turn.lock().await = Some(TurnHandle {
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
            text: input.clone(),
        },
    );

    let mut events = session.subscribe();

    let session_for_turn = session.clone();
    let cancel_for_turn = cancel.clone();
    let app_for_turn = app.clone();
    tokio::spawn(async move {
        let result = session_for_turn.run_turn(input, cancel_for_turn).await;
        let payload = TurnEndEvent {
            error: match result {
                Ok(()) => None,
                Err(e) => Some(e.to_string()),
            },
        };
        if let Err(e) = app_for_turn.emit(TURN_END_EVENT, payload) {
            warn!(error = %e, "failed to emit turn-end");
        }
    });

    let app_for_events = app.clone();
    let cancel_for_events = cancel.clone();
    tokio::spawn(async move {
        let mut text_streamer = TextStreamer::default();
        loop {
            tokio::select! {
                _ = cancel_for_events.cancelled() => {
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

        let state = app_for_events.state::<AwidatState>();
        *state.turn.lock().await = None;
    });

    Ok(())
}

/// Cancel the in-flight turn. No-op if no turn is running.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AwidatState>) -> Result<(), String> {
    if let Some(handle) = state.turn.lock().await.as_ref() {
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
}

/// Map one [`SessionEvent`] into zero or more protocol [`Item`]s.
fn map_event(ev: &SessionEvent, streamer: &mut TextStreamer) -> Vec<Item> {
    match ev {
        SessionEvent::TurnStart | SessionEvent::MessageStart { .. } => vec![],
        SessionEvent::TextDelta(t) => streamer.on_delta(t),
        SessionEvent::SamplingComplete { .. } => streamer.close(),
        SessionEvent::ToolCallStart { id, name } => vec![Item::ToolCall {
            id: Id::new(id),
            phase: ItemLifecycle::Started,
            name: name.clone(),
            args: serde_json::json!({}),
            result: None,
        }],
        SessionEvent::ToolCallArgs { id, name, args } => vec![Item::ToolCall {
            id: Id::new(id),
            phase: ItemLifecycle::Delta,
            name: name.clone(),
            args: args.clone(),
            result: None,
        }],
        SessionEvent::ToolResult { id, name, result } => vec![Item::ToolCall {
            id: Id::new(id),
            phase: ItemLifecycle::Completed,
            name: name.clone(),
            args: serde_json::json!({}),
            result: Some(match result {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(e.clone()),
            }),
        }],
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
        SessionEvent::TurnEnd => streamer.close(),
        SessionEvent::Error(msg) => {
            let mut out = streamer.close();
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
