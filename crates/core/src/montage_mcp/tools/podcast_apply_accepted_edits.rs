//! `podcast_apply_accepted_edits` — compile accepted proposal IDs to EDL.
//! Ported from `crates/core/src/tools/podcast_apply_accepted_edits.rs`.
//!
//! This tool is the deterministic bridge from proposal review to mutation. It
//! does not write the timeline itself; it validates accepted proposal IDs,
//! enforces quality evidence for review/risky items, orders cuts end-to-start,
//! and returns the exact `apply_edl` call the agent must run next.

use std::collections::{HashMap, HashSet};

use montage_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `podcast_apply_accepted_edits`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastApplyAcceptedEditsArgs {
    /// Proposal item IDs accepted by the user/agent.
    pub accepted_ids: Vec<String>,
    /// Required for review/risky items: proof that
    /// `assess_edit_quality` was run.
    #[serde(default)]
    pub quality_evidence: Vec<QualityEvidence>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct QualityEvidence {
    pub item_id: String,
    pub tool: String,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    kind: &'static str,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    risk: &'static str,
    requires_quality: bool,
    evidence: String,
}

#[derive(Debug, Clone)]
struct ClipTarget {
    asset_id: String,
    anchor: String,
    source_start_s: f64,
    source_end_s: f64,
}

pub fn run(args: PodcastApplyAcceptedEditsArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.accepted_ids.is_empty() {
        return Err(
            "podcast_apply_accepted_edits: accepted_ids must contain at least one proposal item id.".into(),
        );
    }

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_apply_accepted_edits: failed to read project: {e}"))?;
    let candidates = collect_candidates(&ctx.project_root, &project.timeline);
    let by_id: HashMap<_, _> = candidates
        .into_iter()
        .map(|candidate| (candidate.id.clone(), candidate))
        .collect();

    let accepted: Vec<_> = args
        .accepted_ids
        .iter()
        .map(|id| {
            by_id.get(id).cloned().ok_or_else(|| {
                format!(
                    "podcast_apply_accepted_edits: unknown proposal item id {id:?}. Re-run podcast_edit_proposal and pass an id it returned."
                )
            })
        })
        .collect::<Result<_, _>>()?;

    let quality_ids: HashSet<_> = args
        .quality_evidence
        .iter()
        .filter(|evidence| evidence.tool == "assess_edit_quality")
        .map(|evidence| evidence.item_id.as_str())
        .collect();
    let missing_quality: Vec<_> = accepted
        .iter()
        .filter(|candidate| {
            candidate.requires_quality && !quality_ids.contains(candidate.id.as_str())
        })
        .map(|candidate| candidate.id.as_str())
        .collect();
    if !missing_quality.is_empty() {
        return Err(format!(
            "podcast_apply_accepted_edits: proposal item(s) {} require assess_edit_quality evidence before batching.",
            missing_quality.join(", ")
        ));
    }

    let clip_targets = collect_clip_targets(&project.timeline);
    let mut accepted = accepted;
    accepted.sort_by(|a, b| {
        b.timeline_start_s
            .partial_cmp(&a.timeline_start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut edl = String::from("*** Begin EDL\n");
    let mut batched = Vec::new();
    for candidate in &accepted {
        let target = find_clip_target(&clip_targets, candidate).ok_or_else(|| {
            format!(
                "podcast_apply_accepted_edits: could not map proposal item {} to a current timeline clip. Re-run view_timeline and podcast_edit_proposal.",
                candidate.id
            )
        })?;
        let right_anchor = format!("{}-b", target.anchor);
        edl.push_str("*** Split Clip\n");
        edl.push_str(&format!("@@ anchor: clip_uuid={}\n", target.anchor));
        edl.push_str(&format!("+ at_s: {:.3}\n", candidate.source_start_s));
        edl.push_str("*** Split Clip\n");
        edl.push_str(&format!("@@ anchor: clip_uuid={right_anchor}\n"));
        edl.push_str(&format!("+ at_s: {:.3}\n", candidate.source_end_s));
        edl.push_str("*** Ripple Delete\n");
        edl.push_str(&format!("@@ anchor: clip_uuid={right_anchor}\n"));
        batched.push(serde_json::json!({
            "id": candidate.id,
            "kind": candidate.kind,
            "risk": candidate.risk,
            "asset_id": candidate.asset_id,
            "source_start_s": candidate.source_start_s,
            "source_end_s": candidate.source_end_s,
            "timeline_start_s": candidate.timeline_start_s,
            "timeline_end_s": candidate.timeline_end_s,
            "anchor": target.anchor,
            "evidence": candidate.evidence,
        }));
    }
    edl.push_str("*** End EDL\n");

    let quality_evidence: Vec<_> = args
        .quality_evidence
        .iter()
        .map(|evidence| {
            serde_json::json!({
                "item_id": evidence.item_id,
                "tool": evidence.tool,
                "summary": evidence.summary,
            })
        })
        .collect();
    let body = serde_json::json!({
        "status": "ready_to_apply",
        "summary_for_agent": format!(
            "Prepared {} accepted podcast edit(s) as one end-to-start apply_edl batch. Run apply_edl next, then view_timeline, vedit_diff, podcast_smooth_cut_boundaries, and podcast_post_draft_check.",
            accepted.len()
        ),
        "batched_edits": batched,
        "quality_evidence": quality_evidence,
        "apply_edl_tool_call": {
            "name": "apply_edl",
            "args": {
                "edl": edl,
                "dry_run": false,
                "reasoning": "Applying user-approved podcast proposal items as a controlled batch; follow with view_timeline, vedit_diff, podcast_smooth_cut_boundaries, and podcast_post_draft_check."
            }
        },
        "smoothing_tool_call": {
            "name": "podcast_smooth_cut_boundaries",
            "args": {
                "applied_edits": batched
            }
        },
        "required_follow_up_tools": [
            "apply_edl",
            "view_timeline",
            "vedit_diff",
            "podcast_smooth_cut_boundaries",
            "podcast_post_draft_check"
        ],
    });
    serde_json::to_string(&body).map_err(|e| format!("podcast_apply_accepted_edits serialize: {e}"))
}

fn collect_candidates(project_root: &std::path::Path, timeline: &Timeline) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (index, finding) in
        crate::tools::find_dead_air::scan_dead_air(project_root, timeline, 1.2, 200)
            .into_iter()
            .enumerate()
    {
        let requires_quality = finding.duration_s < 2.0;
        candidates.push(Candidate {
            id: format!("dead-air-{}", index + 1),
            kind: "dead_air",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: if finding.duration_s >= 2.0 {
                "low"
            } else {
                "medium"
            },
            requires_quality,
            evidence: format!(
                "{:.2}s silence; before={:?}; after={:?}",
                finding.duration_s, finding.transcript_before, finding.transcript_after
            ),
        });
    }

    let fillers = crate::transcript_cleanup::default_filler_tokens(false);
    for (index, finding) in
        crate::tools::find_filler_words::scan_filler_words(project_root, timeline, &fillers, 200)
            .into_iter()
            .enumerate()
    {
        candidates.push(Candidate {
            id: format!("filler-{}", index + 1),
            kind: "filler_word",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: "medium",
            requires_quality: true,
            evidence: format!("matched filler token {:?}", finding.text),
        });
    }

    for (index, finding) in
        crate::tools::find_false_starts::scan_false_starts(project_root, timeline, 200)
            .into_iter()
            .enumerate()
    {
        candidates.push(Candidate {
            id: format!("false-start-{}", index + 1),
            kind: "false_start",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: "medium",
            requires_quality: true,
            evidence: format!(
                "restart marker {:?}; snippet={:?}",
                finding.marker, finding.snippet
            ),
        });
    }
    candidates
}

fn collect_clip_targets(timeline: &Timeline) -> Vec<ClipTarget> {
    let mut targets = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for child in &track.children {
            let TrackChild::Clip(clip) = child else {
                continue;
            };
            let MediaReference::External(ext) = &clip.media_reference else {
                continue;
            };
            let Some(range) = clip.source_range.as_ref() else {
                continue;
            };
            let source_start_s = range.start_time.to_seconds();
            let source_end_s = source_start_s + range.duration.to_seconds();
            let anchor = clip
                .metadata
                .montage
                .as_ref()
                .and_then(|meta| meta.extra.get("clip_uuid"))
                .and_then(|value| value.as_str())
                .unwrap_or(clip.name.as_str())
                .to_string();
            targets.push(ClipTarget {
                asset_id: ext.target_url.clone(),
                anchor,
                source_start_s,
                source_end_s,
            });
        }
    }
    targets
}

fn find_clip_target<'a>(
    targets: &'a [ClipTarget],
    candidate: &Candidate,
) -> Option<&'a ClipTarget> {
    targets.iter().find(|target| {
        candidate.asset_id == target.asset_id
            && candidate.source_start_s > target.source_start_s
            && candidate.source_end_s < target.source_end_s
    })
}

pub const DESCRIPTION: &str =
    "Compile accepted podcast proposal item IDs into one ordered apply_edl batch.";
