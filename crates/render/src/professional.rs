//! Operational professional-pipeline engines and render lowerers.
//!
//! These functions sit above the durable schemas in `montage-proto` and below
//! tool/UI orchestration. They intentionally produce deterministic plans and
//! diagnostics from existing evidence; expensive CV/ML sidecars can feed these
//! APIs later without changing callers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use montage_proto::montage_meta::MontageTimelineMetadata;
use montage_proto::professional::{
    AnimationTarget, AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState,
    AudioMeterReading, AudioRole, CapabilityArea, ColorFinishingState, CompositionGraph,
    CompositionNode, CompositionNodeType, CoordinateSpace, DeliveryPreflightInput, DeliveryProfile,
    Easing, ExportPreset, ExpressionLink, ExpressionSource, ExtrapolationMode, FindingSeverity,
    GradeStack, GradeStage, HardwareAccelerationPolicy, Keyframe, KeyframeInterpolation,
    MaskSidecar, MatteSidecar, MotionGraphicsTemplate, MotionPackage, PackageManifest,
    ParameterAnimation, PreflightReport, ProfessionalDiagnostic, RUNTIME_CLIP_PARAMETERS,
    ReframeKeyframe, ReframePath, ReframeSmoothing, ReviewStatus, SafeAreaRule,
    StreamExportContract, StreamExportMode, TemplateSlot, TemplateSlotKind, TrackKind, TrackSample,
    TrackSidecar, TrackingPackage, canonical_runtime_clip_parameter, is_runtime_clip_parameter,
};
use serde_json::Value;
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

use crate::animation::keyframes_to_ffmpeg_expr;
use crate::timeline::{
    ColorCorrectionPlan, RenderParameterAnimation, TextReveal, TimelineSegment, VideoOverlayMode,
    VideoOverlayPlan,
};
use crate::{
    AudioAutomationPlan, AudioFxPlan, AudioTrackPlan, DuckingPlan, EqBandPlan, LoudnessTargetPlan,
    RenderJobSpec, TitleAnimation, TitlePlan, TitlePosition, TitleWeight,
};

const TRACKING_REVIEW_CONFIDENCE_THRESHOLD: f64 = 0.75;

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
    /// A tracker binding node is malformed or not supported by the evaluator.
    #[error("tracker binding {binding_id} is invalid: {message}")]
    InvalidTrackerBinding {
        /// Composition node / binding id.
        binding_id: String,
        /// Detailed validation message.
        message: String,
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

/// Normalized rectangle used to initialize a tracker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerRegion {
    /// Left coordinate in normalized frame space.
    pub x: f64,
    /// Top coordinate in normalized frame space.
    pub y: f64,
    /// Width in normalized frame space.
    pub width: f64,
    /// Height in normalized frame space.
    pub height: f64,
}

impl TrackerRegion {
    /// Build a normalized `x/y/width/height` region.
    pub fn from_xywh(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Request to create or reuse a tracker for a clip region.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingRequest {
    /// Clip or asset id to track.
    pub clip_id: String,
    /// Requested track primitive.
    pub kind: TrackKind,
    /// Initial normalized tracking rectangle.
    pub region: TrackerRegion,
    /// First sampled frame.
    pub start_frame: u64,
    /// Last sampled frame, inclusive.
    pub end_frame: u64,
}

/// One observed tracker rectangle from a detection/tracking pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerObservation {
    /// Frame number for this observation.
    pub frame: u64,
    /// Observed normalized tracking rectangle.
    pub region: TrackerRegion,
    /// Observation confidence in `0.0..=1.0`.
    pub confidence: f64,
}

/// Opaque handle returned to callers after tracker creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerHandle {
    /// Stable sidecar track id.
    pub track_id: String,
}

/// Agent/UI request to author an overlay insert bound to a tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedInsertRequest {
    /// Overlay clip id that should follow the tracked subject.
    pub overlay_clip_id: String,
    /// Tracker region request for the source clip.
    pub tracker: TrackingRequest,
    /// Optional observed tracker rows from a detector/tracker pass.
    pub observations: Vec<TrackerObservation>,
}

/// Authored tracker insert with concrete runtime animations and review summary.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedInsertPlan {
    /// Stable track id used by the authored insert.
    pub track_id: String,
    /// Generated `overlay.x` and `overlay.y` animations for the insert clip.
    pub generated_animations: Vec<ParameterAnimation>,
    /// Review summary callers can surface before accepting the insert.
    pub review: TrackingReviewPackage,
}

/// Agent/UI request to author a subject-aware reframe path from observed boxes.
#[derive(Debug, Clone, PartialEq)]
pub struct SubjectReframeRequest {
    /// Timeline clip id receiving the reframe path.
    pub clip_id: String,
    /// Delivery aspect ratio label, e.g. `9:16`.
    pub aspect_ratio: String,
    /// Source media width in pixels.
    pub source_width: u32,
    /// Source media height in pixels.
    pub source_height: u32,
    /// Target canvas width in pixels.
    pub target_width: u32,
    /// Target canvas height in pixels.
    pub target_height: u32,
    /// Observation frame rate.
    pub frame_rate: f64,
    /// Smoothing policy for generated crop centers.
    pub smoothing: ReframeSmoothing,
    /// Optional safe-area policy label.
    pub safe_area: Option<String>,
}

/// Authored subject reframe path and review summary.
#[derive(Debug, Clone, PartialEq)]
pub struct SubjectReframePlan {
    /// Stable reframe path id.
    pub reframe_id: String,
    /// Review summary callers can surface before accepting the path.
    pub review: TrackingReviewPackage,
}

/// Compact review surface for tracker packages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackingReviewPackage {
    /// Track rows with confidence and correction cues.
    pub tracks: Vec<TrackingReviewTrack>,
    /// Reframe rows with confidence and correction cues.
    pub reframe_paths: Vec<TrackingReviewReframe>,
}

/// Review row for a tracker sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReviewTrack {
    /// Track id.
    pub track_id: String,
    /// Asset or clip id being tracked.
    pub asset_id: String,
    /// Track primitive kind.
    pub kind: TrackKind,
    /// Number of sampled frames.
    pub sample_count: usize,
    /// Average confidence if available.
    pub confidence: Option<f64>,
    /// Frames that need user/agent correction.
    pub correction_frames: Vec<u64>,
    /// True when correction frames or low aggregate confidence remain.
    pub requires_correction: bool,
}

/// Review row for a subject-aware reframe path.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReviewReframe {
    /// Reframe path id.
    pub reframe_id: String,
    /// Clip being reframed.
    pub clip_id: String,
    /// Number of authored crop keyframes.
    pub keyframe_count: usize,
    /// Average keyframe confidence if available.
    pub confidence: Option<f64>,
    /// Times that need user/agent correction.
    pub correction_times_s: Vec<f64>,
    /// True when correction times or low aggregate confidence remain.
    pub requires_correction: bool,
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

/// Deterministic lowering for a surface track driving a corner-pin distortion.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerPinLowering {
    /// Track id used for the surface binding.
    pub track_id: String,
    /// Clip id receiving the corner-pin distortion.
    pub target_clip_id: String,
    /// FFmpeg perspective filter expression.
    pub filter: String,
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

