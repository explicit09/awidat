//! Awidat desktop app — Tauri backend.
//!
//! Imports `awidat-core` (agent loop) and `awidat-desktop-protocol`
//! (frontend wire types). Exposes Tauri commands that build a
//! `Session`, drive `run_turn`, translate `SessionEvent` into
//! protocol [`Item`]s, and route approval / user-input responses
//! back to the agent loop.
//!
//! # Architecture
//!
//! - [`AwidatState`] is the single piece of app-managed state Tauri
//!   threads through every command. It owns the lazy [`Session`], the
//!   active [`TurnHandle`] (so `cancel_turn` can find it), the project
//!   root, and the `pending_*` maps that match approval / input
//!   responses to their oneshots.
//! - [`start_turn`] spawns the run-loop on a tokio task, opens a
//!   broadcast subscription, maps each [`SessionEvent`] to a protocol
//!   [`Item`], and emits over `awidat://item`. The frontend listens
//!   and renders.
//! - On `TurnEnd` / `Error`, the backend emits `awidat://turn-end` so
//!   the frontend can flip its `running` flag.
//! - [`cancel_turn`], [`respond_approval`], [`respond_user_input`] are
//!   the user-driven inputs that loop back into the running session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, bash::BashTool, broll_candidates::BrollCandidatesTool,
    clip_search::ClipSearchTool, find_beat::FindBeatTool,
    find_eye_contact::FindEyeContactTool, find_moment::FindMomentTool,
    find_speaker_oncam::FindSpeakerOncamTool, inspect_clip::InspectClipTool,
    inspect_moment::InspectMomentTool, list_assets::ListAssetsTool,
    load_skill::LoadSkillTool, poll_render::PollRenderTool,
    read_index::ReadIndexTool, request_user_input::RequestUserInputTool,
    shot_summary::ShotSummaryTool, start_render::StartRenderTool,
    update_plan::UpdatePlanTool, view_episode::ViewEpisodeTool,
    view_frame::ViewFrameTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, SessionEvent, ToolRegistry};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, PlanStep};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

const ITEM_EVENT: &str = "awidat://item";
const TURN_END_EVENT: &str = "awidat://turn-end";
const TURN_ERROR_EVENT: &str = "awidat://turn-error";

const SYSTEM_PROMPT: &str = "\
You are awidat, a desktop agent for editing long-form spoken video. \
You operate inside a GUI: the user sees the chat, the timeline, and \
the video preview live. Be concise. Commit edits via apply_edl \
directly when you're confident.";

/// Tauri-managed app state. One instance per running app.
#[derive(Default)]
struct AwidatState {
    /// Lazily-built `Session`. Reset to `None` when the project root
    /// changes so the next turn rebuilds against the new root.
    session: Mutex<Option<Arc<Session>>>,
    /// Active turn, if any. Set by `start_turn`, cleared on
    /// TurnEnd / Error / cancel.
    turn: Mutex<Option<TurnHandle>>,
    /// Project root the next-built `Session` will use. Set by
    /// `set_project_root`. Defaulted from `AWIDAT_DESKTOP_PROJECT`
    /// env var on startup so dev runs work without configuring.
    project_root: Mutex<Option<PathBuf>>,
    /// Pending approval requests awaiting the user's decision, keyed
    /// by the `call_id` the frontend received in the matching
    /// `ApprovalRequest` Item.
    pending_approvals: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    /// Pending `request_user_input` calls keyed by `call_id`.
    pending_inputs: Mutex<HashMap<String, oneshot::Sender<String>>>,
}

/// Handle on a running turn. Owned by `AwidatState::turn`.
struct TurnHandle {
    cancel: CancellationToken,
}

/// Build the full editorial-tool registry. Mirrors the TUI's set; the
/// desktop is the buyer surface and gets every tool the agent has.
fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ListAssetsTool));
    registry.register(Arc::new(PollRenderTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(RequestUserInputTool));
    registry.register(Arc::new(StartRenderTool));
    registry.register(Arc::new(UpdatePlanTool));
    registry.register(Arc::new(FindBeatTool));
    registry.register(Arc::new(InspectMomentTool));
    registry.register(Arc::new(ViewEpisodeTool));
    registry.register(Arc::new(ViewFrameTool));
    registry.register(Arc::new(ViewTimelineTool));
    registry.register(Arc::new(BrollCandidatesTool));
    registry.register(Arc::new(ClipSearchTool));
    registry.register(Arc::new(FindEyeContactTool));
    registry.register(Arc::new(FindSpeakerOncamTool));
    registry.register(Arc::new(ShotSummaryTool));
    registry.register(Arc::new(LoadSkillTool));
    registry
}

