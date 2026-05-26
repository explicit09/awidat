//! `plan_emphasis` — read-only planner for single-clip emphasis motion.
//! Ported from `crates/core/src/tools/plan_emphasis.rs` to the in-process
//! MCP server. Returns an EDL fragment the agent passes to `apply_edl`.

use awidat_proto::professional::{
    AnimationTarget, Easing, ExtrapolationMode, FindingSeverity, Keyframe, KeyframeInterpolation,
    ParameterAnimation, SpringParameters,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `plan_emphasis`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanEmphasisArgs {
    /// Clip id to animate.
    pub clip_id: String,
    /// Clip-local seconds where emphasis should begin.
    #[serde(default)]
    pub at_s: Option<f64>,
    /// Optional duration in seconds.
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// Optional high-level job.
    #[serde(default)]
    pub objective: Option<String>,
    /// Optional visual target bucket.
    #[serde(default)]
    pub target: Option<String>,
    /// Motion feel: snappy, loose, heavy, or neutral.
    #[serde(default)]
    pub style: Option<String>,
    /// Optional screen direction hint.
    #[serde(default)]
    pub direction: Option<String>,
    /// Strength in [0, 1].
    #[serde(default)]
    pub intensity: Option<f64>,
    /// Optional beat times in clip-local seconds. When present, emits one
    /// rhythm-driven animation with diminishing pulse intensity.
    #[serde(default)]
    pub beat_times_s: Vec<f64>,
    /// Use every Nth beat when beat_times_s is supplied.
    #[serde(default)]
    pub every_nth_beat: Option<usize>,
    /// Optional context from inspect/shot/quality tools.
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
struct EmphasisSignals {
    subject_center: Option<[f64; 2]>,
    motion_direction: Option<String>,
    motion_vector: Option<[f64; 2]>,
    action_score: Option<f64>,
    whip_pan_score: Option<f64>,
    occlusion_score: Option<f64>,
    has_saliency_map: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EmphasisMotion {
    ScalePunch,
    SlowZoom,
    SlideIn,
    RotateNudge,
}

impl EmphasisMotion {
    fn as_str(self) -> &'static str {
        match self {
            Self::ScalePunch => "scale_punch",
            Self::SlowZoom => "slow_zoom",
            Self::SlideIn => "slide_in",
            Self::RotateNudge => "rotate_nudge",
        }
    }
}

/// Run `plan_emphasis`. The project root from [`McpToolCtx`] is unused —
/// this planner builds its plan purely from `args` — but the signature is
/// kept uniform with the rest of the MCP tool surface.
pub fn run(args: PlanEmphasisArgs, _ctx: McpToolCtx) -> Result<String, String> {
    if args.clip_id.trim().is_empty() {
        return Err("plan_emphasis: clip_id must be non-empty.".into());
    }
    let plan = build_plan(&args)?;
    Ok(plan.to_string())
}

fn build_plan(args: &PlanEmphasisArgs) -> Result<serde_json::Value, String> {
    let signals = args.context.as_ref().map(parse_signals).unwrap_or_default();
    let target = args.target.as_deref().unwrap_or("overlay");
    let objective = args.objective.as_deref().unwrap_or("direct_attention");
    let style = args.style.as_deref().unwrap_or("snappy");
    let intensity = args.intensity.unwrap_or(0.62).clamp(0.0, 1.0);
    let motion = choose_motion(objective, target, &signals);
    let animations = if !args.beat_times_s.is_empty() {
        vec![beat_pulse_animation(args, target, style, intensity)?]
    } else {
        vec![single_animation(
            args, target, style, intensity, motion, &signals,
        )?]
    };
    let missing_signals = missing_signals(&signals);
    let edl_fragment = animations_edl(&animations)?;
    Ok(serde_json::json!({
        "recommended": {
            "decision": "parameter_animation",
            "motion": motion.as_str(),
            "objective": objective,
            "target": target,
            "style": style,
            "reason": reason(objective, motion, &signals),
        },
        "signals_used": {
            "subject_center": signals.subject_center,
            "motion_direction": signals.motion_direction,
            "motion_vector": signals.motion_vector,
            "action_score": signals.action_score.map(round3),
            "whip_pan_score": signals.whip_pan_score.map(round3),
            "occlusion_score": signals.occlusion_score.map(round3),
            "saliency_map": signals.has_saliency_map,
        },
        "missing_signals": missing_signals,
        "animations": animations,
        "edl_fragment": edl_fragment,
        "alternates": alternates(motion),
    }))
}

fn choose_motion(objective: &str, target: &str, signals: &EmphasisSignals) -> EmphasisMotion {
    match objective {
        "callout_entry" => EmphasisMotion::SlideIn,
        "settle_subject" | "soft_emphasis" => EmphasisMotion::SlowZoom,
        "style_accent" if target != "title" => EmphasisMotion::RotateNudge,
        "direct_attention" if signals.motion_direction.is_some() => EmphasisMotion::SlideIn,
        _ => EmphasisMotion::ScalePunch,
    }
}

fn single_animation(
    args: &PlanEmphasisArgs,
    target: &str,
    style: &str,
    intensity: f64,
    motion: EmphasisMotion,
    signals: &EmphasisSignals,
) -> Result<ParameterAnimation, String> {
    let start = args.at_s.unwrap_or(0.0).max(0.0);
    let duration = args
        .duration_s
        .unwrap_or(default_duration(motion, style))
        .max(0.05);
    let id = animation_id(&args.clip_id, motion.as_str(), start);
    let parameter = parameter_for(target, motion);
    let easing = easing_for(style);
    let spring = spring_for(style);
    let keyframes = match motion {
        EmphasisMotion::ScalePunch => {
            let peak = 1.0 + 0.04 + intensity * 0.12;
            vec![
                keyframe(start, 1.0, easing, spring),
                keyframe(start + duration * 0.38, peak, easing, spring),
                keyframe(start + duration, 1.0, easing, spring),
            ]
        }
        EmphasisMotion::SlowZoom => {
            let end = 1.0 + 0.025 + intensity * 0.075;
            vec![
                keyframe(start, 1.0, easing, spring),
                keyframe(start + duration, end, easing, spring),
            ]
        }
        EmphasisMotion::SlideIn => {
            let (from, to) = slide_values(args.direction.as_deref(), signals);
            vec![
                keyframe(start, from, easing, spring),
                keyframe(start + duration, to, easing, spring),
            ]
        }
        EmphasisMotion::RotateNudge => {
            let sign = direction_sign(args.direction.as_deref(), signals);
            let peak = sign * (2.0 + intensity * 7.0);
            vec![
                keyframe(start, 0.0, easing, spring),
                keyframe(start + duration * 0.45, peak, easing, spring),
                keyframe(start + duration, 0.0, easing, spring),
            ]
        }
    };
    animation(
        id,
        &args.clip_id,
        parameter,
        keyframes,
        reason_for_animation(motion),
    )
}

fn beat_pulse_animation(
    args: &PlanEmphasisArgs,
    target: &str,
    style: &str,
    intensity: f64,
) -> Result<ParameterAnimation, String> {
    let every = args.every_nth_beat.unwrap_or(1).max(1);
    let duration = args.duration_s.unwrap_or(0.18).clamp(0.08, 0.5);
    let easing = easing_for(style);
    let spring = spring_for(style);
    let mut keyframes = Vec::new();
    let mut emitted = 0usize;
    let usable_beat_count = args
        .beat_times_s
        .iter()
        .filter(|time_s| time_s.is_finite() && **time_s >= 0.0)
        .count();
    for (index, beat) in args
        .beat_times_s
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t >= 0.0)
        .enumerate()
    {
        if (index + 1) % every != 0 {
            continue;
        }
        let decay = 0.88_f64.powi(emitted as i32);
        let peak = 1.0 + (0.035 + intensity * 0.115) * decay;
        push_keyframe(&mut keyframes, keyframe(beat, 1.0, easing, spring));
        push_keyframe(
            &mut keyframes,
            keyframe(beat + duration * 0.32, peak, easing, spring),
        );
        push_keyframe(
            &mut keyframes,
            keyframe(beat + duration, 1.0, easing, spring),
        );
        emitted += 1;
    }
    if keyframes.len() < 2 {
        if emitted == 0 && usable_beat_count > 0 {
            return Err(format!(
                "plan_emphasis: every_nth_beat={every} skipped all {usable_beat_count} beats; lower the stride or supply more beats."
            ));
        }
        return Err("plan_emphasis: beat_times_s did not contain any usable beat times.".into());
    }
    animation(
        animation_id(&args.clip_id, "beat_scale_punch", keyframes[0].time_s),
        &args.clip_id,
        parameter_for(target, EmphasisMotion::ScalePunch),
        keyframes,
        "Rhythm-driven scale pulse with diminishing intensity.".into(),
    )
}

fn animation(
    id: String,
    clip_id: &str,
    parameter: &'static str,
    keyframes: Vec<Keyframe>,
    rationale: String,
) -> Result<ParameterAnimation, String> {
    let animation = ParameterAnimation {
        id,
        target: AnimationTarget::ClipParameter {
            clip_id: clip_id.to_string(),
            parameter: parameter.to_string(),
        },
        keyframes,
        pre_extrapolation: ExtrapolationMode::Hold,
        post_extrapolation: ExtrapolationMode::Hold,
        motion_path: None,
        metadata_only: false,
        rationale: Some(rationale),
    };
    let errors = animation
        .validate()
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == FindingSeverity::Error)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "plan_emphasis: generated invalid animation: {}",
            errors
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    Ok(animation)
}