/// Ensure a stable tracker exists for a clip region and return its handle.
pub fn ensure_tracker(
    package: &mut TrackingPackage,
    request: TrackingRequest,
) -> Result<TrackerHandle, ProfessionalEngineError> {
    validate_tracking_request(&request)?;
    let track_id = tracker_request_id(&request);
    if package.tracks.iter().any(|track| track.id == track_id) {
        return Ok(TrackerHandle { track_id });
    }
    let samples = (request.start_frame..=request.end_frame)
        .map(|frame| TrackSample {
            frame,
            points: tracker_region_points(request.kind, request.region),
            confidence: Some(1.0),
        })
        .collect::<Vec<_>>();
    package.tracks.push(TrackSidecar {
        id: track_id.clone(),
        asset_id: request.clip_id,
        kind: request.kind,
        coordinate_space: CoordinateSpace::Normalized,
        samples,
        confidence: Some(1.0),
    });
    Ok(TrackerHandle { track_id })
}

/// Ensure a stable tracker exists from observed per-frame rectangles.
pub fn ensure_tracker_from_observations(
    package: &mut TrackingPackage,
    request: TrackingRequest,
    observations: Vec<TrackerObservation>,
) -> Result<TrackerHandle, ProfessionalEngineError> {
    validate_tracking_request(&request)?;
    if observations.is_empty() {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "ensure_tracker_from_observations".into(),
            message: "at least one tracker observation is required".into(),
        });
    }
    let track_id = tracker_request_id(&request);
    let samples = tracker_samples_from_observations(&request, observations)?;
    let confidence = average_sample_confidence(&samples);
    let sidecar = TrackSidecar {
        id: track_id.clone(),
        asset_id: request.clip_id,
        kind: request.kind,
        coordinate_space: CoordinateSpace::Normalized,
        samples,
        confidence,
    };
    if let Some(existing) = package.tracks.iter_mut().find(|track| track.id == track_id) {
        *existing = sidecar;
    } else {
        package.tracks.push(sidecar);
    }
    Ok(TrackerHandle { track_id })
}

/// Author a tracked overlay insert from a tracker request and optional observations.
pub fn author_tracked_insert(
    package: &mut TrackingPackage,
    request: TrackedInsertRequest,
    frame_rate: f64,
) -> Result<TrackedInsertPlan, ProfessionalEngineError> {
    if request.overlay_clip_id.trim().is_empty() {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "author_tracked_insert".into(),
            message: "overlay_clip_id is required".into(),
        });
    }
    let overlay_clip_id = request.overlay_clip_id;
    let handle = if request.observations.is_empty() {
        ensure_tracker(package, request.tracker)?
    } else {
        ensure_tracker_from_observations(package, request.tracker, request.observations)?
    };
    let graph = tracked_insert_binding_graph(&handle.track_id, &overlay_clip_id);
    let generated_animations = lower_tracker_parameter_bindings(package, &graph, frame_rate)?;
    Ok(TrackedInsertPlan {
        track_id: handle.track_id,
        generated_animations,
        review: summarize_tracking_package(package),
    })
}

/// Author a subject-aware crop/reframe path from observed tracker boxes.
pub fn author_subject_reframe_path(
    package: &mut TrackingPackage,
    request: SubjectReframeRequest,
    observations: Vec<TrackerObservation>,
) -> Result<SubjectReframePlan, ProfessionalEngineError> {
    validate_subject_reframe_request(&request)?;
    if observations.is_empty() {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "author_subject_reframe_path".into(),
            message: "at least one subject observation is required".into(),
        });
    }
    let mut keyframes = Vec::with_capacity(observations.len());
    let scale = reframe_scale_for_aspect(&request);
    for observation in observations {
        validate_tracker_region(observation.region, &request.clip_id)?;
        if !observation.confidence.is_finite() {
            return Err(ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: "author_subject_reframe_path".into(),
                message: "subject observation confidence must be finite".into(),
            });
        }
        keyframes.push(ReframeKeyframe {
            time_s: observation.frame as f64 / request.frame_rate,
            center: [
                stable_tracker_coordinate(observation.region.x + observation.region.width / 2.0),
                stable_tracker_coordinate(observation.region.y + observation.region.height / 2.0),
            ],
            scale,
            confidence: Some(observation.confidence.clamp(0.0, 1.0)),
        });
    }
    keyframes.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    store_subject_reframe_path(package, request, keyframes, "subject-observations".into())
}

/// Author a subject-aware crop/reframe path from an existing normalized tracker.
pub fn author_subject_reframe_path_from_track(
    package: &mut TrackingPackage,
    track_id: &str,
    request: SubjectReframeRequest,
) -> Result<SubjectReframePlan, ProfessionalEngineError> {
    validate_subject_reframe_request(&request)?;
    let track = package
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or_else(|| ProfessionalEngineError::MissingTrack(track_id.to_string()))?;
    if track.coordinate_space != CoordinateSpace::Normalized {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: track_id.to_string(),
            message: "subject reframe requires normalized tracker coordinate space".into(),
        });
    }
    let scale = reframe_scale_for_aspect(&request);
    let mut keyframes = Vec::new();
    for sample in track
        .samples
        .iter()
        .filter(|sample| !sample.points.is_empty())
    {
        let center = tracker_sample_center(sample, track_id)?;
        let confidence = sample
            .confidence
            .or(track.confidence)
            .filter(|confidence| confidence.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        keyframes.push(ReframeKeyframe {
            time_s: sample.frame as f64 / request.frame_rate,
            center,
            scale,
            confidence: Some(confidence),
        });
    }
    if keyframes.is_empty() {
        return Err(ProfessionalEngineError::MissingTrack(track_id.to_string()));
    }
    keyframes.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    store_subject_reframe_path(package, request, keyframes, track_id.to_string())
}

fn store_subject_reframe_path(
    package: &mut TrackingPackage,
    request: SubjectReframeRequest,
    keyframes: Vec<ReframeKeyframe>,
    evidence_track_id: String,
) -> Result<SubjectReframePlan, ProfessionalEngineError> {
    let reframe_id = subject_reframe_id(&request);
    let path = ReframePath {
        id: reframe_id.clone(),
        clip_id: request.clip_id,
        aspect_ratio: request.aspect_ratio,
        source_width: request.source_width,
        source_height: request.source_height,
        target_width: request.target_width,
        target_height: request.target_height,
        keyframes,
        smoothing: request.smoothing,
        evidence_track_id: Some(evidence_track_id),
        safe_area: request.safe_area,
    };
    if let Some(existing) = package
        .reframe_paths
        .iter_mut()
        .find(|candidate| candidate.id == reframe_id)
    {
        *existing = path;
    } else {
        package.reframe_paths.push(path);
    }
    Ok(SubjectReframePlan {
        reframe_id,
        review: summarize_tracking_package(package),
    })
}

