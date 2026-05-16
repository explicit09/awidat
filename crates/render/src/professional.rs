//! Operational professional-pipeline engines and render lowerers.
//!
//! These functions sit above the durable schemas in `awidat-proto` and below
//! tool/UI orchestration. They intentionally produce deterministic plans and
//! diagnostics from existing evidence; expensive CV/ML sidecars can feed these
//! APIs later without changing callers.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use awidat_proto::professional::{
    AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState, AudioMeterReading,
    AudioRole, CapabilityArea, ColorFinishingState, CompositionGraph, CompositionNode,
    CompositionNodeType, DeliveryPreflightInput, DeliveryProfile, FindingSeverity, GradeStack,
    GradeStage, MaskSidecar, MatteSidecar, MotionGraphicsTemplate, PackageManifest,
    PreflightReport, ProfessionalDiagnostic, TemplateSlotKind, TrackKind, TrackSample,
    TrackSidecar, TrackingPackage,
};
use serde_json::Value;
use thiserror::Error;

use crate::timeline::ColorCorrectionPlan;
use crate::{
    AudioFxPlan, AudioTrackPlan, DuckingPlan, EqBandPlan, LoudnessTargetPlan, RenderJobSpec,
    TitleAnimation, TitlePlan, TitlePosition, TitleWeight,
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
    /// A grade stack has no supported stages.
    #[error("grade stack {0} has no supported render stages")]
    UnsupportedGradeStack(String),
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
        masks: Vec::<MaskSidecar>::new(),
        mattes: Vec::<MatteSidecar>::new(),
    }
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

fn lower_composition_node(node: &CompositionNode) -> Option<RenderLoweringStep> {
    let expression = match node.node_type {
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
    /// Safe-area violations found before render.
    pub safe_area_violations: Vec<ProfessionalDiagnostic>,
    /// Limitations for richer template features.
    pub limitations: Vec<RenderLimitation>,
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
        TemplateSlotKind::Text | TemplateSlotKind::Color | TemplateSlotKind::Image => {
            value.as_str().is_some()
        }
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
                TemplateSlotKind::Color => "color string",
                TemplateSlotKind::Number => "number",
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
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }
    let span = ((timing.end_s - timing.start_s) / chars.len() as f64).max(0.04);
    (1..=chars.len())
        .map(|idx| {
            let partial = chars.iter().take(idx).collect::<String>();
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
    AudioFinishingLowering {
        track_plans,
        loudness_target,
        findings: audio_meter_findings(&state.meters),
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
    AudioTrackPlan {
        name: bus.id.clone(),
        role: audio_role_label(bus.role).into(),
        volume,
        muted: false,
        solo: false,
        ducking: ducking_for_role(bus.role),
        audio_fx: chain_for_bus(bus, chains),
        items: Vec::new(),
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
