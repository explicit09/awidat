//! Read-only planner for native procedural motion scenes.

use std::collections::BTreeMap;

use async_trait::async_trait;
use awidat_proto::professional::{MotionScene, MotionSceneLayer, MotionSceneLayerKind};
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Planned MotionScene with storage instructions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MotionScenePlan {
    /// Native motion scene document.
    pub scene: MotionScene,
    /// EDL snippet that stores the scene in timeline metadata.
    pub edl: String,
    /// Current preview/render support statement.
    pub render_support: String,
    /// Short rationale for review.
    pub rationale: String,
}

/// Read-only tool wrapper.
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
}

/// Build a minimal native MotionScene plan from a freeform request.
pub fn plan_motion_scene_request(
    request: &str,
    scene_id: Option<&str>,
    duration_s: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    image_asset: Option<&str>,
) -> Result<MotionScenePlan, String> {
    let request = request.trim();
    if request.is_empty() {
        return Err("request must be non-empty".into());
    }

    let duration_s = duration_s.unwrap_or(4.0);
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err("duration_s must be positive".into());
    }
    let fps = fps.unwrap_or(30.0);
    if !fps.is_finite() || fps <= 0.0 {
        return Err("fps must be positive".into());
    }
    let width = width.unwrap_or(1920);
    let height = height.unwrap_or(1080);
    if width == 0 || height == 0 {
        return Err("width and height must be positive".into());
    }

    let layers = planned_layers(request, duration_s, image_asset);

    let scene = MotionScene {
        id: scene_id
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| stable_scene_id(request)),
        start_s: 0.0,
        duration_s,
        fps,
        width,
        height,
        layers,
        rationale: Some("planned as a native layered motion scene".into()),
    };

    if let Some(diagnostic) = scene.validate().into_iter().next() {
        return Err(diagnostic.message);
    }

    let scene_json = serde_json::to_string(&scene)
        .map_err(|error| format!("failed to serialize motion scene: {error}"))?;
    let edl =
        format!("*** Begin EDL\n*** Set Motion Scene\n+ scene_json: {scene_json}\n*** End EDL\n");

    Ok(MotionScenePlan {
        scene,
        edl,
        render_support: "native preview/render supports text, rectangle/solid, project-asset image layers, shared transforms, and layer-local overlay transform animations; video/media layers stay stored with explicit limitations and should use B-roll/PiP for footage".into(),
        rationale: "Use MotionScene for freeform animated explainers, diagrams, kinetic text, charts, and callouts that are not footage-like b-roll.".into(),
    })
}

fn wants_panel_layer(request: &str) -> bool {
    let request = request.to_ascii_lowercase();
    [
        "panel",
        "card",
        "callout",
        "explainer",
        "diagram",
        "step",
        "process",
        "framework",
    ]
    .iter()
    .any(|needle| request.contains(needle))
}

fn wants_callout_layer(request: &str) -> bool {
    let request = request.to_ascii_lowercase();
    ["callout", "arrow", "point to", "highlight", "accent"]
        .iter()
        .any(|needle| request.contains(needle))
}

fn wants_image_layer(request: &str) -> bool {
    let request = request.to_ascii_lowercase();
    [
        "logo",
        "screenshot",
        "product still",
        "product image",
        "diagram",
        "chart",
        "generated png",
        "still overlay",
    ]
    .iter()
    .any(|needle| request.contains(needle))
}

fn planned_layers(
    request: &str,
    duration_s: f64,
    image_asset: Option<&str>,
) -> Vec<MotionSceneLayer> {
    let mut layers = Vec::new();
    let step_count = planned_step_count(request);
    let has_image = wants_image_layer(request) || image_asset.is_some();

    if wants_panel_layer(request) || has_image {
        layers.push(background_panel_layer(duration_s));
    }
    if has_image {
        layers.push(image_layer(request, duration_s, image_asset));
    }
    if wants_callout_layer(request) || has_image {
        layers.push(callout_accent_layer(duration_s));
    }

    layers.push(headline_layer(request, duration_s));
    for index in 0..step_count {
        layers.push(step_text_layer(index, duration_s));
    }
    layers
}

fn planned_step_count(request: &str) -> usize {
    let lower = request.to_ascii_lowercase();
    if contains_any(&lower, &["three-step", "3 step", "3-step", "three step"]) {
        return 3;
    }
    if contains_any(&lower, &["two-step", "2 step", "2-step", "two step"]) {
        return 2;
    }
    if contains_any(&lower, &["step", "process", "framework", "list"]) {
        return 3;
    }
    0
}

fn background_panel_layer(duration_s: f64) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "background-panel".into(),
        kind: MotionSceneLayerKind::Solid,
        from_s: 0.0,
        duration_s,
        z_index: 0,
        params: BTreeMap::from([
            ("x".into(), serde_json::json!(0.08)),
            ("y".into(), serde_json::json!(0.16)),
            ("width".into(), serde_json::json!(0.84)),
            ("height".into(), serde_json::json!(0.68)),
            ("color".into(), serde_json::json!("#101820")),
            ("opacity".into(), serde_json::json!(0.72)),
            (
                "animations".into(),
                serde_json::json!([
                    {
                        "parameter": "overlay.opacity",
                        "keyframes": [
                            { "time_s": 0.0, "value": 0.0 },
                            { "time_s": 0.35, "value": 0.72 }
                        ]
                    }
                ]),
            ),
        ]),
    }
}