fn tracker_sample_center(
    sample: &TrackSample,
    binding_id: &str,
) -> Result<[f64; 2], ProfessionalEngineError> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in &sample.points {
        if !point[0].is_finite()
            || !point[1].is_finite()
            || !(0.0..=1.0).contains(&point[0])
            || !(0.0..=1.0).contains(&point[1])
        {
            return Err(ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: binding_id.to_string(),
                message: "tracker samples for subject reframe must be finite normalized points"
                    .into(),
            });
        }
        min_x = min_x.min(point[0]);
        min_y = min_y.min(point[1]);
        max_x = max_x.max(point[0]);
        max_y = max_y.max(point[1]);
    }
    Ok([
        stable_tracker_coordinate((min_x + max_x) / 2.0),
        stable_tracker_coordinate((min_y + max_y) / 2.0),
    ])
}

/// Summarize a tracking package into rows suitable for UI or agent review.
pub fn summarize_tracking_package(package: &TrackingPackage) -> TrackingReviewPackage {
    TrackingReviewPackage {
        tracks: package.tracks.iter().map(tracking_review_track).collect(),
        reframe_paths: package
            .reframe_paths
            .iter()
            .map(tracking_review_reframe)
            .collect(),
    }
}

fn validate_subject_reframe_request(
    request: &SubjectReframeRequest,
) -> Result<(), ProfessionalEngineError> {
    if request.clip_id.trim().is_empty() {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "author_subject_reframe_path".into(),
            message: "clip_id is required".into(),
        });
    }
    if request.aspect_ratio.trim().is_empty()
        || request.source_width == 0
        || request.source_height == 0
        || request.target_width == 0
        || request.target_height == 0
        || !request.frame_rate.is_finite()
        || request.frame_rate <= 0.0
    {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "author_subject_reframe_path".into(),
            message: "aspect ratio, source/target dimensions, and positive frame_rate are required"
                .into(),
        });
    }
    Ok(())
}

fn reframe_scale_for_aspect(request: &SubjectReframeRequest) -> f64 {
    let source_aspect = f64::from(request.source_width) / f64::from(request.source_height);
    let target_aspect = f64::from(request.target_width) / f64::from(request.target_height);
    if target_aspect <= source_aspect {
        source_aspect / target_aspect
    } else {
        target_aspect / source_aspect
    }
    .max(1.0)
}

fn subject_reframe_id(request: &SubjectReframeRequest) -> String {
    let aspect = request
        .aspect_ratio
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!("reframe-{}-{aspect}", request.clip_id)
}

fn tracked_insert_binding_graph(track_id: &str, overlay_clip_id: &str) -> CompositionGraph {
    CompositionGraph {
        id: format!("tracked-insert-{overlay_clip_id}"),
        nodes: vec![
            tracker_bind_node_for_authoring(
                "tracked-insert-x",
                track_id,
                overlay_clip_id,
                "overlay.x",
                "x",
            ),
            tracker_bind_node_for_authoring(
                "tracked-insert-y",
                track_id,
                overlay_clip_id,
                "overlay.y",
                "y",
            ),
        ],
        ..CompositionGraph::default()
    }
}

fn tracker_bind_node_for_authoring(
    id: &str,
    track_id: &str,
    target_clip_id: &str,
    target_parameter: &str,
    channel: &str,
) -> CompositionNode {
    CompositionNode {
        id: id.into(),
        node_type: CompositionNodeType::TrackerBind,
        params: HashMap::from([
            ("track_id".into(), Value::String(track_id.into())),
            (
                "target_clip_id".into(),
                Value::String(target_clip_id.into()),
            ),
            (
                "target_parameter".into(),
                Value::String(target_parameter.into()),
            ),
            ("channel".into(), Value::String(channel.into())),
        ]),
    }
}

fn tracker_samples_from_observations(
    request: &TrackingRequest,
    observations: Vec<TrackerObservation>,
) -> Result<Vec<TrackSample>, ProfessionalEngineError> {
    let mut observations = observations;
    observations.sort_by_key(|observation| observation.frame);
    let mut previous_frame = None;
    let mut samples = Vec::with_capacity(observations.len());
    for observation in observations {
        if observation.frame < request.start_frame || observation.frame > request.end_frame {
            return Err(ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: request.clip_id.clone(),
                message: format!(
                    "tracker observation frame {} is outside requested frame range {}..={}",
                    observation.frame, request.start_frame, request.end_frame
                ),
            });
        }
        if previous_frame == Some(observation.frame) {
            return Err(ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: request.clip_id.clone(),
                message: format!(
                    "duplicate tracker observation for frame {}",
                    observation.frame
                ),
            });
        }
        validate_tracker_region(observation.region, &request.clip_id)?;
        if !observation.confidence.is_finite() {
            return Err(ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: request.clip_id.clone(),
                message: "tracker observation confidence must be finite".into(),
            });
        }
        samples.push(TrackSample {
            frame: observation.frame,
            points: tracker_region_points(request.kind, observation.region),
            confidence: Some(observation.confidence.clamp(0.0, 1.0)),
        });
        previous_frame = Some(observation.frame);
    }
    Ok(samples)
}

fn validate_tracking_request(request: &TrackingRequest) -> Result<(), ProfessionalEngineError> {
    if request.clip_id.trim().is_empty() {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: "ensure_tracker".into(),
            message: "clip_id is required".into(),
        });
    }
    if request.end_frame < request.start_frame {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: request.clip_id.clone(),
            message: "end_frame must be greater than or equal to start_frame".into(),
        });
    }
    validate_tracker_region(request.region, &request.clip_id)
}

fn validate_tracker_region(
    region: TrackerRegion,
    binding_id: &str,
) -> Result<(), ProfessionalEngineError> {
    let valid_region = [region.x, region.y, region.width, region.height]
        .into_iter()
        .all(f64::is_finite)
        && region.width > 0.0
        && region.height > 0.0
        && region.x >= 0.0
        && region.y >= 0.0
        && region.x + region.width <= 1.0
        && region.y + region.height <= 1.0;
    if !valid_region {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: binding_id.to_string(),
            message: "tracker region must be finite normalized xywh within 0..1".into(),
        });
    }
    Ok(())
}

fn tracker_request_id(request: &TrackingRequest) -> String {
    format!(
        "tracker-{}-{}-{}-{}-{}-{}-{}-{}",
        sanitize_track_id_component(&request.clip_id),
        track_kind_id(request.kind),
        request.start_frame,
        request.end_frame,
        tracker_coord_id(request.region.x),
        tracker_coord_id(request.region.y),
        tracker_coord_id(request.region.width),
        tracker_coord_id(request.region.height)
    )
}

fn sanitize_track_id_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    sanitized.trim_matches('-').to_string()
}

fn track_kind_id(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Point => "point",
        TrackKind::Planar => "planar",
        TrackKind::Surface => "surface",
    }
}

fn tracker_coord_id(value: f64) -> i64 {
    (value * 10_000.0).round() as i64
}

