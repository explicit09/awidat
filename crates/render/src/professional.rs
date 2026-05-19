//! Operational professional-pipeline engines and render lowerers.
//!
//! These functions sit above the durable schemas in `awidat-proto` and below
//! tool/UI orchestration. They intentionally produce deterministic plans and
//! diagnostics from existing evidence; expensive CV/ML sidecars can feed these
//! APIs later without changing callers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::professional::{
    AnimationTarget, AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState,
    AudioMeterReading, AudioRole, CapabilityArea, ColorFinishingState, CompositionGraph,
    CompositionNode, CompositionNodeType, DeliveryPreflightInput, DeliveryProfile, Easing,
    ExportPreset, ExpressionLink, ExpressionSource, FindingSeverity, GradeStack, GradeStage,
    Keyframe, KeyframeInterpolation, MaskSidecar, MatteSidecar, MotionGraphicsTemplate,
    MotionPackage, PackageManifest, ParameterAnimation, PreflightReport, ProfessionalDiagnostic,
    ReframePath, ReframeSmoothing, ReviewStatus, SafeAreaRule, StreamExportContract,
    StreamExportMode, TemplateSlot, TemplateSlotKind, TrackKind, TrackSample, TrackSidecar,
    TrackingPackage,
};
use serde_json::Value;
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

use crate::animation::keyframes_to_ffmpeg_expr;
use crate::timeline::ColorCorrectionPlan;
use crate::{
    AudioAutomationPlan, AudioFxPlan, AudioTrackPlan, DuckingPlan, EqBandPlan, LoudnessTargetPlan,
    RenderJobSpec, TitleAnimation, TitlePlan, TitlePosition, TitleWeight,
};

/// Errors returned by professional pipeline engines.
#[derive(Debug, Error)]
pub enum ProfessionalEngineError {
    /// A required template slot was not supplied.
    #[error("motion template {template_id} missing required slot {slot_id}")]
    MissingTemplateSlot {
        /// Template id.
        template_id: String,
        /// Slot id.
        slot_id: String,
    },
    /// A supplied slot value did not match its declared slot kind.
    #[error("motion template {template_id} slot {slot_id} expected {expected}")]
    InvalidTemplateSlot {
        /// Template id.
        template_id: String,
        /// Slot id.
        slot_id: String,
        /// Expected kind label.
        expected: &'static str,
    },
    /// A requested track id was not found in the package.
    #[error("track {0} not found")]
    MissingTrack(String),
    /// A requested reframe path id was not found in the package.
    #[error("reframe path {0} not found")]
    MissingReframePath(String),
    /// A grade stack has no supported stages.
    #[error("grade stack {0} has no supported render stages")]
    UnsupportedGradeStack(String),
    /// An export preset failed validation before lowering.
    #[error("export preset {preset_id} is invalid: {message}")]
    InvalidExportPreset {
        /// Preset id.
        preset_id: String,
        /// Validation message.
        message: String,
    },
    /// A stream export contract failed validation before lowering.
    #[error("stream export contract {contract_id} is invalid: {message}")]
    InvalidStreamExportContract {
        /// Contract id.
        contract_id: String,
        /// Validation message.
        message: String,
    },
    /// A motion package conflicts with existing explicit animations.
    #[error("motion package {package_id} conflicts with existing animation {animation_id}")]
    MotionPackageConflict {
        /// Package id.
        package_id: String,
        /// Existing animation id.
        animation_id: String,
    },
}

/// Lightweight tracking evidence available from existing index sidecars.
#[derive(Debug, Clone)]
pub struct TrackingEvidence {
    /// Asset id being tracked.
    pub asset_id: String,
    /// Desired track kind.
    pub kind: TrackKind,
    /// Number of frames/samples to produce.
    pub frame_count: u64,
    /// Source width in pixels.
    pub width: u32,
    /// Source height in pixels.
    pub height: u32,
    /// Per-sample motion magnitude, usually from the motion sidecar.
    pub motion_signal: Vec<f32>,
}

/// Deterministic lowering for an overlay bound to a tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackBoundOverlayLowering {
    /// Track id used for the binding.
    pub track_id: String,
    /// Overlay clip id being driven.
    pub overlay_clip_id: String,
    /// Current render expression/summary.
    pub expression: String,
}

/// Deterministic lowering for a subject-aware crop/reframe path.
#[derive(Debug, Clone, PartialEq)]
pub struct ReframePathLowering {
    /// Reframe path id.
    pub reframe_id: String,
    /// Clip receiving the crop.
    pub clip_id: String,
    /// Delivery aspect ratio for the crop.
    pub aspect_ratio: String,
    /// Smoothing policy attached to the path.
    pub smoothing: ReframeSmoothing,
    /// Render expression/summary for deterministic review.
    pub expression: String,
}

/// Result of evaluating expression links at one sample time.
#[derive(Debug, Clone, Default)]
pub struct ExpressionEvaluation {
    /// Values keyed as `clip_id/parameter`.
    pub values: BTreeMap<String, f64>,
    /// Missing sources, cycles, or unsupported expressions.
    pub limitations: Vec<RenderLimitation>,
}

/// Manual track correction supplied by review UI or an agent tool.
#[derive(Debug, Clone)]
pub struct TrackCorrection {
    /// Track to replace.
    pub track_id: String,
    /// `(frame, points, confidence)` replacement samples.
    pub samples: Vec<(u64, Vec<[f64; 2]>, f64)>,
}

impl TrackCorrection {
    /// Apply the correction and recompute track confidence.
    pub fn apply(self, package: &mut TrackingPackage) -> Result<(), ProfessionalEngineError> {
        let track = package
            .tracks
            .iter_mut()
            .find(|track| track.id == self.track_id)
            .ok_or_else(|| ProfessionalEngineError::MissingTrack(self.track_id.clone()))?;
        track.samples = self
            .samples
            .into_iter()
            .map(|(frame, points, confidence)| TrackSample {
                frame,
                points,
                confidence: Some(confidence.clamp(0.0, 1.0)),
            })
            .collect();
        track.confidence = average_sample_confidence(&track.samples);
        Ok(())
    }
}

/// Generate a deterministic tracking package from coarse video evidence.
pub fn generate_tracking_package(evidence: TrackingEvidence) -> TrackingPackage {
    let sample_count = evidence
        .frame_count
        .max(evidence.motion_signal.len() as u64)
        .max(1);
    let samples = (0..sample_count)
        .map(|frame| {
            let motion = evidence
                .motion_signal
                .get(frame as usize)
                .copied()
                .unwrap_or(0.0);
            let confidence = (1.0 - f64::from(motion).clamp(0.0, 1.0)).clamp(0.0, 1.0);
            TrackSample {
                frame,
                points: track_points_for_kind(
                    evidence.kind,
                    frame,
                    evidence.width,
                    evidence.height,
                ),
                confidence: Some(confidence),
            }
        })
        .collect::<Vec<_>>();
    let confidence = average_sample_confidence(&samples);
    TrackingPackage {
        tracks: vec![TrackSidecar {
            id: format!("track-{}", evidence.asset_id),
            asset_id: evidence.asset_id,
            kind: evidence.kind,
            samples,
            confidence,
            ..TrackSidecar::default()
        }],
        reframe_paths: Vec::<ReframePath>::new(),
        masks: Vec::<MaskSidecar>::new(),
        mattes: Vec::<MatteSidecar>::new(),
        ..TrackingPackage::default()
    }
}

