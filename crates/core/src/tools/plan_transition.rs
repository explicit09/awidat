//! `plan_transition` agent tool.
//!
//! Consumes a `transition_context` packet and recommends either a hard
//! cut or one motivated visible transition. The tool is read-only: the
//! returned EDL fragment still has to go through `apply_edl`.

use async_trait::async_trait;
use awidat_proto::transitions::{BuiltinTransition, MotionAlignment, lookup_builtin_transition};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Read-only transition planner.
pub struct PlanTransitionTool;

#[derive(Debug, Deserialize)]
struct PlanTransitionArgs {
    /// JSON packet returned by `transition_context`.
    context: serde_json::Value,
    /// Optional high-level job: hide_motion_jump, beat_hit,
    /// soft_time_passage, chapter_reset, visual_match, style_accent.
    #[serde(default)]
    objective: Option<String>,
    /// Optional direction hint: left, right, up, down, in, out.
    #[serde(default)]
    direction: Option<String>,
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
    either_side_static: bool,
}

#[async_trait]
impl ToolHandler for PlanTransitionTool {
    fn name(&self) -> &'static str {
        "plan_transition"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_transition".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "context": {
                        "type": "object",
                        "description": "The JSON object returned by transition_context."
                    },
                    "objective": {
                        "type": "string",
                        "enum": [
                            "hide_motion_jump",
                            "beat_hit",
                            "soft_time_passage",
                            "chapter_reset",
                            "visual_match",
                            "style_accent"
                        ],
                        "description": "Optional named transition job."
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["left", "right", "up", "down", "in", "out"],
                        "description": "Optional screen/motion direction hint."
                    }
                },
                "required": ["context"]
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
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: PlanTransitionArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_transition: invalid args ({e}). Required: context from transition_context."
            ))
        })?;
        let context = parse_context(&args.context)?;
        let recommendation = recommend(
            &context,
            args.objective.as_deref(),
            args.direction.as_deref(),
        );
        Ok(ToolOutput::text(recommendation.to_string()))
    }
}

fn parse_context(value: &serde_json::Value) -> Result<ContextSummary, FunctionCallError> {
    let from_uuid = value
        .pointer("/between/from/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "plan_transition: context is missing between.from.clip_uuid".into(),
            )
        })?
        .to_string();
    let to_uuid = value
        .pointer("/between/to/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "plan_transition: context is missing between.to.clip_uuid".into(),
            )
        })?
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
            "The requested transition job does not map to a supported Awidat transition id.",
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
    if transition.requires_motion_continuity && context.motion_match.as_deref() == Some("opposed") {
        return Some(format!(
            "{} requires motion continuity and the boundary shows opposed screen-direction \
             motion; a motion-following transition would cut against the action.",
            transition.display_name
        ));
    }
    None
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
        "beat_hit" => "awidat.ramp_in_beat",
        "soft_time_passage" => "awidat.cross_dissolve",
        "chapter_reset" => "awidat.fade_black",
        "visual_match" => "awidat.match_dissolve",
        "style_accent" => "awidat.motion_blur",
        "hide_motion_jump" => match direction {
            Some(MotionAlignment::Left) => "awidat.whip_pan_left",
            Some(MotionAlignment::Right) => "awidat.whip_pan_right",
            _ => "awidat.motion_blur",
        },
        _ => "awidat.motion_blur",
    }
}

fn intent_for_job(job: &str) -> &'static str {
    match job {
        "beat_hit" => "beat_hit",
        "soft_time_passage" => "soft_time_passage",
        "chapter_reset" => "chapter_reset",
        "visual_match" => "visual_match",
        "style_accent" => "style_accent",
        _ => "hide_motion_jump",
    }
}