fn tracker_region_points(kind: TrackKind, region: TrackerRegion) -> Vec<[f64; 2]> {
    let left = stable_tracker_coordinate(region.x);
    let top = stable_tracker_coordinate(region.y);
    let right = stable_tracker_coordinate(region.x + region.width);
    let bottom = stable_tracker_coordinate(region.y + region.height);
    match kind {
        TrackKind::Point => vec![[
            stable_tracker_coordinate((left + right) / 2.0),
            stable_tracker_coordinate((top + bottom) / 2.0),
        ]],
        TrackKind::Planar | TrackKind::Surface => {
            vec![[left, top], [right, top], [right, bottom], [left, bottom]]
        }
    }
}

fn stable_tracker_coordinate(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn tracking_review_track(track: &TrackSidecar) -> TrackingReviewTrack {
    let confidence = track
        .confidence
        .filter(|confidence| confidence.is_finite())
        .or_else(|| average_sample_confidence(&track.samples));
    let correction_frames = track
        .samples
        .iter()
        .filter(|sample| track_sample_requires_correction(sample))
        .map(|sample| sample.frame)
        .collect::<Vec<_>>();
    TrackingReviewTrack {
        track_id: track.id.clone(),
        asset_id: track.asset_id.clone(),
        kind: track.kind,
        sample_count: track.samples.len(),
        confidence,
        requires_correction: !correction_frames.is_empty()
            || confidence
                .map(|value| value < TRACKING_REVIEW_CONFIDENCE_THRESHOLD)
                .unwrap_or(true),
        correction_frames,
    }
}

fn track_sample_requires_correction(sample: &TrackSample) -> bool {
    sample.points.is_empty()
        || sample
            .points
            .iter()
            .flatten()
            .any(|coordinate| !coordinate.is_finite())
        || sample
            .confidence
            .filter(|confidence| confidence.is_finite())
            .map(|confidence| confidence < TRACKING_REVIEW_CONFIDENCE_THRESHOLD)
            .unwrap_or(true)
}

fn tracking_review_reframe(path: &ReframePath) -> TrackingReviewReframe {
    let confidence = average_reframe_confidence(path);
    let correction_times_s = path
        .keyframes
        .iter()
        .filter(|keyframe| {
            keyframe
                .confidence
                .filter(|confidence| confidence.is_finite())
                .map(|confidence| confidence < TRACKING_REVIEW_CONFIDENCE_THRESHOLD)
                .unwrap_or(true)
        })
        .map(|keyframe| keyframe.time_s)
        .collect::<Vec<_>>();
    TrackingReviewReframe {
        reframe_id: path.id.clone(),
        clip_id: path.clip_id.clone(),
        keyframe_count: path.keyframes.len(),
        confidence,
        requires_correction: !correction_times_s.is_empty()
            || confidence
                .map(|value| value < TRACKING_REVIEW_CONFIDENCE_THRESHOLD)
                .unwrap_or(true),
        correction_times_s,
    }
}

fn average_reframe_confidence(path: &ReframePath) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in path
        .keyframes
        .iter()
        .filter_map(|keyframe| keyframe.confidence)
    {
        if confidence.is_finite() {
            total += confidence.clamp(0.0, 1.0);
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(total / count as f64)
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

/// Lower tracker-bind composition nodes into executable clip parameter animations.
pub fn lower_tracker_parameter_bindings(
    package: &TrackingPackage,
    graph: &CompositionGraph,
    frame_rate: f64,
) -> Result<Vec<ParameterAnimation>, ProfessionalEngineError> {
    if !frame_rate.is_finite() || frame_rate <= 0.0 {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: graph.id.clone(),
            message: "frame_rate must be finite and positive".into(),
        });
    }
    graph
        .nodes
        .iter()
        .filter(|node| node.node_type == CompositionNodeType::TrackerBind)
        .map(|node| lower_tracker_binding_node(package, node, frame_rate))
        .collect()
}

fn lower_tracker_binding_node(
    package: &TrackingPackage,
    node: &CompositionNode,
    frame_rate: f64,
) -> Result<ParameterAnimation, ProfessionalEngineError> {
    let track_id = required_string_param(node, "track_id")?;
    let target_clip_id = required_string_param(node, "target_clip_id")?;
    let target_parameter = required_string_param(node, "target_parameter")?;
    if !is_runtime_clip_parameter(&target_parameter) {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: node.id.clone(),
            message: format!(
                "unsupported target_parameter {target_parameter:?}; supported clip parameters: {}",
                RUNTIME_CLIP_PARAMETERS.join(", ")
            ),
        });
    }
    let target_parameter = canonical_runtime_clip_parameter(&target_parameter)
        .map(str::to_string)
        .unwrap_or(target_parameter);
    let channel = string_param(node, "channel")
        .map(str::to_string)
        .unwrap_or_else(|| inferred_tracker_channel(&target_parameter).to_string());
    let track = package
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or_else(|| ProfessionalEngineError::MissingTrack(track_id.clone()))?;
    if track.coordinate_space != CoordinateSpace::Normalized {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: node.id.clone(),
            message:
                "only normalized tracker coordinate space can bind directly to overlay parameters"
                    .into(),
        });
    }
    let mut keyframes = Vec::new();
    for sample in track
        .samples
        .iter()
        .filter(|sample| !sample.points.is_empty())
    {
        let value = tracker_channel_value(sample, &channel).ok_or_else(|| {
            ProfessionalEngineError::InvalidTrackerBinding {
                binding_id: node.id.clone(),
                message: format!("unsupported tracker channel {channel:?}"),
            }
        })?;
        keyframes.push(Keyframe::linear(sample.frame as f64 / frame_rate, value));
    }
    if keyframes.is_empty() {
        return Err(ProfessionalEngineError::MissingTrack(track_id));
    }
    Ok(ParameterAnimation {
        id: format!("tracker-bind-{}", node.id),
        target: AnimationTarget::ClipParameter {
            clip_id: target_clip_id,
            parameter: target_parameter,
        },
        keyframes,
        pre_extrapolation: ExtrapolationMode::Hold,
        post_extrapolation: ExtrapolationMode::Hold,
        motion_path: None,
        metadata_only: false,
        rationale: Some(format!("bound to tracker {}", track.id)),
    })
}

fn required_string_param(
    node: &CompositionNode,
    key: &'static str,
) -> Result<String, ProfessionalEngineError> {
    let Some(value) = string_param(node, key).filter(|value| !value.trim().is_empty()) else {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: node.id.clone(),
            message: format!("missing required string parameter {key}"),
        });
    };
    Ok(value.to_string())
}

fn inferred_tracker_channel(target_parameter: &str) -> &'static str {
    if target_parameter.ends_with(".y") {
        "y"
    } else {
        "x"
    }
}

fn tracker_channel_value(sample: &TrackSample, channel: &str) -> Option<f64> {
    let points = sample.points.as_slice();
    match channel {
        "x" | "center_x" => {
            Some(points.iter().map(|point| point[0]).sum::<f64>() / points.len() as f64)
        }
        "y" | "center_y" => {
            Some(points.iter().map(|point| point[1]).sum::<f64>() / points.len() as f64)
        }
        "width" => {
            let (min_x, max_x) = min_max_coordinate(points, 0)?;
            Some(max_x - min_x)
        }
        "height" => {
            let (min_y, max_y) = min_max_coordinate(points, 1)?;
            Some(max_y - min_y)
        }
        _ => None,
    }
}

