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

use awidat_core::edl::{ApplyError, EdlEnvelope, EdlOp, apply, parse};
use awidat_core::tool::ApprovalDecision;
use awidat_desktop_protocol::{
    AdjustField, AppliedDiff, EditAdjustment, Id, Item, ItemLifecycle, ProposalSource, Side,
    TimelineSnapshot,
};
use awidat_proto::otio::Timeline;
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
/// The id is the proposal's stable identifier — re-used across
/// adjustment Deltas. Frontend keys its rendering on it.
pub async fn build_proposal(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    id: String,
    edl_text: String,
    project_root: &Path,
    source: ProposalSource,
    reply: Option<tokio::sync::oneshot::Sender<ApprovalDecision>>,
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
    let summary = summarize_envelope(&envelope);
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

    // Decide whether the desktop writes to disk. Agent-initiated +
    // unchanged means the agent's handler will do the write itself
    // (after we send Allow); writing here would cause a double-apply.
    let desktop_writes = !agent_initiated || user_adjusted;

    if desktop_writes {
        let project_root = proposal.project_root.clone();
        let proposed_timeline = proposal.proposed_timeline.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut project =
                Project::read(&project_root).map_err(|e| format!("project read: {e}"))?;
            project.timeline = proposed_timeline;
            project
                .write(&project_root)
                .map_err(|e| format!("project write: {e}"))
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
                tracks: Vec::new(),
            },
            diff_hints: Vec::new(),
            summary: "rejected".into(),
            revision: proposal.revision,
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
    // We hold the proposal mutex across an apply() call so a second
    // adjust_proposal racing against this one serializes naturally.
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
    let summary = summarize_envelope(&proposal.envelope);
    let edl_text = String::new(); // EDL text could be re-serialized; not needed for Delta

    drop(map); // release before emit so the frontend's response can grab it again

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
                // Find the named track in the proposed snapshot. Use
                // the explicit inserted name when present; otherwise
                // use the requested insertion index clamped to the
                // original track length. This keeps middle inserts
                // highlighted where they actually landed.
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
            // F2 ops; not rendered in v1.
            EdlOp::InsertBRoll { .. } | EdlOp::MoveClip { .. } | EdlOp::InsertTransition { .. } => {
            }
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
        EdlOp::MoveClip { .. } => "MoveClip",
        EdlOp::InsertTransition { .. } => "InsertTransition",
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
    use awidat_core::edl::{Anchor, AppliedOp, ClipLocator, EdlOp};
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
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
}
