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
    /// Optional project-relative still asset path. Ignored when `template`
    /// is set.
    #[serde(default)]
    pub image_asset: Option<String>,
    /// Exact on-screen headline text, drawn from the transcript
    /// evidence. Required unless `evidence_text` is provided to derive
    /// it from. Ignored when `template` is set.
    #[serde(default)]
    pub headline: Option<String>,
    /// Exact on-screen labels for step/process scenes, in order.
    /// Required when the request implies steps; generic "Step N"
    /// placeholders are never generated. Ignored when `template` is set.
    #[serde(default)]
    pub step_labels: Vec<String>,
    /// Transcript window backing the scene's content; derives the
    /// headline when `headline` is omitted and is recorded in the
    /// scene rationale. Ignored when `template` is set.
    #[serde(default)]
    pub evidence_text: Option<String>,
    /// Backdrop mode: `"full"` covers the entire frame edge-to-edge
    /// (use for full-frame cards), `"panel"` is an inset card,
    /// `"none"` skips the backdrop. Default: panel when the request
    /// implies a card/diagram or an image is used. Ignored when
    /// `template` is set.
    #[serde(default)]
    pub backdrop: Option<String>,
    /// Optional motion-template name: `"lower_third"`, `"kinetic_text"`,
    /// `"highlight_box"`, or `"progress_bar"`. When set, the matching
    /// typed fields below replace the heuristic layer builder entirely
    /// and their content becomes the on-screen copy — `request` is
    /// never used as on-screen text in template mode either.
    #[serde(default)]
    pub template: Option<String>,
    /// `lower_third`: name displayed in the primary line. Required
    /// when `template` is `"lower_third"`.
    #[serde(default)]
    pub name: Option<String>,
    /// `lower_third`: optional role/title displayed below the name.
    #[serde(default)]
    pub role: Option<String>,
    /// `kinetic_text`: words in display order. Required (non-empty)
    /// when `template` is `"kinetic_text"`.
    #[serde(default)]
    pub words: Vec<PlanMotionSceneWord>,
    /// `kinetic_text`: optional screen anchor for the text block —
    /// `"lower_left"`, `"center"`, or `"lower_center"`. Defaults to
    /// `"center"` when omitted.
    #[serde(default)]
    pub anchor: Option<String>,
    /// `highlight_box`: region as fractions of the canvas. Required
    /// when `template` is `"highlight_box"`.
    #[serde(default, rename = "box")]
    pub box_region: Option<PlanMotionSceneBox>,
    /// `highlight_box`: whether the box pulses to draw attention.
    /// Default false.
    #[serde(default)]
    pub pulse: Option<bool>,
    /// `progress_bar`: starting fraction, ending fraction, and
    /// vertical position. Required when `template` is `"progress_bar"`.
    #[serde(default)]
    pub progress: Option<PlanMotionSceneProgress>,
    /// `progress_bar`: optional bar color override; defaults to the
    /// template's own accent color.
    #[serde(default)]
    pub color: Option<String>,
}

/// One word in a `kinetic_text` template's `words` array.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanMotionSceneWord {
    /// Word text, displayed exactly as given.
    pub text: String,
    /// Time the word appears, in scene-local seconds.
    pub at_s: f64,
    /// How long the word holds at full opacity AFTER its pop-in, in
    /// seconds. The layer lives for pop_in (~0.12s) + hold_s, clamped
    /// to the scene end. `0` (default) holds through the rest of the
    /// scene.
    #[serde(default)]
    pub hold_s: f64,
}

/// Region for a `highlight_box` template's `box` field, each a
/// fraction of the canvas.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanMotionSceneBox {
    /// Left offset as a fraction of canvas width.
    pub x: f64,
    /// Top offset as a fraction of canvas height.
    pub y: f64,
    /// Box width as a fraction of canvas width.
    pub width: f64,
    /// Box height as a fraction of canvas height.
    pub height: f64,
}

/// `from`/`to`/`y` for a `progress_bar` template's `progress` field.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanMotionSceneProgress {
    /// Starting fraction in `0..=1`.
    pub from: f64,
    /// Ending fraction in `0..=1`.
    pub to: f64,
    /// Vertical position as a fraction of canvas height.
    pub y: f64,
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
        backdrop: args.backdrop,
        template: args.template,
        name: args.name,
        role: args.role,
        words: args
            .words
            .into_iter()
            .map(|word| (word.text, word.at_s, word.hold_s))
            .collect(),
        anchor: args.anchor,
        box_region: args
            .box_region
            .map(|region| (region.x, region.y, region.width, region.height)),
        pulse: args.pulse,
        progress: args
            .progress
            .map(|progress| (progress.from, progress.to, progress.y)),
        color: args.color,
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
layers are stored with explicit limitations and footage should use B-roll/PiP. \
Pass backdrop='full' for scenes that must cover the whole frame (the default \
panel backdrop is inset). Text boxes are authoritative: the planner wraps and \
shrinks font sizes so text cannot overflow its box, and every layer gets a \
default 0.4s enter/exit fade. Pass `template` to use one of four prebuilt \
motion templates instead of the heuristic layer builder: 'lower_third' \
(name/role card, needs `name` and optional `role`), 'kinetic_text' \
(word-by-word cascade, needs `words`: [{text, at_s, hold_s}]), 'highlight_box' \
(attention box, needs `box`: {x,y,width,height} and optional `pulse`), and \
'progress_bar' (growing bar, needs `progress`: {from,to,y} and optional \
`color`). Template mode's expanded layers replace the heuristic layers \
entirely; on-screen copy still comes only from the typed fields (name, role, \
words[].text), never from request.\
";
