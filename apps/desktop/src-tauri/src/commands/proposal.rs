//! Approval-as-diff proposals — the desktop equivalent of the TUI's
//! modal approval card. Agents and users both fire EDL changes
//! through this same machinery: the proposal is computed (apply
//! against a clone, capture the post-state), shown to the user as
//! a ghost overlay on the timeline, then committed (or denied) by
//! the user.
//!
//! State of the world per proposal:
//! - **Pending**: lives in `AwidatState::pending_proposals`, keyed
//!   by call_id. Frontend has the corresponding `Item::ProposedEdit`
//!   ghost rendered on the canvas.
//! - **Adjusted**: user dragged a handle. `adjust_proposal` mutates
//!   the cached envelope, re-runs apply, emits a `ProposedEdit`
//!   Delta with bumped `revision`.
//! - **Accepted**: user pressed Enter. `accept_proposal` writes the
//!   proposed timeline to disk via `Project::write`, sends `Deny`
//!   on the agent's reply oneshot (because the user might have
//!   adjusted — the agent's *original* envelope didn't land), drops
//!   the entry, emits a `ProposedEdit` Completed.
//! - **Rejected**: user pressed Esc. `reject_proposal` sends `Deny`
//!   on the reply oneshot, drops the entry, emits Completed with
//!   the rejected status.
//!
//! Why Deny on accept (per Step 5 architectural decision):
//! we apply the *user's* envelope, not the agent's. Even when the
//! user accepted unchanged, that's still "user took over"; the
//! agent's tool_result text spells out what actually landed. Keeps
//! the agent's history honest.

use std::path::Path;

use awidat_core::awidat_mcp::tools::plan_visual_support_proposals::{
    PlanVisualSupportProposalArgs, merge_visual_support_defaults, plan_visual_support_proposals,
};
use awidat_core::edl::{ApplyError, EdlEnvelope, EdlOp, apply, parse};
use awidat_core::tool::ApprovalDecision;
use awidat_desktop_protocol::{
    AdjustField, AppliedDiff, ConfidenceTier, EditAdjustment, Id, Item, ItemLifecycle,
    ProposalEvidence, ProposalEvidenceKind, ProposalSource, RiskLevel, Side, TimelineSnapshot,
};
use awidat_proto::otio::Timeline;
use awidat_proto::professional::{EvidenceTrace, ReviewStatus};
use awidat_proto::project::Project;
use tauri::{AppHandle, State};

use crate::events::emit_item;
use crate::state::{AwidatState, PendingProposal};

/// Build a fresh proposal from raw EDL text + the project's current
/// timeline. Runs `apply()` against a clone (the original is
/// preserved in `PendingProposal.original_timeline`), produces a
/// `Timeline` for the proposed state, and emits the corresponding
/// `Item::ProposedEdit`.
///
/// `reply` is the agent's approval oneshot for agent-initiated
/// proposals; `None` for user-initiated. On accept we send `Deny` on
/// the agent's oneshot per the plan's "user took over" semantics.
///
/// `reasoning` is the agent's one-sentence justification for this
/// envelope (e.g. "trimmed 0.42s silence per podcast defaults"),
/// captured from `apply_edl(reasoning = …)`. The user-edit path
/// (`propose_user_edit`) has no agent voice and passes `None`; the
/// agent-initiated path threads the field through so Wave 3's
/// ProposalInspector / pill tooltip have the rationale to render.
///
/// The id is the proposal's stable identifier — re-used across
/// adjustment Deltas. Frontend keys its rendering on it.
#[allow(clippy::too_many_arguments)]
pub async fn build_proposal(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    id: String,
    edl_text: String,
    project_root: &Path,
    source: ProposalSource,
    reply: Option<tokio::sync::oneshot::Sender<ApprovalDecision>>,
    reasoning: Option<String>,
) -> Result<(), String> {
    let mut reply = reply;
    let envelope = match parse(&edl_text) {
        Ok(envelope) => envelope,
        Err(e) => {
            allow_agent_to_receive_apply_error(&mut reply);
            return Err(format!("parse EDL: {e}"));
        }
    };

    let project_root_buf = project_root.to_path_buf();
    let project = match tokio::task::spawn_blocking(move || Project::read(&project_root_buf)).await
    {
        Ok(Ok(project)) => project,
        Ok(Err(e)) => {
            allow_agent_to_receive_apply_error(&mut reply);
            return Err(format!("project read: {e}"));
        }
        Err(e) => return Err(format!("project join: {e}")),
    };

    let original_timeline = project.timeline.clone();
    let envelope_for_apply = envelope.clone();
    let project_root_for_ctx = project_root.to_path_buf();
    let original_for_apply = original_timeline.clone();
    let (proposed_timeline, applied) =
        match tokio::task::spawn_blocking(move || -> Result<_, ApplyError> {
            let ctx = awidat_core::edl::AnchorContext::with_project_root(&project_root_for_ctx);
            let (proposed, outcome) = apply(&original_for_apply, &envelope_for_apply, &ctx)?;
            Ok((proposed, outcome.applied))
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                allow_agent_to_receive_apply_error(&mut reply);
                return Err(format!("apply: {e}"));
            }
            Err(e) => return Err(format!("apply join: {e}")),
        };

    let snapshot =
        crate::commands::timeline::flatten_timeline_public(&proposed_timeline, project_root);
    let diff_hints = build_diff_hints(&envelope, &applied, &original_timeline, &proposed_timeline);
    let inspector_metadata = inspector_metadata_from_envelope(&envelope);
    let summary = inspector_metadata
        .as_ref()
        .map(|metadata| metadata.summary.clone())
        .unwrap_or_else(|| summarize_envelope(&envelope));
    let edl_text_for_item = edl_text.clone();

    let proposal = PendingProposal {
        call_id: id.clone(),
        project_root: project_root.to_path_buf(),
        envelope,
        original_timeline,
        proposed_timeline,
        applied,
        revision: 0,
        reply,
    };
    state
        .pending_proposals
        .lock()
        .await
        .insert(id.clone(), proposal);

    emit_item(
        app,
        Item::ProposedEdit {
            id: Id::new(&id),
            phase: ItemLifecycle::Started,
            source,
            edl_text: edl_text_for_item,
            snapshot,
            diff_hints,
            summary,
            revision: 0,
            intent: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.intent.clone()),
            explanation: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.explanation.clone()),
            confidence: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.confidence),
            risk: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.risk),
            evidence: inspector_metadata
                .as_ref()
                .map(|metadata| metadata.evidence.clone())
                .unwrap_or_default(),
            alternatives: vec![],
            // `rationale` is the agent's one-sentence justification
            // ("trimmed 0.42s silence per podcast defaults") that
            // Wave 3 surfaces on every proposal pill / Brief row.
            // For agent-initiated proposals it comes from
            // `apply_edl(reasoning = …)`; user-initiated callers
            // (drag-to-trim, transcript delete) pass `None` here
            // because there's no agent voice to surface.
            rationale: reasoning,
        },
    );
    Ok(())
}

fn allow_agent_to_receive_apply_error(
    reply: &mut Option<tokio::sync::oneshot::Sender<ApprovalDecision>>,
) {
    if let Some(reply) = reply.take() {
        let _ = reply.send(ApprovalDecision::Allow);
    }
}

