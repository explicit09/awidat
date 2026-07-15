//! Shared planner for native procedural motion scenes.
//!
//! Owns the layer heuristics, the motion-template expansion path, and
//! the on-screen content contract. Consumed by both the legacy
//! `ToolHandler` wrapper (`crate::tools::plan_motion_scene`) and the
//! live MCP wrapper (`crate::montage_mcp::tools::plan_motion_scene`).

use std::collections::BTreeMap;

use montage_proto::professional::{MotionScene, MotionSceneLayer, MotionSceneLayerKind};
use serde::Serialize;

use crate::motion_templates::{
    KineticWord, MotionTemplateSpec, TEMPLATE_ACCENT_COLOR, TextAnchor, expand_template,
};

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
    /// Backdrop mode: `"full"` (true full-bleed cover of the whole
    /// frame), `"panel"` (inset card), or `"none"`. Default: panel
    /// when the request implies a card/diagram or an image is used.
    pub backdrop: Option<String>,
    /// Motion-template name: `"lower_third"`, `"kinetic_text"`,
    /// `"highlight_box"`, or `"progress_bar"`. When set, the template's
    /// typed fields below (`name`/`role`, `words`, `box_region`/`pulse`,
    /// or `progress`/`color`) are parsed into a
    /// [`crate::motion_templates::MotionTemplateSpec`] and expanded via
    /// [`crate::motion_templates::expand_template`]; the expansion's
    /// layers replace the heuristic layers entirely and its rationale
    /// flows into the plan. When `None`, behavior is unchanged from the
    /// heuristic planner.
    pub template: Option<String>,
    /// `lower_third`: name displayed in the primary line. Required
    /// when `template` is `"lower_third"`.
    pub name: Option<String>,
    /// `lower_third`: optional role/title displayed below the name.
    pub role: Option<String>,
    /// `kinetic_text`: words in display order as `(text, at_s,
    /// hold_s)`. Required (non-empty) when `template` is
    /// `"kinetic_text"`.
    pub words: Vec<(String, f64, f64)>,
    /// `kinetic_text`: optional screen anchor for the text block —
    /// `"lower_left"`, `"center"`, or `"lower_center"`. Defaults to
    /// `"center"` when omitted.
    pub anchor: Option<String>,
    /// `highlight_box`: region as `(x, y, width, height)`, each a
    /// fraction of the canvas. Required when `template` is
    /// `"highlight_box"`.
    pub box_region: Option<(f64, f64, f64, f64)>,
    /// `highlight_box`: whether the box pulses to draw attention.
    /// Default false.
    pub pulse: Option<bool>,
    /// `progress_bar`: `(from, to, y)`, each a fraction. Required when
    /// `template` is `"progress_bar"`.
    pub progress: Option<(f64, f64, f64)>,
    /// `progress_bar`: optional bar color override; defaults to the
    /// template's own accent color.
    pub color: Option<String>,
}

/// Backdrop behavior for a planned scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackdropMode {
    /// Solid spanning exactly the whole canvas (0,0 → 1,1).
    Full,
    /// Inset panel card (legacy default for cards/diagrams/images).
    Panel,
    /// No backdrop layer.
    None,
    /// Heuristic: panel when the request implies one.
    Auto,
}