/// Build a fresh `Session` rooted at `project_root`.
async fn build_session(
    project_root: PathBuf,
    approval_tx: mpsc::Sender<ApprovalRequest>,
    user_input_tx: mpsc::Sender<UserInputRequest>,
) -> Result<Arc<Session>, String> {
    let client = Client::from_env_or_keychain(ClientConfig::default())
        .map_err(|e| format!("Anthropic client: {e}. Set ANTHROPIC_API_KEY or store in OS keychain (service 'awidat', account 'anthropic_api_key')."))?;

    let session = Session::new(
        client,
        build_registry(),
        models::SONNET.to_string(),
        Some(SYSTEM_PROMPT.into()),
        project_root,
    )
    .with_approval_channel(approval_tx)
    .with_user_input_channel(user_input_tx);

    Ok(Arc::new(session))
}

/// Wraps a protocol [`Item`] for transport over Tauri's event bus.
#[derive(Debug, Clone, Serialize)]
struct ItemEvent {
    item: Item,
}

/// Payload emitted to `awidat://turn-end` when the run-loop returns.
#[derive(Debug, Clone, Serialize)]
struct TurnEndEvent {
    /// `Ok` on clean end, `Err(msg)` on session-level error.
    error: Option<String>,
}

/// Set / change the project root for subsequent turns. Resets any
/// existing `Session` so the next `start_turn` rebuilds against the
/// new root.
#[tauri::command]
async fn set_project_root(
    state: State<'_, AwidatState>,
    path: String,
) -> Result<(), String> {
    let buf = PathBuf::from(&path);
    if !buf.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    *state.project_root.lock().await = Some(buf);
    *state.session.lock().await = None;
    Ok(())
}

/// Read the currently-configured project root, if any.
#[tauri::command]
async fn current_project_root(
    state: State<'_, AwidatState>,
) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .await
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned()))
}