/// Lower an overlay transform from a reviewed tracking package.
pub fn lower_track_bound_overlay(
    package: &TrackingPackage,
    track_id: &str,
    overlay_clip_id: &str,
) -> Result<TrackBoundOverlayLowering, ProfessionalEngineError> {
    let track = package
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or_else(|| ProfessionalEngineError::MissingTrack(track_id.to_string()))?;
    let sample = track
        .samples
        .iter()
        .find(|sample| !sample.points.is_empty())
        .ok_or_else(|| ProfessionalEngineError::MissingTrack(track_id.to_string()))?;
    let point = sample.points[0];
    let mask_ids = package
        .masks
        .iter()
        .filter(|mask| {
            mask.track_id.as_deref() == Some(track_id)
                && mask.attached_clip_id.as_deref() == Some(overlay_clip_id)
        })
        .map(|mask| mask.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    Ok(TrackBoundOverlayLowering {
        track_id: track_id.into(),
        overlay_clip_id: overlay_clip_id.into(),
        expression: format!(
            "overlay=x={}:y={}:track={track_id}:clip={overlay_clip_id}:masks={mask_ids}",
            point[0], point[1]
        ),
    })
}

/// Lower a reviewed subject-aware reframe path into a deterministic crop expression.
pub fn lower_reframe_path(
    package: &TrackingPackage,
    reframe_id: &str,
) -> Result<ReframePathLowering, ProfessionalEngineError> {
    let path = package
        .reframe_paths
        .iter()
        .find(|path| path.id == reframe_id)
        .ok_or_else(|| ProfessionalEngineError::MissingReframePath(reframe_id.to_string()))?;
    let keyframe = path
        .keyframes
        .first()
        .ok_or_else(|| ProfessionalEngineError::MissingReframePath(reframe_id.to_string()))?;
    let safe_area = path.safe_area.as_deref().unwrap_or("none");
    Ok(ReframePathLowering {
        reframe_id: path.id.clone(),
        clip_id: path.clip_id.clone(),
        aspect_ratio: path.aspect_ratio.clone(),
        smoothing: path.smoothing,
        expression: format!(
            "crop=clip={}:aspect={}:target={}x{}:source={}x{}:center={},{}:scale={}:smoothing={:?}:safe_area={safe_area}",
            path.clip_id,
            path.aspect_ratio,
            path.target_width,
            path.target_height,
            path.source_width,
            path.source_height,
            keyframe.center[0],
            keyframe.center[1],
            keyframe.scale,
            path.smoothing,
        ),
    })
}

/// Evaluate deterministic expression links against sampled signals/parameters.
pub fn evaluate_expression_links(
    links: &[ExpressionLink],
    signals: &HashMap<String, Value>,
    parameters: &HashMap<String, Value>,
    _time_s: f64,
) -> ExpressionEvaluation {
    let mut evaluation = ExpressionEvaluation::default();
    if expression_links_have_cycle(links) {
        evaluation.limitations.push(RenderLimitation {
            node_id: "expression_links".into(),
            severity: FindingSeverity::Error,
            message: "expression dependency graph contains a cycle".into(),
        });
        return evaluation;
    }

    for link in links.iter().filter(|link| link.enabled) {
        let Some(source_value) = expression_source_value(&link.source, signals, parameters) else {
            evaluation.limitations.push(RenderLimitation {
                node_id: link.id.clone(),
                severity: FindingSeverity::Warning,
                message: missing_expression_source_message(&link.source),
            });
            continue;
        };
        let Some(mut value) = evaluate_expression_subset(&link.expression, source_value) else {
            evaluation.limitations.push(RenderLimitation {
                node_id: link.id.clone(),
                severity: FindingSeverity::Warning,
                message: format!("expression {} is unsupported", link.expression),
            });
            continue;
        };
        if let Some(clamp) = link.clamp {
            value = value.clamp(clamp.min, clamp.max);
        }
        evaluation.values.insert(
            format!("{}/{}", link.target_clip_id, link.target_parameter),
            value,
        );
    }
    evaluation
}

fn expression_links_have_cycle(links: &[ExpressionLink]) -> bool {
    let targets = links
        .iter()
        .map(|link| {
            (
                format!("{}/{}", link.target_clip_id, link.target_parameter),
                link,
            )
        })
        .collect::<HashMap<_, _>>();
    for link in links {
        let mut seen = std::collections::HashSet::new();
        let mut current = link;
        while let ExpressionSource::Parameter { clip_id, parameter } = &current.source {
            let key = format!("{clip_id}/{parameter}");
            if !seen.insert(key.clone()) {
                return true;
            }
            let Some(next) = targets.get(&key).copied() else {
                break;
            };
            current = next;
        }
    }
    false
}

fn expression_source_value(
    source: &ExpressionSource,
    signals: &HashMap<String, Value>,
    parameters: &HashMap<String, Value>,
) -> Option<f64> {
    match source {
        ExpressionSource::Unset => None,
        ExpressionSource::Signal { signal } => signals.get(signal).and_then(Value::as_f64),
        ExpressionSource::Parameter { clip_id, parameter } => parameters
            .get(&format!("{clip_id}/{parameter}"))
            .and_then(Value::as_f64),
    }
}

fn missing_expression_source_message(source: &ExpressionSource) -> String {
    match source {
        ExpressionSource::Unset => "missing expression source".into(),
        ExpressionSource::Signal { signal } => format!("missing signal {signal}"),
        ExpressionSource::Parameter { clip_id, parameter } => {
            format!("missing source parameter {clip_id}/{parameter}")
        }
    }
}

fn evaluate_expression_subset(expression: &str, source: f64) -> Option<f64> {
    let expression = expression.trim();
    if expression == "source" {
        return Some(source);
    }
    if let Some(inner) = expression
        .strip_prefix("clamp(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let parts = split_args(inner);
        if parts.len() == 3 {
            let value = evaluate_expression_subset(parts[0], source)?;
            let min = parts[1].trim().parse::<f64>().ok()?;
            let max = parts[2].trim().parse::<f64>().ok()?;
            return Some(value.clamp(min, max));
        }
    }
    if let Some(inner) = expression
        .strip_prefix("lerp(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let parts = split_args(inner);
        if parts.len() == 3 {
            let start = evaluate_expression_subset(parts[0], source)?;
            let end = evaluate_expression_subset(parts[1], source)?;
            let progress = evaluate_expression_subset(parts[2], source)?.clamp(0.0, 1.0);
            return Some(start + (end - start) * progress);
        }
    }
    if let Some((base, multiplier)) = expression.split_once("+ source *") {
        let base = base.trim().parse::<f64>().ok()?;
        let multiplier = multiplier.trim().parse::<f64>().ok()?;
        return Some(base + source * multiplier);
    }
    expression.parse::<f64>().ok()
}

fn split_args(value: &str) -> Vec<&str> {
    value.split(',').map(str::trim).collect()
}

fn track_points_for_kind(kind: TrackKind, frame: u64, width: u32, height: u32) -> Vec<[f64; 2]> {
    let drift_x = (frame as f64 * 0.001).min(0.05);
    let drift_y = (frame as f64 * 0.0005).min(0.04);
    match kind {
        TrackKind::Point => vec![[0.5 + drift_x, 0.5 + drift_y]],
        TrackKind::Planar => vec![
            [0.35 + drift_x, 0.35 + drift_y],
            [0.65 + drift_x, 0.65 + drift_y],
        ],
        TrackKind::Surface => {
            let aspect_adjust = if height == 0 {
                0.0
            } else {
                ((f64::from(width) / f64::from(height)) - (16.0 / 9.0)).clamp(-0.05, 0.05)
            };
            vec![
                [0.25 + drift_x, 0.25 + drift_y],
                [0.75 + drift_x, 0.25 + drift_y],
                [0.75 + drift_x + aspect_adjust, 0.75 + drift_y],
                [0.25 + drift_x + aspect_adjust, 0.75 + drift_y],
            ]
        }
    }
}

fn average_sample_confidence(samples: &[TrackSample]) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0_usize;
    for confidence in samples.iter().filter_map(|sample| sample.confidence) {
        if confidence.is_finite() {
            total += confidence.clamp(0.0, 1.0);
            count += 1;
        }
    }
    (count > 0).then_some(total / count as f64)
}

