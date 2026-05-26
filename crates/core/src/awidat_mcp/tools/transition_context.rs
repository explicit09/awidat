//! `transition_context` — read-only transition boundary context.
//! Ported from `crates/core/src/tools/transition_context.rs` to the
//! in-process MCP server. Builds a deterministic packet for a
//! candidate transition boundary.

use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, StackChild, Timeline, TrackChild,
};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::awidat_mcp::tools::assess_continuity::build_inputs;
use crate::continuity::{ContinuityInputs, CutKind, WhisperWord, assess_continuity};
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
        .awidat
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
