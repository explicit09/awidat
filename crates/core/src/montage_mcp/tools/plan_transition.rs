//! `plan_transition` — read-only transition planner. Ported from
//! `crates/core/src/tools/plan_transition.rs` to the in-process MCP
//! server. Consumes a `transition_context` JSON packet and recommends a
//! hard cut or one motivated visible transition; emits an EDL fragment
//! the agent passes to `apply_edl`.

use montage_proto::transitions::{BuiltinTransition, MotionAlignment, lookup_builtin_transition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `plan_transition`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanTransitionArgs {
    /// JSON packet returned by `transition_context`.
    pub context: serde_json::Value,
    /// Optional high-level job: hide_motion_jump, beat_hit,
    /// soft_time_passage, chapter_reset, visual_match, style_accent.
    #[serde(default)]
    pub objective: Option<String>,
    /// Optional direction hint: left, right, up, down, in, out.
    #[serde(default)]
    pub direction: Option<String>,
}

#[derive(Debug)]
struct ContextSummary {
    from_uuid: String,
    to_uuid: String,
    continuity_verdict: String,
    max_centered_duration_s: Option<f64>,
    missing_signals: Vec<String>,
    outgoing_motion_direction: Option<MotionAlignment>,
    incoming_motion_direction: Option<MotionAlignment>,
    motion_match: Option<String>,
    motion_match_confidence: Option<f64>,
    outgoing_occlusion_score: Option<f64>,
    incoming_occlusion_score: Option<f64>,
    either_side_static: bool,
}

/// Run `plan_transition`. The project root from [`McpToolCtx`] is unused —
/// this planner builds purely from `args`.
pub fn run(args: PlanTransitionArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let context = parse_context(&args.context)?;
    let recommendation = recommend(
        &context,
        args.objective.as_deref(),
        args.direction.as_deref(),
    );
    Ok(recommendation.to_string())
}

fn parse_context(value: &serde_json::Value) -> Result<ContextSummary, String> {
    let from_uuid = value
        .pointer("/between/from/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "plan_transition: context is missing between.from.clip_uuid".to_string())?
        .to_string();
    let to_uuid = value
        .pointer("/between/to/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "plan_transition: context is missing between.to.clip_uuid".to_string())?
        .to_string();
    let continuity_verdict = value
        .pointer("/continuity/verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("abstain")
        .to_string();
    let max_centered_duration_s = value
        .pointer("/handles/max_centered_duration_s")
        .and_then(|v| v.as_f64());
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
    let outgoing_motion_direction = value
        .pointer("/visual_signals/outgoing/motion_direction")
        .and_then(|v| v.as_str())
        .and_then(MotionAlignment::from_hint);
    let incoming_motion_direction = value
        .pointer("/visual_signals/incoming/motion_direction")
        .and_then(|v| v.as_str())
        .and_then(MotionAlignment::from_hint);
    let motion_match = value
        .pointer("/visual_signals/motion_match")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let motion_match_confidence = value
        .pointer("/visual_signals/motion_match_confidence")
        .and_then(|v| v.as_f64());
    let outgoing_occlusion_score = value
        .pointer("/visual_signals/outgoing/occlusion_score")
        .and_then(|v| v.as_f64());
    let incoming_occlusion_score = value
        .pointer("/visual_signals/incoming/occlusion_score")
        .and_then(|v| v.as_f64());
    let either_side_static = value
        .pointer("/visual_signals/either_side_static")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(ContextSummary {
        from_uuid,
        to_uuid,
        continuity_verdict,
        max_centered_duration_s,
        missing_signals,
        outgoing_motion_direction,
        incoming_motion_direction,
        motion_match,
        motion_match_confidence,
        outgoing_occlusion_score,
        incoming_occlusion_score,
        either_side_static,
    })
}

fn recommend(
    context: &ContextSummary,
    objective: Option<&str>,
    direction: Option<&str>,
) -> serde_json::Value {
    let Some(job) = objective.or_else(|| job_from_context(context)) else {
        return hard_cut(
            context,
            "clean_story_rhythm",
            "The transition context does not show a named job for a visible transition; keep the hard cut and record the cut intent instead.",
        );
    };

    let direction = direction
        .and_then(MotionAlignment::from_hint)
        .or_else(|| inferred_motion_direction(context));
    let transition_id = transition_for_job(job, direction);
    let Some(transition) = lookup_builtin_transition(transition_id) else {
        return hard_cut(
            context,
            "transition_unavailable",
            "The requested transition job does not map to a supported Montage transition id.",
        );
    };

    if let Some(reason) = incompatibility_reason(transition, context, job) {
        return hard_cut(context, "transition_incompatible", &reason);
    }

    let duration_s = clamped_duration(transition, context.max_centered_duration_s);
    if duration_s < transition.min_duration_s {
        return hard_cut(
            context,
            "insufficient_handles",
            "The boundary does not have enough safe handles for the lowest useful duration; do not emit an impossible transition.",
        );
    }

    let intent = intent_for_job(job);
    let energy = energy_for_job(job);
    let direction = transition_direction(transition, direction);
    let reason = transition_reason(job, transition, context, direction);
    serde_json::json!({
        "recommended": {
            "decision": "transition",
            "id": transition.id,
            "duration_s": round3(duration_s),
            "intent": intent,
            "energy": energy,
            "direction": direction.map(|d| d.as_str()),
            "reason": reason,
        },
        "alternates": [
            {
                "decision": "hard_cut",
                "reason": "Use when the edit reads cleanly without a visible transition."
            }
        ],
        "edl_fragment": transition_edl(context, transition, duration_s, intent, energy, direction),
    })
}

/// Returns a human reason when the chosen transition's metadata is
/// incompatible with the cut, or `None` when the transition is fine.
fn incompatibility_reason(
    transition: &BuiltinTransition,
    context: &ContextSummary,
    job: &str,
) -> Option<String> {
    if transition.avoid_for.contains(&"static_dialogue") && context.either_side_static {
        return Some(format!(
            "{} is marked avoid_for static scenes and the boundary has a near-static side; \
             prefer a hard cut or a non-motion transition.",
            transition.display_name
        ));
    }
    if transition.avoid_for.contains(&job) {
        return Some(format!(
            "{} is marked avoid_for {job}; the metadata says this transition undermines this \
             editorial job rather than serving it.",
            transition.display_name
        ));
    }
    if transition.avoid_for.contains(&"no_occlusion_signal")
        && max_occlusion_score(context).unwrap_or(0.0) < 0.75
    {
        return Some(format!(
            "{} needs a real occlusion, dark-frame, or mask signal; this boundary does not show \
             enough occlusion evidence, so keep the cut or repair it another way.",
            transition.display_name
        ));
    }
    if transition.requires_motion_continuity && context.motion_match.as_deref() == Some("opposed") {
        return Some(format!(
            "{} requires motion continuity and the boundary shows opposed screen-direction \
             motion; a motion-following transition would cut against the action.",
            transition.display_name
        ));
    }
    None
}

fn max_occlusion_score(context: &ContextSummary) -> Option<f64> {
    match (
        context.outgoing_occlusion_score,
        context.incoming_occlusion_score,
    ) {
        (Some(outgoing), Some(incoming)) => Some(outgoing.max(incoming)),
        (Some(outgoing), None) => Some(outgoing),
        (None, Some(incoming)) => Some(incoming),
        (None, None) => None,
    }
}

fn inferred_motion_direction(context: &ContextSummary) -> Option<MotionAlignment> {
    match context.motion_match.as_deref() {
        Some("aligned") => context
            .outgoing_motion_direction
            .or(context.incoming_motion_direction),
        _ => None,
    }
}

fn job_from_context(context: &ContextSummary) -> Option<&'static str> {
    match context.continuity_verdict.as_str() {
        "dirty" | "risky" => Some("hide_motion_jump"),
        _ => None,
    }
}

fn transition_for_job(job: &str, direction: Option<MotionAlignment>) -> &'static str {
    match job {
        "beat_hit" => "montage.ramp_in_beat",
        "soft_time_passage" => "montage.cross_dissolve",
        "chapter_reset" => "montage.fade_black",
        "visual_match" => "montage.match_dissolve",
        "style_accent" => "montage.motion_blur",
        "occlusion_mask" | "pass_by_motion" | "invisible_scene_move" => {
            pass_by_or_invisible(direction)
        }
        "mask_cut" | "occlusion_or_dark_frame" | "hide_camera_reposition" => {
            "montage.invisible_cut"
        }
        "punch_in" | "forward_momentum" => "montage.zoom_in",
        "spatial_shift" => match direction {
            Some(MotionAlignment::In) | Some(MotionAlignment::Out) => "montage.distance_zoom",
            _ => directional_slide(direction),
        },
        "energy_jump" => match direction {
            Some(MotionAlignment::In) | Some(MotionAlignment::Out) => "montage.distance_zoom",
            _ => "montage.flash_white",
        },
        "stylized_reveal" | "vintage_reveal" | "comic_reveal" => "montage.iris_open",
        "stylized_closure" | "comic_button" => "montage.iris_close",
        "graphic_movement" | "related_scene_change" => directional_wipe(direction),
        "social_push" | "screen_direction" => directional_slide(direction),
        "tech_context" | "glitch_moment" => "montage.pixelize",
        "hide_motion_jump" => match direction {
            Some(MotionAlignment::Left) => "montage.whip_pan_left",
            Some(MotionAlignment::Right) => "montage.whip_pan_right",
            _ => "montage.motion_blur",
        },
        _ => "montage.motion_blur",
    }
}

fn pass_by_or_invisible(direction: Option<MotionAlignment>) -> &'static str {
    match direction {
        Some(MotionAlignment::Left) => "montage.pass_by_left",
        Some(MotionAlignment::Right) => "montage.pass_by_right",
        _ => "montage.invisible_cut",
    }
}

fn directional_wipe(direction: Option<MotionAlignment>) -> &'static str {
    match direction {
        Some(MotionAlignment::Left) => "montage.wipe_left",
        Some(MotionAlignment::Right) => "montage.wipe_right",
        Some(MotionAlignment::Up) => "montage.wipe_up",
        Some(MotionAlignment::Down) => "montage.wipe_down",
        _ => "montage.radial",
    }
}