/// Result of lowering a composition graph to current render primitives.
#[derive(Debug, Clone, Default)]
pub struct CompositionLowering {
    /// Supported backend steps.
    pub steps: Vec<RenderLoweringStep>,
    /// Preview/render limitations for unsupported or partial features.
    pub limitations: Vec<RenderLimitation>,
}

/// Compact graph inspection for CLI/Tauri review surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionGraphInspection {
    /// Node ids in graph order.
    pub nodes: Vec<String>,
    /// Edges as compact `from -> to` strings.
    pub edges: Vec<String>,
    /// Unsupported/custom node ids.
    pub unsupported_nodes: Vec<String>,
    /// Summary of current render lowering.
    pub render_plan_summary: String,
}

/// One lowered render step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderLoweringStep {
    /// Source node id.
    pub node_id: String,
    /// Backend family, currently `ffmpeg`.
    pub backend: String,
    /// Filter or planner expression.
    pub expression: String,
}

/// Explicit render/preview limitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderLimitation {
    /// Node id.
    pub node_id: String,
    /// Severity.
    pub severity: FindingSeverity,
    /// User-facing message.
    pub message: String,
}

/// Built-in capability metadata for animating a clip effect parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectParameterCapability {
    /// Effect id, e.g. `awidat.color_correction`.
    pub effect: &'static str,
    /// Parameter id under the effect.
    pub parameter: &'static str,
    /// Human-readable unit family.
    pub unit: &'static str,
    /// Preview evaluator can represent it.
    pub previewable: bool,
    /// Render planner can lower it.
    pub renderable: bool,
    validation: EffectParameterValidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectParameterValidation {
    AnyFinite,
    Normalized,
    Positive,
}

/// Built-in effect animation support matrix.
pub fn effect_parameter_capability_matrix() -> Vec<EffectParameterCapability> {
    vec![
        EffectParameterCapability {
            effect: "awidat.color_correction",
            parameter: "brightness",
            unit: "offset",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::AnyFinite,
        },
        EffectParameterCapability {
            effect: "awidat.color_correction",
            parameter: "contrast",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "awidat.color_correction",
            parameter: "saturation",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "awidat.video_overlay",
            parameter: "opacity",
            unit: "normalized",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Normalized,
        },
        EffectParameterCapability {
            effect: "awidat.video_overlay",
            parameter: "scale",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "awidat.volume",
            parameter: "value",
            unit: "multiplier",
            previewable: false,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
    ]
}

/// Validate an arbitrary effect-parameter animation against the support matrix.
pub fn diagnose_effect_parameter_animation(
    animation: &ParameterAnimation,
) -> Option<ProfessionalReviewFinding> {
    let AnimationTarget::ClipParameter { parameter, .. } = &animation.target else {
        return None;
    };
    let capability = effect_parameter_capability_matrix()
        .into_iter()
        .find(|capability| {
            effect_parameter_path(capability.effect, capability.parameter) == *parameter
        })?;
    for keyframe in &animation.keyframes {
        if !effect_parameter_value_is_valid(keyframe.value, capability.validation) {
            return Some(ProfessionalReviewFinding {
                kind: "invalid_effect_parameter_value".into(),
                severity: FindingSeverity::Error,
                message: format!(
                    "animation {} target {} value {} is invalid for {} units",
                    animation.id, parameter, keyframe.value, capability.unit
                ),
                fix_ref: Some(format!("fix-effect-parameter-{}", animation.id)),
            });
        }
    }
    None
}

fn effect_parameter_path(effect: &str, parameter: &str) -> String {
    format!("{effect}.{parameter}")
}

fn effect_parameter_value_is_valid(value: f64, validation: EffectParameterValidation) -> bool {
    if !value.is_finite() {
        return false;
    }
    match validation {
        EffectParameterValidation::AnyFinite => true,
        EffectParameterValidation::Normalized => (0.0..=1.0).contains(&value),
        EffectParameterValidation::Positive => value > 0.0,
    }
}

/// Lower supported composition nodes to ffmpeg-oriented filter steps.
pub fn lower_composition_graph(graph: &CompositionGraph) -> CompositionLowering {
    let mut lowering = CompositionLowering::default();
    for diagnostic in graph.validate() {
        lowering.limitations.push(RenderLimitation {
            node_id: graph.id.clone(),
            severity: diagnostic.severity,
            message: diagnostic.message,
        });
    }
    for node in &graph.nodes {
        match lower_composition_node(node) {
            Some(step) => lowering.steps.push(step),
            None => lowering.limitations.push(RenderLimitation {
                node_id: node.id.clone(),
                severity: FindingSeverity::Warning,
                message: format!("node {:?} has no current render lowering", node.node_type),
            }),
        }
    }
    lowering
}

/// Inspect a composition graph without requiring visual node UI.
pub fn inspect_composition_graph(graph: &CompositionGraph) -> CompositionGraphInspection {
    let lowering = lower_composition_graph(graph);
    let unsupported_nodes = graph
        .nodes
        .iter()
        .filter(|node| composition_node_needs_future_runtime(node.node_type))
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    CompositionGraphInspection {
        nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
        edges: graph
            .edges
            .iter()
            .map(|edge| format!("{} -> {}", edge.from, edge.to))
            .collect(),
        unsupported_nodes,
        render_plan_summary: format!(
            "{} supported steps, {} limitations",
            lowering.steps.len(),
            lowering.limitations.len()
        ),
    }
}

fn lower_composition_node(node: &CompositionNode) -> Option<RenderLoweringStep> {
    let expression = match node.node_type {
        CompositionNodeType::Unsupported
        | CompositionNodeType::Scene3d
        | CompositionNodeType::ParticleEmitter => return None,
        CompositionNodeType::MediaInput => {
            format!(
                "input={}",
                string_param(node, "asset_id").unwrap_or("unknown")
            )
        }
        CompositionNodeType::Transform => {
            let scale = number_param(node, "scale").unwrap_or(1.0);
            format!("scale=iw*{scale}:ih*{scale},setpts=PTS-STARTPTS")
        }
        CompositionNodeType::Merge => "overlay=x=0:y=0:format=auto".to_string(),
        CompositionNodeType::Mask => "alphamerge".to_string(),
        CompositionNodeType::Matte => "format=rgba,alphamerge".to_string(),
        CompositionNodeType::Text => {
            let text = string_param(node, "text").unwrap_or("");
            format!("drawtext=text='{text}'")
        }
        CompositionNodeType::Blur => {
            let radius = number_param(node, "radius").unwrap_or(2.0).max(0.0);
            format!("boxblur={radius}")
        }
        CompositionNodeType::Color => "eq=brightness=0:contrast=1:saturation=1".to_string(),
        CompositionNodeType::TrackerBind => {
            let track_id = string_param(node, "track_id").unwrap_or("unbound");
            format!("metadata={track_id}")
        }
        CompositionNodeType::Output => "map=output".to_string(),
    };
    Some(RenderLoweringStep {
        node_id: node.id.clone(),
        backend: "ffmpeg".into(),
        expression,
    })
}

fn composition_node_needs_future_runtime(node_type: CompositionNodeType) -> bool {
    matches!(
        node_type,
        CompositionNodeType::Unsupported
            | CompositionNodeType::Scene3d
            | CompositionNodeType::ParticleEmitter
    )
}

fn string_param<'a>(node: &'a CompositionNode, key: &str) -> Option<&'a str> {
    node.params.get(key).and_then(Value::as_str)
}