fn keyframe(time_s: f64, value: f64, easing: Easing, spring: Option<SpringParameters>) -> Keyframe {
    Keyframe {
        time_s: round3(time_s),
        value: round3(value),
        interpolation: if spring.is_some() {
            KeyframeInterpolation::Spring
        } else {
            KeyframeInterpolation::Linear
        },
        easing: if spring.is_some() {
            Easing::Linear
        } else {
            easing
        },
        bezier: None,
        tangent_mode: Default::default(),
        spring,
    }
}

fn push_keyframe(keyframes: &mut Vec<Keyframe>, keyframe: Keyframe) {
    if keyframes
        .last()
        .is_some_and(|last| (last.time_s - keyframe.time_s).abs() < 0.012)
    {
        return;
    }
    keyframes.push(keyframe);
}

fn parameter_for(target: &str, motion: EmphasisMotion) -> &'static str {
    match (target, motion) {
        ("title", EmphasisMotion::ScalePunch | EmphasisMotion::SlowZoom) => "title.font_size",
        ("title", EmphasisMotion::SlideIn) => "title.x",
        (_, EmphasisMotion::SlideIn) => "overlay.x",
        (_, EmphasisMotion::RotateNudge) => "overlay.rotation_deg",
        _ => "overlay.scale",
    }
}