/// User accepted the proposal (possibly with adjustments). Three
/// commit paths depending on origin and whether the user adjusted:
///
/// 1. **Agent-initiated, no adjustments (revision == 0)**: send
///    `Allow` on the oneshot. The agent's `apply_edl` handler
///    runs against the on-disk project, applies its own envelope,
///    writes. Desktop does NOT write — that would double-apply
///    (insert duplicates, fail trims that are already at bounds).
///    The timeline preview the user accepted matches what the
///    handler produces because the envelopes are identical and
///    apply() is deterministic.
/// 2. **Agent-initiated, with adjustments (revision > 0)**: send
///    `Deny` so the agent sees its envelope didn't land verbatim.
///    Desktop writes the adjusted timeline directly — the agent's
///    handler doesn't have the adjusted envelope to re-derive from.
///    The agent's next `read_timeline` sees the committed state.
/// 3. **User-initiated (no reply oneshot)**: write directly. There's
///    no agent path to defer to.
#[tauri::command]
pub async fn accept_proposal(
    app: AppHandle,
    state: State<'_, AwidatState>,
    call_id: String,
) -> Result<(), String> {
    let proposal = state
        .pending_proposals
        .lock()
        .await
        .remove(&call_id)
        .ok_or_else(|| format!("no pending proposal for {call_id}"))?;

    let user_adjusted = proposal.revision > 0;
    let agent_initiated = proposal.reply.is_some();

    // For agent-initiated + unchanged proposals the agent's handler
    // does the write after we send Allow; writing here would cause a
    // double-apply.
    let desktop_writes = !agent_initiated || user_adjusted;

    if desktop_writes {
        let project_root = proposal.project_root.clone();
        let proposed_timeline = proposal.proposed_timeline.clone();
        let applied_descriptions: Vec<String> = proposal
            .applied
            .iter()
            .map(|a| a.description.clone())
            .collect();
        // Resolve the seat-holder identity once, at the handler
        // entry point, so the spawn_blocking closure carries an
        // owned `CommitAuthor`. Without this the auto-commit hook
        // would stamp every desktop apply_edl as "awidat agent" even
        // though the user is the one driving the keyboard.
        let seat_author = crate::commands::vedit::desktop_commit_author();
        let action_metadata = awidat_core::vc::ActionMetadata {
            source: Some("agent".into()),
            operations: proposal
                .applied
                .iter()
                .map(|a| a.metadata.clone())
                .collect(),
        };
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut project =
                Project::read(&project_root).map_err(|e| format!("project read: {e}"))?;
            project.timeline = proposed_timeline;
            project
                .write(&project_root)
                .map_err(|e| format!("project write: {e}"))?;

            // Phase B auto-commit (desktop-writes path). Mirrors the
            // agent-side hook in `apply_edl.rs`. Best-effort: failures
            // are logged but never unwind the disk write. The `_as`
            // variant stamps the seat-holder on the commit; passing
            // `None` for the author falls back to the env + default chain.
            match awidat_core::vc::open_or_init(&project_root) {
                Ok(repo) => {
                    if let Err(e) = awidat_core::vc::auto_commit_apply_as_with_metadata(
                        &repo,
                        &applied_descriptions,
                        None,
                        seat_author,
                        Some(&action_metadata),
                    ) {
                        tracing::warn!(
                            error = %e,
                            "vedit auto-commit failed (desktop-writes path)"
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "vedit repo unavailable; skipping auto-commit"
                    );
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("write join: {e}"))??;
    }

    // Tell the agent what landed:
    //  - revision == 0 → Allow. Agent's handler runs and writes.
    //  - revision > 0  → Deny. Agent sees its envelope didn't land
    //    verbatim; we already wrote the adjusted version above.
    if let Some(reply) = proposal.reply {
        let decision = if user_adjusted {
            ApprovalDecision::Deny
        } else {
            ApprovalDecision::Allow
        };
        let _ = reply.send(decision);
    }

    // Final ProposedEdit Completed so the frontend collapses the
    // ghost overlay. summary mirrors what the agent will see.
    let final_summary = format!(
        "accepted ({} op{})",
        proposal.applied.len(),
        if proposal.applied.len() == 1 { "" } else { "s" },
    );
    let snapshot = crate::commands::timeline::flatten_timeline_public(
        &proposal.proposed_timeline,
        &proposal.project_root,
    );
    let diff_hints = build_diff_hints(
        &proposal.envelope,
        &proposal.applied,
        &proposal.original_timeline,
        &proposal.proposed_timeline,
    );
    emit_item(
        &app,
        Item::ProposedEdit {
            id: Id::new(&call_id),
            phase: ItemLifecycle::Completed,
            // Refresh semantics on the frontend key off this completed
            // source: User means the desktop already wrote the proposed
            // timeline; Agent means the accepted original apply_edl is
            // about to run and emit its own tool completion after disk
            // write.
            source: if desktop_writes {
                ProposalSource::User
            } else {
                ProposalSource::Agent {
                    tool_name: "apply_edl".into(),
                }
            },
            edl_text: String::new(),
            snapshot,
            diff_hints,
            summary: final_summary,
            revision: proposal.revision,
            intent: None,
            explanation: None,
            confidence: None,
            risk: None,
            evidence: vec![],
            alternatives: vec![],
            // The Completed phase ends the lifecycle; rationale lives
            // on the Started / Delta phases where the inspector is
            // open. No need to re-emit it here.
            rationale: None,
        },
    );
    Ok(())
}

/// User rejected the proposal. Sends Deny on the reply oneshot
/// (agent sees its envelope was denied), drops the entry. No disk
/// write.
#[tauri::command]
pub async fn reject_proposal(
    app: AppHandle,
    state: State<'_, AwidatState>,
    call_id: String,
) -> Result<(), String> {
    let proposal = state
        .pending_proposals
        .lock()
        .await
        .remove(&call_id)
        .ok_or_else(|| format!("no pending proposal for {call_id}"))?;

    if let Some(reply) = proposal.reply {
        let _ = reply.send(ApprovalDecision::Deny);
    }

    emit_item(
        &app,
        Item::ProposedEdit {
            id: Id::new(&call_id),
            phase: ItemLifecycle::Completed,
            source: ProposalSource::User,
            edl_text: String::new(),
            snapshot: TimelineSnapshot {
                duration_s: 0.0,
                broadcast_overlay: None,
                cut_boundaries: Vec::new(),
                preview_limitations: Vec::new(),
                tracks: Vec::new(),
            },
            diff_hints: Vec::new(),
            summary: "rejected".into(),
            revision: proposal.revision,
            intent: None,
            explanation: None,
            confidence: None,
            risk: None,
            evidence: vec![],
            alternatives: vec![],
            // Reject ends the lifecycle; rationale lives on the
            // Started / Delta phases.
            rationale: None,
        },
    );
    Ok(())
}

/// User dragged a proposed edge. Mutate the cached envelope,
/// re-run apply against the original, replace the cached
/// `proposed_timeline` + `applied`, bump revision, emit a Delta.
#[tauri::command]
pub async fn adjust_proposal(
    app: AppHandle,
    state: State<'_, AwidatState>,
    call_id: String,
    adjustments: Vec<EditAdjustment>,
) -> Result<(), String> {
    // Hold the mutex across the apply() call so racing adjust_proposal
    // requests serialize.
    let mut map = state.pending_proposals.lock().await;
    let proposal = map
        .get_mut(&call_id)
        .ok_or_else(|| format!("no pending proposal for {call_id}"))?;

    for adj in &adjustments {
        apply_adjustment_to_envelope(&mut proposal.envelope, adj)?;
    }

    let envelope_for_apply = proposal.envelope.clone();
    let original_for_apply = proposal.original_timeline.clone();
    let project_root_for_ctx = proposal.project_root.clone();
    let (proposed_timeline, applied) =
        tokio::task::spawn_blocking(move || -> Result<_, ApplyError> {
            let ctx = awidat_core::edl::AnchorContext::with_project_root(&project_root_for_ctx);
            let (proposed, outcome) = apply(&original_for_apply, &envelope_for_apply, &ctx)?;
            Ok((proposed, outcome.applied))
        })
        .await
        .map_err(|e| format!("apply join: {e}"))?
        .map_err(|e| format!("adjusted apply: {e}"))?;

    proposal.revision += 1;
    let revision = proposal.revision;
    proposal.proposed_timeline = proposed_timeline.clone();
    proposal.applied = applied.clone();

    let snapshot = crate::commands::timeline::flatten_timeline_public(
        &proposed_timeline,
        &proposal.project_root,
    );
    let diff_hints = build_diff_hints(
        &proposal.envelope,
        &applied,
        &proposal.original_timeline,
        &proposed_timeline,
    );
    let inspector_metadata = inspector_metadata_from_envelope(&proposal.envelope);
    let summary = inspector_metadata
        .as_ref()
        .map(|metadata| metadata.summary.clone())
        .unwrap_or_else(|| summarize_envelope(&proposal.envelope));
    let edl_text = String::new(); // not re-serialized; Delta doesn't need it

    drop(map); // release before emit so the frontend's response can grab the lock again

    emit_item(
        &app,
        Item::ProposedEdit {
            id: Id::new(&call_id),
            phase: ItemLifecycle::Delta,
            source: ProposalSource::User,
            edl_text,
            snapshot,
            diff_hints,
            summary,
            revision,
            intent: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.intent.clone()),
            explanation: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.explanation.clone()),
            confidence: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.confidence),
            risk: inspector_metadata
                .as_ref()
                .and_then(|metadata| metadata.risk),
            evidence: inspector_metadata
                .as_ref()
                .map(|metadata| metadata.evidence.clone())
                .unwrap_or_default(),
            alternatives: vec![],
            // Delta phase: the frontend store preserves the rationale
            // from the prior phase when a Delta omits it (same pattern
            // as `intent` / `explanation`). User drags don't carry an
            // agent rationale, so emit `None`.
            rationale: None,
        },
    );
    Ok(())
}

