//! `podcast_smooth_cut_boundaries` — inspect cut boundaries after apply.
//! Ported from `crates/core/src/tools/podcast_smooth_cut_boundaries.rs`
//! to the in-process MCP server.
//!
//! This is the post-apply bridge from "cuts landed" to "cuts read well".
//! It locates the adjacent clips created by accepted podcast edits and returns
//! the exact follow-up tool calls needed to check continuity, audio handoff,
//! visual jump risk, and transition context.

use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_smooth_cut_boundaries`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastSmoothCutBoundariesArgs {
    /// The batched_edits array returned by
    /// `podcast_apply_accepted_edits`.
    pub applied_edits: Vec<AppliedEdit>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AppliedEdit {
    pub id: String,
    pub asset_id: String,
    pub source_start_s: f64,
    pub source_end_s: f64,
}

#[derive(Debug, Serialize)]
struct BoundaryCheck {
    edit_id: String,
    track: String,
    at_s: f64,
    from_clip_uuid: String,
    to_clip_uuid: String,
    deleted_source_range_s: [f64; 2],
    assess_edit_quality_tool_call: serde_json::Value,
    transition_context_tool_call: serde_json::Value,
    smoothing_policy: &'static str,
}

/// Run `podcast_smooth_cut_boundaries` against the project resolved
/// from [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: PodcastSmoothCutBoundariesArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.applied_edits.is_empty() {
        return Err(
            "podcast_smooth_cut_boundaries: applied_edits must contain at least one edit.".into(),
        );
    }

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_smooth_cut_boundaries: failed to read project: {e}"))?;
    let boundaries = find_boundaries(&project.timeline, &args.applied_edits);
    let status = if boundaries.is_empty() {
        "needs_timeline_review"
    } else {
        "checks_required"
    };
    let body = serde_json::json!({
        "status": status,
        "summary_for_agent": format!(
            "Boundary smoothing status: {status}. {} changed cut boundary/boundaries found. Run assess_edit_quality for every boundary before final render.",
            boundaries.len()
        ),
        "boundaries": boundaries,
        "required_follow_up_tools": [
            "assess_edit_quality",
            "transition_context",
            "plan_transition",
            "apply_edl",
            "vedit_diff"
        ],
        "agent_instructions": [
            "Run each assess_edit_quality_tool_call before render, even for cuts originally marked safe.",
            "If assess_edit_quality recommends recut, do not hide the issue with a transition; adjust the cut boundary.",
            "If it recommends J-cut or L-cut, apply the audio lead/trail EDL guidance and review by ear.",
            "If it recommends b-roll or a visible transition, run transition_context; use plan_transition only when a visible transition has a clear editorial job and safe handles.",
            "After any smoothing EDL, run vedit_diff again."
        ]
    });
    serde_json::to_string(&body)
        .map_err(|e| format!("podcast_smooth_cut_boundaries serialize: {e}"))
}

fn find_boundaries(timeline: &Timeline, edits: &[AppliedEdit]) -> Vec<BoundaryCheck> {
    let mut boundaries = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0_f64;
        for index in 0..track.children.len().saturating_sub(1) {
            let from_duration_s = child_duration_s(&track.children[index]);
            let at_s = cursor_s + from_duration_s;
            let Some((from_uuid, from_asset, from_source_end_s)) =
                clip_boundary_end(&track.children[index])
            else {
                cursor_s += from_duration_s;
                continue;
            };
            let Some((to_uuid, to_asset, to_source_start_s)) =
                clip_boundary_start(&track.children[index + 1])
            else {
                cursor_s += from_duration_s;
                continue;
            };
            for edit in edits {
                if from_asset == edit.asset_id
                    && to_asset == edit.asset_id
                    && approx_eq(from_source_end_s, edit.source_start_s)
                    && approx_eq(to_source_start_s, edit.source_end_s)
                {
                    boundaries.push(BoundaryCheck {
                        edit_id: edit.id.clone(),
                        track: track.name.clone(),
                        at_s: round3(at_s),
                        from_clip_uuid: from_uuid.clone(),
                        to_clip_uuid: to_uuid.clone(),
                        deleted_source_range_s: [
                            round3(edit.source_start_s),
                            round3(edit.source_end_s),
                        ],
                        assess_edit_quality_tool_call: serde_json::json!({
                            "name": "assess_edit_quality",
                            "args": {
                                "at_s": round3(at_s),
                                "kind": "cut",
                                "objective": "tighten",
                                "asset_id": edit.asset_id
                            }
                        }),
                        transition_context_tool_call: serde_json::json!({
                            "name": "transition_context",
                            "args": {
                                "between": {
                                    "from": {"clip_uuid": from_uuid},
                                    "to": {"clip_uuid": to_uuid}
                                },
                                "window_s": 6.0
                            }
                        }),
                        smoothing_policy: "hard_cut_first_recut_or_split_edit_before_visible_transition",
                    });
                }
            }
            cursor_s += from_duration_s;
        }
    }
    boundaries
}

fn clip_boundary_end(child: &TrackChild) -> Option<(String, String, f64)> {
    let TrackChild::Clip(clip) = child else {
        return None;
    };
    let MediaReference::External(reference) = &clip.media_reference else {
        return None;
    };
    let range = clip.source_range.as_ref()?;
    Some((
        clip_uuid(clip),
        reference.target_url.clone(),
        range.start_time.to_seconds() + range.duration.to_seconds(),
    ))
}

fn clip_boundary_start(child: &TrackChild) -> Option<(String, String, f64)> {
    let TrackChild::Clip(clip) = child else {
        return None;
    };
    let MediaReference::External(reference) = &clip.media_reference else {
        return None;
    };
    let range = clip.source_range.as_ref()?;
    Some((
        clip_uuid(clip),
        reference.target_url.clone(),
        range.start_time.to_seconds(),
    ))
}

fn child_duration_s(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(clip) => clip
            .source_range
            .as_ref()
            .map(|range| range.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        TrackChild::Transition(transition) => {
            transition.in_offset.to_seconds() + transition.out_offset.to_seconds()
        }
        TrackChild::Stack(_) => 0.0,
    }
}

fn clip_uuid(clip: &awidat_proto::otio::Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|meta| meta.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
        .unwrap_or(clip.name.as_str())
        .to_string()
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.05
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Locate adjacent clip boundaries created by accepted podcast cuts and return \
the required smoothing checks. This tool is read-only. Run it after \
podcast_apply_accepted_edits -> apply_edl -> view_timeline -> vedit_diff, \
then run assess_edit_quality for each returned boundary. Use transition_context \
and plan_transition only when the quality assessment indicates a visual repair \
or motivated transition may be needed.\
";