fn easing_for(style: &str) -> Easing {
    match style {
        "heavy" => Easing::EaseOutCubic,
        "loose" => Easing::EaseOutBack,
        "snappy" => Easing::EaseOutExpo,
        _ => Easing::EaseOut,
    }
}

fn spring_for(style: &str) -> Option<SpringParameters> {
    match style {
        "snappy" => Some(SpringParameters {
            mass: 1.0,
            stiffness: 180.0,
            damping: 18.0,
        }),
        "loose" => Some(SpringParameters {
            mass: 1.0,
            stiffness: 90.0,
            damping: 8.0,
        }),
        "heavy" => Some(SpringParameters {
            mass: 1.6,
            stiffness: 120.0,
            damping: 22.0,
        }),
        _ => None,
    }
}

fn default_duration(motion: EmphasisMotion, style: &str) -> f64 {
    match (motion, style) {
        (EmphasisMotion::SlowZoom, _) => 1.2,
        (_, "heavy") => 0.42,
        (_, "loose") => 0.5,
        _ => 0.28,
    }
}

fn slide_values(direction: Option<&str>, signals: &EmphasisSignals) -> (f64, f64) {
    match inferred_direction(direction, signals).as_deref() {
        Some("right") => (-0.055, 0.0),
        Some("left") => (0.055, 0.0),
        _ => (-0.04, 0.0),
    }
}

fn direction_sign(direction: Option<&str>, signals: &EmphasisSignals) -> f64 {
    match inferred_direction(direction, signals).as_deref() {
        Some("left") | Some("up") => -1.0,
        _ => 1.0,
    }
}

fn inferred_direction(direction: Option<&str>, signals: &EmphasisSignals) -> Option<String> {
    match direction {
        Some("auto") | None => signals.motion_direction.clone(),
        Some(value) => Some(value.to_string()),
    }
}

fn reason(objective: &str, motion: EmphasisMotion, signals: &EmphasisSignals) -> String {
    let subject = signals
        .subject_center
        .map(|p| {
            format!(
                " Subject center {:.2},{:.2} is available for placement checks.",
                p[0], p[1]
            )
        })
        .unwrap_or_else(|| {
            " Subject center is missing, so this avoids face-dependent placement.".into()
        });
    let action = signals
        .action_score
        .map(|score| format!(" Action score {score:.2}."))
        .unwrap_or_default();
    format!(
        "{objective} maps to {} because it is executable as a runtime parameter animation without requiring a cut or unsupported time remap.{subject}{action}",
        motion.as_str()
    )
}