fn directional_slide(direction: Option<MotionAlignment>) -> &'static str {
    match direction {
        Some(MotionAlignment::Left) => "montage.slide_left",
        Some(MotionAlignment::Right) => "montage.slide_right",
        Some(MotionAlignment::Up) => "montage.slide_up",
        _ => "montage.smooth_push_left",
    }
}

fn intent_for_job(job: &str) -> &'static str {
    match job {
        "beat_hit" => "beat_hit",
        "soft_time_passage" => "soft_time_passage",
        "chapter_reset" => "chapter_reset",
        "visual_match" => "visual_match",
        "style_accent" => "style_accent",
        "occlusion_mask" | "pass_by_motion" | "invisible_scene_move" => "invisible_scene_move",
        "mask_cut" | "occlusion_or_dark_frame" | "hide_camera_reposition" => "mask_cut",
        "punch_in" | "forward_momentum" => "punch_in",
        "spatial_shift" => "spatial_shift",
        "energy_jump" => "energy_jump",
        "stylized_reveal" | "vintage_reveal" | "comic_reveal" => "stylized_reveal",
        "stylized_closure" | "comic_button" => "stylized_closure",
        "graphic_movement" | "related_scene_change" => "graphic_movement",
        "social_push" | "screen_direction" => "screen_direction",
        "tech_context" | "glitch_moment" => "glitch_moment",
        _ => "hide_motion_jump",
    }
}

