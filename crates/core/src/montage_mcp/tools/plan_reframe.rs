//! `plan_reframe` — read-only planner for static subject-aware reframes.
//! Ported from `crates/core/src/tools/plan_reframe.rs` to the in-process
//! MCP server. Emits an `montage.reframe` EDL fragment for `apply_edl`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `plan_reframe`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanReframeArgs {
    /// Clip id to reframe.
    pub clip_id: String,
    /// Delivery aspect ratio. Defaults to vertical 9:16.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Source media width in pixels.
    #[serde(default)]
    pub source_width: Option<u32>,
    /// Source media height in pixels.
    #[serde(default)]
    pub source_height: Option<u32>,
    /// Optional normalized subject center.
    #[serde(default)]
    pub subject_center: Option<SubjectCenter>,
    /// Optional context packet from vision tools with a `subject_center`.
    #[serde(default)]
    pub context: Option<serde_json::Value>,
    /// Optional delivery safe-area label, persisted in planner output.
    #[serde(default)]
    pub safe_area: Option<String>,
    /// Optional explicit zoom override.
    #[serde(default)]
    pub zoom: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
pub struct SubjectCenter {
    pub x: f64,
    pub y: f64,
}

/// Run `plan_reframe`. The project root from [`McpToolCtx`] is unused —
/// this planner builds purely from `args`.
pub fn run(args: PlanReframeArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let plan = build_plan(&args)?;
    Ok(plan.to_string())
}

fn build_plan(args: &PlanReframeArgs) -> Result<serde_json::Value, String> {
    let clip_id = args.clip_id.trim();
    if clip_id.is_empty() {
        return Err("plan_reframe: clip_id must be non-empty.".into());
    }

    let aspect_ratio = args.aspect_ratio.as_deref().unwrap_or("9:16");
    let target_aspect = parse_ratio(aspect_ratio)?;
    let mut missing_signals = Vec::new();
    let source_aspect = match (args.source_width, args.source_height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            f64::from(width) / f64::from(height)
        }
        _ => {
            missing_signals.push("source_dimensions");
            16.0 / 9.0
        }
    };
    let zoom = args
        .zoom
        .unwrap_or_else(|| zoom_for_aspect(source_aspect, target_aspect))
        .clamp(1.0, 3.0);
    if !zoom.is_finite() || zoom < 1.0 {
        return Err("plan_reframe: zoom must be finite and >= 1.".into());
    }

    let center = args
        .subject_center
        .or_else(|| args.context.as_ref().and_then(subject_center_from_context));
    let (x, y, basis) = match center {
        Some(center) => {
            validate_subject_center(center)?;
            (
                round2((center.x - 0.5) * 2.0),
                round2((center.y - 0.5) * 2.0),
                "subject_center",
            )
        }
        None => {
            missing_signals.push("subject_center");
            (0.0, 0.0, "center_crop")
        }
    };

    let rationale = if basis == "subject_center" {
        format!("Subject-aware {aspect_ratio} reframe")
    } else {
        format!("Center {aspect_ratio} reframe")
    };
    let params_json = format!(
        "{{\"zoom\":{},\"x\":{},\"y\":{}}}",
        fmt_json_number(zoom),
        fmt_json_number(x),
        fmt_json_number(y),
    );
    let edl_fragment = format!(
        "*** Begin EDL\n\
         *** Set Effect\n\
         + anchor: clip_uuid={clip_id}\n\
         + effect: montage.reframe\n\
         + params_json: {params_json}\n\
         + rationale: {rationale}\n\
         *** End EDL\n"
    );

    Ok(serde_json::json!({
        "recommended": {
            "decision": "set_reframe_effect",
            "clip_id": clip_id,
            "aspect_ratio": aspect_ratio,
            "zoom": round2(zoom),
            "x": x,
            "y": y,
            "basis": basis,
            "safe_area": args.safe_area,
            "reason": reason(aspect_ratio, source_aspect, target_aspect, basis),
        },
        "signals_used": {
            "source_width": args.source_width,
            "source_height": args.source_height,
            "subject_center": center.map(|c| serde_json::json!({"x": c.x, "y": c.y})),
        },
        "missing_signals": missing_signals,
        "edl_fragment": edl_fragment,
        "next_step": "Pass edl_fragment to apply_edl, then render/review.",
    }))
}

fn parse_ratio(raw: &str) -> Result<f64, String> {
    let Some((left, right)) = raw.split_once(':') else {
        return Err(format!(
            "plan_reframe: unsupported aspect_ratio {raw:?}; expected 9:16, 1:1, 4:5, or 16:9."
        ));
    };
    let width = left.parse::<f64>().ok();
    let height = right.parse::<f64>().ok();
    match (width, height) {
        (Some(width), Some(height)) if width > 0.0 && height > 0.0 => Ok(width / height),
        _ => Err(format!(
            "plan_reframe: unsupported aspect_ratio {raw:?}; expected positive W:H."
        )),
    }
}

fn zoom_for_aspect(source_aspect: f64, target_aspect: f64) -> f64 {
    if source_aspect <= target_aspect {
        1.05
    } else {
        (source_aspect / target_aspect).max(1.05)
    }
}

fn subject_center_from_context(value: &serde_json::Value) -> Option<SubjectCenter> {
    let object = value
        .get("subject_center")
        .or_else(|| value.get("center"))?;
    Some(SubjectCenter {
        x: object.get("x")?.as_f64()?,
        y: object.get("y")?.as_f64()?,
    })
}

fn validate_subject_center(center: SubjectCenter) -> Result<(), String> {
    if !center.x.is_finite() || !(0.0..=1.0).contains(&center.x) {
        return Err("plan_reframe: subject_center.x must be in 0..=1.".into());
    }
    if !center.y.is_finite() || !(0.0..=1.0).contains(&center.y) {
        return Err("plan_reframe: subject_center.y must be in 0..=1.".into());
    }
    Ok(())
}

fn reason(aspect_ratio: &str, source_aspect: f64, target_aspect: f64, basis: &str) -> String {
    format!(
        "Plans a static {aspect_ratio} reframe from source aspect {:.3} to target aspect {:.3}, using {basis} evidence. Apply and review the render before delivery.",
        source_aspect, target_aspect
    )
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn fmt_json_number(value: f64) -> String {
    let value = round2(value);
    if value.fract().abs() < 1e-9 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

pub const DESCRIPTION: &str = "\
Plan a static subject-aware reframe for vertical, square, or social \
delivery. Returns an EDL fragment that sets the `montage.reframe` clip \
effect; the agent must pass that fragment to `apply_edl` to mutate the \
timeline. Use this after `find_speaker_oncam`, gaze/face evidence, or \
manual subject-center evidence identifies where the important subject sits \
in frame.\
";