impl BackdropMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            Option::None | Some("") => Ok(Self::Auto),
            Some("full") => Ok(Self::Full),
            Some("panel") => Ok(Self::Panel),
            Some("none") => Ok(Self::None),
            Some(other) => Err(format!(
                "backdrop '{other}' not recognized. Use 'full', 'panel', or 'none'."
            )),
        }
    }
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

    let template_expansion = match args.template.as_deref() {
        Some(template) => Some(parse_and_expand_template(template, args, duration_s, fps)?),
        None => None,
    };

    let (layers, scene_rationale) = if let Some(expansion) = template_expansion {
        // Detect which of the five heuristic inputs were provided in template mode.
        let mut ignored_inputs = Vec::new();
        if args.image_asset.as_deref().is_some_and(|s| !s.is_empty()) {
            ignored_inputs.push("image_asset");
        }
        if args.backdrop.as_deref().is_some_and(|s| !s.is_empty()) {
            ignored_inputs.push("backdrop");
        }
        if args.headline.as_deref().is_some_and(|s| !s.is_empty()) {
            ignored_inputs.push("headline");
        }
        if !args.step_labels.is_empty() {
            ignored_inputs.push("step_labels");
        }
        if args.evidence_text.as_deref().is_some_and(|s| !s.is_empty()) {
            ignored_inputs.push("evidence_text");
        }

        let mut rationale = expansion.rationale;
        if !ignored_inputs.is_empty() {
            rationale.push_str(&format!(
                " Ignored inputs not used by template mode: {}.",
                ignored_inputs.join(", ")
            ));
        }
        (expansion.layers, rationale)
    } else {
        let evidence_text = args
            .evidence_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let headline = resolve_headline(args, evidence_text)?;
        let step_labels = resolve_step_labels(args, request)?;
        let backdrop = BackdropMode::parse(args.backdrop.as_deref())?;

        let layers = planned_layers(
            request,
            duration_s,
            args.image_asset.as_deref(),
            &headline,
            &step_labels,
            backdrop,
        );
        let rationale = match evidence_text {
            Some(evidence) => format!(
                "planned from transcript evidence: {}",
                truncate_at_word(evidence, 120)
            ),
            None => "planned as a native layered motion scene".into(),
        };
        (layers, rationale)
    };

    let mut scene = MotionScene {
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
        rationale: Some(scene_rationale),
    };

    fit_scene_text_layers(&mut scene);

    if let Some(diagnostic) = scene.validate().into_iter().next() {
        return Err(diagnostic.message);
    }

    let scene_json = serde_json::to_string(&scene)
        .map_err(|error| format!("failed to serialize motion scene: {error}"))?;
    let edl =
        format!("*** Begin EDL\n*** Set Motion Scene\n+ scene_json: {scene_json}\n*** End EDL\n");

    let mut render_support = "native preview/render supports text, rectangle/solid, project-asset image layers, shared transforms, and layer-local overlay transform animations; video/media layers stay stored with explicit limitations and should use B-roll/PiP for footage".to_string();
    if let Some(note) = scale_motion_preview_only_disclosure(args) {
        render_support.push(' ');
        render_support.push_str(note);
    }

    Ok(MotionScenePlan {
        scene,
        edl,
        render_support,
        rationale: "Use MotionScene for freeform animated explainers, diagrams, kinetic text, charts, and callouts that are not footage-like b-roll.".into(),
    })
}

/// Disclosure for templates whose motion is driven by an `overlay.scale`
/// ramp/pulse that the export/render lowering does not yet honor — the
/// `progress_bar` bar growth and the `highlight_box` pulse. These animate
/// in the LIVE PREVIEW ONLY today; export lowering for scale-driven
/// motion lands in a later phase. Returns `None` for templates whose
/// motion is already fully supported (`lower_third`, `kinetic_text`, and
/// `highlight_box` without pulse), and for the heuristic (non-template)
/// path.
fn scale_motion_preview_only_disclosure(args: &MotionScenePlanRequest) -> Option<&'static str> {
    match args.template.as_deref() {
        Some("progress_bar") => Some(
            "The progress bar's growth is scale-driven and currently animates in the live \
             preview only; export/render lowering for scale-driven motion lands in a later phase.",
        ),
        Some("highlight_box") if args.pulse.unwrap_or(false) => Some(
            "The highlight box's pulse is scale-driven and currently animates in the live \
             preview only; export/render lowering for scale-driven motion lands in a later phase.",
        ),
        _ => None,
    }
}

