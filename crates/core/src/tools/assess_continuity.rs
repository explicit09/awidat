//! `assess_continuity` agent tool — Phase 2.
//!
//! The agent calls this BEFORE proposing a trim/cut/split to learn
//! whether the cut would jar. Returns a per-rule breakdown plus an
//! aggregate verdict (`clean` / `risky` / `dirty` / `abstain`) so
//! the agent can decide whether to:
//!
//!   - Propose the raw cut (clean).
//!   - Bundle a 0.3s cross-dissolve or surface a Note (risky / dirty).
//!   - Note the missing input + ask the user how to proceed (abstain).
//!
//! The tool is read-only: it doesn't write proposals or notes —
//! that's the agent's job after reading the verdict. Phase 2.4 ships
//! the bundling helper the agent calls when verdict == dirty.

use std::path::Path;

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, TrackChild};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::continuity::{self, ContinuityInputs, CutKind, assess_continuity as run_assess};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Tool that scores whether a proposed cut risks visual/audio continuity.
pub struct AssessContinuityTool;

#[derive(Debug, Deserialize)]
struct AssessArgs {
    /// Timeline-time seconds where the cut would land.
    at_s: f64,
    /// What kind of cut: `cut` (split), `trim_in` (start edge),
    /// or `trim_out` (end edge).
    kind: String,
    /// Asset id to assess against (e.g. `raw/ep.mp4`). The
    /// engine reads sidecars keyed by this. Optional — when
    /// omitted, the tool walks the timeline at `at_s` and picks
    /// the asset whose source range covers that time.
    #[serde(default)]
    asset_id: Option<String>,
}

#[async_trait]
impl ToolHandler for AssessContinuityTool {
    fn name(&self) -> &'static str {
        "assess_continuity"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "assess_continuity".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "at_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Timeline-time seconds where the cut would land."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["cut", "trim_in", "trim_out"],
                        "description": "Which kind of cut: split, trim leading edge, or trim trailing edge."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Optional asset id (e.g. raw/ep.mp4). When omitted, the tool resolves it from the timeline."
                    }
                },
                "required": ["at_s", "kind"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: AssessArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "assess_continuity: invalid args ({e}). Required: at_s, kind."
            ))
        })?;

        let kind = match args.kind.as_str() {
            "cut" => CutKind::Cut,
            "trim_in" => CutKind::TrimIn,
            "trim_out" => CutKind::TrimOut,
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "assess_continuity: unknown kind {other:?}. \
                     Use cut / trim_in / trim_out."
                )));
            }
        };

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "assess_continuity: failed to read project: {e}"
            ))
        })?;

        // Resolve asset_id: either passed by the agent or derived
        // from the timeline by walking video tracks at `at_s`.
        let resolved_asset_id = if let Some(a) = args.asset_id.clone() {
            Some((a, args.at_s, args.at_s))
        } else {
            resolve_asset_at(&project.timeline, args.at_s)
        };

        let Some((asset_id, source_at_s, _)) = resolved_asset_id else {
            return Err(FunctionCallError::RespondToModel(format!(
                "assess_continuity: no asset on the timeline at {} \
                 — the cut point doesn't land on any video clip.",
                args.at_s
            )));
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
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Walk the timeline's video tracks; for each clip, accumulate
/// timeline-time and figure out which clip's source range covers
/// the requested timeline `at_s`. Returns `(asset_id, source_at_s,
/// source_clip_end_s)`. Source-time = clip's source_range.start +
/// (timeline_at - clip's track_start).
pub(crate) fn resolve_asset_at(
    timeline: &awidat_proto::otio::Timeline,
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
pub(crate) fn build_inputs(
    project_root: &Path,
    timeline: &awidat_proto::otio::Timeline,
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
pub(crate) fn collect_nearby_cuts(timeline: &awidat_proto::otio::Timeline, at_s: f64) -> Vec<f64> {
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

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };

    fn timeline_with_clip(
        asset_id: &str,
        source_start_s: f64,
        duration_s: f64,
    ) -> awidat_proto::otio::Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = awidat_proto::otio::Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        tl
    }

    #[test]
    fn resolve_asset_at_finds_clip_covering_time() {
        let tl = timeline_with_clip("raw/ep.mp4", 5.0, 10.0);
        // Timeline 0..10 maps to source 5..15.
        let r = resolve_asset_at(&tl, 3.0).unwrap();
        assert_eq!(r.0, "raw/ep.mp4");
        assert!((r.1 - 8.0).abs() < 1e-9); // source = 5 + 3 = 8
    }

    #[test]
    fn resolve_asset_at_returns_none_past_end() {
        let tl = timeline_with_clip("raw/ep.mp4", 0.0, 5.0);
        assert!(resolve_asset_at(&tl, 100.0).is_none());
    }

    #[test]
    fn collect_nearby_cuts_finds_clip_starts_within_5s() {
        // Three clips: starts at 0, 5, 8.
        let mut tl = awidat_proto::otio::Timeline::empty("p");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, dur) in [(0, 5.0), (1, 3.0), (2, 4.0)].iter() {
            let mut c = Clip::empty(format!("c{i}"));
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(dur * 24.0, 24.0),
            ));
            track.children.push(TrackChild::Clip(c));
        }
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        // at_s=6.0 → cuts at 5 (within 1s) and 8 (within 2s).
        let cuts = collect_nearby_cuts(&tl, 6.0);
        assert_eq!(cuts.len(), 2);
        assert!(cuts.contains(&5.0));
        assert!(cuts.contains(&8.0));
    }
}
