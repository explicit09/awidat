//! Long-lived bridges that forward agent-loop requests
//! (`ApprovalRequest`, `UserInputRequest`) into the protocol stream
//! the frontend consumes.

use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, ProposalSource};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::events::emit_item;
use crate::state::AwidatState;

/// Env-flag for the approval-as-diff overlay on `apply_edl`.
/// **Default: ON.** Set `AWIDAT_DESKTOP_PROPOSAL_OVERLAY=0` to fall
/// back to the legacy inline ApprovalCard (kept around as the
/// escape hatch in case the overlay surfaces a regression on a
/// proposal shape we haven't tested yet — drag-handle math for
/// Insert / Untrim / etc).
const PROPOSAL_OVERLAY_ENV: &str = "AWIDAT_DESKTOP_PROPOSAL_OVERLAY";

fn proposal_overlay_enabled() -> bool {
    match std::env::var(PROPOSAL_OVERLAY_ENV).as_deref() {
        Ok("0") | Ok("false") | Ok("FALSE") => false,
        _ => true,
    }
}

/// Forward `ApprovalRequest`s from the agent loop into either the
/// legacy `Item::ApprovalRequest` card (current default) or the new
/// `Item::ProposedEdit` ghost overlay (`apply_edl` only, behind the
/// `AWIDAT_DESKTOP_PROPOSAL_OVERLAY` env flag).
///
/// Routing:
/// 1. `apply_edl` + flag on → `proposal::build_proposal`. The reply
///    oneshot is stashed inside `PendingProposal`; `accept_proposal`
///    sends `Deny` on accept (per the "user took over" semantics) or
///    `reject_proposal` sends `Deny` on rejection.
/// 2. Anything else → legacy `Item::ApprovalRequest` + entry in
///    `state.pending_approvals` for `respond_approval` to consume.
pub fn spawn_approval_bridge(app: AppHandle, mut rx: mpsc::Receiver<ApprovalRequest>) {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let state = app.state::<AwidatState>();

            if proposal_overlay_enabled() && req.tool_name == "apply_edl" {
                // Route to the proposal pipeline. Pull the EDL text out
                // of args_full — we added that field in commit 5.1
                // exactly so the bridge could re-parse the full EDL
                // without losing characters to args_summary truncation.
                let edl_text = req
                    .args_full
                    .as_object()
                    .and_then(|m| m.get("edl"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let project_root_opt = state.project_root.lock().await.clone();

                if let (Some(edl_text), Some(project_root)) = (edl_text, project_root_opt) {
                    // Decompose the request so we can hand `reply`
                    // to build_proposal but keep the rest available
                    // for an error-recovery path. If preview
                    // construction fails during parse/apply,
                    // build_proposal allows the underlying tool call
                    // to continue so the agent receives the same
                    // actionable apply_edl error it would see in the
                    // TUI. We also surface the preview failure to chat
                    // so the user knows why no overlay appeared.
                    let ApprovalRequest { call_id, reply, .. } = req;
                    if let Err(e) = crate::commands::proposal::build_proposal(
                        &app,
                        &state,
                        call_id.clone(),
                        edl_text,
                        &project_root,
                        ProposalSource::Agent {
                            tool_name: "apply_edl".into(),
                        },
                        Some(reply),
                    )
                    .await
                    {
                        warn!(error = %e, call_id = %call_id, "build_proposal failed");
                        // build_proposal failed before stashing the
                        // PendingProposal. For parse/apply failures,
                        // it allowed the original apply_edl call to
                        // proceed and fail in the tool handler, giving
                        // the agent a self-correction path. Surface
                        // the preview error too so the user knows why
                        // nothing showed up.
                        emit_item(
                            &app,
                            Item::Error {
                                id: Id::new(format!(
                                    "proposal-err-{}",
                                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                                )),
                                message: format!("couldn't build proposal preview: {e}",),
                            },
                        );
                    }
                    continue;
                }

                // Soft failures: missing edl arg, no project loaded.
                // Surface the failure to chat (otherwise the user
                // sees nothing happen — the legacy card would show
                // up but req.reply has not been moved yet so we
                // could fall through here. We Deny explicitly to
                // be safe.)
                warn!(
                    call_id = %req.call_id,
                    "apply_edl missing args.edl or project_root; denying without preview",
                );
                let _ = req.reply.send(ApprovalDecision::Deny);
                emit_item(
                    &app,
                    Item::Error {
                        id: Id::new(format!(
                            "proposal-err-{}",
                            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                        )),
                        message:
                            "apply_edl proposal could not be built — no project loaded or args.edl missing"
                                .into(),
                    },
                );
                continue;
            }

            // Legacy / non-apply_edl path: emit Item::ApprovalRequest,
            // stash reply oneshot for respond_approval.
            let call_id = req.call_id.clone();
            let item = Item::ApprovalRequest {
                id: Id::new(&req.call_id),
                phase: ItemLifecycle::Started,
                tool_name: req.tool_name.clone(),
                args_summary: req.args_summary.clone(),
            };
            state
                .pending_approvals
                .lock()
                .await
                .insert(call_id, req.reply);
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
