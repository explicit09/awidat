//! Long-lived bridges that forward agent-loop requests
//! (`ApprovalRequest`, `UserInputRequest`) into the protocol stream
//! the frontend consumes.

use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_desktop_protocol::{Id, Item, ItemLifecycle, PermissionMode, ProposalSource};
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
            let mode = current_permission_mode(&state).await;

            if auto_allows_tool(mode, &req.tool_name) {
                debug!(
                    call_id = %req.call_id,
                    tool = %req.tool_name,
                    mode = ?mode,
                    "auto-allowing approval request for permission mode",
                );
                let _ = req.reply.send(ApprovalDecision::Allow);
                continue;
            }

            if proposal_overlay_enabled() && req.tool_name == "apply_edl" {
                // Read the full EDL from args_full so the bridge can
                // re-parse without args_summary's truncation.
                let edl_text = req
                    .args_full
                    .as_object()
                    .and_then(|m| m.get("edl"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let project_root_opt = state.project_root.lock().await.clone();

                if let (Some(edl_text), Some(project_root)) = (edl_text, project_root_opt) {
                    // Hand `reply` to build_proposal but keep the rest
                    // around for the error-recovery path. On parse/apply
                    // failure, build_proposal lets the underlying
                    // apply_edl tool call proceed so the agent gets a
                    // self-correction signal; we surface the preview
                    // failure to chat so the user knows why no overlay
                    // appeared.
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
                        // No PendingProposal was stashed; surface the
                        // preview error in chat so the user sees why
                        // nothing rendered.
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

                // Soft failures (missing edl arg, no project loaded).
                // Deny explicitly and surface the failure in chat —
                // otherwise nothing would appear.
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

            // Legacy / non-apply_edl path: emit Item::ApprovalRequest
            // and stash the reply oneshot for respond_approval.
            let call_id = req.call_id.clone();
            let item = Item::ApprovalRequest {
                id: Id::new(&req.call_id),
                phase: ItemLifecycle::Started,
                tool_name: req.tool_name.clone(),
                args_summary: req
                    .retry_reason
                    .as_ref()
                    .map(|reason| format!("{reason}\n{}", req.args_summary))
                    .unwrap_or_else(|| req.args_summary.clone()),
                capability_metadata: serde_json::to_value(&req.capability_metadata)
                    .unwrap_or(serde_json::Value::Null),
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

async fn current_permission_mode(state: &AwidatState) -> PermissionMode {
    let Some(project_root) = state.project_root.lock().await.clone() else {
        return PermissionMode::Manual;
    };
    crate::commands::permission::read_mode_pub(&project_root)
}

fn auto_allows_tool(mode: PermissionMode, _tool_name: &str) -> bool {
    match mode {
        PermissionMode::Manual | PermissionMode::Copilot => false,
        PermissionMode::Autopilot => true,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autopilot_auto_allows_agent_tools() {
        assert!(auto_allows_tool(PermissionMode::Autopilot, "apply_edl"));
        assert!(auto_allows_tool(
            PermissionMode::Autopilot,
            "start_indexing"
        ));
        assert!(auto_allows_tool(PermissionMode::Autopilot, "start_render"));
        assert!(auto_allows_tool(PermissionMode::Autopilot, "bash"));
    }

    #[test]
    fn manual_and_copilot_keep_approval_gate() {
        assert!(!auto_allows_tool(PermissionMode::Manual, "apply_edl"));
        assert!(!auto_allows_tool(PermissionMode::Copilot, "apply_edl"));
    }
}
