//! Read-only planner for native procedural motion scenes.

use std::collections::BTreeMap;

use async_trait::async_trait;
use montage_proto::professional::{MotionScene, MotionSceneLayer, MotionSceneLayerKind};
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
    #[serde(default)]
    headline: Option<String>,
    #[serde(default)]
    step_labels: Vec<String>,
    #[serde(default)]
    evidence_text: Option<String>,
}

/// Inputs for a MotionScene plan.
///
/// On-screen copy comes from the content fields — `headline`,
/// `step_labels`, and `evidence_text` (a transcript window) — never
/// from the `request` prompt. The planner used to truncate the request
/// into the headline and emit generic "Step 1/2/3" labels, which put
/// the editor's instructions on screen instead of the episode's words.
#[derive(Debug, Clone, Default)]
pub struct MotionScenePlanRequest {
    /// Freeform animated visual need; drives layer selection only.
    pub request: String,
    /// Optional stable scene id. Defaults to a slug derived from request.
    pub scene_id: Option<String>,
    /// Scene duration in seconds. Default 4.
    pub duration_s: Option<f64>,
    /// Canvas width. Default 1920.
    pub width: Option<u32>,
    /// Canvas height. Default 1080.
    pub height: Option<u32>,
    /// Frame rate. Default 30.
    pub fps: Option<f64>,
    /// Optional project-relative still asset path.
    pub image_asset: Option<String>,
    /// Exact on-screen headline, drawn from transcript evidence.
    pub headline: Option<String>,
    /// Exact on-screen labels for step/process scenes, in order.
    pub step_labels: Vec<String>,
    /// Transcript window backing the scene; used to derive a headline
    /// when `headline` is not given and recorded in the scene rationale.
    pub evidence_text: Option<String>,
}

/// Build a minimal native MotionScene plan from a freeform request and
/// explicit on-screen content.
pub fn plan_motion_scene_request(args: &MotionScenePlanRequest) -> Result<MotionScenePlan, String> {
    let request = args.request.trim();
    if request.is_empty() {
        return Err("request must be non-empty".into());
    }

    let duration_s = args.duration_s.unwrap_or(4.0);
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err("duration_s must be positive".into());
    }
    let fps = args.fps.unwrap_or(30.0);
    if !fps.is_finite() || fps <= 0.0 {
        return Err("fps must be positive".into());
    }
    let width = args.width.unwrap_or(1920);
    let height = args.height.unwrap_or(1080);
    if width == 0 || height == 0 {
        return Err("width and height must be positive".into());
    }

    let evidence_text = args
        .evidence_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let headline = resolve_headline(args, evidence_text)?;
    let step_labels = resolve_step_labels(args, request)?;

    let layers = planned_layers(
        request,
        duration_s,
        args.image_asset.as_deref(),
        &headline,
        &step_labels,
    );

    let scene = MotionScene {
        id: args
            .scene_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| stable_scene_id(request)),
        start_s: 0.0,
        duration_s,
        fps,
        width,
        height,
        layers,
        rationale: Some(match evidence_text {
            Some(evidence) => format!(
                "planned from transcript evidence: {}",
                truncate_at_word(evidence, 120)
            ),
            None => "planned as a native layered motion scene".into(),
        }),
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

/// Resolve the on-screen headline: an explicit `headline` arg wins,
/// then the first sentence of the transcript evidence. Without either,
/// hard-fail — the planner must not invent or truncate on-screen copy.
fn resolve_headline(
    args: &MotionScenePlanRequest,
    evidence_text: Option<&str>,
) -> Result<String, String> {
    if let Some(headline) = args
        .headline
        .as_deref()
        .map(str::trim)
        .filter(|headline| !headline.is_empty())
    {
        return Ok(headline.to_string());
    }
    if let Some(evidence) = evidence_text {
        return Ok(headline_from_evidence(evidence));
    }
    Err("on-screen text must come from the edit's transcript evidence, not from this tool. \
Pass `headline` (the exact on-screen headline) or `evidence_text` (the transcript window to \
derive it from), plus `step_labels` for step/process scenes. The planner no longer truncates \
the request prompt into a headline or invents placeholder labels."
        .into())
}

/// Resolve step/panel labels. Explicit `step_labels` win; a request
/// that implies steps without labels hard-fails instead of inventing
/// "Step 1/2/3" placeholders.
fn resolve_step_labels(
    args: &MotionScenePlanRequest,
    request: &str,
) -> Result<Vec<String>, String> {
    let labels: Vec<String> = args
        .step_labels
        .iter()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect();
    if !labels.is_empty() {
        return Ok(labels);
    }
    let implied = planned_step_count(request);
    if implied > 0 {
        return Err(format!(
            "the request implies {implied} step/process labels but none were provided. Pass \
`step_labels` with the exact on-screen text for each step, drawn from the transcript evidence \
(see `evidence_text`). Generic \"Step N\" placeholders are no longer generated."
        ));
    }
    Ok(Vec::new())
}

/// First sentence of the evidence window, capped at a headline-sized
/// length on a word boundary.
fn headline_from_evidence(evidence: &str) -> String {
    let first_sentence = evidence
        .split_inclusive(['.', '!', '?', '\n'])
        .next()
        .unwrap_or(evidence)
        .trim()
        .trim_end_matches(['.', '\n'])
        .trim();
    truncate_at_word(first_sentence, 80)
}

/// Truncate `text` to at most `max_chars`, cutting on a word boundary
/// and appending an ellipsis when shortened.
fn truncate_at_word(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(boundary) if boundary > 0 => &cut[..boundary],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
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
    headline: &str,
    step_labels: &[String],
) -> Vec<MotionSceneLayer> {
    let mut layers = Vec::new();
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

    layers.push(headline_layer(headline, duration_s));
    for (index, label) in step_labels.iter().enumerate() {
        layers.push(step_text_layer(index, label, duration_s));
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

fn headline_layer(headline: &str, duration_s: f64) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "headline".into(),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.0,
        duration_s,
        z_index: 10,
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(headline)),
            ("layout".into(), serde_json::json!("center_safe")),
            ("animation".into(), serde_json::json!("fade_slide_in")),
        ]),
    }
}

fn step_text_layer(index: usize, label: &str, duration_s: f64) -> MotionSceneLayer {
    let step_number = index + 1;
    let y = 0.34 + (index as f64 * 0.11);
    MotionSceneLayer {
        id: format!("step-{step_number}-label"),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.2 + (index as f64 * 0.16),
        duration_s: (duration_s - (0.2 + (index as f64 * 0.16))).max(0.01),
        z_index: 12 + i32::try_from(index).unwrap_or(0),
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(label)),
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
                    },
                    "headline": {
                        "type": "string",
                        "description": "Exact on-screen headline text, drawn from the transcript evidence. Required unless evidence_text is provided to derive it from."
                    },
                    "step_labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Exact on-screen labels for step/process scenes, in order. Required when the request implies steps; generic 'Step N' placeholders are never generated."
                    },
                    "evidence_text": {
                        "type": "string",
                        "description": "Transcript window backing the scene's content; derives the headline when headline is omitted and is recorded in the scene rationale."
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
layers are stored with explicit limitations and footage should use B-roll/PiP.\
";
