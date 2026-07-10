//! `transition_context` — read-only transition boundary context.
//! Ported from `crates/core/src/tools/transition_context.rs` to the
//! in-process MCP server. Builds a deterministic packet for a
//! candidate transition boundary.

use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, StackChild, Timeline, TrackChild,
};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::Path;

use crate::continuity::{
    ContinuityInputs, CutKind, WhisperSegment, WhisperWord, assess_continuity,
    load_whisper_segments,
};
use crate::montage_mcp::context::McpToolCtx;
use crate::montage_mcp::tools::assess_continuity::build_inputs;
use crate::visual_signals::{BoundaryVisualSignals, SideSignals, load_boundary_signals};

/// Arguments to `transition_context`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TransitionContextArgs {
    /// The pair of adjacent clips bracketing the boundary.
    pub between: TransitionBetweenArgs,
    /// Transcript/frame context window on each side. Default 6 seconds.
    #[serde(default)]
    pub window_s: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TransitionBetweenArgs {
    pub from: ClipRefArgs,
    pub to: ClipRefArgs,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ClipRefArgs {
    pub clip_uuid: String,
}

/// Run `transition_context` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; project
/// loading or boundary lookup errors return `Err(String)`.
pub fn run(args: TransitionContextArgs, ctx: McpToolCtx) -> Result<String, String> {
    let window_s = args.window_s.unwrap_or(6.0).max(0.1);

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("transition_context: failed to read project: {e}"))?;
    let Some(boundary) = find_boundary(
        &project.timeline,
        &args.between.from.clip_uuid,
        &args.between.to.clip_uuid,
    ) else {
        return Err(format!(
            "transition_context: clips {:?} and {:?} are not adjacent on the same track.",
            args.between.from.clip_uuid, args.between.to.clip_uuid
        ));
    };

    let from_source_end_s = boundary.from.source_start_s + boundary.from.duration_s;
    let inputs = build_inputs(
        &ctx.project_root,
        &project.timeline,
        &boundary.from.asset_id,
        boundary.at_s,
    );
    let continuity = assess_continuity(from_source_end_s, boundary.at_s, CutKind::Cut, &inputs);
    let transcript_before = transcript_window(
        &ctx.project_root,
        &boundary.from.asset_id,
        from_source_end_s - window_s,
        from_source_end_s,
    );
    let transcript_after = transcript_window(
        &ctx.project_root,
        &boundary.to.asset_id,
        boundary.to.source_start_s,
        boundary.to.source_start_s + window_s,
    );
    let signals = load_boundary_signals(
        &ctx.project_root,
        &boundary.from.asset_id,
        from_source_end_s,
        &boundary.to.asset_id,
        boundary.to.source_start_s,
    );

    let body = serde_json::json!({
        "between": {
            "from": {"clip_uuid": args.between.from.clip_uuid},
            "to": {"clip_uuid": args.between.to.clip_uuid},
        },
        "boundary": {
            "track": boundary.track_name,
            "track_index": boundary.track_index,
            "from_child_index": boundary.from_child_index,
            "to_child_index": boundary.to_child_index,
            "at_s": round3(boundary.at_s),
        },
        "from": clip_packet(&boundary.from),
        "to": clip_packet(&boundary.to),
        "handles": {
            "outgoing_s": boundary.handles.outgoing_s.map(round3),
            "incoming_s": boundary.handles.incoming_s.map(round3),
            "max_centered_duration_s": boundary.handles.max_centered_duration_s.map(round3),
            "max_start_on_cut_duration_s": boundary.handles.incoming_s.map(round3),
            "max_end_on_cut_duration_s": boundary.handles.outgoing_s.map(round3),
        },
        "continuity": {
            "verdict": continuity.verdict,
            "rules": continuity.rules,
        },
        "transcript": {
            "before": transcript_before,
            "after": transcript_after,
        },
        "suggested_frame_times": {
            "before_s": round3((boundary.at_s - 0.05).max(0.0)),
            "after_s": round3(boundary.at_s + 0.05),
        },
        "visual_signals": visual_signals_packet(&signals),
        "style_context": {
            "transition_density_last_30s": count_recent_transitions(&project.timeline, boundary.at_s),
        },
        "dialogue": dialogue_packet(
            &ctx.project_root,
            &boundary.from.asset_id,
            from_source_end_s,
            &boundary.to.asset_id,
            boundary.to.source_start_s,
        ),
        "missing_signals": missing_signals(&inputs, &signals),
    });
    Ok(body.to_string())
}