/// Lower a reviewed four-point surface track into an FFmpeg perspective filter.
pub fn lower_surface_track_corner_pin(
    package: &TrackingPackage,
    track_id: &str,
    target_clip_id: &str,
    width: u32,
    height: u32,
) -> Result<CornerPinLowering, ProfessionalEngineError> {
    let track = package
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or_else(|| ProfessionalEngineError::MissingTrack(track_id.to_string()))?;
    if track.kind != TrackKind::Surface {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: track_id.to_string(),
            message: "corner-pin lowering requires a surface track".into(),
        });
    }
    if track.coordinate_space != CoordinateSpace::Normalized {
        return Err(ProfessionalEngineError::InvalidTrackerBinding {
            binding_id: track_id.to_string(),
            message: "corner-pin lowering currently requires normalized surface points".into(),
        });
    }
    let samples = track
        .samples
        .iter()
        .filter(|sample| sample.points.len() >= 4)
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return Err(ProfessionalEngineError::MissingTrack(track_id.to_string()));
    }
    let x0 = surface_corner_expr(&samples, 0, 0, f64::from(width));
    let y0 = surface_corner_expr(&samples, 0, 1, f64::from(height));
    let x1 = surface_corner_expr(&samples, 1, 0, f64::from(width));
    let y1 = surface_corner_expr(&samples, 1, 1, f64::from(height));
    let x2 = surface_corner_expr(&samples, 3, 0, f64::from(width));
    let y2 = surface_corner_expr(&samples, 3, 1, f64::from(height));
    let x3 = surface_corner_expr(&samples, 2, 0, f64::from(width));
    let y3 = surface_corner_expr(&samples, 2, 1, f64::from(height));
    Ok(CornerPinLowering {
        track_id: track_id.into(),
        target_clip_id: target_clip_id.into(),
        filter: format!(
            "perspective=x0='{x0}':y0='{y0}':x1='{x1}':y1='{y1}':x2='{x2}':y2='{y2}':x3='{x3}':y3='{y3}':interpolation=linear:eval=frame"
        ),
    })
}

/// Lower graph-native corner-pin nodes through reviewed surface tracks.
pub fn lower_surface_track_corner_pin_bindings(
    package: &TrackingPackage,
    graph: &CompositionGraph,
    width: u32,
    height: u32,
) -> Result<Vec<CornerPinLowering>, ProfessionalEngineError> {
    graph
        .nodes
        .iter()
        .filter(|node| node.node_type == CompositionNodeType::CornerPin)
        .map(|node| {
            let track_id = required_string_param(node, "track_id")?;
            let target_clip_id = required_string_param(node, "target_clip_id")?;
            lower_surface_track_corner_pin(package, &track_id, &target_clip_id, width, height)
        })
        .collect()
}

fn surface_corner_expr(
    samples: &[&TrackSample],
    point_index: usize,
    coordinate_index: usize,
    scale: f64,
) -> String {
    let mut expr =
        fmt_filter_num(samples[samples.len() - 1].points[point_index][coordinate_index] * scale);
    for sample in samples.iter().rev().skip(1) {
        let value = fmt_filter_num(sample.points[point_index][coordinate_index] * scale);
        expr = format!("if(lte(n\\,{})\\,{value}\\,{expr})", sample.frame);
    }
    expr
}

