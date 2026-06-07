//! `plan_sound_design` — read-only audio/sound-design planner.
//!
//! Converts a high-level audio intent into concrete follow-up tool calls and
//! a parseable EDL template using existing audio primitives. It does not pick
//! a real media file; callers should run `find_audio_asset`, then replace the
//! placeholder asset path in the returned EDL before `apply_edl`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

const PLACEHOLDER_ASSET: &str = "<asset from find_audio_asset>";

/// Arguments to `plan_sound_design`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanSoundDesignArgs {
    /// High-level intent, e.g. whoosh_transition, impact_hit,
    /// ambience_bridge, music_bed, dialogue_ducking.
    pub intent: String,
    /// Optional transition_context-like packet. Used for split-edit anchors
    /// and timing confidence when available.
    #[serde(default)]
    pub context: serde_json::Value,
    /// Absolute timeline time in seconds for the sound event.
    #[serde(default)]
    pub start_s: Option<f64>,
    /// Desired inserted asset duration. Defaults by intent.
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// Optional program loudness target to append as `Set Loudness Target`.
    #[serde(default)]
    pub target_lufs: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoundIntent {
    WhooshTransition,
    ImpactHit,
    Riser,
    AmbienceBridge,
    MusicBed,
    DialogueDucking,
}

#[derive(Debug)]
struct ContextSummary {
    from_uuid: Option<String>,
    to_uuid: Option<String>,
    missing_signals: Vec<String>,
}

/// Run `plan_sound_design`. The project root is unused because this is a
/// pure read-only planner.
pub fn run(args: PlanSoundDesignArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let Some(intent) = classify_intent(&args.intent) else {
        return review_response("unsupported_or_vague_intent", &args.intent);
    };
    if args.start_s.is_none() && !matches!(intent, SoundIntent::DialogueDucking) {
        return review_response("missing_timing", &args.intent);
    }

    let context = parse_context(&args.context);
    let asset_query = asset_query(intent);
    let duration_s = args
        .duration_s
        .unwrap_or_else(|| default_duration_s(intent));
    let at_s = event_start_s(args.start_s.unwrap_or(0.0), intent, duration_s);
    let split_edit = split_edit(intent, &context);
    let edl_template = build_edl(intent, at_s, duration_s, args.target_lufs, &split_edit);
    let status = if context.missing_signals.is_empty() {
        "ready"
    } else {
        "partial_context"
    };

    let body = serde_json::json!({
        "status": status,
        "summary_for_agent": summary(intent, status),
        "recommended": {
            "kind": "sound_design",
            "intent": intent_id(intent),
            "asset_query": asset_query,
            "timeline_start_s": round3(at_s),
            "duration_s": round3(duration_s),
            "split_edit": split_edit,
            "mix_guidance": mix_guidance(intent),
        },
        "follow_up_tools": [
            {
                "name": "find_audio_asset",
                "args": asset_query
            },
            {
                "name": "apply_edl",
                "args": {
                    "edl": "Replace the placeholder asset path in edl_template with a project-relative imported audio asset path."
                }
            },
            {
                "name": "start_render",
                "when": "after apply_edl and any timeline review"
            },
            {
                "name": "verify_render",
                "when": "after render; inspect audio duration/loudness evidence"
            }
        ],
        "edl_template": edl_template,
        "notes": notes(intent),
    });
    serde_json::to_string(&body).map_err(|e| format!("plan_sound_design serialize: {e}"))
}