fn number_param(node: &CompositionNode, key: &str) -> Option<f64> {
    node.params.get(key).and_then(Value::as_f64)
}

/// Filled and type-validated motion graphics template.
#[derive(Debug, Clone)]
pub struct FilledMotionTemplate {
    /// Template id.
    pub template_id: String,
    /// Template display name.
    pub name: String,
    /// Slot values keyed by slot id.
    pub slots: BTreeMap<String, Value>,
    /// Safe-area rules copied from the template.
    pub safe_areas: Vec<awidat_proto::professional::SafeAreaRule>,
}

/// Timing and animation selection for a filled template.
#[derive(Debug, Clone, Copy)]
pub struct MotionTemplateTiming {
    /// Start time.
    pub start_s: f64,
    /// End time.
    pub end_s: f64,
    /// Initial animation style.
    pub animation: TemplateAnimation,
}

/// Template animation styles supported by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateAnimation {
    /// No animation.
    None,
    /// Opacity fade.
    Opacity,
    /// Transform/slide.
    Transform,
    /// Reveal text progressively.
    TextReveal,
    /// Write-on approximation using progressive text.
    WriteOn,
}

/// Render output for a motion graphics template.
#[derive(Debug, Clone, Default)]
pub struct MotionTemplateRender {
    /// Title plans the current renderer can draw.
    pub titles: Vec<TitlePlan>,
    /// Explicit parameter animations generated by the template.
    pub parameter_animations: Vec<ParameterAnimation>,
    /// Safe-area violations found before render.
    pub safe_area_violations: Vec<ProfessionalDiagnostic>,
    /// Limitations for richer template features.
    pub limitations: Vec<RenderLimitation>,
}

