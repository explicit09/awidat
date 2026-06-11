//! `plan_motion_scene` — read-only planner for native procedural motion
//! scenes. Thin MCP wrapper over the shared planner in
//! [`crate::tools::plan_motion_scene`], which owns the layer heuristics
//! and the on-screen content contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::tools::plan_motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};

/// Arguments to `plan_motion_scene`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanMotionSceneArgs {
    /// Freeform animated visual need (explainer, diagram, kinetic-text
    /// section, chart, or callout). Drives layer selection only; it is
    /// never used as on-screen copy.
    pub request: String,
    /// Optional stable scene id. Defaults to a slug derived from request.
    #[serde(default)]
    pub scene_id: Option<String>,
    /// Scene duration in seconds. Default 4.
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// Canvas width. Default 1920.
    #[serde(default)]
    pub width: Option<u32>,
    /// Canvas height. Default 1080.
    #[serde(default)]
    pub height: Option<u32>,
    /// Frame rate. Default 30.
    #[serde(default)]
    pub fps: Option<f64>,
    /// Optional project-relative still asset path.
    #[serde(default)]
    pub image_asset: Option<String>,
    /// Exact on-screen headline text, drawn from the transcript
    /// evidence. Required unless `evidence_text` is provided to derive
    /// it from.
    #[serde(default)]
    pub headline: Option<String>,
    /// Exact on-screen labels for step/process scenes, in order.
    /// Required when the request implies steps; generic "Step N"
    /// placeholders are never generated.
    #[serde(default)]
    pub step_labels: Vec<String>,
    /// Transcript window backing the scene's content; derives the
    /// headline when `headline` is omitted and is recorded in the
    /// scene rationale.
    #[serde(default)]
    pub evidence_text: Option<String>,
}

/// Run `plan_motion_scene`. The project root from [`McpToolCtx`] is
/// unused — this planner builds purely from `args` — but the signature is
/// kept uniform with the rest of the MCP tool surface.
pub fn run(args: PlanMotionSceneArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: args.request,
        scene_id: args.scene_id,
        duration_s: args.duration_s,
        width: args.width,
        height: args.height,
        fps: args.fps,
        image_asset: args.image_asset,
        headline: args.headline,
        step_labels: args.step_labels,
        evidence_text: args.evidence_text,
    })
    .map_err(|message| format!("plan_motion_scene: {message}"))?;
    serde_json::to_string_pretty(&plan)
        .map_err(|e| format!("plan_motion_scene: failed to serialize plan: {e}"))
}

pub const DESCRIPTION: &str = "\
Read-only planner for native procedural MotionScene documents. Use after \
plan_visual_support chooses the motion_scene lane. It returns a valid \
MotionScene plus a Set Motion Scene EDL snippet for apply_edl. On-screen \
copy must come from transcript evidence: pass headline (and step_labels for \
step/process scenes) or evidence_text — the planner never puts the request \
prompt on screen or invents placeholder labels. Text layers, rectangle/solid, \
and project-asset image layers are preview/render supported; video/media \
layers are stored with explicit limitations and footage should use B-roll/PiP.\
";