/// Parse `template` and this request's typed fields into a
/// [`MotionTemplateSpec`] and expand it. On-screen copy comes only
/// from the typed fields (`name`/`role`, `words`, `color`) — never
/// from `request` — matching the content-integrity rule the heuristic
/// path enforces via `resolve_headline`/`resolve_step_labels`. Missing
/// required fields for the selected template fail with an error naming
/// the field.
fn parse_and_expand_template(
    template: &str,
    args: &MotionScenePlanRequest,
    duration_s: f64,
    fps: f64,
) -> Result<crate::motion_templates::TemplateExpansion, String> {
    let spec = match template {
        "lower_third" => {
            let name = args
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    "template 'lower_third' requires `name` (the on-screen name text)".to_string()
                })?;
            MotionTemplateSpec::LowerThird {
                name: name.to_string(),
                role: args
                    .role
                    .as_deref()
                    .map(str::trim)
                    .filter(|role| !role.is_empty())
                    .map(str::to_string),
            }
        }
        "kinetic_text" => {
            if args.words.is_empty() {
                return Err(
                    "template 'kinetic_text' requires `words` (a non-empty array of \
                     {text, at_s, hold_s})"
                        .to_string(),
                );
            }
            let words = args
                .words
                .iter()
                .map(|(text, at_s, hold_s)| KineticWord {
                    text: text.clone(),
                    at_s: *at_s,
                    hold_s: *hold_s,
                })
                .collect();
            MotionTemplateSpec::KineticText {
                words,
                anchor: parse_text_anchor(args.anchor.as_deref())?,
            }
        }
        "highlight_box" => {
            let (x, y, width, height) = args.box_region.ok_or_else(|| {
                "template 'highlight_box' requires `box` ({x, y, width, height})".to_string()
            })?;
            MotionTemplateSpec::HighlightBox {
                x,
                y,
                width,
                height,
                pulse: args.pulse.unwrap_or(false),
            }
        }
        "progress_bar" => {
            let (from, to, y) = args.progress.ok_or_else(|| {
                "template 'progress_bar' requires `progress` ({from, to, y})".to_string()
            })?;
            MotionTemplateSpec::ProgressBar {
                from,
                to,
                y,
                color: args
                    .color
                    .as_deref()
                    .map(str::trim)
                    .filter(|color| !color.is_empty())
                    .map(str::to_string),
            }
        }
        other => {
            return Err(format!(
                "template '{other}' not recognized. Use 'lower_third', 'kinetic_text', \
                 'highlight_box', or 'progress_bar'."
            ));
        }
    };

    expand_template(&spec, duration_s, fps)
}

/// Parse the optional `kinetic_text` `anchor` string into a
/// [`TextAnchor`]. Defaults to [`TextAnchor::Center`] when omitted or
/// empty, matching the historically hardcoded behavior. An unrecognized
/// value fails with an error naming the accepted values.
fn parse_text_anchor(anchor: Option<&str>) -> Result<TextAnchor, String> {
    match anchor.map(str::trim) {
        None | Some("") => Ok(TextAnchor::Center),
        Some("lower_left") => Ok(TextAnchor::LowerLeft),
        Some("center") => Ok(TextAnchor::Center),
        Some("lower_center") => Ok(TextAnchor::LowerCenter),
        Some(other) => Err(format!(
            "kinetic_text `anchor` '{other}' not recognized. Use 'lower_left', 'center', or 'lower_center'."
        )),
    }
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
    Err(
        "on-screen text must come from the edit's transcript evidence, not from this tool. \
Pass `headline` (the exact on-screen headline) or `evidence_text` (the transcript window to \
derive it from), plus `step_labels` for step/process scenes. The planner no longer truncates \
the request prompt into a headline or invents placeholder labels."
            .into(),
    )
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
    backdrop: BackdropMode,
) -> Vec<MotionSceneLayer> {
    let mut layers = Vec::new();
    let has_image = wants_image_layer(request) || image_asset.is_some();

    match backdrop {
        BackdropMode::Full => layers.push(full_bleed_backdrop_layer(duration_s)),
        BackdropMode::Panel => layers.push(background_panel_layer(duration_s)),
        BackdropMode::Auto if wants_panel_layer(request) || has_image => {
            layers.push(background_panel_layer(duration_s));
        }
        BackdropMode::Auto | BackdropMode::None => {}
    }
    if has_image {
        layers.push(image_layer(request, duration_s, image_asset));
    }
    if wants_callout_layer(request) || has_image {
        layers.push(callout_accent_layer(duration_s));
    }

    layers.push(headline_layer(
        headline,
        duration_s,
        !step_labels.is_empty(),
    ));
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

/// Solid spanning exactly the whole canvas — no inset margins — for
/// scenes meant to fully cover the program frame. Enter/exit fades
/// come from the shared scene-fade defaults at preview/render time.
fn full_bleed_backdrop_layer(duration_s: f64) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "backdrop-full".into(),
        kind: MotionSceneLayerKind::Solid,
        from_s: 0.0,
        duration_s,
        z_index: 0,
        params: BTreeMap::from([
            ("x".into(), serde_json::json!(0.0)),
            ("y".into(), serde_json::json!(0.0)),
            ("width".into(), serde_json::json!(1.0)),
            ("height".into(), serde_json::json!(1.0)),
            ("color".into(), serde_json::json!("#070D17")),
            ("opacity".into(), serde_json::json!(0.92)),
        ]),
    }
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
            ("color".into(), serde_json::json!(TEMPLATE_ACCENT_COLOR)),
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