#[derive(Debug)]
struct BoundaryPacket {
    track_name: String,
    track_index: usize,
    from_child_index: usize,
    to_child_index: usize,
    at_s: f64,
    from: ClipPacket,
    to: ClipPacket,
    handles: HandlePacket,
}

#[derive(Debug)]
struct ClipPacket {
    clip_id: String,
    name: String,
    asset_id: String,
    timeline_start_s: f64,
    timeline_end_s: f64,
    source_start_s: f64,
    source_end_s: f64,
    duration_s: f64,
}

#[derive(Debug)]
struct HandlePacket {
    outgoing_s: Option<f64>,
    incoming_s: Option<f64>,
    max_centered_duration_s: Option<f64>,
}

fn find_boundary(timeline: &Timeline, from_id: &str, to_id: &str) -> Option<BoundaryPacket> {
    for (track_index, stack_child) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0_f64;
        for index in 0..track.children.len().saturating_sub(1) {
            let from_start_s = cursor_s;
            let from_duration_s = child_duration_s(&track.children[index]);
            let boundary_at_s = from_start_s + from_duration_s;
            let to_start_s = boundary_at_s;
            let Some(TrackChild::Clip(from_clip)) = track.children.get(index) else {
                cursor_s += from_duration_s;
                continue;
            };
            let Some(TrackChild::Clip(to_clip)) = track.children.get(index + 1) else {
                cursor_s += from_duration_s;
                continue;
            };
            if clip_id(from_clip) == from_id && clip_id(to_clip) == to_id {
                let from = build_clip_packet(from_clip, from_start_s)?;
                let to = build_clip_packet(to_clip, to_start_s)?;
                let outgoing_s = outgoing_handle_s(from_clip);
                let incoming_s = incoming_handle_s(to_clip);
                let max_centered_duration_s = match (outgoing_s, incoming_s) {
                    (Some(outgoing), Some(incoming)) => Some(2.0 * outgoing.min(incoming)),
                    (Some(outgoing), None) => Some(2.0 * outgoing),
                    (None, Some(incoming)) => Some(2.0 * incoming),
                    (None, None) => None,
                };
                return Some(BoundaryPacket {
                    track_name: track.name.clone(),
                    track_index,
                    from_child_index: index,
                    to_child_index: index + 1,
                    at_s: boundary_at_s,
                    from,
                    to,
                    handles: HandlePacket {
                        outgoing_s,
                        incoming_s,
                        max_centered_duration_s,
                    },
                });
            }
            cursor_s += from_duration_s;
        }
    }
    None
}

fn build_clip_packet(clip: &Clip, timeline_start_s: f64) -> Option<ClipPacket> {
    let range = clip.source_range.as_ref()?;
    let source_start_s = range.start_time.to_seconds();
    let duration_s = range.duration.to_seconds();
    Some(ClipPacket {
        clip_id: clip_id(clip),
        name: clip.name.clone(),
        asset_id: asset_id(clip)?,
        timeline_start_s,
        timeline_end_s: timeline_start_s + duration_s,
        source_start_s,
        source_end_s: source_start_s + duration_s,
        duration_s,
    })
}

fn clip_packet(clip: &ClipPacket) -> serde_json::Value {
    serde_json::json!({
        "clip_id": clip.clip_id,
        "name": clip.name,
        "asset_id": clip.asset_id,
        "timeline_start_s": round3(clip.timeline_start_s),
        "timeline_end_s": round3(clip.timeline_end_s),
        "source_start_s": round3(clip.source_start_s),
        "source_end_s": round3(clip.source_end_s),
        "duration_s": round3(clip.duration_s),
    })
}

