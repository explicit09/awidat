//! Long-lived bridges that forward agent-loop requests
//! (`ApprovalRequest`, `UserInputRequest`) into the protocol stream
//! the frontend consumes.

use awidat_core::tool::{ApprovalRequest, UserInputRequest};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use tracing::debug;

use crate::events::emit_item;
use crate::state::AwidatState;

/// Forward `ApprovalRequest`s from the agent loop into protocol
/// `ApprovalRequest` Items, and stash the reply oneshot in
/// `state.pending_approvals` for `respond_approval` to consume.
pub fn spawn_approval_bridge(app: AppHandle, mut rx: mpsc::Receiver<ApprovalRequest>) {
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
            emit_item(&app, item);
        }
        debug!("approval bridge closed");
    });
}

/// Forward `UserInputRequest`s the same way. Records the oneshot in
/// `state.pending_inputs`; the matching `Item::AwaitingUserInput`
/// is emitted by the run-loop's event subscriber, not here.
pub fn spawn_user_input_bridge(app: AppHandle, mut rx: mpsc::Receiver<UserInputRequest>) {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let state = app.state::<AwidatState>();
            let call_id = req.call_id.clone();
            state.pending_inputs.lock().await.insert(call_id, req.reply);
        }
        debug!("user-input bridge closed");
    });
}