fn energy_for_job(job: &str) -> f64 {
    match job {
        "beat_hit" => 0.82,
        "chapter_reset" => 0.55,
        "soft_time_passage" | "visual_match" => 0.42,
        "occlusion_mask" | "pass_by_motion" | "invisible_scene_move" => 0.72,
        "mask_cut" | "occlusion_or_dark_frame" | "hide_camera_reposition" => 0.40,
        "punch_in" | "forward_momentum" | "spatial_shift" => 0.70,
        "energy_jump" | "tech_context" | "glitch_moment" => 0.82,
        "stylized_reveal" | "vintage_reveal" | "comic_reveal" => 0.56,
        "stylized_closure" | "comic_button" => 0.58,
        "graphic_movement" | "related_scene_change" | "social_push" | "screen_direction" => 0.64,
        "style_accent" => 0.62,
        _ => 0.68,
    }
}

fn transition_direction(
    transition: &BuiltinTransition,
    direction: Option<MotionAlignment>,
) -> Option<MotionAlignment> {
    transition.motion_alignment.or(direction)
}

fn clamped_duration(transition: &BuiltinTransition, max_centered_s: Option<f64>) -> f64 {
    let max_safe = max_centered_s.unwrap_or(transition.default_duration_s);
    transition
        .default_duration_s
        .min(max_safe)
        .min(transition.max_duration_s)
}