fn child_duration_s(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(clip) => clip
            .source_range
            .as_ref()
            .map(|range| range.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        TrackChild::Transition(_) | TrackChild::Stack(_) => 0.0,
    }
}

fn clip_id(clip: &Clip) -> String {
    clip.metadata
        .montage
        .as_ref()
        .and_then(|meta| meta.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
        .unwrap_or(clip.name.as_str())
        .to_string()
}

fn asset_id(clip: &Clip) -> Option<String> {
    match &clip.media_reference {
        MediaReference::External(external) => Some(external.target_url.clone()),
        MediaReference::Missing(_) => None,
    }
}

fn incoming_handle_s(clip: &Clip) -> Option<f64> {
    let range = clip.source_range.as_ref()?;
    let start_s = range.start_time.to_seconds();
    match &clip.media_reference {
        MediaReference::External(ExternalReference {
            available_range: Some(available),
            ..
        }) => Some((start_s - available.start_time.to_seconds()).max(0.0)),
        MediaReference::External(_) => Some(start_s.max(0.0)),
        MediaReference::Missing(_) => None,
    }
}

fn outgoing_handle_s(clip: &Clip) -> Option<f64> {
    let range = clip.source_range.as_ref()?;
    let source_end_s = range.start_time.to_seconds() + range.duration.to_seconds();
    match &clip.media_reference {
        MediaReference::External(ExternalReference {
            available_range: Some(available),
            ..
        }) => {
            let available_end_s =
                available.start_time.to_seconds() + available.duration.to_seconds();
            Some((available_end_s - source_end_s).max(0.0))
        }
        MediaReference::External(_) => None,
        MediaReference::Missing(_) => None,
    }
}

fn transcript_window(
    project_root: &std::path::Path,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
) -> Vec<serde_json::Value> {
    crate::continuity::load_whisper_words(project_root, asset_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|word| word.end_s >= start_s && word.start_s <= end_s)
        .map(word_packet)
        .collect()
}

fn word_packet(word: WhisperWord) -> serde_json::Value {
    serde_json::json!({
        "text": word.text,
        "start_s": round3(word.start_s),
        "end_s": round3(word.end_s),
    })
}

/// Count visible transitions on the timeline in `[max(0, at_s - 30), at_s)`,
/// matching the window `assess_edit_quality` uses. Each track carries its own
/// cursor advanced by clip and gap durations; nested stacks are walked too.
fn count_recent_transitions(timeline: &Timeline, at_s: f64) -> usize {
    let window_start_s = (at_s - 30.0).max(0.0);
    timeline
        .tracks
        .children
        .iter()
        .map(|stack_child| count_in_stack_child(stack_child, window_start_s, at_s))
        .sum()
}

fn count_in_stack_child(stack_child: &StackChild, window_start_s: f64, at_s: f64) -> usize {
    let StackChild::Track(track) = stack_child else {
        return 0;
    };
    let mut count = 0_usize;
    let mut cursor_s = 0.0_f64;
    for child in &track.children {
        match child {
            TrackChild::Transition(_) => {
                if window_start_s <= cursor_s && cursor_s < at_s {
                    count += 1;
                }
            }
            TrackChild::Stack(stack) => {
                count += stack
                    .children
                    .iter()
                    .map(|nested| count_in_stack_child(nested, window_start_s, at_s))
                    .sum::<usize>();
            }
            TrackChild::Clip(_) | TrackChild::Gap(_) => cursor_s += child_duration_s(child),
        }
    }
    count
}

/// Which side of the boundary a speaker lookup is for.
#[derive(Clone, Copy)]
enum BoundarySide {
    Outgoing,
    Incoming,
}

/// Resolve the speaker talking at `at_s` in source seconds.
///
/// A segment containing `at_s` wins outright. Otherwise the outgoing
/// side reaches back up to one second for the latest segment that ended
/// just before the cut, and the incoming side reaches forward up to one
/// second for the earliest segment that starts just after it.
fn speaker_near(segments: &[WhisperSegment], at_s: f64, side: BoundarySide) -> Option<&str> {
    let speaker = |segment: &WhisperSegment| {
        segment
            .speaker_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .is_some()
    };
    if let Some(segment) = segments
        .iter()
        .find(|segment| segment.start_s <= at_s && at_s <= segment.end_s && speaker(segment))
    {
        return segment.speaker_id.as_deref();
    }
    let nearby = match side {
        BoundarySide::Outgoing => segments
            .iter()
            .filter(|segment| {
                speaker(segment) && segment.end_s >= at_s - 1.0 && segment.end_s <= at_s
            })
            .max_by(|a, b| a.end_s.total_cmp(&b.end_s)),
        BoundarySide::Incoming => segments
            .iter()
            .filter(|segment| {
                speaker(segment) && segment.start_s >= at_s && segment.start_s <= at_s + 1.0
            })
            .min_by(|a, b| a.start_s.total_cmp(&b.start_s)),
    };
    nearby.and_then(|segment| segment.speaker_id.as_deref())
}

/// Speaker relation across the boundary. Diarization labels are only
/// meaningful within a single asset, so a cross-asset boundary is
/// always `unknown` even when both sides carry the same label.
fn dialogue_packet(
    project_root: &Path,
    from_asset: &str,
    from_source_end_s: f64,
    to_asset: &str,
    to_source_start_s: f64,
) -> serde_json::Value {
    let outgoing_segments = load_whisper_segments(project_root, from_asset).unwrap_or_default();
    let incoming_segments = load_whisper_segments(project_root, to_asset).unwrap_or_default();
    let outgoing = speaker_near(
        &outgoing_segments,
        from_source_end_s,
        BoundarySide::Outgoing,
    );
    let incoming = speaker_near(
        &incoming_segments,
        to_source_start_s,
        BoundarySide::Incoming,
    );
    let same_asset = from_asset == to_asset;
    let relation = match (same_asset, outgoing, incoming) {
        (true, Some(outgoing), Some(incoming)) if outgoing == incoming => "same_speaker",
        (true, Some(_), Some(_)) => "speaker_change",
        _ => "unknown",
    };
    serde_json::json!({
        "relation": relation,
        "same_asset": same_asset,
        "outgoing_speaker": outgoing,
        "incoming_speaker": incoming,
    })
}

fn missing_signals(
    inputs: &ContinuityInputs,
    signals: &BoundaryVisualSignals,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if inputs.whisper_words.is_none() {
        missing.push("whisper_words");
    }
    if inputs.whisper_segments.is_none() {
        missing.push("whisper_segments");
    }
    if inputs.motion_magnitudes.is_none() {
        missing.push("motion");
    }
    if inputs.scene_changes_s.is_none() {
        missing.push("scene_changes");
    }
    if inputs.silences.is_none() {
        missing.push("silences");
    }
    if signals.outgoing.motion_direction.is_none() || signals.incoming.motion_direction.is_none() {
        missing.push("motion_direction");
    }
    missing
}

fn visual_signals_packet(signals: &BoundaryVisualSignals) -> serde_json::Value {
    serde_json::json!({
        "outgoing": side_signals_packet(&signals.outgoing),
        "incoming": side_signals_packet(&signals.incoming),
        "motion_match": signals.motion_match().as_str(),
        "motion_match_confidence": signals.motion_match_confidence().map(round3),
        "either_side_static": signals.either_side_static(),
    })
}

fn side_signals_packet(side: &SideSignals) -> serde_json::Value {
    serde_json::json!({
        "subject_center": side.subject_center,
        "face_center": side.face_center,
        "motion_magnitude": side.motion_magnitude.map(round3),
        "motion_direction": side.motion_direction.map(|d| d.as_str()),
        "whip_pan_score": side.whip_pan_score.map(round3),
        "occlusion_score": side.occlusion_score.map(round3),
        "action_score": side.action_score.map(round3),
    })
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Build a read-only transition decision context packet for one adjacent \
timeline boundary. Returns adjacent clip metadata, timeline/source ranges, \
transition handle availability, continuity verdict, transcript snippets, \
suggested frame timestamps, per-side motion magnitudes and screen \
directions (outgoing/incoming), motion-match classification \
(aligned/opposed/orthogonal/unknown), and missing-signal names. This tool \
does not choose or apply a transition; use it before deciding whether a \
hard cut, semantic cut, split edit, b-roll cover, or visible transition \
is warranted.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::montage_meta::MontageClipMetadata;
    use montage_proto::otio::{Gap, RationalTime, TimeRange, Track, TrackKind, Transition};
    use serde_json::Value;

    fn ctx_at(project_root: &std::path::Path) -> McpToolCtx {
        McpToolCtx {
            project_root: project_root.to_path_buf(),
        }
    }

    fn args(from: &str, to: &str, window_s: Option<f64>) -> TransitionContextArgs {
        TransitionContextArgs {
            between: TransitionBetweenArgs {
                from: ClipRefArgs {
                    clip_uuid: from.to_string(),
                },
                to: ClipRefArgs {
                    clip_uuid: to.to_string(),
                },
            },
            window_s,
        }
    }

    fn clip(
        name: &str,
        uuid: &str,
        asset_id: &str,
        source_start_s: f64,
        duration_s: f64,
        available_duration_s: f64,
    ) -> Clip {
        let mut ext = ExternalReference::new(asset_id);
        ext.available_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(available_duration_s * 24.0, 24.0),
        ));
        let mut clip = Clip::empty(name);
        clip.media_reference = MediaReference::External(ext);
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut meta = MontageClipMetadata::default();
        meta.extra.insert(
            "clip_uuid".into(),
            serde_json::Value::String(uuid.to_string()),
        );
        clip.metadata.montage = Some(meta);
        clip
    }

    /// Write a whisper sidecar with words and optional speaker-labelled segments.
    fn write_whisper(
        project_root: &std::path::Path,
        asset: &str,
        words: Vec<(&str, f64, f64)>,
        segments: Vec<(f64, f64, &str)>,
    ) {
        let path = project_root
            .join("index")
            .join("whisper")
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "data": {
                "words": words
                    .into_iter()
                    .map(|(text, start_s, end_s)| serde_json::json!({
                        "text": text,
                        "start_s": start_s,
                        "end_s": end_s,
                    }))
                    .collect::<Vec<_>>(),
                "segments": segments
                    .into_iter()
                    .map(|(start_s, end_s, speaker_id)| serde_json::json!({
                        "start_s": start_s,
                        "end_s": end_s,
                        "speaker_id": speaker_id,
                    }))
                    .collect::<Vec<_>>(),
            }
        });
        std::fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
    }

    fn body_for(project_root: &std::path::Path, from: &str, to: &str) -> Value {
        let out = run(args(from, to, None), ctx_at(project_root)).unwrap();
        serde_json::from_str(&out).unwrap()
    }

    /// One track: `clip-a` (raw/a.mp4, source 2..7) then `clip-b`
    /// (same asset, source 7..11) — boundary at 5s of timeline time,
    /// 7s of source time. A second track holds a cross-asset boundary.
    fn project_with_dialogue() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();

        let mut same = Track::empty("V1", TrackKind::Video);
        same.children.push(TrackChild::Clip(clip(
            "a",
            "same-a",
            "raw/a.mp4",
            2.0,
            5.0,
            20.0,
        )));
        same.children.push(TrackChild::Clip(clip(
            "b",
            "same-b",
            "raw/a.mp4",
            7.0,
            4.0,
            20.0,
        )));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(same));

        let mut cross = Track::empty("V2", TrackKind::Video);
        cross.children.push(TrackChild::Clip(clip(
            "c",
            "cross-c",
            "raw/b.mp4",
            2.0,
            5.0,
            20.0,
        )));
        cross.children.push(TrackChild::Clip(clip(
            "d",
            "cross-d",
            "raw/c.mp4",
            7.0,
            4.0,
            20.0,
        )));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(cross));

        project.write(dir.path()).unwrap();

        // One continuous SPEAKER_00 segment spanning the same-asset cut at 7s.
        write_whisper(
            dir.path(),
            "raw/a.mp4",
            vec![("outgoing", 6.4, 6.9), ("incoming", 7.1, 7.4)],
            vec![(2.0, 11.0, "SPEAKER_00")],
        );
        // Different assets, both labelled SPEAKER_00 — labels are not comparable.
        write_whisper(
            dir.path(),
            "raw/b.mp4",
            vec![("outgoing", 6.4, 6.9)],
            vec![(2.0, 7.0, "SPEAKER_00")],
        );
        write_whisper(
            dir.path(),
            "raw/c.mp4",
            vec![("incoming", 7.1, 7.4)],
            vec![(7.0, 11.0, "SPEAKER_00")],
        );
        dir
    }

    #[test]
    fn reports_same_speaker_across_a_within_asset_boundary() {
        let dir = project_with_dialogue();
        let body = body_for(dir.path(), "same-a", "same-b");

        assert_eq!(
            body.pointer("/dialogue/relation").and_then(Value::as_str),
            Some("same_speaker")
        );
        assert_eq!(
            body.pointer("/dialogue/same_asset")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/dialogue/outgoing_speaker")
                .and_then(Value::as_str),
            Some("SPEAKER_00")
        );
        assert_eq!(
            body.pointer("/dialogue/incoming_speaker")
                .and_then(Value::as_str),
            Some("SPEAKER_00")
        );
    }

    #[test]
    fn speaker_labels_across_assets_are_not_comparable() {
        let dir = project_with_dialogue();
        let body = body_for(dir.path(), "cross-c", "cross-d");

        assert_eq!(
            body.pointer("/dialogue/relation").and_then(Value::as_str),
            Some("unknown")
        );
        assert_eq!(
            body.pointer("/dialogue/same_asset")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    /// `lead` (20s) T gap(5s) T gap(5s) T `clip-a` (5s) `clip-b`.
    /// The tested boundary sits at 35s; the transitions land at track
    /// cursors 20s, 25s and 30s — all inside `[5, 35)`.
    fn project_with_transition_density() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip(
            "lead",
            "lead",
            "raw/a.mp4",
            0.0,
            20.0,
            60.0,
        )));
        for _ in 0..2 {
            track
                .children
                .push(TrackChild::Transition(Transition::symmetric(
                    "SMPTE_Dissolve",
                    0.5,
                    24.0,
                )));
            track
                .children
                .push(TrackChild::Gap(Gap::of_duration(5.0, 24.0)));
        }
        track
            .children
            .push(TrackChild::Transition(Transition::symmetric(
                "SMPTE_Dissolve",
                0.5,
                24.0,
            )));
        track.children.push(TrackChild::Clip(clip(
            "a",
            "clip-a",
            "raw/a.mp4",
            30.0,
            5.0,
            60.0,
        )));
        track.children.push(TrackChild::Clip(clip(
            "b",
            "clip-b",
            "raw/a.mp4",
            35.0,
            4.0,
            60.0,
        )));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();
        dir
    }

    #[test]
    fn counts_visible_transitions_in_the_thirty_seconds_before_the_boundary() {
        let dir = project_with_transition_density();
        let body = body_for(dir.path(), "clip-a", "clip-b");

        assert_eq!(
            body.pointer("/boundary/at_s").and_then(Value::as_f64),
            Some(35.0)
        );
        assert_eq!(
            body.pointer("/style_context/transition_density_last_30s")
                .and_then(Value::as_u64),
            Some(3)
        );
    }
}
