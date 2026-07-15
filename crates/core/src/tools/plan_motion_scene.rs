//! Read-only planner tool for native procedural motion scenes.
//!
//! Thin `ToolHandler` wrapper over the shared planner in
//! [`crate::motion_scene`], which owns the layer heuristics, the
//! motion-template expansion path, and the on-screen content contract.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

pub struct PlanMotionSceneTool;

#[derive(Debug, Deserialize)]
struct PlanMotionSceneArgs {
    request: String,
    #[serde(default)]
    scene_id: Option<String>,
    #[serde(default)]
    duration_s: Option<f64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    fps: Option<f64>,
    #[serde(default)]
    image_asset: Option<String>,
    #[serde(default)]
    headline: Option<String>,
    #[serde(default)]
    step_labels: Vec<String>,
    #[serde(default)]
    evidence_text: Option<String>,
    #[serde(default)]
    backdrop: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    words: Vec<TemplateWordArg>,
    #[serde(default)]
    anchor: Option<String>,
    #[serde(default, rename = "box")]
    box_region: Option<TemplateBoxArg>,
    #[serde(default)]
    pulse: Option<bool>,
    #[serde(default)]
    progress: Option<TemplateProgressArg>,
    #[serde(default)]
    color: Option<String>,
}

/// One word in a `kinetic_text` template's `words` array.
#[derive(Debug, Deserialize)]
struct TemplateWordArg {
    text: String,
    at_s: f64,
    #[serde(default)]
    hold_s: f64,
}

/// Region for a `highlight_box` template's `box` field.
#[derive(Debug, Deserialize)]
struct TemplateBoxArg {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// `from`/`to`/`y` for a `progress_bar` template's `progress` field.
#[derive(Debug, Deserialize)]
struct TemplateProgressArg {
    from: f64,
    to: f64,
    y: f64,
}

/// Inputs for a MotionScene plan.
///
/// On-screen copy comes from the content fields — `headline`,
/// `step_labels`, and `evidence_text` (a transcript window) — never
/// from the `request` prompt. The planner used to truncate the request
/// into the headline and emit generic "Step 1/2/3" labels, which put
/// the editor's instructions on screen instead of the episode's words.

#[async_trait]
impl ToolHandler for PlanMotionSceneTool {
    fn name(&self) -> &'static str {
        "plan_motion_scene"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "The freeform animated visual need, such as an explainer, diagram, kinetic-text section, chart, or callout."
                    },
                    "scene_id": {
                        "type": "string",
                        "description": "Optional stable scene id. Defaults to a slug derived from request."
                    },
                    "duration_s": {
                        "type": "number",
                        "minimum": 0.01,
                        "description": "Scene duration in seconds. Default 4."
                    },
                    "width": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Canvas width. Default 1920."
                    },
                    "height": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Canvas height. Default 1080."
                    },
                    "fps": {
                        "type": "number",
                        "minimum": 0.01,
                        "description": "Frame rate. Default 30."
                    },
                    "image_asset": {
                        "type": "string",
                        "description": "Optional project-relative still asset path for logo, screenshot, chart, diagram, or generated PNG image layers. Ignored when `template` is set."
                    },
                    "headline": {
                        "type": "string",
                        "description": "Exact on-screen headline text, drawn from the transcript evidence. Required unless evidence_text is provided to derive it from. Ignored when `template` is set."
                    },
                    "step_labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Exact on-screen labels for step/process scenes, in order. Required when the request implies steps; generic 'Step N' placeholders are never generated. Ignored when `template` is set."
                    },
                    "evidence_text": {
                        "type": "string",
                        "description": "Transcript window backing the scene's content; derives the headline when headline is omitted and is recorded in the scene rationale. Ignored when `template` is set."
                    },
                    "backdrop": {
                        "type": "string",
                        "enum": ["full", "panel", "none"],
                        "description": "Backdrop mode: 'full' covers the entire frame edge-to-edge (use for full-frame cards), 'panel' is an inset card, 'none' skips the backdrop. Default: panel when the request implies a card/diagram or an image is used. Ignored when `template` is set."
                    },
                    "template": {
                        "type": "string",
                        "enum": ["lower_third", "kinetic_text", "highlight_box", "progress_bar"],
                        "description": "Optional motion-template name. When set, the template's typed fields below (name/role, words, box/pulse, or progress/color) replace the heuristic layer builder entirely — on-screen copy comes only from those typed fields, never from request. Omit for the freeform heuristic planner (unchanged behavior)."
                    },
                    "name": {
                        "type": "string",
                        "description": "lower_third: name displayed in the primary line. Required when template is 'lower_third'."
                    },
                    "role": {
                        "type": "string",
                        "description": "lower_third: optional role/title displayed below the name."
                    },
                    "words": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "at_s": { "type": "number", "minimum": 0 },
                                "hold_s": { "type": "number", "minimum": 0 }
                            },
                            "required": ["text", "at_s"]
                        },
                        "description": "kinetic_text: words in display order, each with its own timing. `at_s` is when the word appears (scene-local seconds, must be before duration_s); `hold_s` is how long it holds at full opacity AFTER its pop-in — the layer lives for pop_in (~0.12s) + hold_s, clamped to the scene end (0 = holds to the scene's end). Required (non-empty) when template is 'kinetic_text'."
                    },
                    "anchor": {
                        "type": "string",
                        "enum": ["lower_left", "center", "lower_center"],
                        "description": "kinetic_text: optional screen anchor for the text block. Defaults to 'center' when omitted."
                    },
                    "box": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number", "minimum": 0, "maximum": 1 },
                            "y": { "type": "number", "minimum": 0, "maximum": 1 },
                            "width": { "type": "number", "minimum": 0, "maximum": 1 },
                            "height": { "type": "number", "minimum": 0, "maximum": 1 }
                        },
                        "required": ["x", "y", "width", "height"],
                        "description": "highlight_box: region as fractions of the canvas. Required when template is 'highlight_box'."
                    },
                    "pulse": {
                        "type": "boolean",
                        "description": "highlight_box: whether the box pulses to draw attention. Default false."
                    },
                    "progress": {
                        "type": "object",
                        "properties": {
                            "from": { "type": "number", "minimum": 0, "maximum": 1 },
                            "to": { "type": "number", "minimum": 0, "maximum": 1 },
                            "y": { "type": "number", "minimum": 0, "maximum": 1 }
                        },
                        "required": ["from", "to", "y"],
                        "description": "progress_bar: starting fraction, ending fraction, and vertical position, each a fraction of the canvas. Required when template is 'progress_bar'."
                    },
                    "color": {
                        "type": "string",
                        "description": "progress_bar: optional bar color override (e.g. '#00FF00'); defaults to the template's own accent color."
                    }
                },
                "required": ["request"]
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
        let args: PlanMotionSceneArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_motion_scene: invalid args ({e}). Required: request."
            ))
        })?;
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
        .map_err(|message| {
            FunctionCallError::RespondToModel(format!("plan_motion_scene: {message}"))
        })?;
        let body = serde_json::to_string_pretty(&plan).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_motion_scene: failed to serialize plan: {e}"
            ))
        })?;
        Ok(ToolOutput::text(body))
    }
}

const DESCRIPTION: &str = "\
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