fn classify_intent(raw: &str) -> Option<SoundIntent> {
    let normalized = raw.to_ascii_lowercase().replace(['-', ' '], "_");
    if contains_any(
        &normalized,
        &["whoosh", "woosh", "swish", "motion_transition"],
    ) {
        Some(SoundIntent::WhooshTransition)
    } else if contains_any(&normalized, &["impact", "hit", "slam", "punch"]) {
        Some(SoundIntent::ImpactHit)
    } else if contains_any(&normalized, &["riser", "build", "tension"]) {
        Some(SoundIntent::Riser)
    } else if contains_any(
        &normalized,
        &["ambience", "ambiance", "room_tone", "sound_bridge"],
    ) {
        Some(SoundIntent::AmbienceBridge)
    } else if contains_any(&normalized, &["music", "score", "bed", "track"]) {
        Some(SoundIntent::MusicBed)
    } else if contains_any(&normalized, &["duck", "ducking", "dialogue_mix"]) {
        Some(SoundIntent::DialogueDucking)
    } else {
        None
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn parse_context(value: &serde_json::Value) -> ContextSummary {
    let from_uuid = value
        .pointer("/between/from/clip_uuid")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let to_uuid = value
        .pointer("/between/to/clip_uuid")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let missing_signals = value
        .pointer("/missing_signals")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ContextSummary {
        from_uuid,
        to_uuid,
        missing_signals,
    }
}

fn asset_query(intent: SoundIntent) -> serde_json::Value {
    let (kind, mood, max_duration_s) = match intent {
        SoundIntent::WhooshTransition => ("sfx", "whoosh", Some(1.5)),
        SoundIntent::ImpactHit => ("sfx", "impact", Some(1.2)),
        SoundIntent::Riser => ("sfx", "riser", Some(4.0)),
        SoundIntent::AmbienceBridge => ("ambience", "room tone", Some(8.0)),
        SoundIntent::MusicBed => ("music", "bed", None),
        SoundIntent::DialogueDucking => ("music", "bed", None),
    };
    serde_json::json!({
        "kind": kind,
        "mood": mood,
        "max_duration_s": max_duration_s,
        "max_results": 8,
    })
}

fn default_duration_s(intent: SoundIntent) -> f64 {
    match intent {
        SoundIntent::WhooshTransition => 0.45,
        SoundIntent::ImpactHit => 0.35,
        SoundIntent::Riser => 1.5,
        SoundIntent::AmbienceBridge => 1.0,
        SoundIntent::MusicBed | SoundIntent::DialogueDucking => 8.0,
    }
}

fn event_start_s(start_s: f64, intent: SoundIntent, duration_s: f64) -> f64 {
    let offset_s = match intent {
        SoundIntent::WhooshTransition => -0.1,
        SoundIntent::Riser => -duration_s,
        SoundIntent::AmbienceBridge => -duration_s * 0.5,
        SoundIntent::ImpactHit | SoundIntent::MusicBed | SoundIntent::DialogueDucking => 0.0,
    };
    (start_s + offset_s).max(0.0)
}

fn split_edit(intent: SoundIntent, context: &ContextSummary) -> Option<serde_json::Value> {
    match intent {
        SoundIntent::AmbienceBridge => context.from_uuid.as_ref().map(|uuid| {
            serde_json::json!({
                "kind": "l_cut",
                "anchor_clip_uuid": uuid,
                "trail_s": 0.6,
                "reason": "Carry outgoing ambience under the incoming picture to avoid an abrupt room-tone cut."
            })
        }),
        SoundIntent::WhooshTransition => context.to_uuid.as_ref().map(|uuid| {
            serde_json::json!({
                "kind": "j_cut",
                "anchor_clip_uuid": uuid,
                "lead_s": 0.25,
                "reason": "Pre-lap the transition sound slightly so the motion is heard before the visual cut lands."
            })
        }),
        _ => None,
    }
}

fn build_edl(
    intent: SoundIntent,
    at_s: f64,
    duration_s: f64,
    target_lufs: Option<f64>,
    split_edit: &Option<serde_json::Value>,
) -> String {
    let mut lines = vec![
        "*** Begin EDL".to_string(),
        "*** Insert Clip".to_string(),
        format!("+ asset: {PLACEHOLDER_ASSET}"),
        format!("+ track: {}", track_name(intent)),
        "+ track_kind: audio".to_string(),
        format!("+ at_s: {:.3}", round3(at_s)),
        "+ start: 0.000".to_string(),
        format!("+ end: {:.3}", round3(duration_s)),
        format!("+ name: {}", clip_name(intent)),
    ];
    if matches!(
        intent,
        SoundIntent::WhooshTransition | SoundIntent::AmbienceBridge
    ) {
        lines.push("+ snap: nearest_cut".to_string());
        lines.push("+ snap_tolerance_s: 0.120".to_string());
    }
    if let Some(split) = split_edit {
        if split.pointer("/kind").and_then(|v| v.as_str()) == Some("j_cut") {
            if let (Some(uuid), Some(lead_s)) = (
                split.pointer("/anchor_clip_uuid").and_then(|v| v.as_str()),
                split.pointer("/lead_s").and_then(|v| v.as_f64()),
            ) {
                lines.extend([
                    "*** Set Audio Lead".to_string(),
                    format!("@@ anchor: clip_uuid={uuid}"),
                    format!("+ lead_s: {:.3}", round3(lead_s)),
                    "+ reason: sound design prelap".to_string(),
                    "+ confidence: 0.720".to_string(),
                ]);
            }
        } else if let (Some(uuid), Some(trail_s)) = (
            split.pointer("/anchor_clip_uuid").and_then(|v| v.as_str()),
            split.pointer("/trail_s").and_then(|v| v.as_f64()),
        ) {
            lines.extend([
                "*** Set Audio Trail".to_string(),
                format!("@@ anchor: clip_uuid={uuid}"),
                format!("+ trail_s: {:.3}", round3(trail_s)),
                "+ reason: ambience bridge continuity".to_string(),
                "+ confidence: 0.740".to_string(),
            ]);
        }
    }
    if let Some(lufs) = target_lufs {
        lines.extend([
            "*** Set Loudness Target".to_string(),
            format!("+ integrated_lufs: {:.3}", round3(lufs)),
        ]);
    }
    lines.push("*** End EDL".to_string());
    lines.join("\n")
}

fn track_name(intent: SoundIntent) -> &'static str {
    match intent {
        SoundIntent::MusicBed | SoundIntent::DialogueDucking => "Music",
        SoundIntent::AmbienceBridge => "Ambience",
        _ => "SFX",
    }
}

fn clip_name(intent: SoundIntent) -> &'static str {
    match intent {
        SoundIntent::WhooshTransition => "planned_whoosh_transition",
        SoundIntent::ImpactHit => "planned_impact_hit",
        SoundIntent::Riser => "planned_riser",
        SoundIntent::AmbienceBridge => "planned_ambience_bridge",
        SoundIntent::MusicBed => "planned_music_bed",
        SoundIntent::DialogueDucking => "planned_dialogue_ducking_bed",
    }
}