fn fmt_filter_num(value: f64) -> String {
    let mut s = format!("{value:.6}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

fn min_max_coordinate(points: &[[f64; 2]], coordinate: usize) -> Option<(f64, f64)> {
    let first = points.first()?;
    let mut min = first[coordinate];
    let mut max = first[coordinate];
    for point in points.iter().skip(1) {
        min = min.min(point[coordinate]);
        max = max.max(point[coordinate]);
    }
    Some((min, max))
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
    /// Effect id, e.g. `montage.color_correction`.
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
    NonNegative,
    Normalized,
    Positive,
}

/// Built-in effect animation support matrix.
pub fn effect_parameter_capability_matrix() -> Vec<EffectParameterCapability> {
    vec![
        EffectParameterCapability {
            effect: "montage.color_correction",
            parameter: "brightness",
            unit: "offset",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::AnyFinite,
        },
        EffectParameterCapability {
            effect: "montage.color_correction",
            parameter: "contrast",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "montage.color_correction",
            parameter: "saturation",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "montage.video_overlay",
            parameter: "opacity",
            unit: "normalized",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Normalized,
        },
        EffectParameterCapability {
            effect: "montage.video_overlay",
            parameter: "scale",
            unit: "multiplier",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "montage.video_overlay",
            parameter: "blur",
            unit: "px",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::NonNegative,
        },
        EffectParameterCapability {
            effect: "montage.blur",
            parameter: "radius_px",
            unit: "px",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::NonNegative,
        },
        EffectParameterCapability {
            effect: "montage.shake",
            parameter: "intensity_px",
            unit: "px",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::NonNegative,
        },
        EffectParameterCapability {
            effect: "montage.shake",
            parameter: "frequency_hz",
            unit: "hz",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Positive,
        },
        EffectParameterCapability {
            effect: "montage.warp",
            parameter: "k1",
            unit: "coefficient",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::AnyFinite,
        },
        EffectParameterCapability {
            effect: "montage.warp",
            parameter: "k2",
            unit: "coefficient",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::AnyFinite,
        },
        EffectParameterCapability {
            effect: "montage.warp",
            parameter: "center_x",
            unit: "normalized",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Normalized,
        },
        EffectParameterCapability {
            effect: "montage.warp",
            parameter: "center_y",
            unit: "normalized",
            previewable: true,
            renderable: true,
            validation: EffectParameterValidation::Normalized,
        },
        EffectParameterCapability {
            effect: "montage.volume",
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
        EffectParameterValidation::NonNegative => value >= 0.0,
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
                message: composition_node_limitation_message(node.node_type),
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
        // TODO(overnight-wave1-vfx-graph): lower Merge to a real multi-input
        // overlay/blendmode chain instead of a placeholder.
        CompositionNodeType::Merge => "overlay=x=0:y=0:format=auto".to_string(),
        CompositionNodeType::Mask => lower_mask_node(node),
        // TODO(overnight-wave1-vfx-graph): lower Matte to the proper matte
        // sidecar / luma-key chain instead of the current placeholder.
        CompositionNodeType::Matte => "format=rgba,alphamerge".to_string(),
        CompositionNodeType::Text => {
            let text = string_param(node, "text").unwrap_or("");
            format!("drawtext=text='{text}'")
        }
        CompositionNodeType::Blur => lower_blur_node(node),
        // TODO(overnight-wave1-vfx-graph): lower Color to the real color-pipeline
        // filter chain (lift/gamma/gain + LUT) instead of an `eq=` stub.
        CompositionNodeType::Color => "eq=brightness=0:contrast=1:saturation=1".to_string(),
        // TODO(overnight-wave1-vfx-graph): lower TrackerBind to a sendcmd /
        // tracked-overlay graph instead of the current metadata placeholder.
        CompositionNodeType::TrackerBind => {
            let track_id = string_param(node, "track_id").unwrap_or("unbound");
            format!("metadata={track_id}")
        }
        CompositionNodeType::CornerPin => {
            let track_id = string_param(node, "track_id").unwrap_or("unbound");
            let target_clip_id = string_param(node, "target_clip_id").unwrap_or("unbound");
            format!("corner_pin=track:{track_id}:target:{target_clip_id}")
        }
        CompositionNodeType::Output => "map=output".to_string(),
    };
    Some(RenderLoweringStep {
        node_id: node.id.clone(),
        backend: "ffmpeg".into(),
        expression,
    })
}

/// Lower a Blur composition node to a real FFmpeg filter expression.
///
/// Supported `method` params:
/// * `"box"` (default) — emits `boxblur=luma_radius=<r>:luma_power=<power>`
/// * `"gaussian"` — emits `gblur=sigma=<sigma>:steps=<steps>` where sigma
///   falls back to `radius` if no explicit `sigma` param is provided.
///
/// `radius` is clamped to `>= 0` so a negative authoring value cannot leak
/// into the FFmpeg graph (FFmpeg rejects negative radii at runtime).
fn lower_blur_node(node: &CompositionNode) -> String {
    let radius = number_param(node, "radius").unwrap_or(2.0).max(0.0);
    let method = string_param(node, "method").unwrap_or("box").to_lowercase();
    match method.as_str() {
        "gaussian" | "gauss" | "gblur" => {
            let sigma = number_param(node, "sigma").unwrap_or(radius).max(0.0);
            let steps = number_param(node, "steps")
                .map(|value| value.clamp(1.0, 6.0).round() as u32)
                .unwrap_or(1);
            format!("gblur=sigma={sigma}:steps={steps}")
        }
        _ => {
            // boxblur power defaults to 1 (single pass). Higher powers stack the
            // box kernel and approximate gaussian shape for cheap.
            let power = number_param(node, "power")
                .map(|value| value.clamp(1.0, 4.0).round() as u32)
                .unwrap_or(1);
            format!("boxblur=luma_radius={radius}:luma_power={power}")
        }
    }
}

/// Lower a Mask composition node to a real FFmpeg filter chain.
///
/// Supported `kind` params:
/// * `"rect"` (default) — synthesizes a rectangular alpha mask via `geq`
///   and merges it into the input alpha plane with `alphamerge`.
/// * `"ellipse"` — synthesizes an elliptical alpha mask via `geq` using
///   normalized coordinates around the rect center.
/// * `"alpha_in"` — uses the upstream alpha plane as-is via `alphamerge`
///   (no synthesized mask).
/// * `"alpha_out"` — inverts the upstream alpha plane before merging.
///
/// Common params:
/// * `x`, `y`, `width`, `height` — rect/ellipse bounds as normalized
///   `[0.0, 1.0]` fractions of the frame.
/// * `invert` (bool) — inverts the final alpha plane via `negate=alpha=1`.
/// * `feather` (number, pixels) — softens the synthesized mask edges with a
///   `boxblur` pass before merging.
fn lower_mask_node(node: &CompositionNode) -> String {
    let kind = string_param(node, "kind").unwrap_or("rect").to_lowercase();
    let invert = bool_param(node, "invert").unwrap_or(false);
    let feather = number_param(node, "feather").unwrap_or(0.0).max(0.0);
    let x = number_param(node, "x").unwrap_or(0.0).clamp(0.0, 1.0);
    let y = number_param(node, "y").unwrap_or(0.0).clamp(0.0, 1.0);
    let width = number_param(node, "width").unwrap_or(1.0).clamp(0.0, 1.0);
    let height = number_param(node, "height").unwrap_or(1.0).clamp(0.0, 1.0);

    let alpha_expr = match kind.as_str() {
        "ellipse" | "oval" | "circle" => {
            // Ellipse: alpha = 255 if normalized distance from rect center <= 1.
            // Uses `hypot` so the expression reads as an ellipse rather than a
            // square. Width/height fractions become the ellipse radii.
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let rx = (width / 2.0).max(f64::EPSILON);
            let ry = (height / 2.0).max(f64::EPSILON);
            Some(format!(
                "geq=lum='if(lte(hypot((X/W-{cx})/{rx}\\,(Y/H-{cy})/{ry})\\,1)\\,255\\,0)':a='if(lte(hypot((X/W-{cx})/{rx}\\,(Y/H-{cy})/{ry})\\,1)\\,255\\,0)'"
            ))
        }
        "rect" | "rectangle" | "box" => {
            // Rectangular alpha: 255 inside `[x..x+w] x [y..y+h]`, else 0.
            let x_end = (x + width).min(1.0);
            let y_end = (y + height).min(1.0);
            Some(format!(
                "geq=lum='if(between(X/W\\,{x}\\,{x_end})*between(Y/H\\,{y}\\,{y_end})\\,255\\,0)':a='if(between(X/W\\,{x}\\,{x_end})*between(Y/H\\,{y}\\,{y_end})\\,255\\,0)'"
            ))
        }
        // alpha_in / alpha_out (and unknown kinds) reuse upstream alpha rather
        // than synthesizing a geq mask.
        _ => None,
    };

    let mut chain: Vec<String> = Vec::new();
    if let Some(expr) = alpha_expr {
        chain.push(expr);
        if feather > 0.0 {
            chain.push(format!("boxblur=luma_radius={feather}:luma_power=1"));
        }
    }
    // alpha_out always inverts upstream alpha. The `invert` flag XORs on top.
    let net_invert = invert ^ matches!(kind.as_str(), "alpha_out");
    if net_invert {
        // `negate=alpha=1` inverts only the alpha plane, leaving RGB untouched.
        chain.push("negate=alpha=1".to_string());
    }
    // alphamerge consumes the source's alpha as the mask. For synthesized
    // rect/ellipse masks the preceding geq writes that alpha plane; for
    // alpha_in/alpha_out the upstream alpha is used directly.
    chain.push("alphamerge".to_string());
    chain.join(",")
}

fn bool_param(node: &CompositionNode, key: &str) -> Option<bool> {
    node.params.get(key).and_then(Value::as_bool)
}

fn composition_node_needs_future_runtime(node_type: CompositionNodeType) -> bool {
    matches!(
        node_type,
        CompositionNodeType::Unsupported
            | CompositionNodeType::Scene3d
            | CompositionNodeType::ParticleEmitter
    )
}

fn composition_node_limitation_message(node_type: CompositionNodeType) -> String {
    match node_type {
        CompositionNodeType::Scene3d => {
            "Scene3d nodes require a future 3D scene runtime; no executable FFmpeg lowering was emitted".into()
        }
        CompositionNodeType::ParticleEmitter => {
            "ParticleEmitter nodes require a future particle runtime; no executable FFmpeg lowering was emitted".into()
        }
        _ => format!("node {node_type:?} has no current render lowering"),
    }
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
    /// Slot definitions copied from the source template.
    pub slot_defs: Vec<TemplateSlot>,
    /// Safe-area rules copied from the template.
    pub safe_areas: Vec<montage_proto::professional::SafeAreaRule>,
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
    /// Media overlay plans lowered from image, logo, or video template slots.
    pub media_overlays: Vec<VideoOverlayPlan>,
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
        MotionGraphicsTemplate {
            id: "shake-emphasis".into(),
            name: "Shake Emphasis".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("intensity", TemplateSlotKind::Number, false),
                slot("safe_area", TemplateSlotKind::SafeAreaProfile, false),
            ],
            safe_areas: platform_safe_areas(),
            platform_variants: platform_variants(),
        },
        MotionGraphicsTemplate {
            id: "logo-reveal".into(),
            name: "Logo Reveal".into(),
            slots: vec![
                slot("target_clip", TemplateSlotKind::TargetClip, true),
                slot("logo_asset", TemplateSlotKind::Image, true),
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
    metadata: &mut MontageTimelineMetadata,
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
        .push(montage_proto::professional::LearningSignal {
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
    metadata: &MontageTimelineMetadata,
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
        slot_defs: template.slots.clone(),
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
        limitations: unsupported_motion_template_slot_limitations(filled),
        ..MotionTemplateRender::default()
    };
    let name = text_slot(filled, "name")
        .or_else(|| text_slot(filled, "text"))
        .or_else(|| text_slot(filled, "title"))
        .unwrap_or_else(|| filled.name.clone());
    let subtitle = text_slot(filled, "subtitle");
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
    if let Some(color) = color_slot(filled) {
        apply_title_color(&mut render.titles, color);
    }
    let parameter_animations = template_parameter_animations(filled, timing);
    render.media_overlays.extend(template_media_overlays(
        filled,
        timing,
        &parameter_animations,
    ));
    render.parameter_animations.extend(parameter_animations);
    render
}

fn unsupported_motion_template_slot_limitations(
    filled: &FilledMotionTemplate,
) -> Vec<RenderLimitation> {
    filled
        .slot_defs
        .iter()
        .filter(|slot| {
            filled.slots.contains_key(&slot.id) && !motion_template_slot_is_lowered(slot)
        })
        .map(|slot| RenderLimitation {
            node_id: format!("{}:{}", filled.template_id, slot.id),
            severity: FindingSeverity::Warning,
            message: format!(
                "motion template {} slot {} ({:?}) is filled but has no current render lowering",
                filled.template_id, slot.id, slot.kind
            ),
        })
        .collect()
}

fn motion_template_slot_is_lowered(slot: &TemplateSlot) -> bool {
    match slot.kind {
        TemplateSlotKind::Text => {
            matches!(slot.id.as_str(), "name" | "text" | "title" | "subtitle")
        }
        TemplateSlotKind::Number => matches!(slot.id.as_str(), "scale" | "intensity"),
        TemplateSlotKind::Color => matches!(slot.id.as_str(), "color" | "accent_color"),
        TemplateSlotKind::Image => image_template_slot_is_lowered(&slot.id),
        TemplateSlotKind::Video => matches!(slot.id.as_str(), "video_asset"),
        TemplateSlotKind::TargetClip | TemplateSlotKind::SafeAreaProfile => true,
    }
}

fn image_template_slot_is_lowered(slot_id: &str) -> bool {
    matches!(
        slot_id,
        "image_asset" | "logo" | "logo_asset" | "brand_logo" | "brand_logo_path"
    )
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
        "shake-emphasis" => {
            let intensity = number_slot(filled, "intensity")
                .unwrap_or(0.035)
                .abs()
                .clamp(0.005, 0.20);
            let rotation_deg = (intensity * 60.0).clamp(1.0, 8.0);
            vec![
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.x",
                    shake_keyframes(duration_s, intensity),
                ),
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.y",
                    shake_y_keyframes(duration_s, intensity * 0.65),
                ),
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.rotation_deg",
                    vec![
                        keyframe(0.0, 0.0),
                        keyframe(duration_s * 0.20, -rotation_deg),
                        keyframe(duration_s * 0.40, rotation_deg),
                        keyframe(duration_s * 0.65, -rotation_deg * 0.5),
                        keyframe(duration_s, 0.0),
                    ],
                ),
            ]
        }
        "logo-reveal" => {
            let scale = number_slot(filled, "scale").unwrap_or(1.0).max(0.05);
            vec![
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.opacity",
                    fade_keyframes(duration_s),
                ),
                clip_animation(
                    &filled.template_id,
                    &target_clip,
                    "overlay.scale",
                    vec![
                        keyframe(0.0, 0.85 * scale),
                        keyframe((duration_s * 0.25).min(0.45), scale),
                        keyframe(duration_s, scale),
                    ],
                ),
            ]
        }
        _ => Vec::new(),
    }
}

