//! Long-lived bridges that forward agent-loop requests
//! (`ApprovalRequest`, `UserInputRequest`) into the protocol stream
//! the frontend consumes.

use awidat_core::tool::{ApprovalRequest, UserInputRequest};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, ProposalSource};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::events::emit_item;
use crate::state::AwidatState;

/// Env-flag that opts the desktop into the approval-as-diff overlay
/// for `apply_edl`. When unset (the default during the Step-5 rollout),
/// every approval — including `apply_edl` — renders as the legacy
/// inline ApprovalCard. Once 5.6–5.9 land and the overlay is wired
/// end-to-end, commit 5.9 flips this default to on.
const PROPOSAL_OVERLAY_ENV: &str = "AWIDAT_DESKTOP_PROPOSAL_OVERLAY";

fn proposal_overlay_enabled() -> bool {
    matches!(
        std::env::var(PROPOSAL_OVERLAY_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
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
                if let Some(edl_text) = req
                    .args_full
                    .as_object()
                    .and_then(|m| m.get("edl"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                {
                    // Resolve the project root the proposal applies
                    // to — fail soft if no project is loaded
                    // (shouldn't happen in practice; the agent loop
                    // refuses to run without one).
                    if let Some(project_root) =
                        state.project_root.lock().await.clone()
                    {
                        if let Err(e) = crate::commands::proposal::build_proposal(
                            &app,
                            &state,
                            req.call_id.clone(),
                            edl_text,
                            &project_root,
                            ProposalSource::Agent {
                                tool_name: req.tool_name.clone(),
                            },
                            Some(req.reply),
                        )
                        .await
                        {
                            warn!(error = %e, call_id = %req.call_id, "build_proposal failed; falling back to ApprovalRequest");
                            // Defensive: if the proposal build failed
                            // (parse error, apply error, project read
                            // failure), the user still needs *some*
                            // approval surface. Re-emit the original
                            // approval card by reconstructing the
                            // request — but we already moved `reply`
                            // into build_proposal, so we can't here.
                            // Log and bail; the agent will see its
                            // turn time out / get cancelled.
                        }
                        continue;
                    } else {
                        warn!(
                            call_id = %req.call_id,
                            "apply_edl approved with no project_root; routing to legacy ApprovalCard",
                        );
                    }
                } else {
                    warn!(
                        call_id = %req.call_id,
                        "apply_edl args_full had no `edl` string; routing to legacy ApprovalCard",
                    );
                }
                // Fall through to the legacy path on any of the soft
                // failures above — the user still sees an approval
                // surface, even if it's the old card.
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