/// Drive one turn end-to-end. Builds the `Session` if needed,
/// subscribes to its broadcast, maps events to protocol [`Item`]s,
/// and emits them. Returns immediately — the actual work runs on
/// background tasks.
#[tauri::command]
async fn start_turn(
    app: AppHandle,
    state: State<'_, AwidatState>,
    input: String,
) -> Result<(), String> {
    if input.trim().is_empty() {
        return Err("empty input".into());
    }
    // Refuse if a turn is already running. The frontend should
    // disable the Send button while running, but defend anyway.
    if state.turn.lock().await.is_some() {
        return Err("a turn is already running — cancel it first".into());
    }

    // Resolve project root. We don't auto-default to the cwd —
    // the user must explicitly set one (Step 2 will add a picker;
    // for now an env var or the `set_project_root` command works).
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded — call set_project_root first".to_string())?;

    // Build (or reuse) the Session.
    let session = {
        let mut slot = state.session.lock().await;
        match slot.as_ref() {
            Some(s) => s.clone(),
            None => {
                let (approval_tx, approval_rx) = mpsc::channel::<ApprovalRequest>(16);
                let (input_tx, input_rx) = mpsc::channel::<UserInputRequest>(16);

                let session = build_session(project_root, approval_tx, input_tx).await?;
                *slot = Some(session.clone());

                // Spawn long-lived bridges that forward approval /
                // user-input requests from the agent loop into
                // protocol Items. They live as long as the Session.
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

    // Echo the user's input as a UserInput Item so the chat shows
    // both halves of the conversation. Emitted before subscribing
    // to the broadcast — the agent's response can't beat this.
    let user_item_id = format!(
        "user-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    if let Err(e) = app.emit(
        ITEM_EVENT,
        ItemEvent {
            item: Item::UserInput {
                id: Id::new(&user_item_id),
                text: input.clone(),
            },
        },
    ) {
        warn!(error = %e, "emit user-input item failed");
    }

    // Subscribe to the broadcast BEFORE spawning the turn so we
    // don't miss the early TurnStart event.
    let mut events = session.subscribe();

    // Drive the turn.
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

    // Drain events on a separate task. We don't block the command
    // on this — `start_turn` returns as soon as the turn is queued.
    let app_for_events = app.clone();
    let state_for_events = app.state::<AwidatState>().inner();
    // We can't move `State<'_, T>` into a 'static future directly;
    // instead, copy the AppHandle and re-fetch the state from inside
    // the task (Tauri's State is itself a handle, cheap to clone).
    let _ = state_for_events; // suppress unused — explicit re-fetch below
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
                                if let Err(e) = app_for_events.emit(ITEM_EVENT, ItemEvent { item }) {
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

        // Clear the active turn slot so the next `start_turn` is allowed.
        let state = app_for_events.state::<AwidatState>();
        *state.turn.lock().await = None;
    });

    Ok(())
}

/// Cancel the in-flight turn. No-op if no turn is running.
#[tauri::command]
async fn cancel_turn(state: State<'_, AwidatState>) -> Result<(), String> {
    if let Some(handle) = state.turn.lock().await.as_ref() {
        handle.cancel.cancel();
    }
    Ok(())
}

/// Respond to a pending approval request. `decision` is one of
/// `"allow"`, `"allow_for_session"`, `"deny"`. Unknown values are
/// treated as `"deny"` (safer default).
#[tauri::command]
async fn respond_approval(
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
    // If send fails the loop is already gone — nothing to do.
    let _ = tx.send(dec);
    Ok(())
}

/// Respond to a pending `request_user_input` tool call.
#[tauri::command]
async fn respond_user_input(
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
        Id::new(format!("text-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)))
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
///
/// Tool-call lifecycle: ToolCallStart → Started, ToolCallArgs → Delta,
/// ToolResult → Completed. Args carried through.
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
            // Args are no longer in scope here; the frontend already
            // received the latest Delta. Empty {} on the Completed
            // emission is fine — store has the prior args.
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
        SessionEvent::TurnEnd => {
            // Close any dangling text item if SamplingComplete didn't.
            streamer.close()
        }
        SessionEvent::Error(msg) => {
            let mut out = streamer.close();
            out.push(Item::Error {
                id: Id::new(format!("err-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0))),
                message: msg.clone(),
            });
            out
        }
    }
}

/// Forward `ApprovalRequest`s from the agent loop into protocol
/// `ApprovalRequest` Items, and stash the reply oneshot in
/// `state.pending_approvals` for `respond_approval` to consume.
fn spawn_approval_bridge(app: AppHandle, mut rx: mpsc::Receiver<ApprovalRequest>) {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let state = app.state::<AwidatState>();
            let call_id = req.call_id.clone();
            let item = Item::ApprovalRequest {
                id: Id::new(&req.call_id),
                phase: ItemLifecycle::Started,
                tool_name: req.tool_name.clone(),
                args_summary: req.args_summary.clone(),
            };
            state.pending_approvals.lock().await.insert(call_id, req.reply);
            if let Err(e) = app.emit(ITEM_EVENT, ItemEvent { item }) {
                warn!(error = %e, "emit approval item failed");
            }
        }
        debug!("approval bridge closed");
    });
}

/// Forward `UserInputRequest`s the same way.
fn spawn_user_input_bridge(app: AppHandle, mut rx: mpsc::Receiver<UserInputRequest>) {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let state = app.state::<AwidatState>();
            let call_id = req.call_id.clone();
            // `request_user_input` already produces a SessionEvent
            // (`AwaitingUserInput`) that the run-loop subscriber maps
            // to a protocol Item — we don't double-emit. We just
            // record the oneshot here so `respond_user_input` can
            // find it.
            state.pending_inputs.lock().await.insert(call_id, req.reply);
        }
        debug!("user-input bridge closed");
    });
}

/// Surface a hard backend error to the frontend (e.g. session build
/// failed). Currently only used for symmetry with `TURN_END_EVENT`.
#[allow(dead_code)]
fn emit_turn_error(app: &AppHandle, msg: impl Into<String>) {
    if let Err(e) = app.emit(TURN_ERROR_EVENT, msg.into()) {
        error!(error = %e, "emit turn-error failed");
    }
}

/// Tauri entrypoint. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AwidatState::default();

    // Pre-populate project_root from env if set, for dev convenience.
    if let Ok(p) = std::env::var("AWIDAT_DESKTOP_PROJECT") {
        let buf = PathBuf::from(&p);
        if buf.is_dir() {
            // We can't await a Mutex outside a runtime; use blocking.
            *state.project_root.blocking_lock() = Some(buf);
        } else {
            warn!(path = %p, "AWIDAT_DESKTOP_PROJECT is not a directory; ignoring");
        }
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            start_turn,
            cancel_turn,
            respond_approval,
            respond_user_input,
            set_project_root,
            current_project_root,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        error!(error = %err, "tauri bootstrap failed");
        std::process::exit(1);
    }
}
