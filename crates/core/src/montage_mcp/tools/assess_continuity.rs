//! `assess_continuity` — read-only continuity assessor.
//!
//! Ported from `crates/core/src/tools/assess_continuity.rs` to the
//! in-process MCP server. The agent calls this BEFORE proposing a
//! trim/cut/split to learn whether the cut would jar. Returns a
//! per-rule breakdown plus an aggregate verdict (`clean` / `risky`
//! / `dirty` / `abstain`).

use std::path::Path;

use montage_proto::otio::{MediaReference, StackChild, TrackChild};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::continuity::{self, ContinuityInputs, CutKind, assess_continuity as run_assess};
use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `assess_continuity`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct AssessContinuityArgs {
    /// Timeline-time seconds where the cut would land.
    pub at_s: f64,
    /// What kind of cut: `cut` (split), `trim_in` (start edge),
    /// or `trim_out` (end edge).
    pub kind: String,
    /// Asset id to assess against (e.g. `raw/ep.mp4`). The
    /// engine reads sidecars keyed by this. Optional — when
    /// omitted, the tool walks the timeline at `at_s` and picks
    /// the asset whose source range covers that time.
    #[serde(default)]
    pub asset_id: Option<String>,
}

/// Run `assess_continuity` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument
/// or project-read errors return `Err(String)`.
pub fn run(args: AssessContinuityArgs, ctx: McpToolCtx) -> Result<String, String> {
    let kind = match args.kind.as_str() {
        "cut" => CutKind::Cut,
        "trim_in" => CutKind::TrimIn,
        "trim_out" => CutKind::TrimOut,
        other => {
            return Err(format!(
                "assess_continuity: unknown kind {other:?}. \
                 Use cut / trim_in / trim_out."
            ));
        }
    };

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("assess_continuity: failed to read project: {e}"))?;

    // Resolve asset_id: either passed by the agent or derived
    // from the timeline by walking video tracks at `at_s`.
    let resolved_asset_id = if let Some(a) = args.asset_id.clone() {
        Some((a, args.at_s, args.at_s))
    } else {
        resolve_asset_at(&project.timeline, args.at_s)
    };

    let Some((asset_id, source_at_s, _)) = resolved_asset_id else {
        return Err(format!(
            "assess_continuity: no asset on the timeline at {} \
             — the cut point doesn't land on any video clip.",
            args.at_s
        ));
    };

    // build_inputs collects nearby_cuts_s in track-time (the
    // rhythm rule's coord space), so pass the timeline at_s
    // here — NOT source_at_s.
    let inputs = build_inputs(&ctx.project_root, &project.timeline, &asset_id, args.at_s);

    // Engine takes both coords: source-time for whisper /
    // silence / motion / speaker rules; track-time for the
    // rhythm rule (which compares against nearby_cuts_s,
    // populated in track-time by build_inputs).
    let verdict = run_assess(source_at_s, args.at_s, kind, &inputs);

    let body = serde_json::json!({
        "at_s": args.at_s,
        "kind": args.kind,
        "asset_id": asset_id,
        "source_at_s": source_at_s,
        "verdict": verdict.verdict,
        "rules": verdict.rules,
    });
    Ok(body.to_string())
}

/// Walk the timeline's video tracks; for each clip, accumulate
/// timeline-time and figure out which clip's source range covers
/// the requested timeline `at_s`. Returns `(asset_id, source_at_s,
/// source_clip_end_s)`. Source-time = clip's source_range.start +
/// (timeline_at - clip's track_start).
pub fn resolve_asset_at(
    timeline: &montage_proto::otio::Timeline,
    at_s: f64,
) -> Option<(String, f64, f64)> {
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0_f64;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        if let Some(r) = clip.source_range.as_ref() {
                            cursor_s += r.duration.to_seconds();
                        }
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        continue;
                    };
                    let dur = range.duration.to_seconds();
                    let track_end = cursor_s + dur;
                    if at_s >= cursor_s && at_s <= track_end {
                        let source_start = range.start_time.to_seconds();
                        let source_at = source_start + (at_s - cursor_s);
                        let source_end = source_start + dur;
                        return Some((ext.target_url.clone(), source_at, source_end));
                    }
                    cursor_s += dur;
                }
                TrackChild::Gap(g) => {
                    cursor_s += g.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    None
}

/// Assemble [`ContinuityInputs`] for the engine: load all the
/// sidecars + pre-filter nearby cuts. Each loader tolerates
/// missing files — the rule that needs that input will abstain.
pub fn build_inputs(
    project_root: &Path,
    timeline: &montage_proto::otio::Timeline,
    asset_id: &str,
    at_s: f64,
) -> ContinuityInputs {
    let nearby_cuts_s = collect_nearby_cuts(timeline, at_s);
    ContinuityInputs {
        whisper_words: continuity::load_whisper_words(project_root, asset_id),
        whisper_segments: continuity::load_whisper_segments(project_root, asset_id),
        motion_magnitudes: continuity::load_motion_magnitudes(project_root, asset_id),
        scene_changes_s: continuity::load_scene_changes(project_root, asset_id),
        silences: continuity::load_silences(project_root, asset_id),
        nearby_cuts_s,
    }
}

/// Walk the timeline and collect any clip-boundary times within
/// ±5 seconds of `at_s`. Each clip's start (other than 0) is a
/// cut point on the track. Used by the rhythm rule.
pub fn collect_nearby_cuts(timeline: &montage_proto::otio::Timeline, at_s: f64) -> Vec<f64> {
    let mut out = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0_f64;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    if cursor_s > 0.0 && (cursor_s - at_s).abs() <= 5.0 {
                        out.push(cursor_s);
                    }
                    if let Some(r) = clip.source_range.as_ref() {
                        cursor_s += r.duration.to_seconds();
                    }
                }
                TrackChild::Gap(g) => {
                    cursor_s += g.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    out
}

pub const DESCRIPTION: &str = "\
Evaluate whether a proposed cut/trim/split would jar the viewer. \
Reads whisper / silence / motion / scenedetect sidecars and runs \
five rules: mid-sentence, breath-beat preservation, mid-motion, \
speaker-turn boundary, rhythm preservation. Returns a per-rule \
breakdown plus an aggregate verdict.\
\n\nVerdicts:\
\n- `clean`: every concrete rule passed. Propose the raw cut.\
\n- `risky`: at least one rule flagged the cut. Surface as a Note \
or bundle a 0.3s cross-dissolve.\
\n- `dirty`: at least one rule is confident the cut would jar. \
Don't propose the raw cut — bundle a transition or b-roll cover.\
\n- `abstain`: no rule had input data (sidecars missing). Tell the \
user the project may need indexing.\
\n\nArgs: at_s (timeline-time seconds), kind (`cut`/`trim_in`/\
`trim_out`), asset_id (optional — auto-resolved from the timeline). \
Read-only — proposing the actual edit is a separate step (apply_edl).\
\n\nWhen verdict == dirty, the rules array carries reasons the \
agent can quote in the proposal description or in an EditorialNote \
of kind continuity_warning.\
";