fn intent_id(intent: SoundIntent) -> &'static str {
    match intent {
        SoundIntent::WhooshTransition => "whoosh_transition",
        SoundIntent::ImpactHit => "impact_hit",
        SoundIntent::Riser => "riser",
        SoundIntent::AmbienceBridge => "ambience_bridge",
        SoundIntent::MusicBed => "music_bed",
        SoundIntent::DialogueDucking => "dialogue_ducking",
    }
}

fn summary(intent: SoundIntent, status: &str) -> String {
    format!(
        "Sound-design planning status: {status}. Intent {} mapped to an asset query and parseable EDL template.",
        intent_id(intent)
    )
}

fn mix_guidance(intent: SoundIntent) -> Vec<&'static str> {
    match intent {
        SoundIntent::WhooshTransition => vec![
            "Place the whoosh so its transient supports visible motion; keep it subtle under dialogue.",
            "Use a short fade-out if the tail masks the next line.",
        ],
        SoundIntent::ImpactHit => vec![
            "Land the transient on the cut, beat, or visual hit.",
            "Reduce gain if it masks dialogue or clips the master.",
        ],
        SoundIntent::Riser => vec![
            "End the riser exactly at the reveal/drop.",
            "Avoid long risers without a real tension build.",
        ],
        SoundIntent::AmbienceBridge => vec![
            "Use room tone or location ambience to hide the scene boundary.",
            "Prefer an L-cut when outgoing ambience should carry under the next picture.",
        ],
        SoundIntent::MusicBed | SoundIntent::DialogueDucking => vec![
            "Keep dialogue intelligible; duck music before speech starts, not after it masks words.",
            "Verify final program loudness and true peak after render.",
        ],
    }
}

fn notes(intent: SoundIntent) -> Vec<&'static str> {
    let mut notes = vec![
        "Replace the placeholder asset with a project-relative path returned by find_audio_asset or import_media.",
        "Render-review by ear; this planner is read-only and does not measure the final mix.",
    ];
    if matches!(intent, SoundIntent::MusicBed | SoundIntent::DialogueDucking) {
        notes.push("For real ducking automation, use existing audio FX/track automation paths after placing the music bed.");
    }
    notes
}

fn review_response(reason: &str, intent: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "status": "needs_review",
        "summary_for_agent": format!("Sound-design planning needs review: {reason}."),
        "intent": intent,
        "review_questions": [
            "What exact sound role is needed: whoosh, impact, riser, ambience bridge, music bed, or dialogue ducking?",
            "Where should the sound land on the timeline?"
        ],
        "safe_defaults": [
            "Hard cut remains acceptable when no motivated sound cue is clear.",
            "Do not add decorative SFX that mask dialogue or repeat without purpose."
        ]
    });
    serde_json::to_string(&body).map_err(|e| format!("plan_sound_design serialize: {e}"))
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Read-only audio/sound-design planner. Pass an intent such as \
whoosh_transition, impact_hit, riser, ambience_bridge, music_bed, or \
dialogue_ducking plus optional transition_context and timing. The tool returns \
an asset search query, mix guidance, follow-up tool calls, and a parseable EDL \
template using existing Insert Clip, Set Audio Lead/Trail, and Set Loudness \
Target operations. It never applies the edit and never invents media.\
";