/// Built-in Phase 3B motion template catalog.
pub fn built_in_motion_templates() -> Vec<MotionGraphicsTemplate> {
    vec![
        MotionGraphicsTemplate {
            id: "lower-third".into(),
            name: "Lower Third".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("text", TemplateSlotKind::Text, true),
                slot("subtitle", TemplateSlotKind::Text, false),
                slot("color", TemplateSlotKind::Color, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "callout".into(),
            name: "Callout".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("text", TemplateSlotKind::Text, true),
                slot("accent_color", TemplateSlotKind::Color, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "punch-in-zoom".into(),
            name: "Punch-In Zoom".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("scale", TemplateSlotKind::Number, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "focus-highlight".into(),
            name: "Focus Highlight".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("intensity", TemplateSlotKind::Number, false),
                slot("color", TemplateSlotKind::Color, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "title-reveal".into(),
            name: "Title Reveal".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("text", TemplateSlotKind::Text, true),
                slot("color", TemplateSlotKind::Color, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "pip-emphasis".into(),
            name: "PiP Emphasis".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("scale", TemplateSlotKind::Number, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "product-insert-emphasis".into(),
            name: "Product Insert Emphasis".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("image_asset", TemplateSlotKind::Image, false),
                slot("video_asset", TemplateSlotKind::Video, false),
                slot("scale", TemplateSlotKind::Number, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
    ]
}

/// User decision for a motion package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPackageDecision {
    /// Accept and write generated explicit records.
    Accept,
    /// Reject while preserving proposal and learning signal.
    Reject,
}

/// Apply a review decision for a motion package to durable timeline metadata.
pub fn apply_motion_package(
    metadata: &mut AwidatTimelineMetadata,
    mut package: MotionPackage,
    decision: MotionPackageDecision,
) -> Result<(), ProfessionalEngineError> {
    match decision {
        MotionPackageDecision::Accept => {
            if let Some(existing) = first_motion_package_conflict(metadata, &package) {
                return Err(ProfessionalEngineError::MotionPackageConflict {
                    package_id: package.id,
                    animation_id: existing,
                });
            }
            package.status = ReviewStatus::Accepted;
            for animation in &package.generated_animations {
                replace_by_id(
                    &mut metadata.parameter_animations,
                    animation.clone(),
                    |item| item.id.as_str(),
                );
            }
        }
        MotionPackageDecision::Reject => {
            package.status = ReviewStatus::Rejected;
        }
    }
    metadata
        .learning_signals
        .push(awidat_proto::professional::LearningSignal {
            proposal_id: package.id.clone(),
            area: CapabilityArea::MotionGraphicsTemplates,
            status: package.status,
            reason: package.rationale.clone(),
        });
    replace_by_id(&mut metadata.motion_packages, package, |item| {
        item.id.as_str()
    });
    Ok(())
}

/// Compact package diff/summary for review surfaces.
pub fn motion_package_summary(package: &MotionPackage) -> String {
    let mut parts = Vec::new();
    for animation in &package.generated_animations {
        if let AnimationTarget::ClipParameter { clip_id, parameter } = &animation.target {
            parts.push(format!("adds {parameter} on {clip_id}"));
        }
    }
    parts.extend(package.limitations.iter().cloned());
    if parts.is_empty() {
        package.intent.clone()
    } else {
        parts.join("; ")
    }
}

fn first_motion_package_conflict(
    metadata: &AwidatTimelineMetadata,
    package: &MotionPackage,
) -> Option<String> {
    package.generated_animations.iter().find_map(|generated| {
        metadata
            .parameter_animations
            .iter()
            .find(|existing| animations_conflict(existing, generated))
            .map(|existing| existing.id.clone())
    })
}

fn animations_conflict(existing: &ParameterAnimation, generated: &ParameterAnimation) -> bool {
    let (
        AnimationTarget::ClipParameter {
            clip_id: existing_clip,
            parameter: existing_parameter,
        },
        AnimationTarget::ClipParameter {
            clip_id: generated_clip,
            parameter: generated_parameter,
        },
    ) = (&existing.target, &generated.target)
    else {
        return false;
    };
    existing_clip == generated_clip
        && existing_parameter == generated_parameter
        && animation_ranges_overlap(existing, generated)
}

fn animation_ranges_overlap(first: &ParameterAnimation, second: &ParameterAnimation) -> bool {
    let Some(first_range) = animation_time_range(first) else {
        return false;
    };
    let Some(second_range) = animation_time_range(second) else {
        return false;
    };
    first_range.0 <= second_range.1 && second_range.0 <= first_range.1
}

fn animation_time_range(animation: &ParameterAnimation) -> Option<(f64, f64)> {
    let mut start_s = f64::INFINITY;
    let mut end_s = f64::NEG_INFINITY;
    for keyframe in &animation.keyframes {
        if !keyframe.time_s.is_finite() {
            return None;
        }
        start_s = start_s.min(keyframe.time_s);
        end_s = end_s.max(keyframe.time_s);
    }
    (start_s != f64::INFINITY && end_s != f64::NEG_INFINITY).then_some((start_s, end_s))
}

fn replace_by_id<T, F>(items: &mut Vec<T>, item: T, id: F)
where
    F: Fn(&T) -> &str,
{
    let item_id = id(&item).to_string();
    if let Some(existing) = items.iter_mut().find(|existing| id(existing) == item_id) {
        *existing = item;
    } else {
        items.push(item);
    }
}

fn slot(id: &str, kind: TemplateSlotKind, required: bool) -> TemplateSlot {
    TemplateSlot {
        id: id.into(),
        kind,
        required,
        ..TemplateSlot::default()
    }
}

fn platform_variants() -> Vec<String> {
    vec!["16:9".into(), "9:16".into(), "1:1".into()]
}

fn platform_safe_areas() -> Vec<SafeAreaRule> {
    vec![
        SafeAreaRule {
            profile: "16:9".into(),
            margin_pct: 0.06,
        },
        SafeAreaRule {
            profile: "9:16".into(),
            margin_pct: 0.12,
        },
        SafeAreaRule {
            profile: "1:1".into(),
            margin_pct: 0.08,
        },
    ]
}

/// Fill and validate reusable motion graphics template slots.
pub fn fill_motion_template(
    template: &MotionGraphicsTemplate,
    values: BTreeMap<String, Value>,
) -> Result<FilledMotionTemplate, ProfessionalEngineError> {
    for slot in &template.slots {
        let Some(value) = values.get(&slot.id) else {
            if slot.required {
                return Err(ProfessionalEngineError::MissingTemplateSlot {
                    template_id: template.id.clone(),
                    slot_id: slot.id.clone(),
                });
            }
            continue;
        };
        validate_slot_value(template, &slot.id, slot.kind, value)?;
    }
    Ok(FilledMotionTemplate {
        template_id: template.id.clone(),
        name: template.name.clone(),
        slots: values,
        safe_areas: template.safe_areas.clone(),
    })
}

fn validate_slot_value(
    template: &MotionGraphicsTemplate,
    slot_id: &str,
    kind: TemplateSlotKind,
    value: &Value,
) -> Result<(), ProfessionalEngineError> {
    let valid = match kind {
        TemplateSlotKind::Text
        | TemplateSlotKind::Color
        | TemplateSlotKind::Image
        | TemplateSlotKind::Video
        | TemplateSlotKind::TargetClip
        | TemplateSlotKind::SafeAreaProfile => value.as_str().is_some(),
        TemplateSlotKind::Number => value.as_f64().is_some(),
    };
    if valid {
        Ok(())
    } else {
        Err(ProfessionalEngineError::InvalidTemplateSlot {
            template_id: template.id.clone(),
            slot_id: slot_id.to_string(),
            expected: match kind {
                TemplateSlotKind::Text => "text",
                TemplateSlotKind::Image => "image path",
                TemplateSlotKind::Video => "video path",
                TemplateSlotKind::Color => "color string",
                TemplateSlotKind::Number => "number",
                TemplateSlotKind::TargetClip => "clip id",
                TemplateSlotKind::SafeAreaProfile => "safe-area profile",
            },
        })
    }
}

/// Lower a filled template into title overlays and safe-area diagnostics.
pub fn lower_motion_template(
    filled: &FilledMotionTemplate,
    timing: MotionTemplateTiming,
) -> MotionTemplateRender {
    let mut render = MotionTemplateRender {
        safe_area_violations: validate_motion_safe_areas(filled),
        ..MotionTemplateRender::default()
    };
    let name = text_slot(filled, "name")
        .or_else(|| text_slot(filled, "text"))
        .or_else(|| text_slot(filled, "title"))
        .unwrap_or_else(|| filled.name.clone());
    let subtitle = text_slot(filled, "title").filter(|text| text != &name);
    match timing.animation {
        TemplateAnimation::TextReveal | TemplateAnimation::WriteOn => {
            render.titles.extend(progressive_titles(&name, timing));
        }
        TemplateAnimation::Opacity => render.titles.push(title_plan(
            name,
            timing,
            TitlePosition::Bottom,
            TitleAnimation::FadeInOut,
            52,
        )),
        TemplateAnimation::Transform => render.titles.push(title_plan(
            name,
            timing,
            TitlePosition::Bottom,
            TitleAnimation::SlideIn,
            52,
        )),
        TemplateAnimation::None => render.titles.push(title_plan(
            name,
            timing,
            TitlePosition::Bottom,
            TitleAnimation::None,
            52,
        )),
    }
    if let Some(subtitle) = subtitle {
        render.titles.push(title_plan(
            subtitle,
            timing,
            TitlePosition::Bottom,
            TitleAnimation::FadeIn,
            32,
        ));
    }
    render
        .parameter_animations
        .extend(template_parameter_animations(filled, timing));
    render
}

fn template_parameter_animations(
    filled: &FilledMotionTemplate,
    timing: MotionTemplateTiming,
) -> Vec<ParameterAnimation> {
    let Some(target_clip) = text_slot(filled, "target_clip") else {
        return Vec::new();
    };
    let duration_s = (timing.end_s - timing.start_s).max(0.1);
    match filled.template_id.as_str() {
        "lower-third" | "title-reveal" | "callout" => vec![
            clip_animation(
                &filled.template_id,
                &target_clip,
                "title.opacity",
                fade_keyframes(duration_s),
            ),
            clip_animation(
                &filled.template_id,
                &target_clip,
                "title.y",
                vec![
                    keyframe(0.0, 0.08),
                    keyframe((duration_s * 0.20).min(0.35), 0.0),
                    keyframe(duration_s, 0.0),
                ],
            ),
        ],
        "focus-highlight" | "pip-emphasis" | "product-insert-emphasis" | "punch-in-zoom" => {
            let scale = number_slot(filled, "scale")
                .or_else(|| number_slot(filled, "intensity"))
                .unwrap_or(1.12)
                .max(0.05);
            vec![
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.scale",
                    vec![
                        keyframe(0.0, 1.0),
                        keyframe(duration_s / 2.0, scale),
                        keyframe(duration_s, 1.0),
                    ],
                ),
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.opacity",
                    fade_keyframes(duration_s),
                ),
            ]
        }
        _ => Vec::new(),
    }
}

fn clip_animation(
    template_id: &str,
    clip_id: &str,
    parameter: &str,
    keyframes: Vec<Keyframe>,
) -> ParameterAnimation {
    ParameterAnimation {
        id: format!("{template_id}-{clip_id}-{parameter}"),
        target: AnimationTarget::ClipParameter {
            clip_id: clip_id.into(),
            parameter: parameter.into(),
        },
        keyframes,
        rationale: Some(format!("lowered from motion template {template_id}")),
    }
}

fn fade_keyframes(duration_s: f64) -> Vec<Keyframe> {
    let ramp_s = (duration_s * 0.20).clamp(0.1, 0.35);
    vec![
        keyframe(0.0, 0.0),
        keyframe(ramp_s, 1.0),
        keyframe((duration_s - ramp_s).max(ramp_s), 1.0),
        keyframe(duration_s, 0.0),
    ]
}

fn keyframe(time_s: f64, value: f64) -> Keyframe {
    Keyframe {
        time_s,
        value,
        interpolation: KeyframeInterpolation::Linear,
        easing: Easing::EaseInOut,
        bezier: None,
    }
}

fn validate_motion_safe_areas(filled: &FilledMotionTemplate) -> Vec<ProfessionalDiagnostic> {
    filled
        .safe_areas
        .iter()
        .filter(|rule| !rule.margin_pct.is_finite() || !(0.0..=0.5).contains(&rule.margin_pct))
        .map(|rule| {
            ProfessionalDiagnostic::warning(
                CapabilityArea::MotionGraphicsTemplates,
                format!(
                    "motion template {} safe area {} margin must be in 0..=0.5",
                    filled.template_id, rule.profile
                ),
            )
        })
        .collect()
}

fn progressive_titles(text: &str, timing: MotionTemplateTiming) -> Vec<TitlePlan> {
    let clusters = reveal_text_clusters(text);
    if clusters.is_empty() {
        return Vec::new();
    }
    let span = ((timing.end_s - timing.start_s) / clusters.len() as f64).max(0.04);
    (1..=clusters.len())
        .map(|idx| {
            let partial = clusters.iter().take(idx).cloned().collect::<String>();
            let mut t = title_plan(
                partial,
                timing,
                TitlePosition::Bottom,
                TitleAnimation::None,
                52,
            );
            t.start_s = timing.start_s + span * (idx.saturating_sub(1)) as f64;
            t.end_s = timing.end_s;
            t
        })
        .collect()
}

fn reveal_text_clusters(text: &str) -> Vec<String> {
    UnicodeSegmentation::graphemes(text, true)
        .map(str::to_string)
        .collect()
}

fn title_plan(
    text: String,
    timing: MotionTemplateTiming,
    position: TitlePosition,
    animation: TitleAnimation,
    font_size: u32,
) -> TitlePlan {
    TitlePlan {
        text,
        start_s: timing.start_s,
        end_s: timing.end_s.max(timing.start_s + 0.1),
        position,
        font_size,
        color: "#FFFFFF".into(),
        font_weight: TitleWeight::Bold,
        animation,
        role: "motion_template".into(),
        safe_area: Some("title_safe".into()),
        animations: Vec::new(),
    }
}

fn text_slot(filled: &FilledMotionTemplate, key: &str) -> Option<String> {
    filled
        .slots
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn number_slot(filled: &FilledMotionTemplate, key: &str) -> Option<f64> {
    filled.slots.get(key).and_then(Value::as_f64)
}

/// Summary artifacts for color review.
#[derive(Debug, Clone, Default)]
pub struct ColorReviewPackage {
    /// Reference still paths.
    pub reference_stills: Vec<String>,
    /// Shot-group consistency summaries.
    pub consistency_summaries: Vec<String>,
    /// Before/after contact sheet artifact path.
    pub contact_sheet_path: String,
}

/// Build a color review package from durable color finishing state.
pub fn summarize_color_finishing(
    state: &ColorFinishingState,
    review_root: &str,
) -> ColorReviewPackage {
    let reference_stills = state
        .reference_stills
        .iter()
        .map(|still| still.source.clone())
        .collect();
    let consistency_summaries = state
        .shot_groups
        .iter()
        .map(|group| {
            format!(
                "{}: {} shot{} grouped for color consistency",
                group.id,
                group.clip_ids.len(),
                if group.clip_ids.len() == 1 { "" } else { "s" }
            )
        })
        .collect();
    ColorReviewPackage {
        reference_stills,
        consistency_summaries,
        contact_sheet_path: format!(
            "{}/before-after-contact-sheet.json",
            review_root.trim_end_matches('/')
        ),
    }
}

/// Lower supported grade stages to the current color correction plan.
pub fn lower_grade_stack(
    stack: &GradeStack,
) -> Result<ColorCorrectionPlan, ProfessionalEngineError> {
    let mut plan = ColorCorrectionPlan::default();
    let mut supported = false;
    for stage in &stack.stages {
        supported |= lower_grade_stage(stage, &mut plan);
    }
    if supported {
        Ok(plan)
    } else {
        Err(ProfessionalEngineError::UnsupportedGradeStack(
            stack.id.clone(),
        ))
    }
}

fn lower_grade_stage(stage: &GradeStage, plan: &mut ColorCorrectionPlan) -> bool {
    match stage.kind.as_str() {
        "primary" | "basic" => {
            plan.exposure_ev = number_from_map(&stage.params, "exposure_ev").or(plan.exposure_ev);
            plan.contrast = number_from_map(&stage.params, "contrast").or(plan.contrast);
            plan.saturation = number_from_map(&stage.params, "saturation").or(plan.saturation);
            plan.shadows = number_from_map(&stage.params, "shadows").or(plan.shadows);
            plan.highlights = number_from_map(&stage.params, "highlights").or(plan.highlights);
            true
        }
        "white_balance" | "balance" => {
            plan.temperature = number_from_map(&stage.params, "temperature").or(plan.temperature);
            plan.tint = number_from_map(&stage.params, "tint").or(plan.tint);
            true
        }
        _ => false,
    }
}

fn number_from_map(map: &HashMap<String, Value>, key: &str) -> Option<f64> {
    map.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
}

/// Audio finishing lowering and diagnostics.
#[derive(Debug, Clone, Default)]
pub struct AudioFinishingLowering {
    /// Track plans routed to the current audio mixer.
    pub track_plans: Vec<AudioTrackPlan>,
    /// Optional final loudness target.
    pub loudness_target: Option<LoudnessTargetPlan>,
    /// Review findings.
    pub findings: Vec<ProfessionalReviewFinding>,
}

/// Review finding used by render-side professional packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfessionalReviewFinding {
    /// Stable finding kind.
    pub kind: String,
    /// Severity.
    pub severity: FindingSeverity,
    /// Message.
    pub message: String,
    /// Optional fix proposal reference.
    pub fix_ref: Option<String>,
}

/// Lower role/bus/chains/automation into the current audio render plan.
pub fn lower_audio_finishing(state: &AudioFinishingState) -> AudioFinishingLowering {
    let track_plans = state
        .buses
        .iter()
        .map(|bus| audio_track_for_bus(bus, &state.chains, &state.automation))
        .collect();
    let loudness_target = state
        .meters
        .iter()
        .filter_map(|meter| meter.integrated_lufs)
        .next()
        .map(|_| LoudnessTargetPlan {
            integrated_lufs: -14.0,
            true_peak_db: Some(-1.5),
        });
    let mut findings = audio_meter_findings(&state.meters);
    findings.extend(audio_automation_findings(&state.buses, &state.automation));
    AudioFinishingLowering {
        track_plans,
        loudness_target,
        findings,
    }
}

fn audio_track_for_bus(
    bus: &AudioBus,
    chains: &[AudioChainPreset],
    automation: &[AudioAutomationLane],
) -> AudioTrackPlan {
    let volume = automation
        .iter()
        .find(|lane| lane.target == bus.id && lane.parameter == "volume_db")
        .and_then(|lane| lane.keyframes.first())
        .map(|keyframe| 10_f64.powf(keyframe.value / 20.0))
        .unwrap_or(1.0);
    let volume_automation = automation
        .iter()
        .find(|lane| lane.target == bus.id && lane.parameter == "volume_db")
        .filter(|lane| lane.keyframes.len() > 1)
        .map(volume_automation_plan);
    AudioTrackPlan {
        name: bus.id.clone(),
        role: audio_role_label(bus.role).into(),
        volume,
        volume_automation,
        muted: false,
        solo: false,
        ducking: ducking_for_role(bus.role),
        audio_fx: chain_for_bus(bus, chains),
        items: Vec::new(),
    }
}

fn volume_automation_plan(lane: &AudioAutomationLane) -> AudioAutomationPlan {
    let db_expr = keyframes_to_ffmpeg_expr(&lane.keyframes, "t");
    AudioAutomationPlan {
        parameter: lane.parameter.clone(),
        expression: format!("pow(10\\,({db_expr})/20)"),
        keyframes: lane.keyframes.clone(),
    }
}

fn audio_role_label(role: AudioRole) -> &'static str {
    match role {
        AudioRole::Dialogue => "dialogue",
        AudioRole::Music => "music",
        AudioRole::Sfx => "sfx",
        AudioRole::Ambience => "ambience",
        AudioRole::Voiceover => "voiceover",
        AudioRole::Master => "master",
    }
}

fn ducking_for_role(role: AudioRole) -> Option<DuckingPlan> {
    matches!(role, AudioRole::Music).then_some(DuckingPlan {
        enabled: true,
        amount_db: -10.0,
        attack_ms: 80.0,
        release_ms: 300.0,
    })
}

fn chain_for_bus(bus: &AudioBus, chains: &[AudioChainPreset]) -> Option<AudioFxPlan> {
    let chain = chains.iter().find(|chain| chain.id == bus.id)?;
    let mut fx = AudioFxPlan::default();
    for processor in &chain.processors {
        match processor.as_str() {
            "high_pass" => fx.high_pass_hz = Some(80.0),
            "low_pass" => fx.low_pass_hz = Some(16_000.0),
            "compressor" => {
                fx.compressor_threshold_db = Some(-18.0);
                fx.compressor_ratio = Some(3.0);
            }
            "limiter" => fx.limiter_limit_db = Some(-1.0),
            "noise_gate" => fx.noise_gate_threshold_db = Some(-55.0),
            "dialogue_eq" => fx.eq_bands.push(EqBandPlan {
                freq_hz: 3200.0,
                gain_db: 2.0,
                width_hz: Some(900.0),
            }),
            _ => {}
        }
    }
    Some(fx)
}

fn audio_automation_findings(
    buses: &[AudioBus],
    automation: &[AudioAutomationLane],
) -> Vec<ProfessionalReviewFinding> {
    buses
        .iter()
        .filter(|bus| ducking_for_role(bus.role).is_some())
        .filter(|bus| {
            automation
                .iter()
                .any(|lane| lane.target == bus.id && lane.parameter == "ducking_db")
        })
        .map(|bus| ProfessionalReviewFinding {
            kind: "ducking_automation_conflict".into(),
            severity: FindingSeverity::Warning,
            message: format!(
                "bus {} has explicit ducking automation and role-based ducking; review which source should drive gain reduction",
                bus.id
            ),
            fix_ref: Some(format!("fix-ducking-automation-{}", bus.id)),
        })
        .collect()
}

fn audio_meter_findings(meters: &[AudioMeterReading]) -> Vec<ProfessionalReviewFinding> {
    let mut findings = Vec::new();
    for meter in meters {
        if meter.clipping {
            findings.push(ProfessionalReviewFinding {
                kind: "clipping".into(),
                severity: FindingSeverity::Error,
                message: format!("{} has clipped samples", meter.target),
                fix_ref: Some(format!("fix-audio-clipping-{}", meter.target)),
            });
        }
        if let Some(lufs) = meter.integrated_lufs
            && !(-16.0..=-12.0).contains(&lufs)
        {
            findings.push(ProfessionalReviewFinding {
                kind: "loudness_out_of_range".into(),
                severity: FindingSeverity::Warning,
                message: format!(
                    "{} loudness {lufs:.1} LUFS is outside -16..-12",
                    meter.target
                ),
                fix_ref: Some(format!("fix-loudness-{}", meter.target)),
            });
        }
        if let Some(peak) = meter.true_peak_db
            && peak > -1.0
        {
            findings.push(ProfessionalReviewFinding {
                kind: "true_peak_high".into(),
                severity: FindingSeverity::Warning,
                message: format!("{} true peak {peak:.1} dBTP is above -1.0", meter.target),
                fix_ref: Some(format!("fix-peak-{}", meter.target)),
            });
        }
        if let Some(noise) = meter.noise_floor_db
            && noise > -45.0
        {
            findings.push(ProfessionalReviewFinding {
                kind: "noise_floor_high".into(),
                severity: FindingSeverity::Warning,
                message: format!("{} noise floor {noise:.1} dB is high", meter.target),
                fix_ref: Some(format!("fix-noise-{}", meter.target)),
            });
        }
    }
    findings
}

/// Delivery queue request.
#[derive(Debug, Clone)]
pub struct DeliveryQueueRequest {
    /// Selected delivery profile.
    pub profile: DeliveryProfile,
    /// Measured preflight facts.
    pub preflight_input: DeliveryPreflightInput,
    /// Planned output path.
    pub output_path: PathBuf,
}

/// Planned delivery queue item.
#[derive(Debug, Clone)]
pub struct DeliveryQueueItem {
    /// Queue id.
    pub id: String,
    /// Preflight report produced before render.
    pub preflight: PreflightReport,
    /// Package manifest for render and validation artifacts.
    pub manifest: PackageManifest,
}

/// Apply a named delivery profile to an existing render job spec.
pub fn apply_delivery_profile_to_spec(
    mut spec: RenderJobSpec,
    profile: &DeliveryProfile,
) -> RenderJobSpec {
    let insertion = spec.args.len().saturating_sub(1);
    let mut args = Vec::new();
    args.extend([
        "-s:v".into(),
        format!("{}x{}", profile.width, profile.height),
    ]);
    if let Some(bitrate) = profile.video_bitrate_kbps {
        args.extend(["-b:v".into(), format!("{bitrate}k")]);
    }
    spec.args.splice(insertion..insertion, args);
    spec
}

/// Apply a validated export preset to an existing render job spec.
pub fn apply_export_preset_to_spec(
    mut spec: RenderJobSpec,
    preset: &ExportPreset,
) -> Result<RenderJobSpec, ProfessionalEngineError> {
    if let Some(diagnostic) = preset
        .validate()
        .into_iter()
        .find(|diagnostic| diagnostic.severity == FindingSeverity::Error)
    {
        return Err(ProfessionalEngineError::InvalidExportPreset {
            preset_id: preset.id.clone(),
            message: diagnostic.message,
        });
    }
    let insertion = spec.args.len().saturating_sub(1);
    let mut args = Vec::new();
    if preset.video.is_some() {
        args.extend([
            "-s:v".into(),
            format!("{}x{}", preset.profile.width, preset.profile.height),
        ]);
    }
    if let Some(video) = &preset.video {
        args.extend(["-c:v".into(), video.codec.clone()]);
        if let Some(bitrate) = video.bitrate_kbps.or(preset.profile.video_bitrate_kbps) {
            args.extend(["-b:v".into(), format!("{bitrate}k")]);
        }
        if let Some(frame_rate) = video.frame_rate {
            args.extend(["-r".into(), format!("{frame_rate}")]);
        }
    }
    if let Some(audio) = &preset.audio {
        args.extend(["-c:a".into(), audio.codec.clone()]);
        if let Some(bitrate) = audio.bitrate_kbps {
            args.extend(["-b:a".into(), format!("{bitrate}k")]);
        }
        args.extend(["-ar".into(), audio.sample_rate_hz.to_string()]);
        args.extend(["-ac".into(), audio.channels.to_string()]);
    }
    args.extend(["-f".into(), preset.output.container.clone()]);
    spec.args.splice(insertion..insertion, args);
    Ok(spec)
}

/// Lower a stream-level export contract into deterministic FFmpeg arguments.
pub fn plan_stream_export_args(
    input_path: &Path,
    contract: &StreamExportContract,
    output_path: &Path,
) -> Result<Vec<String>, ProfessionalEngineError> {
    if let Some(diagnostic) = contract
        .validate()
        .into_iter()
        .find(|diagnostic| diagnostic.severity == FindingSeverity::Error)
    {
        return Err(ProfessionalEngineError::InvalidStreamExportContract {
            contract_id: contract.id.clone(),
            message: diagnostic.message,
        });
    }

    let mut args = vec![
        "-y".into(),
        "-i".into(),
        input_path.to_string_lossy().into_owned(),
    ];
    for stream in &contract.streams {
        args.extend(["-map".into(), format!("0:{}", stream.source_index)]);
    }
    for (output_index, stream) in contract.streams.iter().enumerate() {
        match stream.mode {
            StreamExportMode::Copy => {
                args.extend([format!("-c:{output_index}"), "copy".into()]);
            }
            StreamExportMode::Transcode => {
                if let Some(codec) = &stream.codec {
                    args.extend([format!("-c:{output_index}"), codec.clone()]);
                }
            }
        }
        if let Some(language) = &stream.language {
            args.extend([
                format!("-metadata:s:{output_index}"),
                format!("language={language}"),
            ]);
        }
        for (key, value) in &stream.metadata {
            args.extend([
                format!("-metadata:s:{output_index}"),
                format!("{key}={value}"),
            ]);
        }
        if !stream.disposition.is_empty() {
            args.extend([
                format!("-disposition:{output_index}"),
                stream.disposition.join("+"),
            ]);
        }
    }
    for (key, value) in &contract.metadata {
        args.extend(["-metadata".into(), format!("{key}={value}")]);
    }
    args.extend([
        "-f".into(),
        contract.container.clone(),
        output_path.to_string_lossy().into_owned(),
    ]);
    Ok(args)
}

/// Build a queued delivery package with actionable fix references.
pub fn plan_delivery_queue_item(request: DeliveryQueueRequest) -> DeliveryQueueItem {
    let mut preflight = request.profile.run_preflight(request.preflight_input);
    attach_preflight_fix_refs(&mut preflight);
    let output = request.output_path.to_string_lossy().into_owned();
    DeliveryQueueItem {
        id: format!("delivery-{}", request.profile.id),
        manifest: PackageManifest {
            id: format!("package-{}", request.profile.id),
            artifacts: vec![output],
            validation_reports: vec![preflight.id.clone()],
        },
        preflight,
    }
}

fn attach_preflight_fix_refs(report: &mut PreflightReport) {
    for finding in &mut report.findings {
        finding.fix_ref = Some(
            match finding.check {
                awidat_proto::professional::PreflightCheckKind::AspectRatio => {
                    "fix-delivery-aspect-ratio"
                }
                awidat_proto::professional::PreflightCheckKind::Duration => "fix-delivery-duration",
                awidat_proto::professional::PreflightCheckKind::Bitrate => "fix-delivery-bitrate",
                awidat_proto::professional::PreflightCheckKind::Captions => "fix-delivery-captions",
                awidat_proto::professional::PreflightCheckKind::Loudness => "fix-delivery-loudness",
                awidat_proto::professional::PreflightCheckKind::SafeAreas => {
                    "fix-delivery-safe-areas"
                }
                awidat_proto::professional::PreflightCheckKind::Metadata => "fix-delivery-metadata",
                awidat_proto::professional::PreflightCheckKind::Thumbnail => {
                    "fix-delivery-thumbnail"
                }
            }
            .into(),
        );
    }
}