fn template_media_overlays(
    filled: &FilledMotionTemplate,
    timing: MotionTemplateTiming,
    parameter_animations: &[ParameterAnimation],
) -> Vec<VideoOverlayPlan> {
    let target_clip = text_slot(filled, "target_clip");
    let duration_s = (timing.end_s - timing.start_s).max(0.1);
    let animations = target_clip
        .as_deref()
        .map(|target_clip| {
            parameter_animations
                .iter()
                .filter_map(|animation| {
                    render_animation_for_template_overlay(animation, target_clip)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    media_asset_slots(filled)
        .into_iter()
        .map(|(slot_id, asset_path)| VideoOverlayPlan {
            segment: TimelineSegment {
                clip_name: format!("{}:{slot_id}", filled.template_id),
                asset_path: PathBuf::from(asset_path),
                start_s: 0.0,
                duration_s,
                ..TimelineSegment::default()
            },
            track_start_s: timing.start_s,
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".into(),
                scale: template_media_overlay_scale(filled),
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: animations.clone(),
        })
        .collect()
}

fn media_asset_slots(filled: &FilledMotionTemplate) -> Vec<(String, String)> {
    filled
        .slot_defs
        .iter()
        .filter(|slot| match slot.kind {
            TemplateSlotKind::Image => image_template_slot_is_lowered(&slot.id),
            TemplateSlotKind::Video => slot.id == "video_asset",
            _ => false,
        })
        .filter_map(|slot| text_slot(filled, &slot.id).map(|value| (slot.id.clone(), value)))
        .collect()
}

fn render_animation_for_template_overlay(
    animation: &ParameterAnimation,
    target_clip: &str,
) -> Option<RenderParameterAnimation> {
    let AnimationTarget::ClipParameter { clip_id, parameter } = &animation.target else {
        return None;
    };
    if clip_id != target_clip || !parameter.starts_with("overlay.") {
        return None;
    }
    let parameter = canonical_runtime_clip_parameter(parameter)
        .map(str::to_string)
        .unwrap_or_else(|| parameter.clone());
    Some(RenderParameterAnimation {
        parameter,
        keyframes: animation.keyframes.clone(),
        pre_extrapolation: animation.pre_extrapolation,
        post_extrapolation: animation.post_extrapolation,
        motion_path: animation.motion_path.clone(),
    })
}

fn template_media_overlay_scale(filled: &FilledMotionTemplate) -> f64 {
    match filled.template_id.as_str() {
        "product-insert-emphasis" => 0.30,
        "pip-emphasis" => 0.28,
        _ => 0.25,
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
        pre_extrapolation: ExtrapolationMode::Hold,
        post_extrapolation: ExtrapolationMode::Hold,
        motion_path: None,
        metadata_only: false,
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

fn shake_keyframes(duration_s: f64, intensity: f64) -> Vec<Keyframe> {
    vec![
        keyframe(0.0, 0.0),
        keyframe(duration_s * 0.16, -intensity),
        keyframe(duration_s * 0.32, intensity),
        keyframe(duration_s * 0.52, -intensity * 0.5),
        keyframe(duration_s * 0.72, intensity * 0.5),
        keyframe(duration_s, 0.0),
    ]
}

fn shake_y_keyframes(duration_s: f64, intensity: f64) -> Vec<Keyframe> {
    vec![
        keyframe(0.0, 0.0),
        keyframe(duration_s * 0.16, intensity),
        keyframe(duration_s * 0.32, -intensity),
        keyframe(duration_s * 0.52, intensity * 0.5),
        keyframe(duration_s * 0.72, -intensity * 0.5),
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
        tangent_mode: Default::default(),
        spring: None,
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
        phases: None,
        reveal: TextReveal::None,
        role: "motion_template".into(),
        safe_area: Some("title_safe".into()),
        rich_segments: Vec::new(),
        word_timings: Vec::new(),
        animations: Vec::new(),
        font_path: None,
        font_family: None,
        caption_style: None,
        layout_box: None,
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

fn color_slot(filled: &FilledMotionTemplate) -> Option<String> {
    text_slot(filled, "color")
        .or_else(|| text_slot(filled, "accent_color"))
        .filter(|value| color_value_is_safe(value))
}

fn color_value_is_safe(value: &str) -> bool {
    let value = value.trim();
    if value.len() > 32 {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '#')
}

fn apply_title_color(titles: &mut [TitlePlan], color: String) {
    for title in titles {
        title.color.clone_from(&color);
    }
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
        let codec = export_video_codec_for_policy(
            &video.codec,
            preset.output.hardware_acceleration,
            &preset.id,
        )?;
        args.extend(["-c:v".into(), codec]);
        if let Some(bitrate) = video.bitrate_kbps.or(preset.profile.video_bitrate_kbps) {
            args.extend(["-b:v".into(), format!("{bitrate}k")]);
        }
        if let Some(frame_rate) = video.frame_rate {
            args.extend(["-r".into(), format!("{frame_rate}")]);
        }
        args.extend(codec_specific_extras(&video.codec));
    }
    if let Some(audio) = &preset.audio {
        args.extend(["-c:a".into(), audio.codec.clone()]);
        if let Some(bitrate) = audio.bitrate_kbps {
            args.extend(["-b:a".into(), format!("{bitrate}k")]);
        }
        args.extend(["-ar".into(), audio.sample_rate_hz.to_string()]);
        args.extend(["-ac".into(), audio.channels.to_string()]);
    }
    if preset.output.container == "mp4" || preset.output.extension == "mp4" {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    args.extend(["-f".into(), preset.output.container.clone()]);
    spec.args.splice(insertion..insertion, args);
    Ok(spec)
}

fn export_video_codec_for_policy(
    codec: &str,
    policy: HardwareAccelerationPolicy,
    preset_id: &str,
) -> Result<String, ProfessionalEngineError> {
    match policy {
        HardwareAccelerationPolicy::Off => Ok(codec.to_string()),
        HardwareAccelerationPolicy::Auto => Ok(native_hardware_codec(codec)
            .map(str::to_string)
            .unwrap_or_else(|| codec.to_string())),
        HardwareAccelerationPolicy::Require => native_hardware_codec(codec)
            .map(str::to_string)
            .ok_or_else(|| ProfessionalEngineError::InvalidExportPreset {
                preset_id: preset_id.to_string(),
                message: format!(
                    "hardware acceleration was required but no native hardware codec mapping exists for {codec}"
                ),
            }),
    }
}

fn native_hardware_codec(codec: &str) -> Option<&'static str> {
    match codec {
        "h264_videotoolbox" => Some("h264_videotoolbox"),
        "hevc_videotoolbox" => Some("hevc_videotoolbox"),
        "h264_nvenc" => Some("h264_nvenc"),
        "hevc_nvenc" => Some("hevc_nvenc"),
        "h264_qsv" => Some("h264_qsv"),
        "hevc_qsv" => Some("hevc_qsv"),
        "h264_amf" => Some("h264_amf"),
        "hevc_amf" => Some("hevc_amf"),
        "h264_vaapi" => Some("h264_vaapi"),
        "hevc_vaapi" => Some("hevc_vaapi"),
        _ => native_software_codec_mapping(codec),
    }
}

#[cfg(target_os = "macos")]
fn native_software_codec_mapping(codec: &str) -> Option<&'static str> {
    match codec {
        "libx264" => Some("h264_videotoolbox"),
        "libx265" | "libx265_hevc" => Some("hevc_videotoolbox"),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn native_software_codec_mapping(_codec: &str) -> Option<&'static str> {
    None
}

/// Codec-specific FFmpeg flags appended after `-c:v <codec>` / `-b:v`.
///
/// The selected values follow the MVP slice for the export-codec catalog:
/// libx265 gets a sensible CRF/preset envelope, prores_ks gets the
/// HQ profile and 10-bit 4:2:2 pixel format. Codecs without listed
/// extras (libx264, hardware variants, png, etc.) get an empty list so
/// the existing argv shape is preserved.
fn codec_specific_extras(codec: &str) -> Vec<String> {
    match codec {
        "libx265" => vec![
            "-preset".into(),
            "medium".into(),
            "-crf".into(),
            "23".into(),
        ],
        "prores_ks" => vec![
            "-profile:v".into(),
            "3".into(),
            "-pix_fmt".into(),
            "yuv422p10le".into(),
        ],
        // TODO(overnight-wave1-export-codecs): DNxHR (`dnxhd`) and AV1
        // (`libsvtav1` / `libaom-av1`) extras land in a follow-up slice.
        _ => Vec::new(),
    }
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
                montage_proto::professional::PreflightCheckKind::AspectRatio => {
                    "fix-delivery-aspect-ratio"
                }
                montage_proto::professional::PreflightCheckKind::Duration => {
                    "fix-delivery-duration"
                }
                montage_proto::professional::PreflightCheckKind::Bitrate => "fix-delivery-bitrate",
                montage_proto::professional::PreflightCheckKind::Captions => {
                    "fix-delivery-captions"
                }
                montage_proto::professional::PreflightCheckKind::Loudness => {
                    "fix-delivery-loudness"
                }
                montage_proto::professional::PreflightCheckKind::SafeAreas => {
                    "fix-delivery-safe-areas"
                }
                montage_proto::professional::PreflightCheckKind::Metadata => {
                    "fix-delivery-metadata"
                }
                montage_proto::professional::PreflightCheckKind::Thumbnail => {
                    "fix-delivery-thumbnail"
                }
            }
            .into(),
        );
    }
}