fn transition_reason(
    job: &str,
    transition: &BuiltinTransition,
    context: &ContextSummary,
    direction: Option<MotionAlignment>,
) -> String {
    let direction_text = direction
        .map(|d| format!(" following {} screen motion", d.as_str()))
        .unwrap_or_default();
    let motion_match_text = match context.motion_match.as_deref() {
        Some("aligned") if transition.requires_motion_continuity => {
            " Outgoing and incoming motion are aligned, which matches this transition's motion-continuity expectation.".to_string()
        }
        Some("opposed") if transition.requires_motion_continuity => {
            " Note: motion appears opposed; verify direction before applying.".to_string()
        }
        _ => String::new(),
    };
    let confidence_text = context
        .motion_match_confidence
        .map(|c| format!(" Motion confidence {c:.2}."))
        .unwrap_or_default();
    let missing_text = if context.missing_signals.is_empty() {
        String::new()
    } else {
        format!(
            " Missing signals: {}; verify visually before applying.",
            context.missing_signals.join(", ")
        )
    };
    format!(
        "{job} gives this transition a named job; {} is the lowest supported visible transition for that job{direction_text}.{motion_match_text}{confidence_text}{missing_text}",
        transition.display_name
    )
}

fn hard_cut(context: &ContextSummary, intent: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "recommended": {
            "decision": "hard_cut",
            "intent": intent,
            "reason": reason,
        },
        "alternates": [],
        "edl_fragment": set_cut_intent_edl(context, intent, reason),
    })
}

fn transition_edl(
    context: &ContextSummary,
    transition: &BuiltinTransition,
    duration_s: f64,
    intent: &str,
    energy: f64,
    direction: Option<MotionAlignment>,
) -> String {
    let mut lines = vec![
        "*** Begin EDL".to_string(),
        "*** Insert Transition".to_string(),
        format!(
            "@@ between: clip_uuid={} and clip_uuid={}",
            context.from_uuid, context.to_uuid
        ),
        format!("+ id: {}", transition.id),
        format!("+ kind: {}", transition.id),
        format!("+ family: {}", transition.family),
        format!("+ intent: {intent}"),
        format!("+ energy: {:.3}", round3(energy)),
    ];
    if let Some(direction) = direction {
        lines.push(format!("+ direction: {}", direction.as_str()));
    }
    lines.extend([
        format!("+ duration_s: {:.3}", round3(duration_s)),
        "+ alignment: center".to_string(),
        "*** End EDL".to_string(),
    ]);
    lines.join("\n")
}

fn set_cut_intent_edl(context: &ContextSummary, intent: &str, reason: &str) -> String {
    format!(
        "*** Begin EDL\n*** Set Cut Intent\n@@ between: clip_uuid={} and clip_uuid={}\n+ cut_type: hard_cut\n+ intent: {intent}\n+ audio_relation: sync\n+ reason: {}\n*** End EDL",
        context.from_uuid,
        context.to_uuid,
        reason.replace('\n', " ")
    )
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Read-only transition planner. Pass the JSON object returned by \
transition_context. The tool recommends either a hard cut with Set Cut \
Intent metadata or one supported visible transition with a named job, safe \
duration, reason, alternate, and EDL fragment. It never applies the edit.\
";