fn reason_for_animation(motion: EmphasisMotion) -> String {
    match motion {
        EmphasisMotion::ScalePunch => {
            "Brief scale punch to create emphasis inside the clip.".into()
        }
        EmphasisMotion::SlowZoom => "Slow zoom to settle attention without creating a cut.".into(),
        EmphasisMotion::SlideIn => {
            "Directional slide to follow screen motion into the emphasized beat.".into()
        }
        EmphasisMotion::RotateNudge => {
            "Small rotation nudge for a style accent while returning to neutral.".into()
        }
    }
}

fn alternates(motion: EmphasisMotion) -> Vec<serde_json::Value> {
    let mut alternates = Vec::new();
    if motion != EmphasisMotion::ScalePunch {
        alternates.push(serde_json::json!({
            "motion": "scale_punch",
            "reason": "Lowest-risk emphasis when composition signals are incomplete."
        }));
    }
    alternates.push(serde_json::json!({
        "motion": "time_ramp",
        "reason": "Bend pacing into the beat with a time remap curve when an animation isn't enough; use sparingly because it retimes the underlying media."
    }));
    alternates
}

fn animations_edl(animations: &[ParameterAnimation]) -> Result<String, String> {
    let mut lines = vec!["*** Begin EDL".to_string()];
    for animation in animations {
        lines.push("*** Set Parameter Animation".to_string());
        lines.push(format!(
            "+ animation_json: {}",
            serde_json::to_string(animation).map_err(|e| {
                format!("plan_emphasis: failed to serialize animation: {e}")
            })?
        ));
    }
    lines.push("*** End EDL".to_string());
    Ok(lines.join("\n"))
}

fn parse_signals(value: &serde_json::Value) -> EmphasisSignals {
    EmphasisSignals {
        subject_center: first_point(
            value,
            &[
                "/subject_center",
                "/visual_context/subject_center",
                "/visual_signals/current/subject_center",
                "/visual_signals/outgoing/subject_center",
                "/visual_signals/incoming/subject_center",
            ],
        ),
        motion_direction: first_string(
            value,
            &[
                "/motion_direction",
                "/dominant_direction",
                "/visual_context/dominant_direction",
                "/visual_signals/current/motion_direction",
                "/visual_signals/outgoing/motion_direction",
                "/visual_signals/incoming/motion_direction",
            ],
        ),
        motion_vector: first_point(
            value,
            &[
                "/motion_vector",
                "/visual_context/motion_vector",
                "/visual_signals/current/motion_vector",
            ],
        ),
        action_score: first_f64(value, &["/action_score", "/visual_context/action_score"]),
        whip_pan_score: first_f64(
            value,
            &["/whip_pan_score", "/visual_context/whip_pan_score"],
        ),
        occlusion_score: first_f64(
            value,
            &["/occlusion_score", "/visual_context/occlusion_score"],
        ),
        has_saliency_map: value.pointer("/saliency_map").is_some()
            || value.pointer("/visual_context/saliency_map").is_some(),
    }
}

fn first_point(value: &serde_json::Value, pointers: &[&str]) -> Option<[f64; 2]> {
    pointers
        .iter()
        .filter_map(|pointer| value.pointer(pointer))
        .find_map(point2)
}

fn first_string(value: &serde_json::Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .filter_map(|pointer| value.pointer(pointer))
        .find_map(|value| value.as_str().map(str::to_string))
}

fn first_f64(value: &serde_json::Value, pointers: &[&str]) -> Option<f64> {
    pointers
        .iter()
        .filter_map(|pointer| value.pointer(pointer))
        .find_map(serde_json::Value::as_f64)
}

fn point2(value: &serde_json::Value) -> Option<[f64; 2]> {
    let arr = value.as_array()?;
    if arr.len() != 2 {
        return None;
    }
    Some([arr[0].as_f64()?, arr[1].as_f64()?])
}

fn missing_signals(signals: &EmphasisSignals) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if signals.subject_center.is_none() {
        missing.push("subject_center");
    }
    if signals.motion_vector.is_none() {
        missing.push("motion_vector");
    }
    if !signals.has_saliency_map {
        missing.push("saliency_map");
    }
    missing
}

fn animation_id(clip_id: &str, motion: &str, start: f64) -> String {
    let clip = clip_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    format!(
        "emphasis_{clip}_{motion}_{:04}",
        (start * 1000.0).round() as u64
    )
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Read-only emphasis planner. Given a clip id, optional beat times, visual \
context, and an LLM-friendly style word, returns the strongest executable \
single-clip parameter animation plus a parseable Set Parameter Animation EDL \
fragment. Use it when the job is emphasis inside a clip, not a cut boundary.\
";