/// User-initiated proposal — drag-to-trim, transcript delete, etc.
/// No agent reply oneshot; on accept we apply directly via the same
/// `accept_proposal` path.
///
/// Returns the freshly-allocated call_id so the frontend knows
/// which proposal to track in subsequent adjust/accept calls.
#[tauri::command]
pub async fn propose_user_edit(
    app: AppHandle,
    state: State<'_, AwidatState>,
    edl_text: String,
) -> Result<String, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let id = format!(
        "user-edit-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    build_proposal(
        &app,
        &state,
        id.clone(),
        edl_text,
        &project_root,
        ProposalSource::User,
        None,
        // User-edit path: no agent voice, so no rationale on the wire.
        None,
    )
    .await?;

    Ok(id)
}

#[derive(Debug, Clone)]
struct VisualSupportProposalRequest {
    selection_text: String,
    request: String,
    anchor_transcript: Option<String>,
    anchor_timeline_start_s: Option<f64>,
    artifact_type: Option<String>,
    broll_asset: Option<String>,
    duration_s: Option<f64>,
    reference_assets: Vec<String>,
}

#[derive(Debug)]
struct PlannedVisualSupportProposal {
    summary: String,
    edl_text: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualSupportProposalPayload {
    selection_text: String,
    request: String,
    anchor_transcript: Option<String>,
    anchor_timeline_start_s: Option<f64>,
    artifact_type: Option<String>,
    broll_asset: Option<String>,
    duration_s: Option<f64>,
    reference_assets: Option<Vec<String>>,
}

#[cfg(test)]
fn visual_support_proposal_edl(
    request: VisualSupportProposalRequest,
) -> Result<PlannedVisualSupportProposal, String> {
    visual_support_proposal_edls(request, None)?
        .into_iter()
        .next()
        .ok_or_else(|| "visual support planner returned no apply-ready proposal".to_string())
}

/// Pick the single planned proposal to open in the Proposal Inspector.
///
/// Sibling proposals from one planner call are all computed against the same
/// pre-accept timeline, and `accept_proposal` writes a cached
/// `proposed_timeline` wholesale — so opening more than one would let
/// accepting the second drop the first's accepted changes. The planner
/// already returns candidates highest-confidence first, so we take the head.
fn first_proposal_to_open(
    planned: Vec<PlannedVisualSupportProposal>,
) -> Option<PlannedVisualSupportProposal> {
    planned.into_iter().next()
}

fn visual_support_proposal_edls(
    request: VisualSupportProposalRequest,
    project_root: Option<&Path>,
) -> Result<Vec<PlannedVisualSupportProposal>, String> {
    let mut args = PlanVisualSupportProposalArgs {
        selection_text: request.selection_text,
        request: request.request,
        anchor_transcript: request.anchor_transcript,
        anchor_timeline_start_s: request.anchor_timeline_start_s,
        broll_asset: request.broll_asset,
        duration_s: request.duration_s,
        reference_assets: request.reference_assets,
        ..PlanVisualSupportProposalArgs::default()
    };
    // Mirror the MCP `run` wrapper so user-initiated transcript-selection
    // proposals honour the editor's saved brand/aspect/safe-area/reference
    // defaults instead of opening with a different export/style contract than
    // agent-planned proposals.
    if let Some(root) = project_root {
        args = merge_visual_support_defaults(args, root);
    }
    let planned = plan_visual_support_proposals(args)?;
    let proposals = planned["proposals"]
        .as_array()
        .ok_or_else(|| "visual support planner returned no proposals array".to_string())?;
    let planned = proposals
        .iter()
        .filter_map(|proposal| {
            let artifact_matches = request
                .artifact_type
                .as_deref()
                .is_none_or(|artifact_type| {
                    proposal["artifact_type"].as_str() == Some(artifact_type)
                });
            if !artifact_matches {
                return None;
            }
            if proposal["missing_information"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
            {
                return None;
            }
            let edl_text = proposal["apply_edl"]["edl"].as_str()?;
            Some(PlannedVisualSupportProposal {
                summary: proposal["title"]
                    .as_str()
                    .unwrap_or("Visual support proposal")
                    .to_string(),
                edl_text: edl_text.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if planned.is_empty() {
        return Err({
            let clarification = proposals
                .iter()
                .flat_map(|proposal| {
                    proposal["missing_information"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .find_map(|item| item["question"].as_str())
                .unwrap_or("answer the proposal's missing information before opening it");
            format!("visual support proposal needs clarification: {clarification}")
        });
    }
    Ok(planned)
}

/// User-initiated visual-support proposal from selected transcript text.
/// Plans through the editorial-skill planner, then opens the chosen
/// apply-ready proposal through the standard Proposal Inspector flow.
#[tauri::command]
pub async fn propose_visual_support(
    app: AppHandle,
    state: State<'_, AwidatState>,
    payload: VisualSupportProposalPayload,
) -> Result<String, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let planned = visual_support_proposal_edls(
        VisualSupportProposalRequest {
            selection_text: payload.selection_text,
            request: payload.request,
            anchor_transcript: payload.anchor_transcript,
            anchor_timeline_start_s: payload.anchor_timeline_start_s,
            artifact_type: payload.artifact_type,
            broll_asset: payload.broll_asset,
            duration_s: payload.duration_s,
            reference_assets: payload.reference_assets.unwrap_or_default(),
        },
        Some(project_root.as_path()),
    )?;
    // Open a single proposal at a time. `build_proposal` snapshots the
    // current timeline and `accept_proposal` later writes that cached
    // `proposed_timeline` wholesale, so opening every sibling here (each
    // computed from the same pre-accept timeline) would let accepting the
    // second one silently drop the first's accepted changes. The planner
    // still ranks the candidates; we surface the highest-confidence one.
    let proposal = first_proposal_to_open(planned)
        .ok_or_else(|| "visual support planner returned no apply-ready proposal".to_string())?;
    let id = format!(
        "visual-support-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let _ = &proposal.summary;
    build_proposal(
        &app,
        &state,
        id.clone(),
        proposal.edl_text,
        &project_root,
        ProposalSource::User,
        None,
        None,
    )
    .await?;
    Ok(id)
}

// --- internal: diff hints + summary + adjust application ----------

/// Walk the parsed envelope alongside the applied outcome and
/// produce per-op `AppliedDiff` metadata for the protocol.
///
/// For ops with a resolved locator (Trim/Untrim/Delete/Split) the
/// locator carries the original-snapshot indexes. We compare the
/// op's args against the original clip's source_range to compute
/// signed deltas. For Insert (no locator), we infer the proposed
/// item index by comparing track lengths.
fn build_diff_hints(
    envelope: &EdlEnvelope,
    applied: &[awidat_core::edl::AppliedOp],
    original: &Timeline,
    proposed: &Timeline,
) -> Vec<AppliedDiff> {
    let mut hints = Vec::with_capacity(envelope.ops.len());
    for (op_index, op) in envelope.ops.iter().enumerate() {
        let locator = applied.get(op_index).and_then(|a| a.locator);
        match op {
            EdlOp::TrimClip { start, end, .. } | EdlOp::UntrimClip { start, end, .. } => {
                if let Some(loc) = locator {
                    let (orig_start, orig_end) = clip_source_range(original, loc);
                    if let Some(end_v) = end {
                        let delta = orig_end - end_v;
                        hints.push(AppliedDiff::TrimEdge {
                            op_index,
                            track_index: loc.track_index,
                            item_index: loc.child_index,
                            side: Side::Right,
                            delta_s: delta,
                        });
                    }
                    if let Some(start_v) = start {
                        let delta = start_v - orig_start;
                        hints.push(AppliedDiff::TrimEdge {
                            op_index,
                            track_index: loc.track_index,
                            item_index: loc.child_index,
                            side: Side::Left,
                            delta_s: delta,
                        });
                    }
                }
            }
            EdlOp::DeleteClip { .. } => {
                if let Some(loc) = locator {
                    hints.push(AppliedDiff::Delete {
                        op_index,
                        track_index: loc.track_index,
                        item_index: loc.child_index,
                    });
                }
            }
            EdlOp::SplitClip { at_s, .. } => {
                if let Some(loc) = locator {
                    hints.push(AppliedDiff::Split {
                        op_index,
                        track_index: loc.track_index,
                        item_index: loc.child_index,
                        at_s: *at_s,
                    });
                }
            }
            EdlOp::InsertClip {
                track,
                at_position,
                name,
                ..
            } => {
                // Locate by inserted name when present; otherwise by
                // the requested insertion index clamped to the original
                // track length. Keeps middle inserts highlighted at
                // their landed position.
                if let Some((track_index, item_index)) =
                    find_inserted_position(proposed, track, original, *at_position, name.as_deref())
                {
                    hints.push(AppliedDiff::Insert {
                        op_index,
                        track_index,
                        item_index,
                    });
                }
            }
            EdlOp::InsertBRoll { .. } => {
                if let Some((track_index, item_index)) =
                    find_inserted_clip_by_prefix(proposed, "broll-from-")
                {
                    hints.push(AppliedDiff::InsertBRoll {
                        op_index,
                        track_index,
                        item_index,
                    });
                }
            }
            // Metadata/effect-only ops are left to the post-state
            // snapshot; they don't have a dedicated visual hint yet.
            EdlOp::InsertPiP { .. } => {
                if let Some((track_index, item_index)) =
                    find_inserted_clip_by_prefix(proposed, "pip-from-")
                {
                    hints.push(AppliedDiff::InsertPiP {
                        op_index,
                        track_index,
                        item_index,
                    });
                }
            }
            EdlOp::MoveClip { .. } | EdlOp::RippleMove { .. } => {
                if let Some(loc) = locator
                    && let Some((to_track_index, to_item_index)) =
                        find_moved_clip_position(original, proposed, loc)
                {
                    hints.push(AppliedDiff::Move {
                        op_index,
                        from_track_index: loc.track_index,
                        from_item_index: loc.child_index,
                        to_track_index,
                        to_item_index,
                    });
                }
            }
            EdlOp::RippleDelete { .. } => {
                if let Some(loc) = locator {
                    hints.push(AppliedDiff::Delete {
                        op_index,
                        track_index: loc.track_index,
                        item_index: loc.child_index,
                    });
                }
            }
            EdlOp::RippleTrim { edge, value_s, .. } => {
                if let Some(loc) = locator {
                    let (orig_start, orig_end) = clip_source_range(original, loc);
                    let (side, delta_s) = match edge {
                        awidat_core::edl::op::RippleTrimEdge::Start => {
                            (Side::Left, value_s - orig_start)
                        }
                        awidat_core::edl::op::RippleTrimEdge::End => {
                            (Side::Right, orig_end - value_s)
                        }
                    };
                    hints.push(AppliedDiff::TrimEdge {
                        op_index,
                        track_index: loc.track_index,
                        item_index: loc.child_index,
                        side,
                        delta_s,
                    });
                }
            }
            EdlOp::InsertTransition { .. }
            | EdlOp::DeleteGap { .. }
            | EdlOp::TrimTrackTail { .. }
            | EdlOp::InsertTrack { .. }
            | EdlOp::DeleteTrack { .. }
            | EdlOp::ApplyMulticamPlan { .. }
            | EdlOp::DeleteTransition { .. }
            | EdlOp::SetCutIntent { .. }
            | EdlOp::SetVolume { .. }
            | EdlOp::SetAudioFade { .. }
            | EdlOp::SetAudioLead { .. }
            | EdlOp::SetAudioTrail { .. }
            | EdlOp::SetTrackAudio { .. }
            | EdlOp::SetDucking { .. }
            | EdlOp::SetSyncGroup { .. }
            | EdlOp::SetClipAudioFx { .. }
            | EdlOp::SetTrackAudioFx { .. }
            | EdlOp::SetEffect { .. }
            | EdlOp::SetSpeed { .. }
            | EdlOp::SetTimeRemap { .. }
            | EdlOp::SetFreeze { .. }
            | EdlOp::SetColorCorrection { .. }
            | EdlOp::ApplyLut { .. }
            | EdlOp::RemoveLut { .. }
            | EdlOp::SetBroadcastOverlay { .. }
            | EdlOp::SetAssetCatalog { .. }
            | EdlOp::SetSourceReview { .. }
            | EdlOp::ProfessionalTimelineEdit { .. }
            | EdlOp::AuthorSubjectReframeFromTrack { .. }
            | EdlOp::AddProposalPackage { .. }
            | EdlOp::SetParameterAnimation { .. }
            | EdlOp::SetMotionTemplate { .. }
            | EdlOp::SetMotionScene { .. }
            | EdlOp::AttachComposition { .. }
            | EdlOp::SetTrackingPackage { .. }
            | EdlOp::SetColorFinishing { .. }
            | EdlOp::SetAudioFinishing { .. }
            | EdlOp::SelectDeliveryProfile { .. }
            | EdlOp::AddPreflightReport { .. }
            | EdlOp::SetWorkflowLens { .. }
            | EdlOp::SetPipelineReadiness { .. }
            | EdlOp::InsertTitle { .. }
            | EdlOp::InsertRichTitle { .. }
            | EdlOp::InstantiateMotionTemplate { .. }
            | EdlOp::SetTitle { .. }
            | EdlOp::InsertCaption { .. }
            | EdlOp::InsertAnnotation { .. }
            | EdlOp::SetOutputFormat { .. }
            | EdlOp::SetLoudnessTarget { .. }
            | EdlOp::MuteClip { .. }
            | EdlOp::RemoveAudio { .. }
            | EdlOp::SetBrandKit { .. }
            | EdlOp::SetPackageMetadata { .. } => {}
        }
    }
    hints
}

/// Read a clip's source_range bounds at the given locator. Returns
/// (0.0, 0.0) for non-clip locators (transitions/gaps shouldn't be
/// the target of trim ops; defensive).
fn clip_source_range(timeline: &Timeline, loc: awidat_core::edl::ClipLocator) -> (f64, f64) {
    use awidat_proto::otio::{StackChild, TrackChild};
    let Some(StackChild::Track(track)) = timeline.tracks.children.get(loc.track_index) else {
        return (0.0, 0.0);
    };
    let Some(TrackChild::Clip(clip)) = track.children.get(loc.child_index) else {
        return (0.0, 0.0);
    };
    let Some(range) = clip.source_range.as_ref() else {
        return (0.0, 0.0);
    };
    let start = range.start_time.to_seconds();
    let end = start + range.duration.to_seconds();
    (start, end)
}

/// Find where an InsertClip landed in the proposed snapshot. Returns
/// `(track_index, item_index)` of the new clip in `proposed`. We
/// match by (track_name → end of track) since `at_position` is
/// fold-equivalent to "wherever the insert ended up."
fn find_inserted_position(
    proposed: &Timeline,
    track_name: &str,
    original: &Timeline,
    at_position: Option<usize>,
    name: Option<&str>,
) -> Option<(usize, usize)> {
    use awidat_proto::otio::{StackChild, TrackChild};
    let original_len = original
        .tracks
        .children
        .iter()
        .find_map(|child| match child {
            StackChild::Track(track) if track.name == track_name => Some(track.children.len()),
            _ => None,
        })
        .unwrap_or(0);
    for (track_index, child) in proposed.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        if track.name != track_name {
            continue;
        }
        if let Some(name) = name {
            for (item_index, tc) in track.children.iter().enumerate() {
                if matches!(tc, TrackChild::Clip(c) if c.name == name) {
                    return Some((track_index, item_index));
                }
            }
        }
        let item_index = at_position.unwrap_or(original_len).min(original_len);
        if matches!(track.children.get(item_index), Some(TrackChild::Clip(_))) {
            return Some((track_index, item_index));
        }
    }
    None
}

fn find_inserted_clip_by_prefix(timeline: &Timeline, name_prefix: &str) -> Option<(usize, usize)> {
    use awidat_proto::otio::{StackChild, TrackChild};
    for (track_index, child) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        for (item_index, item) in track.children.iter().enumerate() {
            if matches!(item, TrackChild::Clip(c) if c.name.starts_with(name_prefix)) {
                return Some((track_index, item_index));
            }
        }
    }
    None
}

fn find_moved_clip_position(
    original: &Timeline,
    proposed: &Timeline,
    loc: awidat_core::edl::ClipLocator,
) -> Option<(usize, usize)> {
    use awidat_proto::otio::{StackChild, TrackChild};
    let StackChild::Track(original_track) = original.tracks.children.get(loc.track_index)? else {
        return None;
    };
    let TrackChild::Clip(original_clip) = original_track.children.get(loc.child_index)? else {
        return None;
    };
    let uuid = clip_uuid_or_name(original_clip);
    for (track_index, child) in proposed.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        for (item_index, item) in track.children.iter().enumerate() {
            let TrackChild::Clip(clip) = item else {
                continue;
            };
            if clip_uuid_or_name(clip) == uuid {
                return Some((track_index, item_index));
            }
        }
    }
    None
}

fn clip_uuid_or_name(clip: &awidat_proto::otio::Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|m| m.extra.get("clip_uuid"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| clip.name.clone())
}

/// Apply one EditAdjustment to the envelope by mutating the op at
/// `op_index`. Errors on out-of-range indexes or field/op
/// mismatches (e.g. SplitAt against a TrimClip).
fn apply_adjustment_to_envelope(
    envelope: &mut EdlEnvelope,
    adj: &EditAdjustment,
) -> Result<(), String> {
    let op = envelope
        .ops
        .get_mut(adj.op_index)
        .ok_or_else(|| format!("op index {} out of range", adj.op_index))?;
    match (op, adj.field) {
        (EdlOp::TrimClip { start, .. }, AdjustField::TrimStart)
        | (EdlOp::UntrimClip { start, .. }, AdjustField::TrimStart) => {
            *start = Some(adj.value_s);
        }
        (EdlOp::TrimClip { end, .. }, AdjustField::TrimEnd)
        | (EdlOp::UntrimClip { end, .. }, AdjustField::TrimEnd) => {
            *end = Some(adj.value_s);
        }
        (EdlOp::SplitClip { at_s, .. }, AdjustField::SplitAt) => {
            *at_s = adj.value_s;
        }
        (EdlOp::InsertClip { start, .. }, AdjustField::InsertStart) => {
            *start = Some(adj.value_s);
        }
        (EdlOp::InsertClip { end, .. }, AdjustField::InsertEnd) => {
            *end = Some(adj.value_s);
        }
        (op, field) => {
            return Err(format!(
                "field {field:?} doesn't apply to op variant {}",
                op_kind_label(op)
            ));
        }
    }
    Ok(())
}

fn op_kind_label(op: &EdlOp) -> &'static str {
    match op {
        EdlOp::TrimClip { .. } => "TrimClip",
        EdlOp::UntrimClip { .. } => "UntrimClip",
        EdlOp::DeleteClip { .. } => "DeleteClip",
        EdlOp::SplitClip { .. } => "SplitClip",
        EdlOp::InsertClip { .. } => "InsertClip",
        EdlOp::InsertBRoll { .. } => "InsertBRoll",
        EdlOp::InsertPiP { .. } => "InsertPiP",
        EdlOp::MoveClip { .. } => "MoveClip",
        EdlOp::RippleMove { .. } => "RippleMove",
        EdlOp::RippleDelete { .. } => "RippleDelete",
        EdlOp::RippleTrim { .. } => "RippleTrim",
        EdlOp::DeleteGap { .. } => "DeleteGap",
        EdlOp::TrimTrackTail { .. } => "TrimTrackTail",
        EdlOp::InsertTrack { .. } => "InsertTrack",
        EdlOp::DeleteTrack { .. } => "DeleteTrack",
        EdlOp::ApplyMulticamPlan { .. } => "ApplyMulticamPlan",
        EdlOp::InsertTransition { .. } => "InsertTransition",
        EdlOp::DeleteTransition { .. } => "DeleteTransition",
        EdlOp::SetCutIntent { .. } => "SetCutIntent",
        EdlOp::SetVolume { .. } => "SetVolume",
        EdlOp::MuteClip { .. } => "MuteClip",
        EdlOp::RemoveAudio { .. } => "RemoveAudio",
        EdlOp::SetAudioFade { .. } => "SetAudioFade",
        EdlOp::SetAudioLead { .. } => "SetAudioLead",
        EdlOp::SetAudioTrail { .. } => "SetAudioTrail",
        EdlOp::SetTrackAudio { .. } => "SetTrackAudio",
        EdlOp::SetDucking { .. } => "SetDucking",
        EdlOp::SetSyncGroup { .. } => "SetSyncGroup",
        EdlOp::SetClipAudioFx { .. } => "SetClipAudioFx",
        EdlOp::SetTrackAudioFx { .. } => "SetTrackAudioFx",
        EdlOp::SetEffect { .. } => "SetEffect",
        EdlOp::SetSpeed { .. } => "SetSpeed",
        EdlOp::SetTimeRemap { .. } => "SetTimeRemap",
        EdlOp::SetFreeze { .. } => "SetFreeze",
        EdlOp::SetColorCorrection { .. } => "SetColorCorrection",
        EdlOp::ApplyLut { .. } => "ApplyLut",
        EdlOp::RemoveLut { .. } => "RemoveLut",
        EdlOp::SetBroadcastOverlay { .. } => "SetBroadcastOverlay",
        EdlOp::SetAssetCatalog { .. } => "SetAssetCatalog",
        EdlOp::SetSourceReview { .. } => "SetSourceReview",
        EdlOp::ProfessionalTimelineEdit { .. } => "ProfessionalTimelineEdit",
        EdlOp::AuthorSubjectReframeFromTrack { .. } => "AuthorSubjectReframeFromTrack",
        EdlOp::AddProposalPackage { .. } => "AddProposalPackage",
        EdlOp::SetParameterAnimation { .. } => "SetParameterAnimation",
        EdlOp::SetMotionTemplate { .. } => "SetMotionTemplate",
        EdlOp::SetMotionScene { .. } => "SetMotionScene",
        EdlOp::AttachComposition { .. } => "AttachComposition",
        EdlOp::SetTrackingPackage { .. } => "SetTrackingPackage",
        EdlOp::SetColorFinishing { .. } => "SetColorFinishing",
        EdlOp::SetAudioFinishing { .. } => "SetAudioFinishing",
        EdlOp::SelectDeliveryProfile { .. } => "SelectDeliveryProfile",
        EdlOp::AddPreflightReport { .. } => "AddPreflightReport",
        EdlOp::SetWorkflowLens { .. } => "SetWorkflowLens",
        EdlOp::SetPipelineReadiness { .. } => "SetPipelineReadiness",
        EdlOp::InsertTitle { .. } => "InsertTitle",
        EdlOp::InsertRichTitle { .. } => "InsertRichTitle",
        EdlOp::InstantiateMotionTemplate { .. } => "InstantiateMotionTemplate",
        EdlOp::SetTitle { .. } => "SetTitle",
        EdlOp::InsertCaption { .. } => "InsertCaption",
        EdlOp::InsertAnnotation { .. } => "InsertAnnotation",
        EdlOp::SetOutputFormat { .. } => "SetOutputFormat",
        EdlOp::SetLoudnessTarget { .. } => "SetLoudnessTarget",
        EdlOp::SetBrandKit { .. } => "SetBrandKit",
        EdlOp::SetPackageMetadata { .. } => "SetPackageMetadata",
    }
}

#[derive(Clone, Debug)]
struct InspectorMetadata {
    summary: String,
    intent: Option<String>,
    explanation: Option<String>,
    confidence: Option<f32>,
    risk: Option<RiskLevel>,
    evidence: Vec<ProposalEvidence>,
}

fn inspector_metadata_from_envelope(envelope: &EdlEnvelope) -> Option<InspectorMetadata> {
    let package = envelope.ops.iter().find_map(|op| {
        if let EdlOp::AddProposalPackage { package } = op {
            Some(package)
        } else {
            None
        }
    })?;
    let confidence = package
        .evidence
        .iter()
        .filter_map(|evidence| evidence.confidence)
        .next()
        .map(|confidence| confidence as f32);
    let evidence = package
        .evidence
        .iter()
        .map(proposal_evidence_from_trace)
        .collect::<Vec<_>>();
    let explanation = package
        .evidence
        .iter()
        .filter_map(|evidence| evidence.reason.as_deref())
        .find(|reason| !reason.trim().is_empty())
        .map(str::to_string);
    Some(InspectorMetadata {
        summary: package.summary.clone(),
        intent: Some(package.summary.clone()),
        explanation,
        confidence,
        risk: Some(risk_from_package_status(package.status, confidence)),
        evidence,
    })
}

fn proposal_evidence_from_trace(trace: &EvidenceTrace) -> ProposalEvidence {
    let confidence = trace.confidence.map(|confidence| confidence as f32);
    ProposalEvidence {
        kind: proposal_evidence_kind(&trace.source),
        label: trace
            .reason
            .clone()
            .unwrap_or_else(|| evidence_label_from_trace(trace)),
        confidence,
        confidence_level: confidence.map(confidence_tier),
    }
}

fn evidence_label_from_trace(trace: &EvidenceTrace) -> String {
    if trace.refs.is_empty() {
        trace.source.clone()
    } else {
        format!("{}: {}", trace.source, trace.refs.join(", "))
    }
}

fn proposal_evidence_kind(source: &str) -> ProposalEvidenceKind {
    match source {
        "transcript" => ProposalEvidenceKind::Transcript,
        "pacing" => ProposalEvidenceKind::Pacing,
        "filler" => ProposalEvidenceKind::Filler,
        "audio_energy" => ProposalEvidenceKind::AudioEnergy,
        "speaker_handoff" => ProposalEvidenceKind::SpeakerHandoff,
        "silence" => ProposalEvidenceKind::Silence,
        "cut_boundary" => ProposalEvidenceKind::CutBoundary,
        _ => ProposalEvidenceKind::Visual,
    }
}

fn confidence_tier(confidence: f32) -> ConfidenceTier {
    if confidence >= 0.80 {
        ConfidenceTier::High
    } else if confidence >= 0.55 {
        ConfidenceTier::Medium
    } else if confidence >= 0.30 {
        ConfidenceTier::Low
    } else {
        ConfidenceTier::VeryLow
    }
}

fn risk_from_package_status(status: ReviewStatus, confidence: Option<f32>) -> RiskLevel {
    match status {
        ReviewStatus::Rejected => RiskLevel::VeryHigh,
        ReviewStatus::Superseded => RiskLevel::High,
        ReviewStatus::Accepted => RiskLevel::Low,
        ReviewStatus::Proposed => match confidence {
            Some(value) if value < 0.30 => RiskLevel::VeryHigh,
            Some(value) if value < 0.55 => RiskLevel::High,
            Some(value) if value < 0.80 => RiskLevel::Medium,
            _ => RiskLevel::Low,
        },
    }
}

fn summarize_envelope(envelope: &EdlEnvelope) -> String {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for op in &envelope.ops {
        *counts.entry(op_kind_label(op)).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    use awidat_core::edl::{Anchor, AppliedOp, AppliedOpMetadata, ClipLocator, EdlOp};
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };
    use awidat_proto::professional::{
        CapabilityArea, EvidenceTrace, ProposalPackage, ReviewStatus,
    };

    #[test]
    fn summarize_counts_per_op_kind() {
        let envelope = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet { text: "a".into() },
                    start: None,
                    end: Some(2.0),
                },
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet { text: "b".into() },
                    start: None,
                    end: Some(4.0),
                },
                EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet { text: "c".into() },
                },
            ],
        };
        assert_eq!(summarize_envelope(&envelope), "1 DeleteClip, 2 TrimClip");
    }