/// Headline text box. `x`/`y` are the box CENTER (preview/render
/// place the text block centered on that point); the headline sits at
/// frame center, or in the upper third when step rows follow below.
fn headline_layer(headline: &str, duration_s: f64, has_steps: bool) -> MotionSceneLayer {
    MotionSceneLayer {
        id: "headline".into(),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.0,
        duration_s,
        z_index: 10,
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(headline)),
            ("font_size".into(), serde_json::json!(64)),
            ("font_weight".into(), serde_json::json!("bold")),
            ("align".into(), serde_json::json!("center")),
            ("x".into(), serde_json::json!(0.5)),
            (
                "y".into(),
                serde_json::json!(if has_steps { 0.26 } else { 0.5 }),
            ),
            ("width".into(), serde_json::json!(0.76)),
            ("height".into(), serde_json::json!(0.2)),
        ]),
    }
}

/// One step/process label box, stacked below the headline. `x` is the
/// box center of a left-aligned 0.36-wide column starting at 0.14.
fn step_text_layer(index: usize, label: &str, duration_s: f64) -> MotionSceneLayer {
    let step_number = index + 1;
    let y = 0.4 + (index as f64 * 0.12);
    MotionSceneLayer {
        id: format!("step-{step_number}-label"),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.2 + (index as f64 * 0.16),
        duration_s: (duration_s - (0.2 + (index as f64 * 0.16))).max(0.01),
        z_index: 12 + i32::try_from(index).unwrap_or(0),
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(label)),
            ("font_size".into(), serde_json::json!(40)),
            ("align".into(), serde_json::json!("left")),
            ("x".into(), serde_json::json!(0.32)),
            ("y".into(), serde_json::json!(y)),
            ("width".into(), serde_json::json!(0.36)),
            ("height".into(), serde_json::json!(0.1)),
        ]),
    }
}

/// Reference line-height multiplier for fitted text.
const TEXT_LINE_HEIGHT: f64 = 1.2;

/// Smallest font size the fitter will shrink to (matches the render).
const MIN_TEXT_FONT_SIZE: u32 = 24;

/// Auto-fit every text layer to its box: the box is authoritative.
///
/// Uses the same estimate as the render's drawtext lowering
/// ([`montage_render::fit_text_to_width_px`]) to wrap the text into
/// the layer's width and steps `font_size` down until the wrapped
/// block also fits the box height (or 90% of the canvas when the
/// layer has no height). The fitted size is written back into the
/// layer params; wrapping itself stays a render/preview concern.
fn fit_scene_text_layers(scene: &mut MotionScene) {
    let canvas_width = f64::from(scene.width.max(1));
    let canvas_height = f64::from(scene.height.max(1));
    for layer in &mut scene.layers {
        if layer.kind != MotionSceneLayerKind::Text {
            continue;
        }
        let Some(text) = layer
            .params
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let font_size = layer
            .params
            .get("font_size")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(64);
        let width_frac = normalized_fraction(layer.params.get("width"), canvas_width)
            .filter(|width| *width > 0.0)
            .or_else(|| {
                normalized_fraction(layer.params.get("x"), canvas_width)
                    .map(|x| (2.0 * x.min(1.0 - x)).clamp(0.05, 0.92))
            })
            .unwrap_or(0.78);
        let max_width_px = width_frac * canvas_width;
        let max_height_px = normalized_fraction(layer.params.get("height"), canvas_height)
            .filter(|height| *height > 0.0)
            .map(|height| height * canvas_height)
            .unwrap_or(canvas_height * 0.9);

        let mut fitted = font_size;
        loop {
            let (wrapped, size) = montage_render::fit_text_to_width_px(&text, fitted, max_width_px);
            fitted = size;
            let lines = wrapped.split('\n').count();
            let block_height = lines as f64 * f64::from(fitted) * TEXT_LINE_HEIGHT;
            if block_height <= max_height_px || fitted <= MIN_TEXT_FONT_SIZE {
                break;
            }
            fitted = fitted.saturating_sub(4).max(MIN_TEXT_FONT_SIZE);
        }
        if fitted != font_size {
            layer
                .params
                .insert("font_size".into(), serde_json::json!(fitted));
        }
    }
}

/// Numeric layer param normalized to a `[0, 1]` fraction of `extent`
/// (values above 1 are scene-space pixels).
fn normalized_fraction(value: Option<&serde_json::Value>, extent: f64) -> Option<f64> {
    let value = value.and_then(serde_json::Value::as_f64)?;
    if !value.is_finite() {
        return None;
    }
    if value.abs() > 1.0 && extent > 0.0 {
        Some(value / extent)
    } else {
        Some(value)
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