fn image_layer(request: &str, duration_s: f64, image_asset: Option<&str>) -> MotionSceneLayer {
    MotionSceneLayer {
        id: if request.to_ascii_lowercase().contains("product") {
            "product-image".into()
        } else {
            "supporting-image".into()
        },
        kind: MotionSceneLayerKind::Image,
        from_s: 0.0,
        duration_s,
        z_index: 5,
        params: BTreeMap::from([
            (
                "asset".into(),
                serde_json::json!(
                    image_asset
                        .filter(|asset| !asset.trim().is_empty())
                        .unwrap_or("generated/overlays/motion-scene-still.png")
                ),
            ),
            ("x".into(), serde_json::json!(0.58)),
            ("y".into(), serde_json::json!(0.22)),
            ("width".into(), serde_json::json!(0.30)),
            ("height".into(), serde_json::json!(0.34)),
            ("opacity".into(), serde_json::json!(1.0)),
            ("fit".into(), serde_json::json!("contain")),
            ("scale".into(), serde_json::json!(1.0)),
            (
                "animations".into(),
                serde_json::json!([
                    {
                        "parameter": "overlay.opacity",
                        "keyframes": [
                            { "time_s": 0.0, "value": 0.0 },
                            { "time_s": 0.4, "value": 1.0 }
                        ]
                    },
                    {
                        "parameter": "overlay.scale",
                        "keyframes": [
                            { "time_s": 0.0, "value": 0.94 },
                            { "time_s": 0.4, "value": 1.0 }
                        ]
                    }
                ]),
            ),
        ]),
    }
}

fn callout_accent_layer(duration_s: f64) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "callout-accent".into(),
        kind: MotionSceneLayerKind::Shape,
        from_s: 0.18,
        duration_s: (duration_s - 0.18).max(0.01),
        z_index: 6,
        params: BTreeMap::from([
            ("shape".into(), serde_json::json!("rectangle")),
            ("x".into(), serde_json::json!(0.53)),
            ("y".into(), serde_json::json!(0.29)),
            ("width".into(), serde_json::json!(0.025)),
            ("height".into(), serde_json::json!(0.18)),
            ("color".into(), serde_json::json!("#F6C85F")),
            ("opacity".into(), serde_json::json!(0.92)),
            ("rotation_deg".into(), serde_json::json!(0.0)),
            (
                "animations".into(),
                serde_json::json!([
                    {
                        "parameter": "overlay.opacity",
                        "keyframes": [
                            { "time_s": 0.0, "value": 0.0 },
                            { "time_s": 0.3, "value": 0.92 }
                        ]
                    }
                ]),
            ),
        ]),
    }
}

fn headline_layer(request: &str, duration_s: f64) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "headline".into(),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.0,
        duration_s,
        z_index: 10,
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(headline_text(request))),
            ("layout".into(), serde_json::json!("center_safe")),
            ("animation".into(), serde_json::json!("fade_slide_in")),
        ]),
    }
}

fn step_text_layer(index: usize, duration_s: f64) -> MotionSceneLayer {
    let step_number = index + 1;
    let y = 0.34 + (index as f64 * 0.11);
    MotionSceneLayer {
        id: format!("step-{step_number}-label"),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.2 + (index as f64 * 0.16),
        duration_s: (duration_s - (0.2 + (index as f64 * 0.16))).max(0.01),
        z_index: 12 + i32::try_from(index).unwrap_or(0),
        params: BTreeMap::from([
            (
                "text".into(),
                serde_json::json!(format!("Step {step_number}")),
            ),
            ("layout".into(), serde_json::json!("left_safe")),
            ("x".into(), serde_json::json!(0.14)),
            ("y".into(), serde_json::json!(y)),
            ("width".into(), serde_json::json!(0.36)),
            ("height".into(), serde_json::json!(0.08)),
            ("animation".into(), serde_json::json!("fade_slide_in")),
        ]),
    }
}

fn stable_scene_id(request: &str) -> String {
    let slug = request
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "motion-scene".into()
    } else {
        format!("motion-scene-{slug}")
    }
}

fn headline_text(request: &str) -> String {
    let trimmed = request.trim();
    if trimmed.len() <= 80 {
        return trimmed.to_string();
    }
    let mut text = trimmed.chars().take(77).collect::<String>();
    text.push_str("...");
    text
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

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
                        "description": "Optional project-relative still asset path for logo, screenshot, chart, diagram, or generated PNG image layers."
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
        let plan = plan_motion_scene_request(
            &args.request,
            args.scene_id.as_deref(),
            args.duration_s,
            args.width,
            args.height,
            args.fps,
            args.image_asset.as_deref(),
        )
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
MotionScene plus a Set Motion Scene EDL snippet for apply_edl. Text layers \
text, rectangle/solid, and project-asset image layers are preview/render \
supported; video/media layers are stored with explicit limitations and \
footage should use B-roll/PiP.\
";