    #[test]
    fn proposal_package_populates_inspector_metadata() {
        let envelope = EdlEnvelope {
            ops: vec![EdlOp::AddProposalPackage {
                package: ProposalPackage {
                    id: "visual-support-quote".into(),
                    area: CapabilityArea::EditorialIntentAndReview,
                    status: ReviewStatus::Proposed,
                    summary: "Quote highlight".into(),
                    evidence: vec![EvidenceTrace {
                        source: "transcript".into(),
                        refs: vec!["transcript:quote that should land harder".into()],
                        reason: Some(
                            "Selected transcript anchor: quote that should land harder".into(),
                        ),
                        confidence: Some(0.92),
                    }],
                    rollback_refs: vec![],
                },
            }],
        };

        let metadata = inspector_metadata_from_envelope(&envelope).unwrap();

        assert_eq!(metadata.summary, "Quote highlight");
        assert_eq!(metadata.intent.as_deref(), Some("Quote highlight"));
        assert_eq!(metadata.confidence, Some(0.92));
        assert_eq!(metadata.risk, Some(RiskLevel::Low));
        assert_eq!(metadata.evidence.len(), 1);
        assert_eq!(metadata.evidence[0].kind, ProposalEvidenceKind::Transcript);
        assert!(
            metadata.evidence[0]
                .label
                .contains("quote that should land harder")
        );
    }