fn energy_for_job(job: &str) -> f64 {
    match job {
        "beat_hit" => 0.82,
        "chapter_reset" => 0.55,
        "soft_time_passage" | "visual_match" => 0.42,
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

const DESCRIPTION: &str = "\
Read-only transition planner. Pass the JSON object returned by \
transition_context. The tool recommends either a hard cut with Set Cut \
Intent metadata or one supported visible transition with a named job, safe \
duration, reason, alternate, and EDL fragment. It never applies the edit.\
";

#[cfg(test)]
mod tests {
    use super::*;

    fn context(verdict: &str, max_duration_s: f64) -> serde_json::Value {
        serde_json::json!({
            "between": {
                "from": {"clip_uuid": "clip-a"},
                "to": {"clip_uuid": "clip-b"}
            },
            "handles": {
                "max_centered_duration_s": max_duration_s
            },
            "continuity": {
                "verdict": verdict,
                "rules": []
            },
            "missing_signals": ["motion"]
        })
    }

    fn context_with_motion(
        verdict: &str,
        max_duration_s: f64,
        outgoing_direction: Option<&str>,
        incoming_direction: Option<&str>,
        motion_match: &str,
        either_side_static: bool,
    ) -> serde_json::Value {
        serde_json::json!({
            "between": {
                "from": {"clip_uuid": "clip-a"},
                "to": {"clip_uuid": "clip-b"}
            },
            "handles": {
                "max_centered_duration_s": max_duration_s
            },
            "continuity": {
                "verdict": verdict,
                "rules": []
            },
            "visual_signals": {
                "outgoing": {"motion_direction": outgoing_direction},
                "incoming": {"motion_direction": incoming_direction},
                "motion_match": motion_match,
                "motion_match_confidence": 0.6,
                "either_side_static": either_side_static
            },
            "missing_signals": []
        })
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "plan_transition".into(),
            args,
        }
    }

    #[test]
    fn plan_transition_is_read_only() {
        assert!(!PlanTransitionTool.is_mutating(&invoke(serde_json::json!({
            "context": context("clean", 1.0)
        }))));
    }

    #[test]
    fn beat_hit_prefers_speed_ramp_transition() {
        assert_eq!(transition_for_job("beat_hit", None), "awidat.ramp_in_beat");
    }

    #[tokio::test]
    async fn clean_context_prefers_hard_cut_intent() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({"context": context("clean", 1.0)})),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(
            body.pointer("/recommended/decision")
                .and_then(|v| v.as_str()),
            Some("hard_cut")
        );
        assert!(
            body.pointer("/edl_fragment")
                .and_then(|v| v.as_str())
                .is_some_and(|edl| edl.contains("*** Set Cut Intent"))
        );
    }

    #[tokio::test]
    async fn dirty_motion_context_recommends_motivated_transition_edl() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({
                    "context": context("dirty", 0.4),
                    "objective": "hide_motion_jump",
                    "direction": "left"
                })),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(
            body.pointer("/recommended/decision")
                .and_then(|v| v.as_str()),
            Some("transition")
        );
        assert_eq!(
            body.pointer("/recommended/id").and_then(|v| v.as_str()),
            Some("awidat.whip_pan_left")
        );
        let edl = body
            .pointer("/edl_fragment")
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(edl.contains("*** Insert Transition"));
        assert!(edl.contains("+ intent: hide_motion_jump"));
        assert!(edl.contains("+ duration_s: 0.180"));
        crate::edl::parse(edl).expect("visible transition fragment should parse as EDL");
    }

    #[tokio::test]
    async fn opposed_motion_blocks_motion_continuity_transition() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({
                    "context": context_with_motion(
                        "dirty",
                        0.4,
                        Some("left"),
                        Some("right"),
                        "opposed",
                        false,
                    ),
                    "objective": "hide_motion_jump",
                    "direction": "left"
                })),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/recommended/decision")
                .and_then(|v| v.as_str()),
            Some("hard_cut")
        );
        assert!(
            body.pointer("/recommended/reason")
                .and_then(|v| v.as_str())
                .is_some_and(|r| r.contains("motion continuity"))
        );
    }

    #[tokio::test]
    async fn static_side_blocks_motion_blur() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({
                    "context": context_with_motion(
                        "dirty",
                        0.4,
                        Some("left"),
                        Some("left"),
                        "aligned",
                        true,
                    ),
                    "objective": "hide_motion_jump"
                })),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/recommended/decision")
                .and_then(|v| v.as_str()),
            Some("hard_cut")
        );
        assert!(
            body.pointer("/recommended/reason")
                .and_then(|v| v.as_str())
                .is_some_and(|r| r.contains("static"))
        );
    }

    #[tokio::test]
    async fn aligned_motion_infers_direction_when_not_supplied() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({
                    "context": context_with_motion(
                        "dirty",
                        0.4,
                        Some("right"),
                        Some("right"),
                        "aligned",
                        false,
                    ),
                    "objective": "hide_motion_jump"
                })),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/recommended/id").and_then(|v| v.as_str()),
            Some("awidat.whip_pan_right")
        );
        assert_eq!(
            body.pointer("/recommended/direction")
                .and_then(|v| v.as_str()),
            Some("right")
        );
        assert!(
            body.pointer("/recommended/reason")
                .and_then(|v| v.as_str())
                .is_some_and(|r| r.contains("aligned"))
        );
    }

    #[tokio::test]
    async fn insufficient_handles_fall_back_to_hard_cut() {
        let out = PlanTransitionTool
            .handle(
                invoke(serde_json::json!({
                    "context": context("dirty", 0.02),
                    "objective": "hide_motion_jump"
                })),
                fake_ctx(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(
            body.pointer("/recommended/decision")
                .and_then(|v| v.as_str()),
            Some("hard_cut")
        );
        assert!(
            body.pointer("/recommended/reason")
                .and_then(|v| v.as_str())
                .is_some_and(|reason| reason.contains("handles"))
        );
    }

    fn fake_ctx() -> ToolContext {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }
}
