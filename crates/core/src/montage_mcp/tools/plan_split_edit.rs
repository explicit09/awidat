//! `plan_split_edit` — read-only J/L-cut planner.
//!
//! Consumes a `transition_context` packet and emits a concrete
//! `Set Audio Lead` or `Set Audio Trail` EDL fragment. This is the
//! audio-led counterpart to `plan_transition`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `plan_split_edit`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanSplitEditArgs {
    /// JSON packet returned by `transition_context`.
    pub context: serde_json::Value,
    /// Optional high-level job: speaker_handoff, thought_continuity,
    /// breath_preservation, reaction_timing.
    #[serde(default)]
    pub objective: Option<String>,
    /// Optional split-edit direction: j_cut or l_cut.
    #[serde(default)]
    pub direction: Option<String>,
}

#[derive(Debug)]
struct ContextSummary {
    from_uuid: String,
    to_uuid: String,
    from_duration_s: Option<f64>,
    to_duration_s: Option<f64>,
    before_word_count: usize,
    after_word_count: usize,
    missing_signals: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitKind {
    JCut,
    LCut,
}

/// Run `plan_split_edit`. The project root from [`McpToolCtx`] is unused
/// because this planner builds from a `transition_context` packet.
pub fn run(args: PlanSplitEditArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let context = parse_context(&args.context)?;
    let kind = split_kind(args.direction.as_deref(), args.objective.as_deref());
    let recommendation = recommend(&context, kind, args.objective.as_deref());
    Ok(recommendation.to_string())
}

fn parse_context(value: &serde_json::Value) -> Result<ContextSummary, String> {
    let from_uuid = value
        .pointer("/between/from/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "plan_split_edit: context is missing between.from.clip_uuid".to_string())?
        .to_string();
    let to_uuid = value
        .pointer("/between/to/clip_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "plan_split_edit: context is missing between.to.clip_uuid".to_string())?
        .to_string();
    let from_duration_s = value.pointer("/from/duration_s").and_then(|v| v.as_f64());
    let to_duration_s = value.pointer("/to/duration_s").and_then(|v| v.as_f64());
    let before_word_count = value
        .pointer("/transcript/before")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);
    let after_word_count = value
        .pointer("/transcript/after")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);
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

    Ok(ContextSummary {
        from_uuid,
        to_uuid,
        from_duration_s,
        to_duration_s,
        before_word_count,
        after_word_count,
        missing_signals,
    })
}

fn split_kind(direction: Option<&str>, objective: Option<&str>) -> SplitKind {
    match direction {
        Some("j_cut") | Some("audio_lead") | Some("lead") => SplitKind::JCut,
        Some("l_cut") | Some("audio_trail") | Some("trail") => SplitKind::LCut,
        _ => match objective {
            Some("thought_continuity" | "breath_preservation") => SplitKind::LCut,
            _ => SplitKind::JCut,
        },
    }
}

fn recommend(
    context: &ContextSummary,
    kind: SplitKind,
    objective: Option<&str>,
) -> serde_json::Value {
    match kind {
        SplitKind::JCut => {
            let lead_s = clamp_offset(0.45, context.to_duration_s);
            serde_json::json!({
                "recommended": {
                    "decision": "split_edit",
                    "kind": "j_cut",
                    "lead_s": round3(lead_s),
                    "intent": objective.unwrap_or("speaker_handoff"),
                    "confidence": confidence(context),
                    "reason": reason(context, SplitKind::JCut),
                },
                "alternates": [
                    {
                        "decision": "hard_cut",
                        "reason": "Use when the incoming audio should not pre-lap the picture."
                    }
                ],
                "edl_fragment": audio_lead_edl(context, lead_s),
            })
        }
        SplitKind::LCut => {
            let trail_s = clamp_offset(0.5, context.from_duration_s);
            serde_json::json!({
                "recommended": {
                    "decision": "split_edit",
                    "kind": "l_cut",
                    "trail_s": round3(trail_s),
                    "intent": objective.unwrap_or("thought_continuity"),
                    "confidence": confidence(context),
                    "reason": reason(context, SplitKind::LCut),
                },
                "alternates": [
                    {
                        "decision": "hard_cut",
                        "reason": "Use when the outgoing audio should not carry under the next picture."
                    }
                ],
                "edl_fragment": audio_trail_edl(context, trail_s),
            })
        }
    }
}

fn clamp_offset(default_s: f64, clip_duration_s: Option<f64>) -> f64 {
    let max_safe = clip_duration_s
        .map(|duration| (duration * 0.5).min(1.0))
        .unwrap_or(1.0);
    default_s.min(max_safe).max(0.12)
}

fn confidence(context: &ContextSummary) -> f64 {
    if context.before_word_count > 0 && context.after_word_count > 0 {
        0.82
    } else if context.missing_signals.iter().any(|s| s == "whisper_words") {
        0.55
    } else {
        0.68
    }
}

fn reason(context: &ContextSummary, kind: SplitKind) -> String {
    let transcript_text = if context.before_word_count > 0 && context.after_word_count > 0 {
        " Transcript words exist on both sides, so the planner can make a dialogue-aware timing suggestion."
    } else {
        " Transcript evidence is incomplete; render-review the timing by ear."
    };
    match kind {
        SplitKind::JCut => format!(
            "Use a J-cut so incoming audio starts before the picture for a smoother speaker handoff.{transcript_text}"
        ),
        SplitKind::LCut => format!(
            "Use an L-cut so outgoing audio carries under the incoming picture and preserves thought continuity.{transcript_text}"
        ),
    }
}

fn audio_lead_edl(context: &ContextSummary, lead_s: f64) -> String {
    format!(
        "*** Begin EDL\n*** Set Audio Lead\n@@ anchor: clip_uuid={}\n+ lead_s: {:.3}\n+ reason: {}\n+ confidence: {:.3}\n*** End EDL",
        context.to_uuid,
        round3(lead_s),
        reason(context, SplitKind::JCut).replace('\n', " "),
        round3(confidence(context))
    )
}

fn audio_trail_edl(context: &ContextSummary, trail_s: f64) -> String {
    format!(
        "*** Begin EDL\n*** Set Audio Trail\n@@ anchor: clip_uuid={}\n+ trail_s: {:.3}\n+ reason: {}\n+ confidence: {:.3}\n*** End EDL",
        context.from_uuid,
        round3(trail_s),
        reason(context, SplitKind::LCut).replace('\n', " "),
        round3(confidence(context))
    )
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Read-only J-cut/L-cut planner. Pass the JSON object returned by \
transition_context. The tool recommends one audio-led split edit with a \
concrete lead_s or trail_s, reason, confidence, alternate hard-cut \
fallback, and apply-ready Set Audio Lead/Trail EDL fragment. It never \
applies the edit.\
";