    #[test]
    fn visual_support_selection_plans_apply_ready_proposal_edl() {
        let planned = visual_support_proposal_edl(VisualSupportProposalRequest {
            selection_text: "This quote should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote should land harder".into()),
            anchor_timeline_start_s: None,
            artifact_type: Some("quote_highlight".into()),
            broll_asset: None,
            duration_s: Some(3.0),
            reference_assets: vec![],
        })
        .unwrap();

        assert_eq!(planned.summary, "Quote highlight");
        assert!(planned.edl_text.contains("*** Add Proposal Package"));
        assert!(planned.edl_text.contains("*** Set Motion Scene"));
        assert!(planned.edl_text.contains("quote should land harder"));
    }

    #[test]
    fn visual_support_selection_keeps_multiple_apply_ready_proposals() {
        let planned = visual_support_proposal_edls(
            VisualSupportProposalRequest {
                selection_text: "This quote explains why the 42% growth matters.".into(),
                request: "add visual support with a quote highlight and statistic counter".into(),
                anchor_transcript: Some("42% growth matters".into()),
                anchor_timeline_start_s: None,
                artifact_type: None,
                broll_asset: None,
                duration_s: Some(3.0),
                reference_assets: vec![],
            },
            None,
        )
        .unwrap();
        let summaries = planned
            .iter()
            .map(|proposal| proposal.summary.as_str())
            .collect::<Vec<_>>();

        assert!(summaries.contains(&"Quote highlight"));
        assert!(summaries.contains(&"Counter/stat graphic"));
        assert!(
            planned
                .iter()
                .all(|proposal| proposal.edl_text.contains("*** Add Proposal Package"))
        );
    }

    #[test]
    fn visual_support_selection_opens_only_one_proposal() {
        // The planner can return several apply-ready siblings, but only one
        // may be opened: each is computed from the same pre-accept timeline,
        // and accepting two would clobber the first's accepted changes.
        let planned = visual_support_proposal_edls(
            VisualSupportProposalRequest {
                selection_text: "This quote explains why the 42% growth matters.".into(),
                request: "add visual support with a quote highlight and statistic counter".into(),
                anchor_transcript: Some("42% growth matters".into()),
                anchor_timeline_start_s: None,
                artifact_type: None,
                broll_asset: None,
                duration_s: Some(3.0),
                reference_assets: vec![],
            },
            None,
        )
        .unwrap();
        assert!(
            planned.len() > 1,
            "precondition: planner should offer multiple siblings here"
        );
        let first_edl = planned[0].edl_text.clone();

        let chosen = first_proposal_to_open(planned).expect("one proposal to open");
        assert_eq!(
            chosen.edl_text, first_edl,
            "must open the first (highest-confidence) planned proposal"
        );
    }

    #[test]
    fn visual_support_selection_applies_saved_project_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".awidat")).unwrap();
        std::fs::write(
            dir.path()
                .join(".awidat")
                .join("visual_support_defaults.json"),
            r#"{"reference_assets":["references/brand-style.png"]}"#,
        )
        .unwrap();

        let request = VisualSupportProposalRequest {
            selection_text: "This quote should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote should land harder".into()),
            anchor_timeline_start_s: None,
            artifact_type: Some("quote_highlight".into()),
            broll_asset: None,
            duration_s: Some(3.0),
            reference_assets: vec![],
        };

        // Without a project root the saved default is not applied.
        let without = visual_support_proposal_edls(request.clone(), None).unwrap();
        assert!(
            !without[0].edl_text.contains("references/brand-style.png"),
            "baseline should not carry the saved reference default"
        );

        // With the project root, the saved reference default flows into the
        // planned proposal just like the MCP `run` wrapper.
        let with = visual_support_proposal_edls(request, Some(dir.path())).unwrap();
        assert!(
            with[0].edl_text.contains("references/brand-style.png"),
            "desktop proposal should apply saved project defaults: {}",
            with[0].edl_text
        );
    }

    #[test]
    fn adjust_trim_end_writes_to_op_in_place() {
        let mut env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet { text: "a".into() },
                start: None,
                end: Some(2.0),
            }],
        };
        let adj = EditAdjustment {
            op_index: 0,
            field: AdjustField::TrimEnd,
            value_s: 3.5,
        };
        apply_adjustment_to_envelope(&mut env, &adj).unwrap();
        match &env.ops[0] {
            EdlOp::TrimClip { end, .. } => {
                assert_eq!(*end, Some(3.5));
            }
            _ => panic!("expected TrimClip"),
        }
    }

    #[test]
    fn adjust_split_at_writes_to_op_in_place() {
        let mut env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                at_s: 1.0,
                snap: None,
            }],
        };
        let adj = EditAdjustment {
            op_index: 0,
            field: AdjustField::SplitAt,
            value_s: 7.5,
        };
        apply_adjustment_to_envelope(&mut env, &adj).unwrap();
        match &env.ops[0] {
            EdlOp::SplitClip { at_s, .. } => {
                assert!((*at_s - 7.5).abs() < 1e-9);
            }
            _ => panic!("expected SplitClip"),
        }
    }

    #[test]
    fn adjust_field_op_mismatch_errors() {
        let mut env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
            }],
        };
        let adj = EditAdjustment {
            op_index: 0,
            field: AdjustField::TrimStart,
            value_s: 1.0,
        };
        let err = apply_adjustment_to_envelope(&mut env, &adj).unwrap_err();
        assert!(err.contains("DeleteClip"), "got: {err}");
    }

    #[test]
    fn adjust_op_index_out_of_range_errors() {
        let mut env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet { text: "a".into() },
                start: None,
                end: Some(2.0),
            }],
        };
        let adj = EditAdjustment {
            op_index: 5,
            field: AdjustField::TrimEnd,
            value_s: 1.0,
        };
        let err = apply_adjustment_to_envelope(&mut env, &adj).unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn trim_diff_hint_uses_applied_locator_for_handles() {
        let mut original = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        original.tracks.children.push(StackChild::Track(track));

        let mut proposed = original.clone();
        let StackChild::Track(track) = &mut proposed.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &mut track.children[0] else {
            panic!()
        };
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));

        let envelope = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                start: None,
                end: Some(5.0),
            }],
        };
        let applied = vec![AppliedOp {
            index: 0,
            description: "trimmed".into(),
            locator: Some(ClipLocator {
                track_index: 0,
                child_index: 0,
            }),
            metadata: applied_metadata("trim_clip"),
        }];

        let hints = build_diff_hints(&envelope, &applied, &original, &proposed);
        assert!(matches!(
            hints.as_slice(),
            [AppliedDiff::TrimEdge {
                op_index: 0,
                track_index: 0,
                item_index: 0,
                side: Side::Right,
                delta_s
            }] if (*delta_s - 5.0).abs() < 1e-9
        ));
    }

    #[test]
    fn broll_diff_hint_highlights_inserted_broll_clip() {
        let original = one_track_timeline(vec![clip("clip-0", "u0", 0.0, 5.0)]);
        let mut proposed = original.clone();
        let mut overlay = Track::empty("V2", TrackKind::Video);
        overlay.children.push(TrackChild::Clip(clip(
            "broll-from-clip-0",
            "broll-u0",
            0.0,
            2.0,
        )));
        proposed.tracks.children.push(StackChild::Track(overlay));
        let envelope = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::ClipUuid { uuid: "u0".into() },
                asset: "raw/broll.mp4".into(),
                duration_s: 2.0,
                position: awidat_core::edl::BRollPosition::Overlay,
            }],
        };
        let applied = vec![applied_at(0, 0)];

        let hints = build_diff_hints(&envelope, &applied, &original, &proposed);
        assert!(matches!(
            hints.as_slice(),
            [AppliedDiff::InsertBRoll {
                op_index: 0,
                track_index: 1,
                item_index: 0
            }]
        ));
    }

    #[test]
    fn pip_diff_hint_highlights_inserted_pip_clip() {
        let original = one_track_timeline(vec![clip("clip-0", "u0", 0.0, 5.0)]);
        let mut proposed = original.clone();
        let mut overlay = Track::empty("V2", TrackKind::Video);
        overlay.children.push(TrackChild::Clip(clip(
            "pip-from-clip-0",
            "pip-u0",
            0.0,
            2.0,
        )));
        proposed.tracks.children.push(StackChild::Track(overlay));
        let envelope = EdlEnvelope {
            ops: vec![EdlOp::InsertPiP {
                anchor: Anchor::ClipUuid { uuid: "u0".into() },
                asset: "raw/pip.mp4".into(),
                duration_s: 2.0,
                source_start_s: 0.0,
                corner: awidat_core::edl::PiPCorner::BottomRight,
                scale: 0.28,
                margin_pct: 0.035,
            }],
        };
        let applied = vec![applied_at(0, 0)];

        let hints = build_diff_hints(&envelope, &applied, &original, &proposed);
        assert!(matches!(
            hints.as_slice(),
            [AppliedDiff::InsertPiP {
                op_index: 0,
                track_index: 1,
                item_index: 0
            }]
        ));
    }

    #[test]
    fn move_diff_hint_carries_before_and_after_indexes() {
        let original = one_track_timeline(vec![
            clip("clip-0", "u0", 0.0, 5.0),
            clip("clip-1", "u1", 0.0, 5.0),
        ]);
        let proposed = one_track_timeline(vec![
            clip("clip-1", "u1", 0.0, 5.0),
            clip("clip-0", "u0", 0.0, 5.0),
        ]);
        let envelope = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::ClipUuid { uuid: "u0".into() },
                to_position: 1,
                at_s: None,
                snap: None,
            }],
        };
        let applied = vec![applied_at(0, 0)];

        let hints = build_diff_hints(&envelope, &applied, &original, &proposed);
        assert!(matches!(
            hints.as_slice(),
            [AppliedDiff::Move {
                op_index: 0,
                from_track_index: 0,
                from_item_index: 0,
                to_track_index: 0,
                to_item_index: 1
            }]
        ));
    }

    fn one_track_timeline(children: Vec<Clip>) -> Timeline {
        let mut timeline = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .extend(children.into_iter().map(TrackChild::Clip));
        timeline.tracks.children.push(StackChild::Track(track));
        timeline
    }

    fn clip(name: &str, uuid: &str, start: f64, duration: f64) -> Clip {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(start * 24.0, 24.0),
            RationalTime::new(duration * 24.0, 24.0),
        ));
        clip.metadata
            .awidat
            .get_or_insert_with(Default::default)
            .extra
            .insert("clip_uuid".into(), serde_json::Value::String(uuid.into()));
        clip
    }

    fn applied_at(track_index: usize, child_index: usize) -> AppliedOp {
        AppliedOp {
            index: 0,
            description: "applied".into(),
            locator: Some(ClipLocator {
                track_index,
                child_index,
            }),
            metadata: applied_metadata("test_op"),
        }
    }

    fn applied_metadata(kind: &str) -> AppliedOpMetadata {
        AppliedOpMetadata {
            kind: kind.into(),
            affected_clip_ids: Vec::new(),
            affected_track_ids: Vec::new(),
            source: None,
            parameters: Default::default(),
        }
    }
}
