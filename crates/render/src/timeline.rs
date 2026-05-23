//! Timeline-render planning: walk a project's OTIO and produce the
//! [`RenderJobSpec`] that ffmpeg will execute.
//!
//! Extracted from `awidat-core::tools::start_render` so the desktop's
//! Export button can call into the same logic without going through
//! the agent tool. Both call sites depend on this module; both
//! produce identical specs.
//!
//! The non-obvious bit is the **single re-encode at concat
//! boundaries**. Stream-copy concat at non-keyframe-aligned cut
//! points produces audible clicks (DTS-seam scratch); we accept
//! the encoder cost to avoid that.
//!
//! Audio tracks aren't enumerated separately — most awidat
//! projects keep video and audio paired in the same source file,
//! so the concat filter pulls each input's audio stream alongside
//! its video stream.
//!
//! Video transitions are planned explicitly from adjacent OTIO
//! `Transition.1` nodes; unsupported or invalid placements fail before
//! FFmpeg is invoked.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use awidat_proto::awidat_meta::{
    AwidatTimelineMetadata, BroadcastHost, BroadcastOverlayConfig, BroadcastOverlayStyle,
};
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::professional::{
    MaskOperation, ReframePath, TrackingPackage, canonical_runtime_clip_parameter,
};
use awidat_proto::project::{files, read_otio_timeline};
use awidat_proto::transitions::{self, TransitionComposition};
use chrono::Utc;
use serde::Deserialize;
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

use crate::animation::{
    MotionPathCoordinate, evaluate_keyframes_with_modes, is_phase_3a_parameter,
    keyframes_to_ffmpeg_expr_with_extrapolation, motion_path_to_ffmpeg_expr_with_keyframes,
};
use crate::job::{RenderJobSpec, RenderPlanLimitation};
use crate::output_safety::{OutputPathPolicy, validate_render_output_path};

const TIMELINE_RENDER_WIDTH: u32 = 1920;
const TIMELINE_RENDER_HEIGHT: u32 = 1080;
const OVERLAY_BLUR_MAX_RADIUS_PX: f64 = 24.0;

/// Errors building a timeline-render spec.
#[derive(Debug, Error)]
pub enum RenderTimelineError {
    /// `<project_root>/project.otio.json` doesn't exist.
    #[error("no project.otio.json found at {0} — this isn't an awidat project root")]
    NoOtio(PathBuf),
    /// OTIO file present but parse / validation failed.
    #[error("timeline parse failed: {message}")]
    OtioParse {
        /// Diagnostic from the underlying parser.
        message: String,
    },
    /// A clip referenced an asset path that isn't on disk.
    #[error("timeline references missing asset {missing} (clip '{clip_name}')")]
    MissingAsset {
        /// Clip name from the OTIO.
        clip_name: String,
        /// Absolute path that didn't resolve.
        missing: PathBuf,
    },
    /// A clip lacked a `source_range`.
    #[error("clip '{clip_name}' has no source_range — can't extract a renderable segment")]
    ClipMissingRange {
        /// Clip name from the OTIO.
        clip_name: String,
    },
    /// A centered transition needs source media outside the visible
    /// edit range, but that handle is not available.
    #[error(
        "transition {kind:?} around clip '{clip_name}' needs {needed_s:.3}s {side} handle, but only {available_s:.3}s is available"
    )]
    TransitionHandleUnavailable {
        /// Transition kind or Awidat id from OTIO.
        kind: String,
        /// Clip whose source range lacks the requested handle.
        clip_name: String,
        /// "incoming" for pre-roll, "outgoing" for post-roll.
        side: &'static str,
        /// Timeline seconds requested by the transition.
        needed_s: f64,
        /// Timeline seconds available in the source media.
        available_s: f64,
    },
    /// A clip referenced a LUT path that isn't on disk.
    #[error("clip '{clip_name}' references missing LUT {missing}")]
    MissingLut {
        /// Clip name from the OTIO.
        clip_name: String,
        /// Absolute LUT path that didn't resolve.
        missing: PathBuf,
    },
    /// The timeline parsed but has no clips on any video track.
    #[error("timeline has no clips to render")]
    EmptyTimeline,
    /// A transition kind/id cannot be exported by the phase-one renderer.
    #[error("unsupported timeline transition {kind:?}: {message}")]
    UnsupportedTransition {
        /// Transition kind or Awidat id from OTIO.
        kind: String,
        /// Detailed registry/lookup message.
        message: String,
    },
    /// A transition node is not placed between exactly two clips.
    #[error("invalid transition placement: {message}")]
    InvalidTransitionPlacement {
        /// Detailed placement failure.
        message: String,
    },
    /// Awidat transition metadata exists but is malformed or outside
    /// the supported primitive API.
    #[error("invalid transition metadata for {kind:?}: {message}")]
    InvalidTransitionMetadata {
        /// Transition kind or Awidat id from OTIO.
        kind: String,
        /// Detailed metadata failure.
        message: String,
    },
    /// Awidat clip effect metadata is malformed or outside the supported API.
    #[error("invalid clip effect {effect:?} on clip '{clip_name}': {message}")]
    InvalidClipEffectMetadata {
        /// Clip name from OTIO.
        clip_name: String,
        /// Effect id such as `awidat.time_remap`.
        effect: String,
        /// Detailed metadata failure.
        message: String,
    },
    /// Browser-backed broadcast overlay generation failed before the
    /// final ffmpeg render could start.
    #[error("broadcast overlay render failed: {0}")]
    BroadcastOverlayRender(String),
    /// Requested guide track marker/range was not found.
    #[error("guide section {guide_track_id}/{marker_id} was not found")]
    GuideSectionNotFound {
        /// Guide track id.
        guide_track_id: String,
        /// Marker id inside the guide track.
        marker_id: String,
    },
    /// Requested guide marker exists but cannot define a section.
    #[error("guide section {guide_track_id}/{marker_id} needs a positive duration")]
    GuideSectionMissingDuration {
        /// Guide track id.
        guide_track_id: String,
        /// Marker id inside the guide track.
        marker_id: String,
    },
}

#[derive(Debug, Clone)]
struct TimelineSectionRange {
    marker_id: String,
    start_s: f64,
    duration_s: f64,
}

/// Broadcast overlay config plus project root for resolving optional
/// project-relative assets.
#[derive(Debug, Clone)]
pub struct BroadcastOverlayPlan {
    /// Timeline-level overlay config read from OTIO metadata.
    pub config: BroadcastOverlayConfig,
    /// Project root used to resolve project-relative overlay assets.
    pub project_root: PathBuf,
}

/// Timeline-level final loudness target from `metadata.awidat`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessTargetPlan {
    /// Integrated loudness target in LUFS.
    pub integrated_lufs: f64,
    /// True peak ceiling in dBTP.
    pub true_peak_db: Option<f64>,
}

/// Source-media HDR transfer function that needs SDR tone mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrTransfer {
    /// SMPTE ST 2084 / PQ transfer.
    Smpte2084,
    /// ARIB STD-B67 / HLG transfer.
    AribStdB67,
}

/// One source-media segment to feed into the timeline-render concat.
/// Public so callers can sum durations or otherwise inspect the plan
/// before kicking off ffmpeg.
#[derive(Debug, Clone, Default)]
pub struct TimelineSegment {
    /// Clip display name from OTIO, used for diagnostics.
    pub clip_name: String,
    /// Absolute path to the source media.
    pub asset_path: PathBuf,
    /// Seconds into the source media where the cut starts.
    pub start_s: f64,
    /// Seconds of duration to take from the source.
    pub duration_s: f64,
    /// Extra timeline seconds included before the visible edit-in so
    /// a centered incoming transition has media to fade up from.
    pub pre_handle_s: f64,
    /// Extra timeline seconds included after the visible edit-out so
    /// a centered outgoing transition has media to fade down through.
    pub post_handle_s: f64,
    /// Optional source-media lower bound from OTIO available_range.
    pub source_available_start_s: Option<f64>,
    /// Optional source-media upper bound from OTIO available_range.
    pub source_available_end_s: Option<f64>,
    /// Optional FFmpeg input index for a renderable mask source.
    pub mask_input_index: Option<usize>,
    /// Linear gain multiplier for this segment's audio. `None` means
    /// no `awidat.volume` effect is on the underlying clip — the
    /// FilterPlanner skips emitting a `volume=` filter and the audio
    /// passes through unchanged. `Some(1.0)` is unity (functionally
    /// identical to `None` but the planner still emits the filter
    /// for explicitness).
    pub volume: Option<f64>,
    /// Optional keyframed clip-local volume automation.
    pub volume_automation: Option<AudioAutomationPlan>,
    /// Optional constant-speed retime controls read from `awidat.speed`.
    pub speed: Option<SpeedPlan>,
    /// Optional source-local → timeline-local retime curve.
    pub time_remap: Option<TimeRemapPlan>,
    /// Optional inserted freeze-frame hold.
    pub freeze: Option<FreezePlan>,
    /// Optional clip-level color correction controls, read from the
    /// `awidat.color_correction` effect.
    pub color_correction: Option<ColorCorrectionPlan>,
    /// Optional clip-level blur controls, read from the
    /// `awidat.blur` effect.
    pub blur: Option<BlurPlan>,
    /// Optional clip-level chroma key controls.
    pub chroma_key: Option<ChromaKeyPlan>,
    /// Optional clip-level luma key controls.
    pub luma_key: Option<LumaKeyPlan>,
    /// Optional clip-level static region blur controls.
    pub region_blur: Option<RegionBlurPlan>,
    /// Optional clip-level blur radius animation, read from a runtime
    /// `awidat.blur.radius_px` parameter animation.
    pub blur_radius_animation: Option<RenderParameterAnimation>,
    /// Optional HDR transfer metadata, read from `awidat.source_color`.
    pub hdr_transfer: Option<HdrTransfer>,
    /// Optional clip-level reframe / punch-in controls, read from the
    /// `awidat.reframe` effect.
    pub reframe: Option<ReframePlan>,
    /// Optional subject-aware reframe path, read from the tracking package.
    pub reframe_path: Option<ReframePathPlan>,
    /// Optional clip-level lens warp controls, read from the
    /// `awidat.warp` effect.
    pub warp: Option<WarpPlan>,
    /// Optional animated lens warp controls, read from
    /// `awidat.warp.*` parameter animations.
    pub warp_animation: Option<WarpAnimationPlan>,
    /// Optional procedural handheld shake controls, read from the
    /// `awidat.shake` effect.
    pub shake: Option<ShakePlan>,
    /// Optional animated shake intensity, read from
    /// `awidat.shake.intensity_px`.
    pub shake_intensity_animation: Option<RenderParameterAnimation>,
    /// Optional animated shake frequency, read from
    /// `awidat.shake.frequency_hz`.
    pub shake_frequency_animation: Option<RenderParameterAnimation>,
    /// Optional absolute LUT path, read from the `awidat.lut` effect.
    pub lut_path: Option<PathBuf>,
    /// Optional FFmpeg `lut3d` interpolation mode.
    pub lut_interpolation: Option<String>,
    /// Optional LUT blend strength in `0.0..=1.0`. `None` or `1.0`
    /// emits a single `lut3d` filter (full strength). Below `1.0`
    /// emits a `split → lut3d → blend=all_opacity=<s>` block so the
    /// LUT mixes with the un-graded source.
    pub lut_strength: Option<f64>,
    /// Optional atomic color pipeline read from `awidat.color_pipeline`.
    /// When present, supersedes the legacy `lut_path` slots and
    /// drives a multi-stage LUT chain (input transform → shaper →
    /// look → output transform) plus output color-space tagging.
    pub color_pipeline: Option<ColorPipelinePlan>,
    /// Optional FFmpeg-native audio FX chain.
    pub audio_fx: Option<AudioFxPlan>,
    /// Optional full-frame overlay blend mode.
    pub overlay_blend_mode: Option<String>,
    /// Incoming audio lead for J-cut export, in seconds.
    pub audio_lead_s: Option<f64>,
    /// Outgoing audio trail for L-cut export, in seconds.
    pub audio_trail_s: Option<f64>,
}

/// Clip-level curve-based time remap extracted from `awidat.time_remap`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeRemapPlan {
    /// Strictly increasing source-local to timeline-local mapping points.
    pub points: Vec<TimeRemapPoint>,
}

impl TimeRemapPlan {
    fn timeline_time_at_source(&self, source_time_s: f64) -> f64 {
        let first = self.points[0];
        if source_time_s <= first.source_time_s {
            return first.timeline_time_s;
        }
        for pair in self.points.windows(2) {
            let start = pair[0];
            let end = pair[1];
            if source_time_s <= end.source_time_s {
                return interpolate_time_remap_segment(start, end, source_time_s);
            }
        }
        let len = self.points.len();
        interpolate_time_remap_segment(self.points[len - 2], self.points[len - 1], source_time_s)
    }
}

/// One point on a clip-local time-remap curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRemapPoint {
    /// Source-local seconds in the decoded clip.
    pub source_time_s: f64,
    /// Output timeline-local seconds for that source frame.
    pub timeline_time_s: f64,
}

fn interpolate_time_remap_segment(
    start: TimeRemapPoint,
    end: TimeRemapPoint,
    source_time_s: f64,
) -> f64 {
    let source_span = end.source_time_s - start.source_time_s;
    if source_span <= 0.0 {
        return start.timeline_time_s;
    }
    let timeline_span = end.timeline_time_s - start.timeline_time_s;
    start.timeline_time_s + (source_time_s - start.source_time_s) * timeline_span / source_span
}

/// Render-time plan for an `awidat.color_pipeline` effect.
///
/// Slots that are `None` are skipped. The filter chain runs each
/// non-empty slot in order:
/// `input_transform_lut → shaper_lut → look_lut → output_transform_lut`.
/// Color-space identifiers (`clip_input_space`, `working_space`,
/// `lut_input_space`, `output_space`) are stable strings from
/// [`awidat_effects::KNOWN_COLOR_SPACES`]. `output_space` flows out
/// to the encoder as `-color_primaries / -color_trc / -colorspace /
/// -color_range`; the other space fields are carried for the agent's
/// reasoning today and reserved for automatic conversion in a
/// follow-up.
#[derive(Debug, Clone, Default)]
pub struct ColorPipelinePlan {
    /// Stable id of the color space the decoded pixels live in.
    pub clip_input_space: String,
    /// Optional intermediate space the chain operates in.
    pub working_space: Option<String>,
    /// Optional declared input space of the `look_lut`.
    pub lut_input_space: Option<String>,
    /// Optional delivery space (drives encoder color tags).
    pub output_space: Option<String>,
    /// Optional camera-to-working IDT (3D LUT, project-relative).
    pub input_transform_lut: Option<PathBuf>,
    /// Optional 1D shaper applied before the look LUT.
    pub shaper_lut: Option<PathBuf>,
    /// Optional creative 3D LUT.
    pub look_lut: Option<PathBuf>,
    /// Optional FFmpeg `lut3d` interpolation for `look_lut`.
    pub look_interpolation: Option<String>,
    /// Optional `look_lut` blend strength in `0.0..=1.0`.
    pub look_strength: Option<f64>,
    /// Optional working-to-display ODT (3D LUT, project-relative).
    pub output_transform_lut: Option<PathBuf>,
    /// Optional regional-grading mask source (PNG with alpha,
    /// grayscale image, or video). Render supports the v1 masked
    /// look-LUT path and surfaces a limitation for combinations
    /// outside that supported scope.
    pub mask_source: Option<PathBuf>,
}

impl ColorPipelinePlan {
    /// `true` if at least one LUT slot is populated. A pipeline with
    /// only color-space metadata and no LUTs still affects encoder
    /// tagging but emits no filter chain.
    pub fn has_any_lut(&self) -> bool {
        self.input_transform_lut.is_some()
            || self.shaper_lut.is_some()
            || self.look_lut.is_some()
            || self.output_transform_lut.is_some()
    }
}

/// Render-time media overlay extracted from an upper video track.
#[derive(Debug, Clone)]
pub struct VideoOverlayPlan {
    /// Source-media segment for the overlay input.
    pub segment: TimelineSegment,
    /// Start time on the master timeline, in seconds.
    pub track_start_s: f64,
    /// Visual layout mode.
    pub mode: VideoOverlayMode,
    /// Static clockwise rotation in degrees, applied before compositing.
    pub rotation_deg: f64,
    /// Optional transform-derived temporal blur for animated overlay motion.
    pub motion_blur: Option<OverlayMotionBlurPlan>,
    /// Optional graph-derived perspective/corner-pin filter for the overlay.
    pub corner_pin_filter: Option<String>,
    /// Optional graph-derived matte alpha source for this overlay.
    pub matte_source: Option<PathBuf>,
    /// Optional FFmpeg input index assigned to `matte_source`.
    pub matte_input_index: Option<usize>,
    /// Optional graph-derived alpha mask bounds for this overlay.
    pub mask: Option<OverlayMaskPlan>,
    /// Supported Phase 3A parameter animations attached to this overlay.
    pub animations: Vec<RenderParameterAnimation>,
}

/// Render-time alpha mask lowered from a graph `mask` node.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayMaskPlan {
    /// Normalized alpha-gate keyframes derived from mask path bounds.
    pub keyframes: Vec<OverlayMaskKeyframe>,
    /// Mask combination operation from the tracking package sidecar.
    pub operation: MaskOperation,
}

/// One render-time alpha mask keyframe.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayMaskKeyframe {
    /// Clip-local time in seconds.
    pub time_s: f64,
    /// Left bound as a normalized fraction of overlay width.
    pub x0: f64,
    /// Top bound as a normalized fraction of overlay height.
    pub y0: f64,
    /// Right bound as a normalized fraction of overlay width.
    pub x1: f64,
    /// Bottom bound as a normalized fraction of overlay height.
    pub y1: f64,
    /// Alpha multiplier applied inside the mask bounds.
    pub opacity: f64,
}

/// Render settings for transform-derived overlay motion blur.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayMotionBlurPlan {
    /// Exposure duration sampled around the current frame, in seconds.
    pub shutter_s: f64,
}

/// Visual layout mode for upper-track media overlays.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoOverlayMode {
    /// Full-frame cutaway composited over the base program.
    FullFrame,
    /// Picture-in-picture overlay.
    PiP {
        /// Output corner string: top_left, top_right, bottom_left, bottom_right.
        corner: String,
        /// Fraction of output width.
        scale: f64,
        /// Fractional output margin.
        margin_pct: f64,
    },
}

/// Render-time annotation extracted from virtual `awidat.annotation` clips.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationPlan {
    /// Primitive to draw or apply.
    pub kind: AnnotationKind,
    /// Start time on the master timeline.
    pub start_s: f64,
    /// End time on the master timeline.
    pub end_s: f64,
    /// Normalized x coordinate.
    pub x: f64,
    /// Normalized y coordinate.
    pub y: f64,
    /// Normalized width.
    pub width: f64,
    /// Normalized height.
    pub height: f64,
    /// Stroke/accent color.
    pub color: String,
    /// Stroke width in pixels.
    pub stroke_width: u32,
    /// Optional review label.
    pub label: Option<String>,
}

/// Render-time annotation primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    /// Rectangle outline.
    Rectangle,
    /// Circle/ellipse approximation.
    Circle,
    /// Arrow callout.
    Arrow,
    /// Corner bracket callout.
    Bracket,
    /// Blur a rectangular region.
    Blur,
}

/// Render-time audio clip span extracted from an OTIO audio track.
#[derive(Debug, Clone, Default)]
pub struct AudioClipPlan {
    /// Absolute source media path.
    pub asset_path: PathBuf,
    /// Source start time in seconds.
    pub start_s: f64,
    /// Source duration in seconds.
    pub duration_s: f64,
    /// Optional gain multiplier for the clip.
    pub volume: Option<f64>,
    /// Optional keyframed clip-local volume automation.
    pub volume_automation: Option<AudioAutomationPlan>,
    /// Optional constant-speed retime controls.
    pub speed: Option<SpeedPlan>,
    /// Optional fade-in duration in seconds.
    pub fade_in_s: Option<f64>,
    /// Optional fade-out duration in seconds.
    pub fade_out_s: Option<f64>,
    /// Optional FFmpeg-native audio FX chain.
    pub audio_fx: Option<AudioFxPlan>,
}

/// Constant-speed retime controls read from `awidat.speed`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedPlan {
    /// Playback rate multiplier.
    pub factor: f64,
    /// Reverse playback before timeline-speed normalization.
    pub reverse: bool,
    /// Preserve pitch with `atempo` when true; retime sample rate when false.
    pub maintain_pitch: bool,
    /// Frame interpolation quality mode.
    pub frame_blending: FrameBlending,
}

impl SpeedPlan {
    /// Build a default forward, pitch-preserving speed plan.
    pub fn new(factor: f64) -> Self {
        Self {
            factor,
            reverse: false,
            maintain_pitch: true,
            frame_blending: FrameBlending::Nearest,
        }
    }
}

/// Frame interpolation mode for speed/time-remap lowering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FrameBlending {
    /// Current FFmpeg `setpts` behavior.
    #[default]
    Nearest,
    /// Frame-blended interpolation.
    Blend,
    /// Motion-compensated interpolation.
    Flow,
}

/// Clip-level freeze-frame hold extracted from `awidat.freeze`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreezePlan {
    /// Source time, in seconds, where the frame is held.
    pub freeze_at_source_s: f64,
    /// Hold duration inserted into the timeline.
    pub duration_s: f64,
}

/// Render-time item on an audio track.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum AudioTrackItemPlan {
    /// Media-backed audio clip.
    Clip(AudioClipPlan),
    /// Silent gap with a duration in seconds.
    Gap {
        /// Gap duration in seconds.
        duration_s: f64,
    },
}

/// Render-time audio track plan including mix metadata and items.
#[derive(Debug, Clone)]
pub struct AudioTrackPlan {
    /// Track display name.
    pub name: String,
    /// Semantic role such as dialogue, music, effects, or ambience.
    pub role: String,
    /// Track gain multiplier.
    pub volume: f64,
    /// Optional keyframed volume automation for the track.
    pub volume_automation: Option<AudioAutomationPlan>,
    /// Whether the track is muted.
    pub muted: bool,
    /// Whether the track is soloed.
    pub solo: bool,
    /// Optional ducking behavior for this track.
    pub ducking: Option<DuckingPlan>,
    /// Optional FFmpeg-native audio FX chain for the full mixed track.
    pub audio_fx: Option<AudioFxPlan>,
    /// Ordered audio items on the track.
    pub items: Vec<AudioTrackItemPlan>,
}

/// FFmpeg-ready audio automation expression.
#[derive(Debug, Clone)]
pub struct AudioAutomationPlan {
    /// Automated parameter name.
    pub parameter: String,
    /// FFmpeg expression for the parameter value.
    pub expression: String,
    /// Original keyframes in project units.
    pub keyframes: Vec<awidat_proto::professional::Keyframe>,
}

/// Audio ducking parameters for music/effects under dialogue.
#[derive(Debug, Clone)]
pub struct DuckingPlan {
    /// Whether ducking is active.
    pub enabled: bool,
    /// Gain reduction applied while ducking.
    pub amount_db: f64,
    /// Ducking attack in milliseconds.
    pub attack_ms: f64,
    /// Ducking release in milliseconds.
    pub release_ms: f64,
}

/// FFmpeg-native audio cleanup/EQ/dynamics settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AudioFxPlan {
    /// High-pass cutoff in Hz.
    pub high_pass_hz: Option<f64>,
    /// Low-pass cutoff in Hz.
    pub low_pass_hz: Option<f64>,
    /// Stereo pan position in [-1, 1], where -1 is left and 1 is right.
    pub pan: Option<f64>,
    /// Stereo balance in [-1, 1], where -1 attenuates right and 1 attenuates left.
    pub balance: Option<f64>,
    /// Enable FFmpeg adeclick click/pop repair.
    pub adeclick: Option<bool>,
    /// Enable FFmpeg adeclip clipped-sample repair.
    pub adeclip: Option<bool>,
    /// Project-relative or absolute arnndn/RNNoise model path.
    pub arnndn_model: Option<String>,
    /// Parametric EQ bands.
    #[serde(default)]
    pub eq_bands: Vec<EqBandPlan>,
    /// Compressor threshold in dB.
    pub compressor_threshold_db: Option<f64>,
    /// Compressor ratio.
    pub compressor_ratio: Option<f64>,
    /// Limiter ceiling in dBFS.
    pub limiter_limit_db: Option<f64>,
    /// Noise gate threshold in dBFS.
    pub noise_gate_threshold_db: Option<f64>,
    /// Hum notch frequency in Hz.
    pub hum_notch_hz: Option<f64>,
    /// Center frequency for the de-ess approximation.
    pub de_ess_hz: Option<f64>,
    /// Gain reduction for the de-ess approximation.
    pub de_ess_reduction_db: Option<f64>,
    /// Loudnorm integrated loudness target.
    pub loudnorm_i: Option<f64>,
    /// Loudnorm true-peak target.
    pub loudnorm_tp: Option<f64>,
}

/// One parametric EQ band.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EqBandPlan {
    /// Center frequency in Hz.
    pub freq_hz: f64,
    /// Gain in dB.
    pub gain_db: f64,
    /// Band width in Hz.
    pub width_hz: Option<f64>,
}

/// Clip-level color controls that render maps into FFmpeg filters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorCorrectionPlan {
    /// Exposure offset in stops.
    pub exposure_ev: Option<f64>,
    /// Contrast multiplier.
    pub contrast: Option<f64>,
    /// Saturation multiplier.
    pub saturation: Option<f64>,
    /// Normalized warm/cool control.
    pub temperature: Option<f64>,
    /// Normalized green/magenta control.
    pub tint: Option<f64>,
    /// Normalized shadow control.
    pub shadows: Option<f64>,
    /// Normalized highlight control.
    pub highlights: Option<f64>,
}

/// Clip-level Gaussian blur controls.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlurPlan {
    /// Blur radius in source pixels.
    pub radius_px: f64,
}

/// Clip-level chroma key controls.
#[derive(Debug, Clone, PartialEq)]
pub struct ChromaKeyPlan {
    /// FFmpeg color literal accepted by `chromakey`, usually `0xRRGGBB`.
    pub key_color: String,
    /// Color-distance threshold.
    pub similarity: f64,
    /// Edge blend softness.
    pub blend: f64,
}

/// Clip-level luma key controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LumaKeyPlan {
    /// Luma threshold in `0..=1`.
    pub threshold: f64,
    /// Edge softness in `0..=1`.
    pub softness: f64,
}

/// Static region blur controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionBlurPlan {
    /// Region shape.
    pub shape: RegionBlurShape,
    /// Normalized left edge.
    pub x: f64,
    /// Normalized top edge.
    pub y: f64,
    /// Normalized width.
    pub width: f64,
    /// Normalized height.
    pub height: f64,
    /// Blur radius in pixels.
    pub radius_px: f64,
}

/// Supported static region blur shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionBlurShape {
    /// Rectangular crop bounds.
    Rect,
    /// Ellipse inscribed inside the crop bounds.
    Ellipse,
}

/// Clip-level scale/crop controls for simple punch-ins and reframes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReframePlan {
    /// Scale multiplier. `1.0` is unchanged.
    pub zoom: f64,
    /// Horizontal crop bias in `[-1, 1]`: `-1` left, `0` center, `1` right.
    pub x: f64,
    /// Vertical crop bias in `[-1, 1]`: `-1` top, `0` center, `1` bottom.
    pub y: f64,
}

/// Subject-aware center crop authored from a tracking package.
#[derive(Debug, Clone, PartialEq)]
pub struct ReframePathPlan {
    /// Stable reframe path id.
    pub reframe_id: String,
    /// Ordered crop center/scale samples in clip-local time.
    pub keyframes: Vec<ReframePathKeyframePlan>,
}

/// One renderable crop sample from a subject-aware reframe path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReframePathKeyframePlan {
    /// Clip-local time in seconds.
    pub time_s: f64,
    /// Normalized crop center x in `0..1`.
    pub center_x: f64,
    /// Normalized crop center y in `0..1`.
    pub center_y: f64,
    /// Crop zoom scale.
    pub scale: f64,
}

/// Clip-level lens warp controls.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WarpPlan {
    /// Quadratic radial distortion coefficient.
    pub k1: f64,
    /// Double-quadratic radial distortion coefficient.
    pub k2: f64,
    /// Relative distortion center on the x axis.
    pub center_x: f64,
    /// Relative distortion center on the y axis.
    pub center_y: f64,
}

/// Animated lens warp controls selected from runtime parameter animations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WarpAnimationPlan {
    /// Animated quadratic radial distortion coefficient.
    pub k1: Option<RenderParameterAnimation>,
    /// Animated double-quadratic radial distortion coefficient.
    pub k2: Option<RenderParameterAnimation>,
    /// Animated relative distortion center on the x axis.
    pub center_x: Option<RenderParameterAnimation>,
    /// Animated relative distortion center on the y axis.
    pub center_y: Option<RenderParameterAnimation>,
}

impl WarpAnimationPlan {
    fn has_any(&self) -> bool {
        self.k1.is_some() || self.k2.is_some() || self.center_x.is_some() || self.center_y.is_some()
    }
}

/// Clip-level procedural camera shake controls.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShakePlan {
    /// Maximum horizontal/vertical displacement in source pixels.
    pub intensity_px: f64,
    /// Oscillation frequency in Hertz.
    pub frequency_hz: f64,
}

fn default_shake_plan() -> ShakePlan {
    ShakePlan {
        intensity_px: 8.0,
        frequency_hz: 12.0,
    }
}

fn default_warp_plan() -> WarpPlan {
    WarpPlan {
        k1: -0.08,
        k2: 0.0,
        center_x: 0.5,
        center_y: 0.5,
    }
}

/// Pull a numeric metadata field off the first effect on `clip` whose
/// `effect_name` matches. Returns `None` when no such effect exists
/// or the metadata field is missing / non-numeric. Used to surface
/// awidat.volume / awidat.speed values into the render pipeline.
fn read_effect_number(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<f64> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(serde_json::Value::as_f64)
}

fn read_effect_bool(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<bool> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(serde_json::Value::as_bool)
}

fn read_effect_string(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<String> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn read_speed(clip: &awidat_proto::otio::Clip) -> Option<SpeedPlan> {
    let factor = read_effect_number(clip, "awidat.speed", "factor")?;
    if !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    let frame_blending = match read_effect_string(clip, "awidat.speed", "frame_blending").as_deref()
    {
        Some("blend") => FrameBlending::Blend,
        Some("flow") => FrameBlending::Flow,
        _ => FrameBlending::Nearest,
    };
    Some(SpeedPlan {
        factor,
        reverse: read_effect_bool(clip, "awidat.speed", "reverse").unwrap_or(false),
        maintain_pitch: read_effect_bool(clip, "awidat.speed", "maintain_pitch").unwrap_or(true),
        frame_blending,
    })
}

fn read_time_remap(
    clip: &awidat_proto::otio::Clip,
) -> Result<Option<TimeRemapPlan>, RenderTimelineError> {
    let Some(effect) = clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.time_remap")
    else {
        return Ok(None);
    };
    let invalid = |message: String| RenderTimelineError::InvalidClipEffectMetadata {
        clip_name: clip.name.clone(),
        effect: effect.effect_name.clone(),
        message,
    };
    let curve = effect
        .metadata
        .get("curve")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("curve must be an array".to_string()))?;
    if curve.len() < 2 {
        return Err(invalid(
            "curve must contain at least two time-remap points".to_string(),
        ));
    }
    let mut points = Vec::with_capacity(curve.len());
    for (idx, item) in curve.iter().enumerate() {
        let point = item
            .as_object()
            .ok_or_else(|| invalid(format!("curve point #{idx} must be an object")))?;
        let source_time_s = point
            .get("source_time_s")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                invalid(format!(
                    "curve point #{idx} needs finite non-negative source_time_s"
                ))
            })?;
        let timeline_time_s = point
            .get("timeline_time_s")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                invalid(format!(
                    "curve point #{idx} needs finite non-negative timeline_time_s"
                ))
            })?;
        points.push(TimeRemapPoint {
            source_time_s,
            timeline_time_s,
        });
    }
    let first = points[0];
    if first.source_time_s.abs() > 1e-9 || first.timeline_time_s.abs() > 1e-9 {
        return Err(invalid(
            "curve must start at source_time_s=0 and timeline_time_s=0".to_string(),
        ));
    }
    for pair in points.windows(2) {
        // Source axis must be strictly increasing — zero or negative slope
        // there would encode a reverse / duplicate sample, which the setpts
        // mapping cannot represent.
        if pair[1].source_time_s <= pair[0].source_time_s {
            return Err(invalid(
                "curve source_time_s values must be strictly increasing".to_string(),
            ));
        }
        // Output (timeline) axis is allowed to stay flat: a zero-slope
        // segment expresses a freeze plateau (source advances, output holds).
        // Negative slope is still illegal — it would scrub the timeline
        // backwards, which the renderer does not support.
        if pair[1].timeline_time_s < pair[0].timeline_time_s {
            return Err(invalid(
                "curve timeline_time_s values must be non-decreasing".to_string(),
            ));
        }
    }
    Ok(Some(TimeRemapPlan { points }))
}

fn read_freeze(clip: &awidat_proto::otio::Clip) -> Option<FreezePlan> {
    let freeze_at_source_s = read_effect_number(clip, "awidat.freeze", "freeze_at_source_s")?;
    let duration_s = read_effect_number(clip, "awidat.freeze", "duration_s")?;
    let freeze_position =
        read_effect_string(clip, "awidat.freeze", "freeze_position").unwrap_or_else(|| "at".into());
    let audio_behavior = read_effect_string(clip, "awidat.freeze", "audio_behavior")
        .unwrap_or_else(|| "silence".into());
    if !freeze_at_source_s.is_finite()
        || freeze_at_source_s < 0.0
        || !duration_s.is_finite()
        || duration_s <= 0.0
        || freeze_position != "at"
        || audio_behavior != "silence"
    {
        return None;
    }
    Some(FreezePlan {
        freeze_at_source_s,
        duration_s,
    })
}

fn read_color_correction(clip: &awidat_proto::otio::Clip) -> Option<ColorCorrectionPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.color_correction")?;
    let m = &effect.metadata;
    let plan = ColorCorrectionPlan {
        exposure_ev: m.get("exposure_ev").and_then(serde_json::Value::as_f64),
        contrast: m.get("contrast").and_then(serde_json::Value::as_f64),
        saturation: m.get("saturation").and_then(serde_json::Value::as_f64),
        temperature: m.get("temperature").and_then(serde_json::Value::as_f64),
        tint: m.get("tint").and_then(serde_json::Value::as_f64),
        shadows: m.get("shadows").and_then(serde_json::Value::as_f64),
        highlights: m.get("highlights").and_then(serde_json::Value::as_f64),
    };
    Some(plan)
}

fn read_blur(clip: &awidat_proto::otio::Clip) -> Option<BlurPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.blur")?;
    let radius_px = effect
        .metadata
        .get("radius_px")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(8.0);
    if !radius_px.is_finite() || radius_px <= 0.0 {
        return None;
    }
    Some(BlurPlan {
        radius_px: radius_px.clamp(0.0, 100.0),
    })
}

fn read_chroma_key(clip: &awidat_proto::otio::Clip) -> Option<ChromaKeyPlan> {
    let key_color = read_effect_string(clip, "awidat.chroma_key", "key_color")?;
    let similarity = read_effect_number(clip, "awidat.chroma_key", "similarity").unwrap_or(0.1);
    let blend = read_effect_number(clip, "awidat.chroma_key", "blend").unwrap_or(0.0);
    if !similarity.is_finite() || !blend.is_finite() {
        return None;
    }
    Some(ChromaKeyPlan {
        key_color: normalize_ffmpeg_color(&key_color),
        similarity: similarity.clamp(0.0, 1.0),
        blend: blend.clamp(0.0, 1.0),
    })
}

fn read_luma_key(clip: &awidat_proto::otio::Clip) -> Option<LumaKeyPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.luma_key")?;
    let threshold = effect
        .metadata
        .get("threshold")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.5);
    let softness = effect
        .metadata
        .get("softness")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.1);
    if !threshold.is_finite() || !softness.is_finite() {
        return None;
    }
    Some(LumaKeyPlan {
        threshold: threshold.clamp(0.0, 1.0),
        softness: softness.clamp(0.0, 1.0),
    })
}

fn read_region_blur(clip: &awidat_proto::otio::Clip) -> Option<RegionBlurPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.region_blur")?;
    let m = &effect.metadata;
    let x = m.get("x").and_then(serde_json::Value::as_f64)?;
    let y = m.get("y").and_then(serde_json::Value::as_f64)?;
    let width = m.get("width").and_then(serde_json::Value::as_f64)?;
    let height = m.get("height").and_then(serde_json::Value::as_f64)?;
    let radius_px = m
        .get("radius_px")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(12.0);
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || !radius_px.is_finite()
        || width <= 0.0
        || height <= 0.0
        || radius_px <= 0.0
    {
        return None;
    }
    let shape = match m
        .get("shape")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("rect")
    {
        "ellipse" => RegionBlurShape::Ellipse,
        _ => RegionBlurShape::Rect,
    };
    Some(RegionBlurPlan {
        shape,
        x: x.clamp(0.0, 1.0),
        y: y.clamp(0.0, 1.0),
        width: width.clamp(f64::MIN_POSITIVE, 1.0),
        height: height.clamp(f64::MIN_POSITIVE, 1.0),
        radius_px: radius_px.clamp(0.0, 100.0),
    })
}

fn normalize_ffmpeg_color(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        format!("0x{hex}")
    } else {
        trimmed.to_string()
    }
}

fn read_overlay_blend_mode(clip: &awidat_proto::otio::Clip) -> Option<String> {
    let blend_mode = read_effect_string(clip, "awidat.video_overlay", "blend_mode")?;
    matches!(blend_mode.as_str(), "multiply" | "screen").then_some(blend_mode)
}

fn read_hdr_transfer(clip: &awidat_proto::otio::Clip) -> Option<HdrTransfer> {
    let raw = read_effect_string(clip, "awidat.source_color", "transfer")
        .or_else(|| read_effect_string(clip, "awidat.source_color", "hdr_transfer"))?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "smpte2084" | "smpte2084-pq" | "pq" | "bt2020-10" => Some(HdrTransfer::Smpte2084),
        "arib-std-b67" | "arib_std_b67" | "hlg" => Some(HdrTransfer::AribStdB67),
        _ => None,
    }
}

fn read_reframe(clip: &awidat_proto::otio::Clip) -> Option<ReframePlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.reframe")?;
    let m = &effect.metadata;
    let zoom = m.get("zoom").and_then(serde_json::Value::as_f64)?;
    if !zoom.is_finite() || zoom <= 1.0 {
        return None;
    }
    Some(ReframePlan {
        zoom: zoom.clamp(1.0, 3.0),
        x: m.get("x")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0),
        y: m.get("y")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0),
    })
}

fn read_reframe_path(package: Option<&TrackingPackage>, clip_id: &str) -> Option<ReframePathPlan> {
    package?
        .reframe_paths
        .iter()
        .find(|path| path.clip_id == clip_id)
        .and_then(reframe_path_plan)
}

fn reframe_path_plan(path: &ReframePath) -> Option<ReframePathPlan> {
    // Re-smooth the authored keyframes at lowering time so the stored
    // path stays raw evidence; re-rendering with a different policy
    // works by changing `smoothing` rather than re-authoring.
    let smoothed = crate::reframe_smoothing::smooth_keyframes(&path.keyframes, path.smoothing);
    let mut keyframes = smoothed
        .iter()
        .filter_map(|keyframe| {
            let center_x = keyframe.center[0];
            let center_y = keyframe.center[1];
            let scale = keyframe.scale;
            (keyframe.time_s.is_finite()
                && center_x.is_finite()
                && center_y.is_finite()
                && scale.is_finite()
                && scale > 1.0)
                .then_some(ReframePathKeyframePlan {
                    time_s: keyframe.time_s,
                    center_x: center_x.clamp(0.0, 1.0),
                    center_y: center_y.clamp(0.0, 1.0),
                    scale: scale.clamp(1.0, 8.0),
                })
        })
        .collect::<Vec<_>>();
    keyframes.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    (!keyframes.is_empty()).then_some(ReframePathPlan {
        reframe_id: path.id.clone(),
        keyframes,
    })
}

fn read_shake(clip: &awidat_proto::otio::Clip) -> Option<ShakePlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.shake")?;
    let m = &effect.metadata;
    let intensity_px = m
        .get("intensity_px")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(8.0);
    let frequency_hz = m
        .get("frequency_hz")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(12.0);
    if !intensity_px.is_finite()
        || !frequency_hz.is_finite()
        || intensity_px <= 0.0
        || frequency_hz <= 0.0
    {
        return None;
    }
    Some(ShakePlan {
        intensity_px: intensity_px.clamp(0.0, 200.0),
        frequency_hz: frequency_hz.clamp(0.1, 60.0),
    })
}

fn read_warp(clip: &awidat_proto::otio::Clip) -> Option<WarpPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.warp")?;
    let m = &effect.metadata;
    let k1 = m
        .get("k1")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(-0.08);
    let k2 = m
        .get("k2")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let center_x = m
        .get("center_x")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.5);
    let center_y = m
        .get("center_y")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.5);
    if !k1.is_finite() || !k2.is_finite() || !center_x.is_finite() || !center_y.is_finite() {
        return None;
    }
    if k1.abs() <= 1e-9 && k2.abs() <= 1e-9 {
        return None;
    }
    Some(WarpPlan {
        k1: k1.clamp(-1.0, 1.0),
        k2: k2.clamp(-1.0, 1.0),
        center_x: center_x.clamp(0.0, 1.0),
        center_y: center_y.clamp(0.0, 1.0),
    })
}

/// Read an `awidat.color_pipeline` effect off `clip` and resolve its
/// LUT paths against `project_root`. Returns `None` when no
/// pipeline effect is stamped. Surfaces `MissingLut` for any
/// declared-but-not-on-disk LUT path so render fails fast like
/// `awidat.lut` does.
fn read_color_pipeline(
    clip: &awidat_proto::otio::Clip,
    project_root: &Path,
) -> Result<Option<ColorPipelinePlan>, RenderTimelineError> {
    let Some(effect) = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.color_pipeline")
    else {
        return Ok(None);
    };
    let metadata = &effect.metadata;
    let Some(clip_input_space) = metadata
        .get("clip_input_space")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
    else {
        // Apply-time validation enforces this field; if it's absent at
        // render time we treat the effect as malformed and skip rather
        // than crash. Render-time errors would just confuse the agent.
        return Ok(None);
    };
    let resolve = |key: &str| -> Result<Option<PathBuf>, RenderTimelineError> {
        let Some(raw) = metadata.get(key).and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        let full = project_root.join(raw);
        if !full.exists() {
            return Err(RenderTimelineError::MissingLut {
                clip_name: clip.name.clone(),
                missing: full,
            });
        }
        Ok(Some(full))
    };
    let string_field = |key: &str| {
        metadata
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let interpolation = string_field("look_interpolation").filter(|value| {
        matches!(
            value.as_str(),
            "nearest" | "trilinear" | "tetrahedral" | "pyramid" | "prism"
        )
    });
    let look_strength = metadata
        .get("look_strength")
        .and_then(serde_json::Value::as_f64)
        .filter(|s| s.is_finite() && (0.0..=1.0).contains(s));
    let mask_source = metadata
        .get("mask_source")
        .and_then(serde_json::Value::as_str)
        .map(|raw| project_root.join(raw));
    // We don't require the mask to exist on disk while parsing
    // metadata. The render plan adds supported mask sources as
    // explicit inputs, so filesystem failures remain render-time
    // diagnostics like other media assets.
    Ok(Some(ColorPipelinePlan {
        clip_input_space,
        working_space: string_field("working_space"),
        lut_input_space: string_field("lut_input_space"),
        output_space: string_field("output_space"),
        input_transform_lut: resolve("input_transform_lut")?,
        shaper_lut: resolve("shaper_lut")?,
        look_lut: resolve("look_lut")?,
        look_interpolation: interpolation,
        look_strength,
        output_transform_lut: resolve("output_transform_lut")?,
        mask_source,
    }))
}

fn read_lut_interpolation(clip: &awidat_proto::otio::Clip) -> Option<String> {
    let raw = read_effect_string(clip, "awidat.lut", "interpolation")?;
    let value = raw.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "nearest" | "trilinear" | "tetrahedral" | "pyramid" | "prism"
    )
    .then_some(value)
}

fn read_audio_fade(clip: &awidat_proto::otio::Clip) -> (Option<f64>, Option<f64>) {
    let Some(effect) = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.audio_fade")
    else {
        return (None, None);
    };
    (
        effect
            .metadata
            .get("fade_in_s")
            .and_then(serde_json::Value::as_f64),
        effect
            .metadata
            .get("fade_out_s")
            .and_then(serde_json::Value::as_f64),
    )
}

fn read_clip_audio_fx(clip: &awidat_proto::otio::Clip) -> Option<AudioFxPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.audio_fx")?;
    serde_json::from_value(serde_json::Value::Object(effect.metadata.clone())).ok()
}

fn read_timeline_loudness_target(
    metadata: &awidat_proto::awidat_meta::AwidatTimelineMetadata,
) -> Option<LoudnessTargetPlan> {
    let value = metadata.extra.get("loudness_target")?;
    let integrated_lufs = value.get("integrated_lufs")?.as_f64()?;
    if !integrated_lufs.is_finite() || integrated_lufs >= 0.0 {
        return None;
    }
    let true_peak_db = value
        .get("true_peak_db")
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite() && *v <= 0.0);
    Some(LoudnessTargetPlan {
        integrated_lufs,
        true_peak_db,
    })
}

type TimelineFullPlan = (
    Vec<TimelineSegment>,
    Vec<TransitionPlan>,
    Vec<VideoOverlayPlan>,
    Vec<TitlePlan>,
    Vec<AnnotationPlan>,
    Option<BroadcastOverlayPlan>,
    Vec<AudioTrackPlan>,
    Option<LoudnessTargetPlan>,
    Vec<RenderPlanLimitation>,
);

/// Walk `<project_root>/project.otio.json` and collect every
/// video-track clip's `(asset, source_range)` in playback order.
/// Skips Gap and Transition children; nested Stack children are flattened
/// in playback order for render planning.
///
/// Wraps [`collect_timeline_plan`] and drops the transitions +
/// titles — preserved for callers that don't need either.
pub fn collect_timeline_segments(
    project_root: &Path,
) -> Result<Vec<TimelineSegment>, RenderTimelineError> {
    let (segs, _, _, _, _, _, _, _, _) = collect_timeline_full_plan(project_root)?;
    Ok(segs)
}

/// Walk `<project_root>/project.otio.json` and collect both the
/// renderable segments AND the transitions between adjacent
/// segments. Returned in playback order; `TransitionPlan` indices
/// reference the returned segments slice. The render pipeline uses
/// this to splice xfade filters between segments that have a
/// [`TrackChild::Transition`] between them on the OTIO track.
///
/// Wraps [`collect_timeline_full_plan`] and drops the titles —
/// preserved for callers that don't need title awareness.
pub fn collect_timeline_plan(
    project_root: &Path,
) -> Result<(Vec<TimelineSegment>, Vec<TransitionPlan>), RenderTimelineError> {
    let (segs, transitions, _, _, _, _, _, _, _) = collect_timeline_full_plan(project_root)?;
    Ok((segs, transitions))
}

/// Walk `<project_root>/project.otio.json` and collect segments +
/// transitions + titles. The Titles track (flagged via
/// `track.metadata["awidat_track_role"] = "titles"` or matched by
/// name `"Titles"` for backwards-compat) is excluded from segment
/// production — its clips are virtual, not media-bearing.
pub fn collect_timeline_full_plan(
    project_root: &Path,
) -> Result<TimelineFullPlan, RenderTimelineError> {
    let otio_path = project_root.join(files::OTIO);
    if !otio_path.exists() {
        return Err(RenderTimelineError::NoOtio(otio_path));
    }
    let mut warnings = Vec::new();
    let timeline = read_otio_timeline(&otio_path, &mut warnings).map_err(|e| {
        RenderTimelineError::OtioParse {
            message: e.to_string(),
        }
    })?;
    let mut graph_binding_limitations = Vec::new();
    let frame_rate = timeline_frame_rate(&timeline);
    let parameter_animations = collect_effective_parameter_animations(
        timeline.metadata.awidat.as_ref(),
        frame_rate,
        &mut graph_binding_limitations,
    );
    let corner_pin_filters = collect_corner_pin_filters(
        timeline.metadata.awidat.as_ref(),
        &mut graph_binding_limitations,
    );
    let overlay_matte_sources = collect_overlay_matte_sources(
        project_root,
        timeline.metadata.awidat.as_ref(),
        &mut graph_binding_limitations,
    );
    let overlay_masks = collect_overlay_masks(
        timeline.metadata.awidat.as_ref(),
        &mut graph_binding_limitations,
    );

    let mut segs = Vec::new();
    let mut transitions = Vec::new();
    let mut video_overlays = Vec::new();
    let mut titles = Vec::new();
    let mut annotations = Vec::new();
    let mut audio_tracks = Vec::new();
    let mut render_limitations = graph_binding_limitations;
    let mut consumed_animation_ids = BTreeSet::new();
    let mut saw_base_video_track = false;
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if matches!(track.kind, TrackKind::Audio) {
            let audio_selection =
                collect_audio_track_plan(project_root, track, &parameter_animations)?;
            consumed_animation_ids.extend(audio_selection.consumed_animation_ids);
            audio_tracks.push(audio_selection.track);
            continue;
        }
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        if is_titles_track(track) || is_annotations_track(track) {
            // Walk virtual overlay tracks separately; don't try to read media off them.
            for tc in &track.children {
                let TrackChild::Clip(clip) = tc else { continue };
                if let Some((plan, animation_selection)) =
                    parse_title_plan(clip, &parameter_animations)
                {
                    render_limitations.extend(animation_selection.limitations);
                    consumed_animation_ids.extend(animation_selection.consumed_animation_ids);
                    titles.push(plan);
                }
                if let Some(annotation) = parse_annotation_plan(clip) {
                    annotations.push(annotation);
                }
            }
            continue;
        }
        if saw_base_video_track {
            let mut track_cursor_s = 0.0_f64;
            for tc in &track.children {
                match tc {
                    TrackChild::Clip(clip) => {
                        let Some(segment) = collect_timeline_segment(
                            project_root,
                            clip,
                            &parameter_animations,
                            timeline
                                .metadata
                                .awidat
                                .as_ref()
                                .and_then(|m| m.tracking_package.as_ref()),
                            &mut consumed_animation_ids,
                        )?
                        else {
                            continue;
                        };
                        let mode = read_video_overlay_mode(clip);
                        let clip_id = render_clip_id(clip);
                        let animation_selection = render_animations_for_clip_with_limitations(
                            &parameter_animations,
                            &clip_id,
                            "overlay",
                        );
                        render_limitations.extend(animation_selection.limitations);
                        render_limitations.extend(tracker_bind_coverage_limitations(
                            &parameter_animations,
                            &clip_id,
                            track_cursor_s,
                            effective_duration(&segment),
                        ));
                        consumed_animation_ids.extend(animation_selection.consumed_animation_ids);
                        video_overlays.push(VideoOverlayPlan {
                            segment,
                            track_start_s: track_cursor_s,
                            mode,
                            rotation_deg: read_video_overlay_rotation_deg(clip),
                            motion_blur: read_video_overlay_motion_blur_with_limitations(
                                clip,
                                &clip_id,
                                &mut render_limitations,
                            ),
                            corner_pin_filter: corner_pin_filters.get(&clip_id).cloned(),
                            matte_source: overlay_matte_sources.get(&clip_id).cloned(),
                            matte_input_index: None,
                            mask: overlay_masks.get(&clip_id).cloned(),
                            animations: animation_selection.animations,
                        });
                        if let Some(range) = clip.source_range.as_ref() {
                            track_cursor_s += range.duration.to_seconds();
                        }
                    }
                    TrackChild::Gap(gap) => {
                        track_cursor_s += gap.source_range.duration.to_seconds();
                    }
                    TrackChild::Transition(_) | TrackChild::Stack(_) => {}
                }
            }
            continue;
        }
        saw_base_video_track = true;
        // Walk the track's children. Clips become segments; a
        // Transition immediately following a Clip queues a transition
        // pointing at the *next* clip we'll see (`pending_transition`).
        // Other children (Gap, Stack) reset the pending state — they
        // can't sit between clips that share a transition in v1.
        let mut pending_transition: Option<(String, f64, f64, Option<TransitionComposition>)> =
            None;
        let mut saw_clip_on_track = false;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let Some(segment) = collect_timeline_segment(
                        project_root,
                        clip,
                        &parameter_animations,
                        timeline
                            .metadata
                            .awidat
                            .as_ref()
                            .and_then(|m| m.tracking_package.as_ref()),
                        &mut consumed_animation_ids,
                    )?
                    else {
                        pending_transition = None;
                        continue;
                    };
                    let new_index = segs.len();
                    segs.push(segment);
                    saw_clip_on_track = true;
                    if let Some((kind, in_offset_s, out_offset_s, composition)) =
                        pending_transition.take()
                        && new_index > 0
                    {
                        extend_transition_handles(
                            &mut segs,
                            new_index - 1,
                            new_index,
                            &kind,
                            in_offset_s,
                            out_offset_s,
                        )?;
                        let gpu_shader = composition
                            .as_ref()
                            .and_then(awidat_proto::transitions::resolve_composition_gpu_shader)
                            .and_then(awidat_render_gpu::TransitionShader::from_proto_id);
                        transitions.push(TransitionPlan {
                            from_segment_index: new_index - 1,
                            to_segment_index: new_index,
                            kind,
                            in_offset_s,
                            out_offset_s,
                            duration_s: in_offset_s + out_offset_s,
                            composition,
                            gpu_shader,
                        });
                    }
                }
                TrackChild::Transition(t) => {
                    let xfade =
                        transitions::resolve_ffmpeg_xfade(&t.transition_type).map_err(|e| {
                            RenderTimelineError::UnsupportedTransition {
                                kind: t.transition_type.clone(),
                                message: e.to_string(),
                            }
                        })?;
                    if xfade.is_none() {
                        return Err(RenderTimelineError::UnsupportedTransition {
                            kind: t.transition_type.clone(),
                            message: "semantic-only transition cannot be exported".into(),
                        });
                    }
                    if !saw_clip_on_track {
                        return Err(RenderTimelineError::InvalidTransitionPlacement {
                            message: format!(
                                "transition {:?} appears before any renderable clip",
                                t.transition_type
                            ),
                        });
                    }
                    if pending_transition.is_some() {
                        return Err(RenderTimelineError::InvalidTransitionPlacement {
                            message: format!(
                                "transition {:?} follows another transition before an incoming clip",
                                t.transition_type
                            ),
                        });
                    }
                    let composition = read_transition_composition(t)?;
                    validate_renderable_composite_transition(t, composition.as_ref())?;
                    pending_transition = Some((
                        t.transition_type.clone(),
                        t.in_offset.to_seconds(),
                        t.out_offset.to_seconds(),
                        composition,
                    ));
                }
                TrackChild::Gap(_) => {
                    pending_transition = None;
                }
                TrackChild::Stack(stack) => {
                    pending_transition = None;
                    if collect_nested_video_stack_segments(
                        project_root,
                        stack,
                        &parameter_animations,
                        timeline
                            .metadata
                            .awidat
                            .as_ref()
                            .and_then(|m| m.tracking_package.as_ref()),
                        &mut consumed_animation_ids,
                        &mut segs,
                    )? {
                        saw_clip_on_track = true;
                    }
                }
            }
        }
        if let Some((kind, _, _, _)) = pending_transition {
            return Err(RenderTimelineError::InvalidTransitionPlacement {
                message: format!("transition {kind:?} has no incoming clip"),
            });
        }
    }
    apply_dynamic_title_keywords(
        &mut titles,
        &segs,
        timeline.metadata.awidat.as_ref(),
        project_root,
        frame_rate,
    );
    let broadcast_overlay = timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|m| m.broadcast_overlay.clone())
        .map(|config| BroadcastOverlayPlan {
            config,
            project_root: project_root.to_path_buf(),
        });
    let loudness_target = timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(read_timeline_loudness_target);
    if audio_tracks.is_empty() {
        audio_tracks = synthesize_split_edit_audio_tracks(&segs)?;
    }
    render_limitations.extend(unconsumed_animation_limitations(
        &parameter_animations,
        &consumed_animation_ids,
    ));
    render_limitations.extend(mask_source_limitations(&segs));
    render_limitations.extend(log_clip_without_shaper_limitations(&segs));
    Ok((
        segs,
        transitions,
        video_overlays,
        titles,
        annotations,
        broadcast_overlay,
        audio_tracks,
        loudness_target,
        render_limitations,
    ))
}

fn collect_effective_parameter_animations(
    metadata: Option<&AwidatTimelineMetadata>,
    frame_rate: f64,
    limitations: &mut Vec<RenderPlanLimitation>,
) -> Vec<awidat_proto::professional::ParameterAnimation> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let mut animations = metadata.parameter_animations.clone();
    let Some(package) = metadata.tracking_package.as_ref() else {
        return animations;
    };
    for graph in &metadata.composition_graphs {
        match crate::professional::lower_tracker_parameter_bindings(package, graph, frame_rate) {
            Ok(mut generated) => animations.append(&mut generated),
            Err(err) => limitations.push(RenderPlanLimitation {
                kind: "tracker_bind_not_rendered".to_string(),
                animation_id: None,
                clip_id: None,
                parameter: None,
                message: format!(
                    "render ignored tracker bindings in composition graph {}: {err}",
                    graph.id
                ),
            }),
        }
    }
    animations
}

fn collect_corner_pin_filters(
    metadata: Option<&AwidatTimelineMetadata>,
    limitations: &mut Vec<RenderPlanLimitation>,
) -> BTreeMap<String, String> {
    let Some(metadata) = metadata else {
        return BTreeMap::new();
    };
    let Some(package) = metadata.tracking_package.as_ref() else {
        return BTreeMap::new();
    };
    let mut filters = BTreeMap::new();
    for graph in &metadata.composition_graphs {
        match crate::professional::lower_surface_track_corner_pin_bindings(
            package,
            graph,
            TIMELINE_RENDER_WIDTH,
            TIMELINE_RENDER_HEIGHT,
        ) {
            Ok(lowerings) => {
                for lowering in lowerings {
                    filters.insert(lowering.target_clip_id, lowering.filter);
                }
            }
            Err(err) => limitations.push(RenderPlanLimitation {
                kind: "corner_pin_not_rendered".to_string(),
                animation_id: None,
                clip_id: None,
                parameter: None,
                message: format!(
                    "render ignored corner-pin bindings in composition graph {}: {err}",
                    graph.id
                ),
            }),
        }
    }
    filters
}

fn collect_overlay_matte_sources(
    project_root: &Path,
    metadata: Option<&AwidatTimelineMetadata>,
    limitations: &mut Vec<RenderPlanLimitation>,
) -> BTreeMap<String, PathBuf> {
    let Some(metadata) = metadata else {
        return BTreeMap::new();
    };
    let has_matte_nodes = metadata.composition_graphs.iter().any(|graph| {
        graph
            .nodes
            .iter()
            .any(|node| node.node_type == awidat_proto::professional::CompositionNodeType::Matte)
    });
    let Some(package) = metadata.tracking_package.as_ref() else {
        if has_matte_nodes {
            limitations.push(RenderPlanLimitation {
                kind: "matte_not_rendered".to_string(),
                animation_id: None,
                clip_id: None,
                parameter: None,
                message: "render ignored matte nodes because no tracking package was present"
                    .to_string(),
            });
        }
        return BTreeMap::new();
    };
    let mut sources = BTreeMap::new();
    for graph in &metadata.composition_graphs {
        for node in graph
            .nodes
            .iter()
            .filter(|node| node.node_type == awidat_proto::professional::CompositionNodeType::Matte)
        {
            let Some(matte_id) = node
                .params
                .get("matte_id")
                .and_then(serde_json::Value::as_str)
            else {
                limitations.push(matte_node_limitation(
                    graph,
                    node,
                    "missing required string parameter matte_id",
                ));
                continue;
            };
            let Some(target_clip_id) = node
                .params
                .get("target_clip_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                limitations.push(matte_node_limitation(
                    graph,
                    node,
                    "missing required string parameter target_clip_id",
                ));
                continue;
            };
            let Some(matte) = package.mattes.iter().find(|matte| matte.id == matte_id) else {
                limitations.push(matte_node_limitation(
                    graph,
                    node,
                    &format!("matte {matte_id:?} was not found in the tracking package"),
                ));
                continue;
            };
            if matte.alpha_source.trim().is_empty() {
                limitations.push(matte_node_limitation(
                    graph,
                    node,
                    &format!("matte {matte_id:?} has an empty alpha source"),
                ));
                continue;
            }
            let source = PathBuf::from(&matte.alpha_source);
            let source = if source.is_absolute() {
                source
            } else {
                project_root.join(source)
            };
            sources.insert(target_clip_id.to_string(), source);
        }
    }
    sources
}

fn matte_node_limitation(
    graph: &awidat_proto::professional::CompositionGraph,
    node: &awidat_proto::professional::CompositionNode,
    message: &str,
) -> RenderPlanLimitation {
    RenderPlanLimitation {
        kind: "matte_not_rendered".to_string(),
        animation_id: None,
        clip_id: None,
        parameter: Some("composition.matte".to_string()),
        message: format!(
            "render ignored matte node {} in composition graph {}: {message}",
            node.id, graph.id
        ),
    }
}

fn collect_overlay_masks(
    metadata: Option<&AwidatTimelineMetadata>,
    limitations: &mut Vec<RenderPlanLimitation>,
) -> BTreeMap<String, OverlayMaskPlan> {
    let Some(metadata) = metadata else {
        return BTreeMap::new();
    };
    let has_mask_nodes = metadata.composition_graphs.iter().any(|graph| {
        graph
            .nodes
            .iter()
            .any(|node| node.node_type == awidat_proto::professional::CompositionNodeType::Mask)
    });
    let Some(package) = metadata.tracking_package.as_ref() else {
        if has_mask_nodes {
            limitations.push(RenderPlanLimitation {
                kind: "mask_not_rendered".to_string(),
                animation_id: None,
                clip_id: None,
                parameter: None,
                message: "render ignored mask nodes because no tracking package was present"
                    .to_string(),
            });
        }
        return BTreeMap::new();
    };

    let mut masks = BTreeMap::new();
    for graph in &metadata.composition_graphs {
        for node in graph
            .nodes
            .iter()
            .filter(|node| node.node_type == awidat_proto::professional::CompositionNodeType::Mask)
        {
            let Some(mask_id) = node
                .params
                .get("mask_id")
                .and_then(serde_json::Value::as_str)
            else {
                limitations.push(mask_node_limitation(
                    graph,
                    node,
                    "missing required string parameter mask_id",
                ));
                continue;
            };
            let Some(target_clip_id) = node
                .params
                .get("target_clip_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                limitations.push(mask_node_limitation(
                    graph,
                    node,
                    "missing required string parameter target_clip_id",
                ));
                continue;
            };
            let Some(mask) = package.masks.iter().find(|mask| mask.id == mask_id) else {
                limitations.push(mask_node_limitation(
                    graph,
                    node,
                    &format!("mask {mask_id:?} was not found in the tracking package"),
                ));
                continue;
            };
            let mut keyframes = Vec::new();
            for keyframe in &mask.keyframes {
                if keyframe.points.len() < 2 {
                    continue;
                }
                if !keyframe.time_s.is_finite() {
                    keyframes.clear();
                    break;
                }
                let Some((x0, y0, x1, y1)) = normalized_points_bounds(&keyframe.points) else {
                    keyframes.clear();
                    break;
                };
                keyframes.push(OverlayMaskKeyframe {
                    time_s: keyframe.time_s,
                    x0,
                    y0,
                    x1,
                    y1,
                    opacity: keyframe.opacity.clamp(0.0, 1.0),
                });
            }
            keyframes.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
            if keyframes.is_empty() {
                limitations.push(mask_node_limitation(
                    graph,
                    node,
                    &format!("mask {mask_id:?} has no renderable normalized keyframe path"),
                ));
                continue;
            }
            masks.insert(
                target_clip_id.to_string(),
                OverlayMaskPlan {
                    keyframes,
                    operation: mask.operation,
                },
            );
        }
    }
    masks
}

fn normalized_points_bounds(points: &[[f64; 2]]) -> Option<(f64, f64, f64, f64)> {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for [x, y] in points {
        if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y)
        {
            return None;
        }
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

fn mask_node_limitation(
    graph: &awidat_proto::professional::CompositionGraph,
    node: &awidat_proto::professional::CompositionNode,
    message: &str,
) -> RenderPlanLimitation {
    RenderPlanLimitation {
        kind: "mask_not_rendered".to_string(),
        animation_id: None,
        clip_id: None,
        parameter: Some("composition.mask".to_string()),
        message: format!(
            "render ignored mask node {} in composition graph {}: {message}",
            node.id, graph.id
        ),
    }
}

fn timeline_frame_rate(timeline: &awidat_proto::otio::Timeline) -> f64 {
    timeline
        .tracks
        .children
        .iter()
        .find_map(stack_child_frame_rate)
        .unwrap_or(30.0)
}

fn stack_child_frame_rate(child: &StackChild) -> Option<f64> {
    match child {
        StackChild::Track(track) => track.children.iter().find_map(track_child_frame_rate),
        StackChild::Stack(stack) => stack.children.iter().find_map(stack_child_frame_rate),
        StackChild::Clip(clip) => clip_source_range_rate(clip),
        StackChild::Gap(gap) => positive_rate(gap.source_range.duration.rate),
    }
}

fn track_child_frame_rate(child: &TrackChild) -> Option<f64> {
    match child {
        TrackChild::Clip(clip) => clip_source_range_rate(clip),
        TrackChild::Gap(gap) => positive_rate(gap.source_range.duration.rate),
        TrackChild::Transition(transition) => positive_rate(transition.in_offset.rate),
        TrackChild::Stack(stack) => stack.children.iter().find_map(stack_child_frame_rate),
    }
}

fn clip_source_range_rate(clip: &awidat_proto::otio::Clip) -> Option<f64> {
    clip.source_range
        .as_ref()
        .and_then(|range| positive_rate(range.duration.rate))
}

fn positive_rate(rate: f64) -> Option<f64> {
    (rate.is_finite() && rate > 0.0).then_some(rate)
}

fn read_transition_composition(
    transition: &awidat_proto::otio::Transition,
) -> Result<Option<TransitionComposition>, RenderTimelineError> {
    let Some(value) = transition.metadata.get("awidat_transition") else {
        return Ok(None);
    };
    let Some(composition_value) = value.get("composition") else {
        return Ok(None);
    };
    if composition_value.is_null() {
        return Ok(None);
    }
    let composition: TransitionComposition = serde_json::from_value(composition_value.clone())
        .map_err(|e| RenderTimelineError::InvalidTransitionMetadata {
            kind: transition.transition_type.clone(),
            message: format!("composition could not be decoded: {e}"),
        })?;
    transitions::validate_transition_composition(&composition).map_err(|e| {
        RenderTimelineError::InvalidTransitionMetadata {
            kind: transition.transition_type.clone(),
            message: e.to_string(),
        }
    })?;
    Ok(Some(composition))
}

fn validate_renderable_composite_transition(
    transition: &awidat_proto::otio::Transition,
    composition: Option<&TransitionComposition>,
) -> Result<(), RenderTimelineError> {
    if transition.transition_type != "awidat.composite" {
        return Ok(());
    }
    let Some(composition) = composition else {
        return Err(RenderTimelineError::InvalidTransitionMetadata {
            kind: transition.transition_type.clone(),
            message: "awidat.composite requires metadata.awidat_transition.composition".into(),
        });
    };
    if transitions::resolve_composition_ffmpeg_xfade(composition).is_none() {
        return Err(RenderTimelineError::InvalidTransitionMetadata {
            kind: transition.transition_type.clone(),
            message: "awidat.composite composition has no phase-one FFmpeg lowering".into(),
        });
    }
    Ok(())
}

fn collect_nested_video_stack_segments(
    project_root: &Path,
    stack: &awidat_proto::otio::Stack,
    parameter_animations: &[awidat_proto::professional::ParameterAnimation],
    tracking_package: Option<&TrackingPackage>,
    consumed_animation_ids: &mut BTreeSet<String>,
    segs: &mut Vec<TimelineSegment>,
) -> Result<bool, RenderTimelineError> {
    let mut saw_clip = false;
    for child in &stack.children {
        match child {
            StackChild::Clip(clip) => {
                if let Some(segment) = collect_timeline_segment(
                    project_root,
                    clip,
                    parameter_animations,
                    tracking_package,
                    consumed_animation_ids,
                )? {
                    segs.push(segment);
                    saw_clip = true;
                }
            }
            StackChild::Track(track)
                if matches!(track.kind, TrackKind::Video)
                    && !is_titles_track(track)
                    && !is_annotations_track(track) =>
            {
                if collect_nested_video_track_segments(
                    project_root,
                    track,
                    parameter_animations,
                    tracking_package,
                    consumed_animation_ids,
                    segs,
                )? {
                    saw_clip = true;
                }
            }
            StackChild::Stack(nested) => {
                if collect_nested_video_stack_segments(
                    project_root,
                    nested,
                    parameter_animations,
                    tracking_package,
                    consumed_animation_ids,
                    segs,
                )? {
                    saw_clip = true;
                }
            }
            StackChild::Track(_) | StackChild::Gap(_) => {}
        }
    }
    Ok(saw_clip)
}

fn collect_nested_video_track_segments(
    project_root: &Path,
    track: &awidat_proto::otio::Track,
    parameter_animations: &[awidat_proto::professional::ParameterAnimation],
    tracking_package: Option<&TrackingPackage>,
    consumed_animation_ids: &mut BTreeSet<String>,
    segs: &mut Vec<TimelineSegment>,
) -> Result<bool, RenderTimelineError> {
    let mut saw_clip = false;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                if let Some(segment) = collect_timeline_segment(
                    project_root,
                    clip,
                    parameter_animations,
                    tracking_package,
                    consumed_animation_ids,
                )? {
                    segs.push(segment);
                    saw_clip = true;
                }
            }
            TrackChild::Stack(stack) => {
                if collect_nested_video_stack_segments(
                    project_root,
                    stack,
                    parameter_animations,
                    tracking_package,
                    consumed_animation_ids,
                    segs,
                )? {
                    saw_clip = true;
                }
            }
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
    Ok(saw_clip)
}

fn collect_timeline_segment(
    project_root: &Path,
    clip: &awidat_proto::otio::Clip,
    parameter_animations: &[awidat_proto::professional::ParameterAnimation],
    tracking_package: Option<&TrackingPackage>,
    consumed_animation_ids: &mut BTreeSet<String>,
) -> Result<Option<TimelineSegment>, RenderTimelineError> {
    let MediaReference::External(ext) = &clip.media_reference else {
        return Ok(None);
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(RenderTimelineError::ClipMissingRange {
            clip_name: clip.name.clone(),
        });
    };
    let asset_path = project_root.join(&ext.target_url);
    if !asset_path.exists() {
        return Err(RenderTimelineError::MissingAsset {
            clip_name: clip.name.clone(),
            missing: asset_path,
        });
    }
    let lut_path = read_effect_string(clip, "awidat.lut", "lut_path")
        .map(|lut_path| project_root.join(lut_path));
    if let Some(lut_path) = lut_path.as_ref()
        && !lut_path.exists()
    {
        return Err(RenderTimelineError::MissingLut {
            clip_name: clip.name.clone(),
            missing: lut_path.clone(),
        });
    }
    let lut_interpolation = read_lut_interpolation(clip);
    let lut_strength = read_effect_number(clip, "awidat.lut", "strength")
        .filter(|s| s.is_finite() && (0.0..=1.0).contains(s));
    let color_pipeline = read_color_pipeline(clip, project_root)?;
    let time_remap = read_time_remap(clip)?;
    let clip_id = render_clip_id(clip);
    let shake_intensity_animation = select_clip_effect_animation(
        parameter_animations,
        &clip_id,
        "awidat.shake.intensity_px",
        consumed_animation_ids,
    );
    let shake_frequency_animation = select_clip_effect_animation(
        parameter_animations,
        &clip_id,
        "awidat.shake.frequency_hz",
        consumed_animation_ids,
    );
    let shake = read_shake(clip).or_else(|| {
        (shake_intensity_animation.is_some() || shake_frequency_animation.is_some())
            .then(default_shake_plan)
    });
    let warp_animation =
        select_warp_animation(parameter_animations, &clip_id, consumed_animation_ids);
    let warp = read_warp(clip).or_else(|| warp_animation.as_ref().map(|_| default_warp_plan()));
    let audio_volume_automation = audio_volume_automation_for_clip(parameter_animations, &clip_id);
    consumed_animation_ids.extend(audio_volume_automation.consumed_animation_ids.clone());
    Ok(Some(TimelineSegment {
        asset_path,
        clip_name: clip.name.clone(),
        start_s: range.start_time.to_seconds(),
        duration_s: range.duration.to_seconds(),
        pre_handle_s: 0.0,
        post_handle_s: 0.0,
        source_available_start_s: ext
            .available_range
            .as_ref()
            .map(|r| r.start_time.to_seconds()),
        source_available_end_s: ext
            .available_range
            .as_ref()
            .map(|r| r.start_time.to_seconds() + r.duration.to_seconds()),
        mask_input_index: None,
        volume: read_effect_number(clip, "awidat.volume", "value"),
        volume_automation: audio_volume_automation.automation,
        speed: read_speed(clip),
        time_remap,
        freeze: read_freeze(clip),
        color_correction: read_color_correction(clip),
        blur: read_blur(clip),
        chroma_key: read_chroma_key(clip),
        luma_key: read_luma_key(clip),
        region_blur: read_region_blur(clip),
        blur_radius_animation: select_clip_effect_animation(
            parameter_animations,
            &clip_id,
            "awidat.blur.radius_px",
            consumed_animation_ids,
        ),
        hdr_transfer: read_hdr_transfer(clip),
        reframe: read_reframe(clip),
        reframe_path: read_reframe_path(tracking_package, &clip_id),
        warp,
        warp_animation,
        shake,
        shake_intensity_animation,
        shake_frequency_animation,
        lut_path,
        lut_interpolation,
        lut_strength,
        color_pipeline,
        audio_fx: read_clip_audio_fx(clip),
        overlay_blend_mode: read_overlay_blend_mode(clip),
        audio_lead_s: clip
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.split_edit.as_ref())
            .and_then(|s| s.audio_lead_s),
        audio_trail_s: clip
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.split_edit.as_ref())
            .and_then(|s| s.audio_trail_s),
    }))
}

fn extend_transition_handles(
    segs: &mut [TimelineSegment],
    outgoing_index: usize,
    incoming_index: usize,
    kind: &str,
    in_offset_s: f64,
    out_offset_s: f64,
) -> Result<(), RenderTimelineError> {
    if in_offset_s > 0.0 {
        add_pre_handle(&mut segs[incoming_index], kind, in_offset_s)?;
    }
    if out_offset_s > 0.0 {
        add_post_handle(&mut segs[outgoing_index], kind, out_offset_s)?;
    }
    Ok(())
}

fn add_pre_handle(
    seg: &mut TimelineSegment,
    kind: &str,
    handle_timeline_s: f64,
) -> Result<(), RenderTimelineError> {
    let speed = segment_speed(seg);
    let handle_source_s = handle_timeline_s * speed;
    let available_source_s = seg
        .source_available_start_s
        .map(|start| (seg.start_s - start).max(0.0))
        .unwrap_or(seg.start_s.max(0.0));
    let available_timeline_s = available_source_s / speed;
    if available_timeline_s + 1e-6 < handle_timeline_s {
        return Err(RenderTimelineError::TransitionHandleUnavailable {
            kind: kind.to_string(),
            clip_name: seg.clip_name.clone(),
            side: "incoming",
            needed_s: handle_timeline_s,
            available_s: available_timeline_s,
        });
    }
    seg.start_s -= handle_source_s;
    seg.duration_s += handle_source_s;
    seg.pre_handle_s += handle_timeline_s;
    Ok(())
}

fn add_post_handle(
    seg: &mut TimelineSegment,
    kind: &str,
    handle_timeline_s: f64,
) -> Result<(), RenderTimelineError> {
    let speed = segment_speed(seg);
    let handle_source_s = handle_timeline_s * speed;
    if let Some(end) = seg.source_available_end_s {
        let visible_end = seg.start_s + seg.duration_s;
        let available_source_s = (end - visible_end).max(0.0);
        let available_timeline_s = available_source_s / speed;
        if available_timeline_s + 1e-6 < handle_timeline_s {
            return Err(RenderTimelineError::TransitionHandleUnavailable {
                kind: kind.to_string(),
                clip_name: seg.clip_name.clone(),
                side: "outgoing",
                needed_s: handle_timeline_s,
                available_s: available_timeline_s,
            });
        }
    }
    seg.duration_s += handle_source_s;
    seg.post_handle_s += handle_timeline_s;
    Ok(())
}

fn read_video_overlay_mode(clip: &awidat_proto::otio::Clip) -> VideoOverlayMode {
    let Some(effect) = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.video_overlay")
    else {
        return VideoOverlayMode::FullFrame;
    };
    if effect.metadata.get("mode").and_then(|v| v.as_str()) != Some("pip") {
        return VideoOverlayMode::FullFrame;
    }
    VideoOverlayMode::PiP {
        corner: effect
            .metadata
            .get("corner")
            .and_then(|v| v.as_str())
            .unwrap_or("bottom_right")
            .to_string(),
        scale: effect
            .metadata
            .get("scale")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.28)
            .clamp(0.10, 0.60),
        margin_pct: effect
            .metadata
            .get("margin_pct")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.035)
            .clamp(0.0, 0.15),
    }
}

fn read_video_overlay_rotation_deg(clip: &awidat_proto::otio::Clip) -> f64 {
    clip.effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.video_overlay")
        .and_then(|effect| effect.metadata.get("rotation_deg"))
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .clamp(-360.0, 360.0)
}

fn read_video_overlay_motion_blur_with_limitations(
    clip: &awidat_proto::otio::Clip,
    clip_id: &str,
    limitations: &mut Vec<RenderPlanLimitation>,
) -> Option<OverlayMotionBlurPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.video_overlay")?;
    if !effect
        .metadata
        .get("motion_blur")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let authored_shutter_s = effect
        .metadata
        .get("motion_blur_shutter_s")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    let mut shutter_s = authored_shutter_s.unwrap_or(1.0 / 60.0);
    if shutter_s > 0.2 {
        limitations.push(RenderPlanLimitation {
            kind: "motion_blur_shutter_clamped".to_string(),
            animation_id: None,
            clip_id: Some(clip_id.to_string()),
            parameter: Some("overlay.motion_blur_shutter_s".to_string()),
            message: format!(
                "render clamped overlay motion blur shutter from {}s to 0.2s",
                fmt_filter_num(shutter_s)
            ),
        });
        shutter_s = 0.2;
    }
    Some(OverlayMotionBlurPlan { shutter_s })
}

#[derive(Debug)]
struct AudioTrackAnimationSelection {
    track: AudioTrackPlan,
    consumed_animation_ids: BTreeSet<String>,
}

fn collect_audio_track_plan(
    project_root: &Path,
    track: &awidat_proto::otio::Track,
    animations: &[awidat_proto::professional::ParameterAnimation],
) -> Result<AudioTrackAnimationSelection, RenderTimelineError> {
    let settings = parse_audio_track_settings(track);
    let mut items = Vec::new();
    let mut consumed_animation_ids = BTreeSet::new();
    for tc in &track.children {
        match tc {
            TrackChild::Clip(clip) => {
                let MediaReference::External(ext) = &clip.media_reference else {
                    continue;
                };
                let Some(range) = clip.source_range.as_ref() else {
                    return Err(RenderTimelineError::ClipMissingRange {
                        clip_name: clip.name.clone(),
                    });
                };
                let asset_path = project_root.join(&ext.target_url);
                if !asset_path.exists() {
                    return Err(RenderTimelineError::MissingAsset {
                        clip_name: clip.name.clone(),
                        missing: asset_path,
                    });
                }
                let (fade_in_s, fade_out_s) = read_audio_fade(clip);
                let audio_fx = read_clip_audio_fx(clip);
                let clip_id = render_clip_id(clip);
                let audio_volume_automation =
                    audio_volume_automation_for_clip(animations, &clip_id);
                consumed_animation_ids.extend(audio_volume_automation.consumed_animation_ids);
                items.push(AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path,
                    start_s: range.start_time.to_seconds(),
                    duration_s: range.duration.to_seconds(),
                    volume: read_effect_number(clip, "awidat.volume", "value"),
                    volume_automation: audio_volume_automation.automation,
                    speed: read_speed(clip),
                    fade_in_s,
                    fade_out_s,
                    audio_fx,
                }));
            }
            TrackChild::Gap(gap) => items.push(AudioTrackItemPlan::Gap {
                duration_s: gap.source_range.duration.to_seconds(),
            }),
            TrackChild::Transition(_) | TrackChild::Stack(_) => {}
        }
    }
    let automation_selection = audio_volume_automation_for_track(animations, &track.name);
    consumed_animation_ids.extend(automation_selection.consumed_animation_ids);
    Ok(AudioTrackAnimationSelection {
        track: AudioTrackPlan {
            name: track.name.clone(),
            role: settings.role,
            volume: settings.volume,
            volume_automation: automation_selection.automation,
            muted: settings.muted,
            solo: settings.solo,
            ducking: settings.ducking,
            audio_fx: settings.audio_fx,
            items,
        },
        consumed_animation_ids,
    })
}

#[derive(Debug, Default)]
struct AudioAutomationSelection {
    automation: Option<AudioAutomationPlan>,
    consumed_animation_ids: BTreeSet<String>,
}

fn audio_volume_automation_for_track(
    animations: &[awidat_proto::professional::ParameterAnimation],
    track_name: &str,
) -> AudioAutomationSelection {
    let mut selection = AudioAutomationSelection::default();
    for animation in animations {
        let awidat_proto::professional::AnimationTarget::TrackParameter { track, parameter } =
            &animation.target
        else {
            continue;
        };
        if track != track_name || !matches!(parameter.as_str(), "volume" | "volume_db") {
            continue;
        }
        selection
            .consumed_animation_ids
            .insert(animation.id.clone());
        if selection.automation.is_some() {
            continue;
        }
        let expression = audio_volume_automation_expression(
            parameter,
            &animation.keyframes,
            animation.pre_extrapolation,
            animation.post_extrapolation,
        );
        selection.automation = Some(AudioAutomationPlan {
            parameter: parameter.clone(),
            expression,
            keyframes: animation.keyframes.clone(),
        });
    }
    selection
}

fn audio_volume_automation_for_clip(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
) -> AudioAutomationSelection {
    let mut selection = AudioAutomationSelection::default();
    for animation in animations {
        let awidat_proto::professional::AnimationTarget::ClipParameter {
            clip_id: target_clip_id,
            parameter,
        } = &animation.target
        else {
            continue;
        };
        if target_clip_id != clip_id || !matches!(parameter.as_str(), "volume" | "volume_db") {
            continue;
        }
        selection
            .consumed_animation_ids
            .insert(animation.id.clone());
        if selection.automation.is_some() {
            continue;
        }
        let expression = audio_volume_automation_expression(
            parameter,
            &animation.keyframes,
            animation.pre_extrapolation,
            animation.post_extrapolation,
        );
        selection.automation = Some(AudioAutomationPlan {
            parameter: parameter.clone(),
            expression,
            keyframes: animation.keyframes.clone(),
        });
    }
    selection
}

fn audio_volume_automation_expression(
    parameter: &str,
    keyframes: &[awidat_proto::professional::Keyframe],
    pre_extrapolation: awidat_proto::professional::ExtrapolationMode,
    post_extrapolation: awidat_proto::professional::ExtrapolationMode,
) -> String {
    let value_expr = keyframes_to_ffmpeg_expr_with_extrapolation(
        keyframes,
        "t",
        pre_extrapolation,
        post_extrapolation,
    );
    if parameter == "volume_db" {
        format!("pow(10\\,({value_expr})/20)")
    } else {
        value_expr
    }
}

fn synthesize_split_edit_audio_tracks(
    segments: &[TimelineSegment],
) -> Result<Vec<AudioTrackPlan>, RenderTimelineError> {
    if !segments.iter().any(segment_has_split_edit_audio) {
        return Ok(Vec::new());
    }
    let mut tracks = Vec::new();
    let mut picture_start_s = 0.0_f64;
    for (idx, segment) in segments.iter().enumerate() {
        let lead_s = normalized_split_offset(segment.audio_lead_s);
        let trail_s = normalized_split_offset(segment.audio_trail_s);
        validate_split_edit_handles(segment, lead_s, trail_s)?;
        let source_start_s = segment.start_s - lead_s * segment_speed(segment);
        let duration_s = segment.duration_s + (lead_s + trail_s) * segment_speed(segment);
        let audio_start_s = (picture_start_s - lead_s).max(0.0);
        let mut items = Vec::new();
        if audio_start_s > 0.0 {
            items.push(AudioTrackItemPlan::Gap {
                duration_s: audio_start_s,
            });
        }
        items.push(AudioTrackItemPlan::Clip(AudioClipPlan {
            asset_path: segment.asset_path.clone(),
            start_s: source_start_s,
            duration_s,
            volume: segment.volume,
            volume_automation: segment.volume_automation.clone(),
            speed: segment.speed,
            fade_in_s: None,
            fade_out_s: None,
            audio_fx: segment.audio_fx.clone(),
        }));
        tracks.push(AudioTrackPlan {
            name: format!("split-edit-a{}", idx + 1),
            role: "dialogue".into(),
            volume: 1.0,
            volume_automation: None,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items,
        });
        picture_start_s += visible_effective_duration(segment);
    }
    Ok(tracks)
}

fn segment_has_split_edit_audio(segment: &TimelineSegment) -> bool {
    normalized_split_offset(segment.audio_lead_s) > 0.0
        || normalized_split_offset(segment.audio_trail_s) > 0.0
}

fn normalized_split_offset(value: Option<f64>) -> f64 {
    value.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0)
}

fn validate_split_edit_handles(
    segment: &TimelineSegment,
    lead_s: f64,
    trail_s: f64,
) -> Result<(), RenderTimelineError> {
    if lead_s > 0.0 {
        let needed_source_s = lead_s * segment_speed(segment);
        let available_source_s = segment
            .source_available_start_s
            .map(|start| (segment.start_s - start).max(0.0))
            .unwrap_or(segment.start_s.max(0.0));
        if available_source_s + 1e-6 < needed_source_s {
            return Err(RenderTimelineError::TransitionHandleUnavailable {
                kind: "split_edit_audio_lead".into(),
                clip_name: segment.clip_name.clone(),
                side: "incoming",
                needed_s: lead_s,
                available_s: available_source_s / segment_speed(segment),
            });
        }
    }
    if trail_s > 0.0
        && let Some(end_s) = segment.source_available_end_s
    {
        let needed_source_s = trail_s * segment_speed(segment);
        let visible_end_s = segment.start_s + segment.duration_s;
        let available_source_s = (end_s - visible_end_s).max(0.0);
        if available_source_s + 1e-6 < needed_source_s {
            return Err(RenderTimelineError::TransitionHandleUnavailable {
                kind: "split_edit_audio_trail".into(),
                clip_name: segment.clip_name.clone(),
                side: "outgoing",
                needed_s: trail_s,
                available_s: available_source_s / segment_speed(segment),
            });
        }
    }
    Ok(())
}

fn parse_audio_track_settings(track: &awidat_proto::otio::Track) -> AudioTrackPlan {
    let value = track.metadata.get("awidat_audio");
    let role = value
        .and_then(|v| v.get("role"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default_audio_role(&track.name));
    let volume = value
        .and_then(|v| v.get("volume"))
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(1.0);
    let muted = value
        .and_then(|v| v.get("muted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let solo = value
        .and_then(|v| v.get("solo"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let ducking = value.and_then(|v| v.get("ducking")).map(|d| DuckingPlan {
        enabled: d
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        amount_db: d
            .get("amount_db")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(-12.0),
        attack_ms: d
            .get("attack_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(80.0),
        release_ms: d
            .get("release_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(300.0),
    });
    let audio_fx = value
        .and_then(|v| v.get("fx"))
        .and_then(|v| serde_json::from_value::<AudioFxPlan>(v.clone()).ok());
    AudioTrackPlan {
        name: track.name.clone(),
        role,
        volume,
        volume_automation: None,
        muted,
        solo,
        ducking,
        audio_fx,
        items: Vec::new(),
    }
}

fn default_audio_role(track_name: &str) -> String {
    let name = track_name.trim().to_ascii_lowercase();
    if name == "a1" || name == "audio 1" {
        "dialogue".into()
    } else if name.contains("music") {
        "music".into()
    } else {
        "sfx".into()
    }
}

/// True iff the track is the project's Titles track. Mirrors the
/// apply-side check in `crates/core/src/edl/apply.rs` —
/// `track.metadata["awidat_track_role"] = "titles"` flag with a
/// fallback to the canonical name.
fn is_titles_track(track: &awidat_proto::otio::Track) -> bool {
    if track
        .metadata
        .get("awidat_track_role")
        .and_then(|v| v.as_str())
        == Some("titles")
    {
        return true;
    }
    track.name == "Titles"
}

fn is_annotations_track(track: &awidat_proto::otio::Track) -> bool {
    if track
        .metadata
        .get("awidat_track_role")
        .and_then(|v| v.as_str())
        == Some("annotations")
    {
        return true;
    }
    track.name == "Annotations"
}

/// Parse one synthesized title-clip into a [`TitlePlan`]. Returns
/// `None` if the clip carries no awidat.title effect or required
/// metadata is missing — the render walk just skips invalid titles
/// rather than aborting.
fn parse_title_plan(
    clip: &awidat_proto::otio::Clip,
    parameter_animations: &[awidat_proto::professional::ParameterAnimation],
) -> Option<(TitlePlan, RenderAnimationSelection)> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.title")?;
    let m = &effect.metadata;
    let text = m.get("text").and_then(|v| v.as_str())?.to_string();
    let start_s = m.get("start_s").and_then(serde_json::Value::as_f64)?;
    let end_s = m.get("end_s").and_then(serde_json::Value::as_f64)?;
    if end_s <= start_s {
        return None;
    }
    let position = match m
        .get("position")
        .and_then(|v| v.as_str())
        .unwrap_or("center")
    {
        "top" => TitlePosition::Top,
        "bottom" => TitlePosition::Bottom,
        _ => TitlePosition::Center,
    };
    let font_size = m
        .get("font_size")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or(64);
    let color = m
        .get("color")
        .and_then(|v| v.as_str())
        .unwrap_or("#FFFFFF")
        .to_string();
    let font_weight = match m
        .get("font_weight")
        .and_then(|v| v.as_str())
        .unwrap_or("normal")
    {
        "bold" => TitleWeight::Bold,
        _ => TitleWeight::Normal,
    };
    let animation = match m
        .get("animation")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "fade_in" => TitleAnimation::FadeIn,
        "fade_out" => TitleAnimation::FadeOut,
        "fade_in_out" => TitleAnimation::FadeInOut,
        "slide_in" => TitleAnimation::SlideIn,
        "slide_out" => TitleAnimation::SlideOut,
        _ => TitleAnimation::None,
    };
    let reveal = match m.get("reveal").and_then(|v| v.as_str()).unwrap_or("none") {
        "typewriter" => TextReveal::Typewriter,
        "word" => TextReveal::Word,
        "line" => TextReveal::Line,
        _ => TextReveal::None,
    };
    let role = m
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("title")
        .to_string();
    let safe_area = m
        .get("safe_area")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let rich_segments = m
        .get("rich_segments")
        .and_then(|value| serde_json::from_value::<Vec<RichTextSegment>>(value.clone()).ok())
        .unwrap_or_default();
    let word_timings = m
        .get("word_timings")
        .and_then(|value| serde_json::from_value::<Vec<CaptionWordTiming>>(value.clone()).ok())
        .unwrap_or_default();
    let clip_id = render_clip_id(clip);
    let animation_selection =
        render_animations_for_clip_with_limitations(parameter_animations, &clip_id, "title");
    let animations = animation_selection.animations.clone();
    let plan = TitlePlan {
        text,
        start_s,
        end_s,
        position,
        font_size,
        color,
        font_weight,
        animation,
        reveal,
        role,
        safe_area,
        rich_segments,
        word_timings,
        animations,
    };
    Some((plan, animation_selection))
}

fn parse_annotation_plan(clip: &awidat_proto::otio::Clip) -> Option<AnnotationPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.annotation")?;
    let metadata = &effect.metadata;
    let kind = match metadata.get("kind").and_then(|value| value.as_str())? {
        "rectangle" => AnnotationKind::Rectangle,
        "circle" => AnnotationKind::Circle,
        "arrow" => AnnotationKind::Arrow,
        "bracket" => AnnotationKind::Bracket,
        "blur" => AnnotationKind::Blur,
        _ => return None,
    };
    let start_s = metadata
        .get("start_s")
        .and_then(serde_json::Value::as_f64)?;
    let end_s = metadata.get("end_s").and_then(serde_json::Value::as_f64)?;
    let x = metadata.get("x").and_then(serde_json::Value::as_f64)?;
    let y = metadata.get("y").and_then(serde_json::Value::as_f64)?;
    let width = metadata.get("width").and_then(serde_json::Value::as_f64)?;
    let height = metadata.get("height").and_then(serde_json::Value::as_f64)?;
    if end_s <= start_s || !valid_annotation_region(x, y, width, height) {
        return None;
    }
    Some(AnnotationPlan {
        kind,
        start_s,
        end_s,
        x,
        y,
        width,
        height,
        color: metadata
            .get("color")
            .and_then(|value| value.as_str())
            .unwrap_or("#FFCC00")
            .to_string(),
        stroke_width: metadata
            .get("stroke_width")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as u32)
            .filter(|value| *value > 0)
            .unwrap_or(4),
        label: metadata
            .get("label")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn valid_annotation_region(x: f64, y: f64, width: f64, height: f64) -> bool {
    [x, y, width, height].into_iter().all(f64::is_finite)
        && x >= 0.0
        && y >= 0.0
        && width > 0.0
        && height > 0.0
        && x + width <= 1.0
        && y + height <= 1.0
}

fn apply_dynamic_title_keywords(
    titles: &mut [TitlePlan],
    segments: &[TimelineSegment],
    metadata: Option<&AwidatTimelineMetadata>,
    project_root: &Path,
    frame_rate: f64,
) {
    for title in titles {
        if !title.text.contains('#') {
            continue;
        }
        title.text = resolve_dynamic_title_text(
            &title.text,
            title.start_s,
            segments,
            metadata,
            project_root,
            frame_rate,
        );
    }
}

fn resolve_dynamic_title_text(
    text: &str,
    timeline_s: f64,
    segments: &[TimelineSegment],
    metadata: Option<&AwidatTimelineMetadata>,
    project_root: &Path,
    frame_rate: f64,
) -> String {
    let frame = timeline_frame_number(timeline_s, frame_rate);
    let filename = active_segment_at(segments, timeline_s)
        .and_then(|segment| segment.asset_path.file_name())
        .and_then(|filename| filename.to_str())
        .or_else(|| project_root.file_name().and_then(|name| name.to_str()))
        .unwrap_or_default();
    let chapter_title = active_chapter_title(metadata, timeline_s).unwrap_or_default();
    let speaker = metadata_dynamic_string(metadata, "speaker_label")
        .or_else(|| metadata_dynamic_string(metadata, "speaker"))
        .unwrap_or_default();
    text.replace("#timecode#", &format_timecode(frame, frame_rate))
        .replace("#frame#", &frame.to_string())
        .replace("#filename#", filename)
        .replace("#chapter_title#", &chapter_title)
        .replace("#speaker#", &speaker)
        .replace(
            "#date#",
            &chrono::Local::now().format("%Y-%m-%d").to_string(),
        )
}

fn timeline_frame_number(timeline_s: f64, frame_rate: f64) -> u64 {
    let safe_time = if timeline_s.is_finite() {
        timeline_s.max(0.0)
    } else {
        0.0
    };
    let safe_rate = if frame_rate.is_finite() && frame_rate > 0.0 {
        frame_rate
    } else {
        24.0
    };
    (safe_time * safe_rate).round() as u64
}

fn format_timecode(frame: u64, frame_rate: f64) -> String {
    let frames_per_second = frame_rate.round().max(1.0) as u64;
    let total_seconds = frame / frames_per_second;
    let frame_remainder = frame % frames_per_second;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}:{frame_remainder:02}")
}

fn active_segment_at(segments: &[TimelineSegment], timeline_s: f64) -> Option<&TimelineSegment> {
    let mut cursor_s = 0.0_f64;
    for segment in segments {
        let duration_s = effective_duration(segment);
        let end_s = cursor_s + duration_s;
        if timeline_s >= cursor_s && timeline_s < end_s {
            return Some(segment);
        }
        cursor_s = end_s;
    }
    segments.last().filter(|segment| {
        (timeline_s - cursor_s).abs() < f64::EPSILON && effective_duration(segment) > 0.0
    })
}

fn active_chapter_title(
    metadata: Option<&AwidatTimelineMetadata>,
    timeline_s: f64,
) -> Option<String> {
    metadata
        .and_then(|metadata| metadata.broadcast_overlay.as_ref())
        .and_then(|overlay| {
            overlay
                .chapters
                .iter()
                .filter(|chapter| chapter.time_seconds <= timeline_s)
                .max_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds))
        })
        .map(|chapter| chapter.text.clone())
}

fn metadata_dynamic_string(metadata: Option<&AwidatTimelineMetadata>, key: &str) -> Option<String> {
    let metadata = metadata?;
    metadata
        .extra
        .get(key)
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            metadata
                .extra
                .get("dynamic_text")
                .and_then(|dynamic| dynamic.get(key))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string)
}

fn render_clip_id(clip: &awidat_proto::otio::Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| clip.name.clone())
}

/// Render-time parameter animation attached to a title or media overlay plan.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderParameterAnimation {
    /// Phase 3A parameter path such as `title.opacity`.
    pub parameter: String,
    /// Clip-local keyframes.
    pub keyframes: Vec<awidat_proto::professional::Keyframe>,
    /// Behavior before the first keyframe.
    pub pre_extrapolation: awidat_proto::professional::ExtrapolationMode,
    /// Behavior after the last keyframe.
    pub post_extrapolation: awidat_proto::professional::ExtrapolationMode,
    /// Optional 2D path for position animations.
    pub motion_path: Option<awidat_proto::professional::MotionPath>,
}

#[derive(Debug, Default)]
struct RenderAnimationSelection {
    animations: Vec<RenderParameterAnimation>,
    limitations: Vec<RenderPlanLimitation>,
    consumed_animation_ids: BTreeSet<String>,
}

#[cfg(test)]
fn render_animations_for_clip(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
    surface: &str,
) -> Vec<RenderParameterAnimation> {
    render_animations_for_clip_with_limitations(animations, clip_id, surface).animations
}

fn render_animations_for_clip_with_limitations(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
    surface: &str,
) -> RenderAnimationSelection {
    let mut selection = RenderAnimationSelection::default();
    animations.iter().for_each(|animation| {
        let awidat_proto::professional::AnimationTarget::ClipParameter {
            clip_id: target_clip_id,
            parameter,
        } = &animation.target
        else {
            return;
        };

        if target_clip_id != clip_id {
            return;
        }
        selection
            .consumed_animation_ids
            .insert(animation.id.clone());
        if !is_phase_3a_parameter(parameter) {
            selection.limitations.push(RenderPlanLimitation {
                kind: "unsupported_parameter".to_string(),
                animation_id: Some(animation.id.clone()),
                clip_id: Some(target_clip_id.clone()),
                parameter: Some(parameter.clone()),
                message: format!(
                    "render ignored animation {} targeting unsupported parameter {parameter}",
                    animation.id
                ),
            });
            return;
        }
        let surface_matches = match surface {
            "title" => parameter.starts_with("title."),
            "overlay" => parameter.starts_with("overlay."),
            _ => false,
        };
        if !surface_matches {
            selection.limitations.push(RenderPlanLimitation {
                kind: "unsupported_animation_surface".to_string(),
                animation_id: Some(animation.id.clone()),
                clip_id: Some(target_clip_id.clone()),
                parameter: Some(parameter.clone()),
                message: format!(
                    "render ignored animation {} because {parameter} is not valid for {surface}",
                    animation.id
                ),
            });
            return;
        }
        if parameter == "overlay.blur" {
            selection
                .limitations
                .extend(overlay_blur_radius_limitations(animation));
        }
        selection.animations.push(RenderParameterAnimation {
            parameter: parameter.clone(),
            keyframes: animation.keyframes.clone(),
            pre_extrapolation: animation.pre_extrapolation,
            post_extrapolation: animation.post_extrapolation,
            motion_path: animation.motion_path.clone(),
        });
    });
    selection
}

fn select_clip_effect_animation(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
    parameter: &str,
    consumed_animation_ids: &mut BTreeSet<String>,
) -> Option<RenderParameterAnimation> {
    let animation = animations.iter().find(|animation| {
        matches!(
            &animation.target,
            awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: target_clip_id,
                parameter: target_parameter,
            } if target_clip_id == clip_id
                && canonical_runtime_clip_parameter(target_parameter) == Some(parameter)
        )
    })?;
    consumed_animation_ids.insert(animation.id.clone());
    Some(RenderParameterAnimation {
        parameter: parameter.to_string(),
        keyframes: animation.keyframes.clone(),
        pre_extrapolation: animation.pre_extrapolation,
        post_extrapolation: animation.post_extrapolation,
        motion_path: animation.motion_path.clone(),
    })
}

fn select_warp_animation(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
    consumed_animation_ids: &mut BTreeSet<String>,
) -> Option<WarpAnimationPlan> {
    let plan = WarpAnimationPlan {
        k1: select_clip_effect_animation(
            animations,
            clip_id,
            "awidat.warp.k1",
            consumed_animation_ids,
        ),
        k2: select_clip_effect_animation(
            animations,
            clip_id,
            "awidat.warp.k2",
            consumed_animation_ids,
        ),
        center_x: select_clip_effect_animation(
            animations,
            clip_id,
            "awidat.warp.center_x",
            consumed_animation_ids,
        ),
        center_y: select_clip_effect_animation(
            animations,
            clip_id,
            "awidat.warp.center_y",
            consumed_animation_ids,
        ),
    };
    plan.has_any().then_some(plan)
}

fn overlay_blur_radius_limitations(
    animation: &awidat_proto::professional::ParameterAnimation,
) -> Vec<RenderPlanLimitation> {
    animation
        .keyframes
        .iter()
        .filter(|keyframe| {
            keyframe.value.is_finite() && keyframe.value > OVERLAY_BLUR_MAX_RADIUS_PX
        })
        .map(|keyframe| RenderPlanLimitation {
            kind: "overlay_blur_radius_clamped".to_string(),
            animation_id: Some(animation.id.clone()),
            clip_id: match &animation.target {
                awidat_proto::professional::AnimationTarget::ClipParameter { clip_id, .. } => {
                    Some(clip_id.clone())
                }
                _ => None,
            },
            parameter: Some("overlay.blur".to_string()),
            message: format!(
                "render clamps overlay.blur keyframe value {} to maximum supported radius {} px",
                keyframe.value,
                fmt_filter_num(OVERLAY_BLUR_MAX_RADIUS_PX)
            ),
        })
        .collect()
}

fn tracker_bind_coverage_limitations(
    animations: &[awidat_proto::professional::ParameterAnimation],
    clip_id: &str,
    track_start_s: f64,
    duration_s: f64,
) -> Vec<RenderPlanLimitation> {
    animations
        .iter()
        .filter(|animation| animation.id.starts_with("tracker-bind-"))
        .filter_map(|animation| {
            let awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: target_clip_id,
                parameter,
            } = &animation.target
            else {
                return None;
            };
            if target_clip_id != clip_id || !parameter.starts_with("overlay.") {
                return None;
            }
            let first = animation.keyframes.first()?;
            let last = animation.keyframes.last()?;
            let starts_late = first.time_s > 1e-6;
            let ends_early = last.time_s + 1e-6 < duration_s;
            if !starts_late && !ends_early {
                return None;
            }
            Some(RenderPlanLimitation {
                kind: "tracker_bind_coverage_shortfall".to_string(),
                animation_id: Some(animation.id.clone()),
                clip_id: Some(clip_id.to_string()),
                parameter: Some(parameter.clone()),
                message: format!(
                    "tracker-bound animation {} covers clip-local {}s..{}s, but overlay window is timeline {}s..{}s; Hold extrapolation is applied outside the tracked range",
                    animation.id,
                    fmt_filter_num(first.time_s),
                    fmt_filter_num(last.time_s),
                    fmt_filter_num(track_start_s),
                    fmt_filter_num(track_start_s + duration_s),
                ),
            })
        })
        .collect()
}

fn unconsumed_animation_limitations(
    animations: &[awidat_proto::professional::ParameterAnimation],
    consumed_animation_ids: &BTreeSet<String>,
) -> Vec<RenderPlanLimitation> {
    animations
        .iter()
        .filter(|animation| !consumed_animation_ids.contains(&animation.id))
        .map(render_limitation_for_unconsumed_animation)
        .collect()
}

/// Surface a `log_clip_without_shaper` limitation when a clip
/// declares a camera Log `clip_input_space` (something `zscale`
/// can't normalize), has a non-Log `lut_input_space`, and didn't
/// provide a shaper or IDT to bridge. Render falls back to
/// applying the LUT in Log space, which is the wrong call — the
/// agent needs to know.
fn log_clip_without_shaper_limitations(segs: &[TimelineSegment]) -> Vec<RenderPlanLimitation> {
    segs.iter()
        .filter_map(|seg| {
            let plan = seg.color_pipeline.as_ref()?;
            // No look LUT means no space-mismatch to worry about.
            plan.look_lut.as_ref()?;
            let lut_space = plan.lut_input_space.as_deref()?;
            if plan.clip_input_space == lut_space {
                return None;
            }
            // Agent provided their own chain — trust it.
            if plan.shaper_lut.is_some() || plan.input_transform_lut.is_some() {
                return None;
            }
            // Auto-zscale will handle display-referred pairs.
            let clip_supported = zscale_params_for_space(&plan.clip_input_space).is_some();
            let lut_supported = zscale_params_for_space(lut_space).is_some();
            if clip_supported && lut_supported {
                return None;
            }
            Some(RenderPlanLimitation {
                kind: "log_clip_without_shaper".to_string(),
                animation_id: None,
                clip_id: Some(seg.clip_name.clone()),
                parameter: Some("color_pipeline.shaper_lut".to_string()),
                message: format!(
                    "clip {:?}: clip_input_space={:?} (camera Log) but no shaper_lut or input_transform_lut was provided; render will apply the look LUT to log-encoded pixels which is likely incorrect",
                    seg.clip_name, plan.clip_input_space,
                ),
            })
        })
        .collect()
}

/// Surface a limitation for every masked color-pipeline combination
/// outside render's supported v1 scope. The current executable path
/// supports a single masked look LUT; more complex LUT chains and
/// mask-only pipelines stay explicit so the agent knows the request
/// was not silently honored.
fn mask_source_limitations(segs: &[TimelineSegment]) -> Vec<RenderPlanLimitation> {
    segs.iter()
        .filter_map(|seg| {
            let plan = seg.color_pipeline.as_ref()?;
            let mask = plan.mask_source.as_ref()?;
            if is_renderable_masked_color_pipeline(plan) {
                return None;
            }
            let kind = if plan.look_lut.is_some() {
                "mask_in_complex_chain"
            } else {
                "mask_not_implemented"
            };
            Some(RenderPlanLimitation {
                kind: kind.to_string(),
                animation_id: None,
                clip_id: Some(seg.clip_name.clone()),
                parameter: Some("color_pipeline.mask_source".to_string()),
                message: format!(
                    "render ignored awidat.color_pipeline.mask_source {} on clip {:?}: this masked color-pipeline combination is not renderable yet \
                     (see docs/color-pipeline/render-mask-lowering.md for the supported v1 scope)",
                    mask.display(),
                    seg.clip_name,
                ),
            })
        })
        .collect()
}

fn is_renderable_masked_color_pipeline(plan: &ColorPipelinePlan) -> bool {
    plan.mask_source.is_some()
        && plan.look_lut.is_some()
        && plan.input_transform_lut.is_none()
        && plan.shaper_lut.is_none()
        && plan.output_transform_lut.is_none()
}

fn renderable_mask_source(seg: &TimelineSegment) -> Option<&Path> {
    let plan = seg.color_pipeline.as_ref()?;
    if is_renderable_masked_color_pipeline(plan) {
        plan.mask_source.as_deref()
    } else {
        None
    }
}

fn renderable_mask_count(segs: &[TimelineSegment]) -> usize {
    segs.iter()
        .filter(|segment| renderable_mask_source(segment).is_some())
        .count()
}

fn renderable_overlay_matte_count(video_overlays: &[VideoOverlayPlan]) -> usize {
    video_overlays
        .iter()
        .filter(|overlay| overlay.matte_source.is_some())
        .count()
}

fn assign_overlay_matte_input_indices(
    video_overlays: &[VideoOverlayPlan],
    first_matte_input: usize,
) -> Vec<VideoOverlayPlan> {
    let mut next_matte_input = first_matte_input;
    video_overlays
        .iter()
        .cloned()
        .map(|mut overlay| {
            if overlay.matte_source.is_some() {
                overlay.matte_input_index = Some(next_matte_input);
                next_matte_input += 1;
            } else {
                overlay.matte_input_index = None;
            }
            overlay
        })
        .collect()
}

fn assign_mask_input_indices(
    segs: &[TimelineSegment],
    first_mask_input: usize,
) -> Vec<TimelineSegment> {
    let mut next_mask_input = first_mask_input;
    segs.iter()
        .cloned()
        .map(|mut seg| {
            if renderable_mask_source(&seg).is_some() {
                seg.mask_input_index = Some(next_mask_input);
                next_mask_input += 1;
            } else {
                seg.mask_input_index = None;
            }
            seg
        })
        .collect()
}

fn render_limitation_for_unconsumed_animation(
    animation: &awidat_proto::professional::ParameterAnimation,
) -> RenderPlanLimitation {
    match &animation.target {
        awidat_proto::professional::AnimationTarget::ClipParameter { clip_id, parameter } => {
            if !is_phase_3a_parameter(parameter) {
                RenderPlanLimitation {
                    kind: "unsupported_parameter".to_string(),
                    animation_id: Some(animation.id.clone()),
                    clip_id: Some(clip_id.clone()),
                    parameter: Some(parameter.clone()),
                    message: format!(
                        "render ignored animation {} targeting unsupported parameter {parameter}",
                        animation.id
                    ),
                }
            } else {
                RenderPlanLimitation {
                    kind: "animation_clip_not_rendered".to_string(),
                    animation_id: Some(animation.id.clone()),
                    clip_id: Some(clip_id.clone()),
                    parameter: Some(parameter.clone()),
                    message: format!(
                        "render ignored animation {} because target clip {clip_id} was not rendered as a supported title or overlay surface",
                        animation.id
                    ),
                }
            }
        }
        _ => RenderPlanLimitation {
            kind: "unsupported_animation_target".to_string(),
            animation_id: Some(animation.id.clone()),
            clip_id: None,
            parameter: None,
            message: format!(
                "render ignored animation {} because its target is not a clip parameter",
                animation.id
            ),
        },
    }
}

/// One title overlay parsed from the project's Titles track. The
/// FilterPlanner emits one `drawtext=` per title at the end of the
/// filter graph, chained off the master concat output.
#[derive(Debug, Clone)]
pub struct TitlePlan {
    /// Text to render.
    pub text: String,
    /// When the title appears, in master-timeline seconds.
    pub start_s: f64,
    /// When the title disappears, in master-timeline seconds.
    pub end_s: f64,
    /// Vertical band on the frame.
    pub position: TitlePosition,
    /// Font size in pixels (rendered against a 1080p reference frame;
    /// ffmpeg scales proportionally).
    pub font_size: u32,
    /// Hex colour string like `"#FFFFFF"`.
    pub color: String,
    /// Bold vs normal weight.
    pub font_weight: TitleWeight,
    /// Entry / exit animation.
    pub animation: TitleAnimation,
    /// Text reveal/write-on behavior.
    pub reveal: TextReveal,
    /// Overlay role, usually `"title"` or `"caption"`.
    pub role: String,
    /// Optional safe-area profile carried by caption nodes.
    pub safe_area: Option<String>,
    /// Optional inline rich-text runs.
    pub rich_segments: Vec<RichTextSegment>,
    /// Optional transcript word timings for word-accurate caption reveal.
    pub word_timings: Vec<CaptionWordTiming>,
    /// Supported Phase 3A parameter animations attached to this title.
    pub animations: Vec<RenderParameterAnimation>,
}

/// One transcript word timing attached to a caption title.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct CaptionWordTiming {
    /// Word text.
    pub text: String,
    /// Word start in master-timeline seconds.
    pub start_s: f64,
    /// Word end in master-timeline seconds.
    pub end_s: f64,
}

/// One styled inline run inside a rich title.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct RichTextSegment {
    /// Text for this run.
    pub text: String,
    /// Optional run color. Falls back to the title color.
    #[serde(default)]
    pub color: Option<String>,
    /// Optional run weight. Falls back to the title weight.
    #[serde(default)]
    pub font_weight: Option<TitleWeight>,
}

/// Text reveal/write-on mode for title overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextReveal {
    /// Show the full string for the whole title window.
    None,
    /// Reveal one Unicode scalar at a time.
    Typewriter,
    /// Reveal one whitespace-delimited word at a time.
    Word,
    /// Reveal one line at a time.
    Line,
}

/// Mirrors `awidat_core::edl::op::TitlePosition` to avoid a render
/// → core dep. Render only needs the variants for emitting drawtext
/// y= expressions; the parsing happens in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlePosition {
    /// Near the top edge.
    Top,
    /// Vertically centered.
    Center,
    /// Near the bottom edge.
    Bottom,
}

/// Mirrors `awidat_core::edl::op::TitleWeight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleWeight {
    /// Regular weight.
    Normal,
    /// Bold weight.
    Bold,
}

/// Mirrors `awidat_core::edl::op::TitleAnimation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAnimation {
    /// No animation.
    None,
    /// Fade in over the leading 500ms.
    FadeIn,
    /// Fade out over the trailing 500ms.
    FadeOut,
    /// Fade in at start_s, fade out at end_s.
    FadeInOut,
    /// Slide in from off-screen.
    SlideIn,
    /// Slide out off-screen.
    SlideOut,
}

/// One transition between two segments in the timeline. Callers may
/// pass an empty `transitions` slice; the planner then emits the
/// monolithic concat filter without xfade splicing.
///
/// `from_segment_index` and `to_segment_index` are indices into the
/// segments slice the planner is fed. They MUST be adjacent
/// (`to == from + 1`); the apply layer rejects non-adjacent
/// transitions before they reach here.
#[derive(Debug, Clone)]
pub struct TransitionPlan {
    /// Index of the outgoing segment.
    pub from_segment_index: usize,
    /// Index of the incoming segment. Must be `from_segment_index + 1`.
    pub to_segment_index: usize,
    /// Transition kind (`"SMPTE_Dissolve"`, `"awidat.fade_in"`, etc).
    /// Mapped to ffmpeg's xfade transition names by the planner.
    pub kind: String,
    /// Timeline seconds before the cut occupied by the transition.
    pub in_offset_s: f64,
    /// Timeline seconds after the cut occupied by the transition.
    pub out_offset_s: f64,
    /// Total transition duration on the timeline, in seconds.
    pub duration_s: f64,
    /// Optional data-only composition recipe carried from
    /// `metadata.awidat_transition.composition`. Phase-one FFmpeg still
    /// exports by `kind`; future backends lower this recipe directly.
    pub composition: Option<TransitionComposition>,
    /// Optional GPU shader identifier. When `Some`, the render
    /// orchestrator routes this transition through the raw-stream
    /// composer (wgpu fragment shader instead of FFmpeg `xfade`).
    /// `None` keeps the legacy filter-graph path unchanged.
    pub gpu_shader: Option<awidat_render_gpu::TransitionShader>,
}

/// Plans the `-filter_complex` argument + map labels for a render.
/// With empty `transitions` AND empty `titles` the output is the
/// monolithic concat filter that the rest of the pipeline expects.
///
/// The planner borrows segments — callers feed the slices by reference.
pub struct FilterPlanner<'a> {
    segments: &'a [TimelineSegment],
    transitions: &'a [TransitionPlan],
    titles: &'a [TitlePlan],
    broadcast_overlay: Option<&'a BroadcastOverlayPlan>,
    /// When set, eligible captions (role=="caption" with non-empty
    /// `word_timings`) are written as `.ass` files into this dir
    /// and rendered via the `subtitles=` libass filter instead of
    /// the drawtext fallback. When `None`, every title goes through
    /// drawtext — that's the legacy path callers default to.
    ass_workdir: Option<PathBuf>,
}

/// Output of [`FilterPlanner::plan`]. Carries everything the caller
/// needs to splice into an ffmpeg argv: the filter graph string and
/// the `[outv]` / `[outa]` map labels (the planner picks these so
/// the xfade chain can rename intermediate stages without breaking
/// the caller's `-map` args).
#[derive(Debug, Clone)]
pub struct FilterPlan {
    /// Value for `-filter_complex`.
    pub filter_complex: String,
    /// Label for `-map` on the video output (typically `[outv]`).
    pub video_out_label: String,
    /// Label for `-map` on the audio output (typically `[outa]`).
    pub audio_out_label: String,
}

fn transition_edge_map(
    segment_count: usize,
    transitions: &[TransitionPlan],
) -> Vec<Option<&TransitionPlan>> {
    let mut transition_after = vec![None; segment_count.saturating_sub(1)];
    for t in transitions {
        if t.from_segment_index >= segment_count || t.to_segment_index != t.from_segment_index + 1 {
            tracing::debug!(
                transition = ?t,
                "FilterPlanner: dropping transition with non-adjacent indices"
            );
            continue;
        }
        if let Some(slot) = transition_after.get_mut(t.from_segment_index) {
            if slot.is_some() {
                tracing::debug!(
                    transition = ?t,
                    "FilterPlanner: dropping duplicate transition edge"
                );
                continue;
            }
            *slot = Some(t);
        }
    }
    transition_after
}

impl<'a> FilterPlanner<'a> {
    /// Construct a planner over segments + transitions, with no
    /// titles. Equivalent to [`Self::with_titles`] passing `&[]`.
    pub fn new(segments: &'a [TimelineSegment], transitions: &'a [TransitionPlan]) -> Self {
        Self::with_titles(segments, transitions, &[])
    }

    /// Construct a planner over segments + transitions + titles.
    /// Title overlays land as `drawtext=` filters appended to the
    /// master video output of the segment + transition graph.
    pub fn with_titles(
        segments: &'a [TimelineSegment],
        transitions: &'a [TransitionPlan],
        titles: &'a [TitlePlan],
    ) -> Self {
        Self {
            segments,
            transitions,
            titles,
            broadcast_overlay: None,
            ass_workdir: None,
        }
    }

    /// Construct a planner over media segments, transitions, sparse
    /// title overlays, and an optional timeline-level broadcast overlay.
    pub fn with_titles_and_broadcast_overlay(
        segments: &'a [TimelineSegment],
        transitions: &'a [TransitionPlan],
        titles: &'a [TitlePlan],
        broadcast_overlay: Option<&'a BroadcastOverlayPlan>,
    ) -> Self {
        Self {
            segments,
            transitions,
            titles,
            broadcast_overlay,
            ass_workdir: None,
        }
    }

    /// Opt eligible captions into the ASS / libass burn-in path.
    /// `.ass` files for word-timed captions will be written under
    /// `workdir` and referenced from the filter graph via
    /// `subtitles=`. Titles that aren't libass-eligible (no word
    /// timings, or non-caption role) keep the drawtext path.
    pub(crate) fn with_ass_workdir(mut self, workdir: PathBuf) -> Self {
        self.ass_workdir = Some(workdir);
        self
    }

    /// Build the filter complex + output labels.
    ///
    /// With no transitions: emits the same monolithic
    /// `[0:v:0][0:a:0]…concat=n=N:v=1:a=1[outv][outa]` graph the
    /// pre-extract code produced.
    ///
    /// With transitions: builds maximal runs of consecutive segments
    /// connected by transition edges. Each run emits a chained
    /// `xfade` + `acrossfade` graph. Hard-cut boundaries between runs
    /// still feed into a final concat.
    pub fn plan(&self) -> FilterPlan {
        let base = if self.transitions.is_empty() {
            self.plan_no_transitions()
        } else {
            self.plan_with_transitions()
        };
        let base = if let Some(overlay) = self.broadcast_overlay {
            self.append_broadcast_overlay(base, overlay)
        } else {
            base
        };
        if self.titles.is_empty() || broadcast_overlay_owns_program_titles(self.broadcast_overlay) {
            base
        } else {
            self.append_titles(base)
        }
    }

    /// Decorate a video-only graph with broadcast overlay and titles.
    /// Used by explicit-audio renders, where audio is mixed separately.
    pub fn decorate_video_filter(
        &self,
        filter_complex: String,
        video_out_label: String,
    ) -> FilterPlan {
        let mut base = FilterPlan {
            filter_complex,
            video_out_label,
            audio_out_label: String::new(),
        };
        if let Some(overlay) = self.broadcast_overlay {
            base = self.append_broadcast_overlay(base, overlay);
        }
        if !self.titles.is_empty() && !broadcast_overlay_owns_program_titles(self.broadcast_overlay)
        {
            base = self.append_titles(base);
        }
        base
    }

    /// Splice a `drawtext=` chain onto `base.video_out_label` and
    /// rename the master video output to `[outv]` afterwards. Audio
    /// passes through untouched. The `enable='between(t,start,end)'`
    /// expression on each drawtext bounds the title to its window so
    /// concurrent titles all live in the same chain without
    /// interfering.
    fn append_titles(&self, base: FilterPlan) -> FilterPlan {
        // Pick a stable intermediate label. The base already produced
        // [outv] / [outa]; we rename [outv] → [base_v] inside the
        // filter_complex by appending a drawtext chain that consumes
        // [base_v] and produces a fresh [titled_v]. We then expose
        // [titled_v] as the new video_out_label.
        //
        // Strategy: don't try to rename the existing label (we'd have
        // to rewrite the filter graph); instead, take the base's
        // video_out_label as our INPUT and produce a new output.
        let in_label = base.video_out_label.clone();
        let out_label = "[titled_v]".to_string();

        let mut filter = base.filter_complex.clone();
        filter.push(';');
        filter.push_str(&in_label);
        // Comma-separate the title filters so they all run on the
        // same input → single output. drawtext's `enable=` keeps each
        // bounded to its window without cross-contamination, and
        // libass `subtitles=` carries its own per-event start/end.
        let parts: Vec<String> = self
            .titles
            .iter()
            .enumerate()
            .map(|(index, title)| self.format_title_filter(title, index))
            .collect();
        filter.push_str(&parts.join(","));
        filter.push_str(&out_label);

        FilterPlan {
            filter_complex: filter,
            video_out_label: out_label,
            audio_out_label: base.audio_out_label,
        }
    }

    /// Pick the right rendering substrate for one title:
    ///   - libass `subtitles=` when [`crate::ass::is_libass_eligible`]
    ///     is true and the planner has a workdir for ASS files
    ///   - drawtext otherwise
    ///
    /// Fail-soft: if writing the ASS file errors, fall back to the
    /// drawtext path rather than aborting the whole render.
    fn format_title_filter(&self, title: &TitlePlan, index: usize) -> String {
        if let Some(workdir) = self.ass_workdir.as_ref()
            && crate::ass::is_libass_eligible(title)
        {
            match crate::ass::render_ass_file(title, workdir, index) {
                Ok(path) => return crate::ass::ffmpeg_subtitles_filter_arg(&path),
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        workdir = %workdir.display(),
                        "FilterPlanner: ASS write failed; falling back to drawtext",
                    );
                }
            }
        }
        format_drawtext_filter(title, self.broadcast_overlay)
    }

    fn append_broadcast_overlay(
        &self,
        base: FilterPlan,
        overlay: &BroadcastOverlayPlan,
    ) -> FilterPlan {
        if !overlay.config.enabled {
            return base;
        }
        let in_label = base.video_out_label.clone();
        let out_label = "[broadcast_v]".to_string();
        let parts = format_broadcast_overlay_filters(overlay);
        if parts.is_empty() {
            return base;
        }
        let mut filter = base.filter_complex.clone();
        filter.push(';');
        filter.push_str(&format_broadcast_overlay_graph(
            &in_label, &out_label, overlay, &parts,
        ));
        FilterPlan {
            filter_complex: filter,
            video_out_label: out_label,
            audio_out_label: base.audio_out_label,
        }
    }

    fn plan_no_transitions(&self) -> FilterPlan {
        let n = self.segments.len();
        let mut filter = String::new();
        // Pre-stage so per-segment effects (volume in 15.3, speed in
        // 15.4) prepend their filter chain before the concat. Each
        // call returns the (video, audio) labels to feed into concat.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();
        let inputs: Vec<(String, String)> = inputs
            .into_iter()
            .enumerate()
            .map(|(i, (video, audio))| {
                let audio = apply_hard_cut_audio_fade(
                    &mut filter,
                    &audio,
                    &format!("[bfade{i}]"),
                    effective_duration(&self.segments[i]),
                    i > 0,
                    i + 1 < n,
                );
                (video, audio)
            })
            .collect();
        for (v, a) in &inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!("concat=n={n}:v=1:a=1[outv][outa]"));
        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }

    fn plan_with_transitions(&self) -> FilterPlan {
        let n = self.segments.len();
        let transition_after = transition_edge_map(n, self.transitions);

        let mut filter = String::new();

        // Pre-stage each segment's video / audio inputs. When a
        // segment carries an awidat.volume effect, its audio stream
        // goes through `volume=<v>` first; downstream nodes use the
        // resulting [av<i>] label in place of [i:a:0]. Speed runs
        // through a parallel setpts pass on video + atempo on audio.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();
        let inputs: Vec<(String, String)> = inputs
            .into_iter()
            .enumerate()
            .map(|(i, (video, audio))| reset_segment_pts(&mut filter, i, &video, &audio))
            .collect();

        // Track the order of hard-cut groups. A group may be one raw
        // segment or a chained transition run.
        let mut concat_inputs: Vec<(String, String, f64)> = Vec::with_capacity(n);

        let mut i = 0;
        let mut transition_id: usize = 0;
        while i < n {
            let mut current_v = inputs[i].0.clone();
            let mut current_a = inputs[i].1.clone();
            let mut current_duration = effective_duration(&self.segments[i]);
            let mut group_end = i;

            while group_end + 1 < n {
                let Some(t) = transition_after[group_end] else {
                    break;
                };
                let next = group_end + 1;
                let v_label = format!("[xv{transition_id}]");
                let a_label = format!("[xa{transition_id}]");
                let xfade_kind = map_transition_plan_kind(t);
                // xfade offset is relative to the first input of the
                // current operation. For a chain, that first input is
                // the already-shortened output of previous xfades.
                let offset = (current_duration - t.duration_s).max(0.0);
                // If the transition's composition carries a TimeRemap
                // primitive, retime the outgoing tail and incoming
                // head through `setpts` before feeding them to xfade.
                // This is what makes `awidat.ramp_in_beat` actually
                // feel like a speed ramp instead of just a flash —
                // the xfade fallback covers the visual blend, the
                // setpts covers the speed curve.
                let next_v = inputs[next].0.clone();
                let (from_v_retimed, to_v_retimed) = apply_time_remap_setpts_stage(
                    &mut filter,
                    transition_id,
                    t,
                    &current_v,
                    &next_v,
                    current_duration,
                );
                filter.push_str(&format!(
                    "{from_v}{to_v}xfade=transition={kind}:duration={dur}:offset={off}{out};",
                    from_v = from_v_retimed,
                    to_v = to_v_retimed,
                    kind = xfade_kind,
                    dur = t.duration_s,
                    off = offset,
                    out = v_label,
                ));
                filter.push_str(&format_audio_transition_filter(
                    &current_a,
                    &inputs[next].1,
                    &a_label,
                    t,
                ));
                current_v = v_label;
                current_a = a_label;
                current_duration =
                    current_duration + effective_duration(&self.segments[next]) - t.duration_s;
                transition_id += 1;
                group_end = next;
            }
            concat_inputs.push((current_v, current_a, current_duration));
            i = group_end + 1;
        }
        let concat_len = concat_inputs.len();
        let concat_inputs: Vec<(String, String)> = concat_inputs
            .into_iter()
            .enumerate()
            .map(|(i, (video, audio, duration))| {
                let audio = apply_hard_cut_audio_fade(
                    &mut filter,
                    &audio,
                    &format!("[hfade{i}]"),
                    duration,
                    i > 0,
                    i + 1 < concat_len,
                );
                (video, audio)
            })
            .collect();

        // Tail: single-input concat would just rename, so when we
        // have one chunk it might be a paired-xfade output already.
        // ffmpeg's concat takes n>=1; we always wrap so the caller's
        // `-map [outv] -map [outa]` is uniform.
        for (v, a) in &concat_inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!(
            "concat=n={n}:v=1:a=1[outv][outa]",
            n = concat_inputs.len(),
        ));

        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }
}

fn reset_segment_pts(
    filter: &mut String,
    i: usize,
    video_label: &str,
    audio_label: &str,
) -> (String, String) {
    let video_out = format!("[vpts{i}]");
    let audio_out = format!("[apts{i}]");
    filter.push_str(&format!(
        "{video_label}setpts=PTS-STARTPTS,fps=30000/1001{video_out};"
    ));
    filter.push_str(&format!("{audio_label}asetpts=PTS-STARTPTS{audio_out};"));
    (video_out, audio_out)
}

/// Insert `setpts` retime stages for the outgoing tail and incoming
/// head when a transition's composition recipe contains a TimeRemap
/// primitive. Returns the labels the surrounding xfade emission
/// should consume. When the composition has no TimeRemap (or the
/// curve resolves to identity), passes the input labels through
/// unchanged so the existing xfade emission stays byte-identical
/// for non-speed-ramp transitions.
///
/// The outgoing tail starts at `outgoing_duration - t.duration_s`
/// inside the staged outgoing stream; the incoming head starts at
/// `0` inside the staged incoming stream. Frames outside the window
/// pass through at identity so xfade still reads correctly-timed
/// surrounding frames.
fn apply_time_remap_setpts_stage(
    filter: &mut String,
    transition_id: usize,
    transition: &TransitionPlan,
    from_label: &str,
    to_label: &str,
    outgoing_duration_s: f64,
) -> (String, String) {
    let Some(composition) = transition.composition.as_ref() else {
        return (from_label.to_string(), to_label.to_string());
    };
    let Some(curve) = transitions::extract_time_remap_setpts(composition, transition.duration_s)
    else {
        return (from_label.to_string(), to_label.to_string());
    };
    // Skip when the warped curve never diverges meaningfully from
    // realtime — emitting setpts would add filter graph cost without
    // changing playback.
    if curve.is_identity(1e-6) {
        return (from_label.to_string(), to_label.to_string());
    }

    let from_window_start = (outgoing_duration_s - transition.duration_s).max(0.0);
    let from_expr = time_remap_transition_setpts_expr(&curve, from_window_start);
    let from_out = format!("[xtrv{transition_id}o]");
    filter.push_str(&format!("{from_label}setpts='{from_expr}/TB'{from_out};"));

    let to_expr = time_remap_transition_setpts_expr(&curve, 0.0);
    let to_out = format!("[xtrv{transition_id}i]");
    filter.push_str(&format!("{to_label}setpts='{to_expr}/TB'{to_out};"));

    (from_out, to_out)
}

fn apply_hard_cut_audio_fade(
    filter: &mut String,
    audio_label: &str,
    out_label: &str,
    duration_s: f64,
    fade_in: bool,
    fade_out: bool,
) -> String {
    if !fade_in && !fade_out {
        return audio_label.to_string();
    }
    let fade_s = HARD_CUT_AUDIO_FADE_S.min((duration_s / 2.0).max(0.0));
    if fade_s <= 0.0 {
        return audio_label.to_string();
    }

    filter.push_str(audio_label);
    if fade_in {
        filter.push_str(&format!("afade=t=in:st=0:d={fade_s}"));
    }
    if fade_out {
        if fade_in {
            filter.push(',');
        }
        filter.push_str(&format!(
            "afade=t=out:st={start_s}:d={fade_s}",
            start_s = (duration_s - fade_s).max(0.0),
        ));
    }
    filter.push_str(out_label);
    filter.push(';');
    out_label.to_string()
}

fn format_audio_transition_filter(
    from_a: &str,
    to_a: &str,
    out: &str,
    transition: &TransitionPlan,
) -> String {
    match transitions::resolve_audio_policy(&transition.kind)
        .unwrap_or(transitions::TransitionAudioPolicy::Crossfade)
    {
        transitions::TransitionAudioPolicy::Crossfade => format!(
            "{from_a}{to_a}acrossfade=d={dur}{out};",
            dur = transition.duration_s,
        ),
        transitions::TransitionAudioPolicy::Cut => format!(
            "{from_a}{to_a}acrossfade=d={dur}:c1=nofade:c2=nofade{out};",
            dur = transition.duration_s,
        ),
    }
}

fn append_video_overlays(
    base: FilterPlan,
    video_overlays: &[VideoOverlayPlan],
    first_overlay_input: usize,
) -> FilterPlan {
    if video_overlays.is_empty() {
        return base;
    }
    let mut filter = base.filter_complex;
    let mut current = base.video_out_label;
    for (idx, overlay) in video_overlays.iter().enumerate() {
        let input_idx = first_overlay_input + idx;
        let pts_label = format!("[media_overlay_pts{idx}]");
        let scaled_label = format!("[media_overlay_scaled{idx}]");
        let ref_label = format!("[media_overlay_ref{idx}]");
        let next = format!("[media_overlay_v{idx}]");
        let start = overlay.track_start_s;
        let end = overlay.track_start_s + effective_duration(&overlay.segment);
        let overlay_input = stage_overlay_video_input(&mut filter, input_idx, &overlay.segment);
        filter.push(';');
        filter.push_str(&format!(
            "{overlay_input}setpts=PTS-STARTPTS+{start}/TB{pts_label};"
        ));
        if let Some(overlay_filter) =
            motion_blurred_overlay_transform_filter(MotionBlurTransformContext {
                idx,
                overlay,
                source_label: &pts_label,
                base_label: &current,
                start,
                end,
                out_label: &next,
            })
        {
            filter.push_str(&overlay_filter);
            current = next;
            continue;
        }
        let rotation_deg = overlay_rotation_deg_expr(overlay, "t");
        let has_rotation = rotation_deg.is_some();
        let scale_expr = overlay_scale_expr(overlay, "t");
        let (x_expr, y_expr) = overlay_position_exprs(overlay, has_rotation, "t");
        let (rotation_filter, rotated_label) = if let Some(rotation_deg) = rotation_deg {
            let rotated_label = format!("[media_overlay_rot{idx}]");
            let filter = format!(
                "{scaled_label}format=rgba,rotate='(PI/180)*({rotation_deg})':ow='hypot(iw\\,ih)':oh='hypot(iw\\,ih)':c=none{rotated_label};"
            );
            (filter, rotated_label)
        } else {
            (String::new(), scaled_label.clone())
        };
        let (corner_pin_filter, transformed_label) =
            apply_overlay_corner_pin_filter(overlay, idx, &rotated_label);
        let (mask_filter, transformed_label) =
            apply_overlay_mask_filter(overlay, idx, &transformed_label);
        let (matte_filter, transformed_label) =
            apply_overlay_matte_filter(overlay, idx, &transformed_label);
        let (blur_filter, transformed_label) =
            apply_overlay_blur_filter(overlay, idx, &transformed_label);
        let opacity_filter = if has_overlay_animation(overlay, "overlay.opacity") {
            let opacity = overlay_animation_value_expr(overlay, "overlay.opacity", "1", "T");
            let alpha_label = format!("[media_overlay_alpha{idx}]");
            let filter = format!(
                "{transformed_label}format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*({opacity})'{alpha_label};"
            );
            (filter, alpha_label)
        } else {
            (String::new(), transformed_label)
        };
        let overlay_filter = if let (VideoOverlayMode::FullFrame, Some(blend_mode)) =
            (&overlay.mode, overlay.segment.overlay_blend_mode.as_deref())
        {
            format!(
                "{ref_label}{overlay_video_label}blend=all_mode={blend_mode}:all_opacity=1:enable='between(t\\,{start}\\,{end})'{next}",
                overlay_video_label = opacity_filter.1,
            )
        } else {
            format!(
                "{ref_label}{overlay_video_label}overlay=x={x_expr}:y={y_expr}:enable='between(t\\,{start}\\,{end})'{next}",
                overlay_video_label = opacity_filter.1,
            )
        };
        filter.push_str(&format!(
            "{pts_label}{current}scale2ref={scale_expr}{scaled_label}{ref_label};\
             {rotation_filter}\
             {corner_pin_filter}\
             {mask_filter}\
             {matte_filter}\
             {blur_filter}\
             {opacity_filter}\
             {overlay_filter}",
            rotation_filter = rotation_filter,
            corner_pin_filter = corner_pin_filter,
            mask_filter = mask_filter,
            matte_filter = matte_filter,
            blur_filter = blur_filter,
            opacity_filter = opacity_filter.0,
        ));
        current = next;
    }
    FilterPlan {
        filter_complex: filter,
        video_out_label: current,
        audio_out_label: base.audio_out_label,
    }
}

fn append_annotations(base: FilterPlan, annotations: &[AnnotationPlan]) -> FilterPlan {
    if annotations.is_empty() {
        return base;
    }
    let mut filter = base.filter_complex;
    let mut current = base.video_out_label;
    for (idx, annotation) in annotations.iter().enumerate() {
        let next = format!("[annotation_v{idx}]");
        if !filter.ends_with(';') && !filter.is_empty() {
            filter.push(';');
        }
        filter.push_str(&annotation_filter(&current, &next, annotation, idx));
        current = next;
    }
    FilterPlan {
        filter_complex: filter,
        video_out_label: current,
        audio_out_label: base.audio_out_label,
    }
}

fn annotation_filter(
    in_label: &str,
    out_label: &str,
    annotation: &AnnotationPlan,
    idx: usize,
) -> String {
    match annotation.kind {
        AnnotationKind::Rectangle => rectangle_annotation_filter(in_label, out_label, annotation),
        AnnotationKind::Blur => blur_annotation_filter(in_label, out_label, annotation, idx),
        AnnotationKind::Circle => circle_annotation_filter(in_label, out_label, annotation),
        AnnotationKind::Arrow => arrow_annotation_filter(in_label, out_label, annotation),
        AnnotationKind::Bracket => bracket_annotation_filter(in_label, out_label, annotation),
    }
}

fn rectangle_annotation_filter(
    in_label: &str,
    out_label: &str,
    annotation: &AnnotationPlan,
) -> String {
    let color = annotation_filter_color(&annotation.color);
    format!(
        "{in_label}drawbox=x=iw*{x}:y=ih*{y}:w=iw*{width}:h=ih*{height}:color={color}:t={stroke}:enable='between(t\\,{start}\\,{end})'{out_label}",
        x = fmt_filter_num(annotation.x),
        y = fmt_filter_num(annotation.y),
        width = fmt_filter_num(annotation.width),
        height = fmt_filter_num(annotation.height),
        color = color,
        stroke = annotation.stroke_width,
        start = fmt_filter_num(annotation.start_s),
        end = fmt_filter_num(annotation.end_s),
    )
}

fn blur_annotation_filter(
    in_label: &str,
    out_label: &str,
    annotation: &AnnotationPlan,
    idx: usize,
) -> String {
    let keep = format!("[annotation_keep{idx}]");
    let source = format!("[annotation_src{idx}]");
    let blur = format!("[annotation_blur{idx}]");
    format!(
        "{in_label}split{keep}{source};\
         {source}crop=w=iw*{width}:h=ih*{height}:x=iw*{x}:y=ih*{y},boxblur=12{blur};\
         {keep}{blur}overlay=x=main_w*{x}:y=main_h*{y}:enable='between(t\\,{start}\\,{end})'{out_label}",
        x = fmt_filter_num(annotation.x),
        y = fmt_filter_num(annotation.y),
        width = fmt_filter_num(annotation.width),
        height = fmt_filter_num(annotation.height),
        start = fmt_filter_num(annotation.start_s),
        end = fmt_filter_num(annotation.end_s),
    )
}

fn circle_annotation_filter(
    in_label: &str,
    out_label: &str,
    annotation: &AnnotationPlan,
) -> String {
    let size = format!("h*{}", fmt_filter_num(annotation.height));
    let color = annotation_filter_color(&annotation.color);
    format!(
        "{in_label}drawtext=text='○':fontsize={size}:fontcolor={color}:x=w*{x}:y=h*{y}:enable='between(t\\,{start}\\,{end})'{out_label}",
        color = color,
        x = fmt_filter_num(annotation.x),
        y = fmt_filter_num(annotation.y),
        start = fmt_filter_num(annotation.start_s),
        end = fmt_filter_num(annotation.end_s),
    )
}

fn arrow_annotation_filter(in_label: &str, out_label: &str, annotation: &AnnotationPlan) -> String {
    let size = format!("h*{}", fmt_filter_num(annotation.height.max(0.04)));
    let color = annotation_filter_color(&annotation.color);
    format!(
        "{in_label}drawtext=text='→':fontsize={size}:fontcolor={color}:x=w*{x}:y=h*{y}:enable='between(t\\,{start}\\,{end})'{out_label}",
        color = color,
        x = fmt_filter_num(annotation.x),
        y = fmt_filter_num(annotation.y),
        start = fmt_filter_num(annotation.start_s),
        end = fmt_filter_num(annotation.end_s),
    )
}

fn bracket_annotation_filter(
    in_label: &str,
    out_label: &str,
    annotation: &AnnotationPlan,
) -> String {
    let color = annotation_filter_color(&annotation.color);
    let top = format!(
        "[annotation_bracket_top{}]",
        annotation_label_suffix(annotation)
    );
    let left = format!(
        "[annotation_bracket_left{}]",
        annotation_label_suffix(annotation)
    );
    let right = format!(
        "[annotation_bracket_right{}]",
        annotation_label_suffix(annotation)
    );
    let common = format!(
        ":color={}:t={}:enable='between(t\\,{}\\,{})'",
        color,
        annotation.stroke_width,
        fmt_filter_num(annotation.start_s),
        fmt_filter_num(annotation.end_s)
    );
    format!(
        "{in_label}drawbox=x=iw*{x}:y=ih*{y}:w=iw*{width}:h=1{common}{top};\
         {top}drawbox=x=iw*{x}:y=ih*{y}:w=1:h=ih*{height}{common}{left};\
         {left}drawbox=x=iw*({x}+{width}):y=ih*{y}:w=1:h=ih*{height}{common}{right};\
         {right}copy{out_label}",
        x = fmt_filter_num(annotation.x),
        y = fmt_filter_num(annotation.y),
        width = fmt_filter_num(annotation.width),
        height = fmt_filter_num(annotation.height),
        common = common,
    )
}

fn annotation_filter_color(raw: &str) -> &str {
    if valid_filter_color(raw) {
        raw
    } else {
        "#FFCC00"
    }
}

fn valid_filter_color(raw: &str) -> bool {
    let color = raw.trim();
    if color.is_empty() || color != raw {
        return false;
    }
    if let Some(hex) = color.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit());
    }
    color.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn annotation_label_suffix(annotation: &AnnotationPlan) -> String {
    format!(
        "_{:.0}_{:.0}",
        annotation.start_s * 1000.0,
        annotation.end_s * 1000.0
    )
}

fn apply_overlay_corner_pin_filter(
    overlay: &VideoOverlayPlan,
    idx: usize,
    input_label: &str,
) -> (String, String) {
    let Some(filter_expr) = overlay.corner_pin_filter.as_ref() else {
        return (String::new(), input_label.to_string());
    };
    let corner_pin_label = format!("[media_overlay_corner_pin{idx}]");
    (
        format!("{input_label}{filter_expr}{corner_pin_label};"),
        corner_pin_label,
    )
}

fn apply_overlay_matte_filter(
    overlay: &VideoOverlayPlan,
    idx: usize,
    input_label: &str,
) -> (String, String) {
    let Some(matte_input_index) = overlay.matte_input_index else {
        return (String::new(), input_label.to_string());
    };
    let matte_rgba = format!("[media_overlay_matte_rgba{idx}]");
    let matte_scaled = format!("[media_overlay_matte_scaled{idx}]");
    let matte_ref = format!("[media_overlay_matte_ref{idx}]");
    let matte_label = format!("[media_overlay_matte{idx}]");
    let (matte_source_filter, matte_stream_label) = overlay_matte_source_filter(
        overlay,
        matte_input_index,
        "[media_overlay_matte_gray",
        idx,
        None,
    );
    (
        format!(
            "{input_label}format=rgba{matte_rgba};\
             {matte_source_filter}\
             {matte_stream_label}{matte_rgba}scale2ref=w=main_w:h=main_h{matte_scaled}{matte_ref};\
             {matte_ref}{matte_scaled}alphamerge=shortest=1{matte_label};"
        ),
        matte_label,
    )
}

fn overlay_matte_source_filter(
    overlay: &VideoOverlayPlan,
    matte_input_index: usize,
    gray_label_prefix: &str,
    overlay_idx: usize,
    sample_idx: Option<usize>,
) -> (String, String) {
    let gray_label = match sample_idx {
        Some(sample_idx) => format!("{gray_label_prefix}{overlay_idx}_{sample_idx}]"),
        None => format!("{gray_label_prefix}{overlay_idx}]"),
    };
    if overlay_matte_source_is_still(overlay) {
        let loop_label = match sample_idx {
            Some(sample_idx) => format!("[media_overlay_blur{overlay_idx}_matte_loop{sample_idx}]"),
            None => format!("[media_overlay_matte_loop{overlay_idx}]"),
        };
        (
            format!("[{matte_input_index}:v:0]format=gray,loop=-1:1{loop_label};"),
            loop_label,
        )
    } else {
        (
            format!("[{matte_input_index}:v:0]format=gray{gray_label};"),
            gray_label,
        )
    }
}

fn overlay_matte_source_is_still(overlay: &VideoOverlayPlan) -> bool {
    overlay
        .matte_source
        .as_ref()
        .and_then(|path| path.extension())
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "tif" | "tiff"
            )
        })
        .unwrap_or(true)
}

fn apply_overlay_mask_filter(
    overlay: &VideoOverlayPlan,
    idx: usize,
    input_label: &str,
) -> (String, String) {
    let Some(mask) = overlay.mask.as_ref() else {
        return (String::new(), input_label.to_string());
    };
    let mask_label = format!("[media_overlay_mask{idx}]");
    let alpha_expr = overlay_mask_alpha_expr(mask, &format!("(T-{})", overlay.track_start_s));
    (
        format!(
            "{input_label}format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{alpha_expr}'{mask_label};"
        ),
        mask_label,
    )
}

fn overlay_mask_alpha_expr(mask: &OverlayMaskPlan, time_var: &str) -> String {
    let x0 = overlay_mask_keyframe_value_expr(mask, |keyframe| keyframe.x0, time_var);
    let y0 = overlay_mask_keyframe_value_expr(mask, |keyframe| keyframe.y0, time_var);
    let x1 = overlay_mask_keyframe_value_expr(mask, |keyframe| keyframe.x1, time_var);
    let y1 = overlay_mask_keyframe_value_expr(mask, |keyframe| keyframe.y1, time_var);
    let opacity = overlay_mask_keyframe_value_expr(mask, |keyframe| keyframe.opacity, time_var);
    let inside = format!("between(X\\,W*{x0}\\,W*{x1})*between(Y\\,H*{y0}\\,H*{y1})");
    match mask.operation {
        MaskOperation::Add | MaskOperation::Intersect => {
            format!("if({inside}\\,{opacity}\\,0)")
        }
        MaskOperation::Subtract => format!("if({inside}\\,0\\,1)"),
    }
}

fn overlay_mask_keyframe_value_expr(
    mask: &OverlayMaskPlan,
    value: fn(&OverlayMaskKeyframe) -> f64,
    time_var: &str,
) -> String {
    let keyframes = mask
        .keyframes
        .iter()
        .map(|keyframe| {
            awidat_proto::professional::Keyframe::linear(keyframe.time_s, value(keyframe))
        })
        .collect::<Vec<_>>();
    let expr = keyframes_to_ffmpeg_expr_with_extrapolation(
        &keyframes,
        time_var,
        awidat_proto::professional::ExtrapolationMode::Hold,
        awidat_proto::professional::ExtrapolationMode::Hold,
    );
    if keyframes.len() == 1 {
        fmt_filter_num(value(&mask.keyframes[0]))
    } else {
        expr
    }
}

fn overlay_scale_expr(overlay: &VideoOverlayPlan, time_var: &str) -> String {
    let scale_multiplier = overlay_animation_value_expr(overlay, "overlay.scale", "1", time_var);
    match &overlay.mode {
        VideoOverlayMode::FullFrame => {
            if has_overlay_animation(overlay, "overlay.scale") {
                format!("w=main_w*({scale_multiplier}):h=main_h*({scale_multiplier}):eval=frame")
            } else {
                "w=main_w:h=main_h".to_string()
            }
        }
        VideoOverlayMode::PiP { scale, .. } => {
            if has_overlay_animation(overlay, "overlay.scale") {
                format!("w=main_w*{scale}*({scale_multiplier}):h=-2:eval=frame")
            } else {
                format!("w=main_w*{scale}:h=-2")
            }
        }
    }
}

fn overlay_position_exprs(
    overlay: &VideoOverlayPlan,
    has_rotation: bool,
    time_var: &str,
) -> (String, String) {
    let (base_x, base_y) = match &overlay.mode {
        VideoOverlayMode::FullFrame => {
            if has_rotation {
                (
                    "(main_w-overlay_w)/2".to_string(),
                    "(main_h-overlay_h)/2".to_string(),
                )
            } else {
                ("0".to_string(), "0".to_string())
            }
        }
        VideoOverlayMode::PiP {
            corner, margin_pct, ..
        } => {
            let margin_x = format!("main_w*{margin_pct}");
            let margin_y = format!("main_h*{margin_pct}");
            let x = match corner.as_str() {
                "top_left" | "bottom_left" => margin_x,
                _ => format!("main_w-overlay_w-main_w*{margin_pct}"),
            };
            let y = match corner.as_str() {
                "top_left" | "top_right" => margin_y,
                _ => format!("main_h-overlay_h-main_h*{margin_pct}"),
            };
            (x, y)
        }
    };
    let x_expr = if has_overlay_animation(overlay, "overlay.position") {
        format!(
            "({base_x})+main_w*({})",
            overlay_motion_path_expr(overlay, MotionPathCoordinate::X, time_var)
                .unwrap_or_else(|| "0".to_string())
        )
    } else if has_overlay_animation(overlay, "overlay.x") {
        format!(
            "({base_x})+main_w*({})",
            overlay_animation_value_expr(overlay, "overlay.x", "0", time_var)
        )
    } else {
        base_x
    };
    let y_expr = if has_overlay_animation(overlay, "overlay.position") {
        format!(
            "({base_y})+main_h*({})",
            overlay_motion_path_expr(overlay, MotionPathCoordinate::Y, time_var)
                .unwrap_or_else(|| "0".to_string())
        )
    } else if has_overlay_animation(overlay, "overlay.y") {
        format!(
            "({base_y})+main_h*({})",
            overlay_animation_value_expr(overlay, "overlay.y", "0", time_var)
        )
    } else {
        base_y
    };
    (x_expr, y_expr)
}

struct MotionBlurTransformContext<'a> {
    idx: usize,
    overlay: &'a VideoOverlayPlan,
    source_label: &'a str,
    base_label: &'a str,
    start: f64,
    end: f64,
    out_label: &'a str,
}

fn motion_blurred_overlay_transform_filter(
    context: MotionBlurTransformContext<'_>,
) -> Option<String> {
    let blur = context.overlay.motion_blur.as_ref()?;
    if !has_overlay_transform_motion(context.overlay) {
        return None;
    }
    let half_shutter = (blur.shutter_s / 2.0).clamp(0.0, 0.1);
    if half_shutter <= f64::EPSILON {
        return None;
    }

    let sample_labels = [
        format!("[media_overlay_blur{}_src0]", context.idx),
        format!("[media_overlay_blur{}_src1]", context.idx),
        format!("[media_overlay_blur{}_src2]", context.idx),
    ];
    let base_labels = [
        format!("[media_overlay_blur{}_base0]", context.idx),
        format!("[media_overlay_blur{}_base1]", context.idx),
        format!("[media_overlay_blur{}_base2]", context.idx),
    ];
    let scaled_labels = [
        format!("[media_overlay_blur{}_scaled0]", context.idx),
        format!("[media_overlay_blur{}_scaled1]", context.idx),
        format!("[media_overlay_blur{}_scaled2]", context.idx),
    ];
    let ref_labels = [
        format!("[media_overlay_blur{}_ref0]", context.idx),
        format!("[media_overlay_blur{}_ref1]", context.idx),
        format!("[media_overlay_blur{}_ref2]", context.idx),
    ];
    let rotated_labels = [
        format!("[media_overlay_blur{}_rot0]", context.idx),
        format!("[media_overlay_blur{}_rot1]", context.idx),
        format!("[media_overlay_blur{}_rot2]", context.idx),
    ];
    let alpha_labels = [
        format!("[media_overlay_blur{}_alpha0]", context.idx),
        format!("[media_overlay_blur{}_alpha1]", context.idx),
        format!("[media_overlay_blur{}_alpha2]", context.idx),
    ];
    let composite_labels = [
        format!("[media_overlay_blur{}_v0]", context.idx),
        format!("[media_overlay_blur{}_v1]", context.idx),
    ];
    let offsets = [-half_shutter, 0.0, half_shutter];
    let weight = fmt_filter_num(1.0 / offsets.len() as f64);
    let mut filter = format!(
        "{source_label}split=3{}{}{};{base_label}split=3{}{}{};",
        sample_labels[0],
        sample_labels[1],
        sample_labels[2],
        base_labels[0],
        base_labels[1],
        base_labels[2],
        source_label = context.source_label,
        base_label = context.base_label,
    );
    let mut current_base = String::new();

    for (sample_idx, offset) in offsets.into_iter().enumerate() {
        let sample_time_var = offset_time_var("t", offset);
        let sample_scale = overlay_scale_expr(context.overlay, &sample_time_var);
        filter.push_str(&format!(
            "{}{}scale2ref={sample_scale}{}{};",
            sample_labels[sample_idx],
            base_labels[sample_idx],
            scaled_labels[sample_idx],
            ref_labels[sample_idx],
        ));
        if sample_idx == 0 {
            current_base = ref_labels[sample_idx].clone();
        } else {
            filter.push_str(&format!("{}nullsink;", ref_labels[sample_idx]));
        }
        let rotation_deg = overlay_rotation_deg_expr(context.overlay, &sample_time_var);
        let has_rotation = rotation_deg.is_some();
        let overlay_video_label = if let Some(rotation_deg) = rotation_deg {
            filter.push_str(&format!(
                "{}format=rgba,rotate='(PI/180)*({rotation_deg})':ow='hypot(iw\\,ih)':oh='hypot(iw\\,ih)':c=none{};",
                scaled_labels[sample_idx],
                rotated_labels[sample_idx],
            ));
            rotated_labels[sample_idx].clone()
        } else {
            scaled_labels[sample_idx].clone()
        };
        let overlay_video_label = apply_overlay_sample_corner_pin_filter(
            context.overlay,
            context.idx,
            sample_idx,
            &mut filter,
            &overlay_video_label,
        );
        let overlay_video_label = apply_overlay_sample_mask_filter(
            context.overlay,
            context.idx,
            sample_idx,
            &mut filter,
            &overlay_video_label,
        );
        let overlay_video_label = apply_overlay_sample_matte_filter(
            context.overlay,
            context.idx,
            sample_idx,
            &mut filter,
            &overlay_video_label,
        );
        let blur_time_var = offset_time_var("T", offset);
        let overlay_video_label = apply_overlay_sample_blur_filter(
            context.overlay,
            context.idx,
            sample_idx,
            &mut filter,
            &overlay_video_label,
            &blur_time_var,
        );
        let (sample_x, sample_y) =
            overlay_position_exprs(context.overlay, has_rotation, &sample_time_var);
        let alpha_label = &alpha_labels[sample_idx];
        let alpha_expr = overlay_motion_blur_alpha_expr(context.overlay, offset, &weight);
        filter.push_str(&format!(
            "{overlay_video_label}format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='{alpha_expr}'{alpha_label};",
        ));
        let next_label = if sample_idx == offsets.len() - 1 {
            context.out_label.to_string()
        } else {
            composite_labels[sample_idx].clone()
        };
        filter.push_str(&format!(
            "{current_base}{alpha_label}overlay=x={sample_x}:y={sample_y}:enable='between(t\\,{start}\\,{end})'{next_label};",
            start = context.start,
            end = context.end,
        ));
        current_base = next_label;
    }
    Some(filter.trim_end_matches(';').to_string())
}

fn apply_overlay_sample_corner_pin_filter(
    overlay: &VideoOverlayPlan,
    overlay_idx: usize,
    sample_idx: usize,
    filter: &mut String,
    input_label: &str,
) -> String {
    let Some(filter_expr) = overlay.corner_pin_filter.as_ref() else {
        return input_label.to_string();
    };
    let corner_pin_label = format!("[media_overlay_blur{overlay_idx}_pin{sample_idx}]");
    filter.push_str(&format!("{input_label}{filter_expr}{corner_pin_label};"));
    corner_pin_label
}

fn apply_overlay_sample_matte_filter(
    overlay: &VideoOverlayPlan,
    overlay_idx: usize,
    sample_idx: usize,
    filter: &mut String,
    input_label: &str,
) -> String {
    let Some(matte_input_index) = overlay.matte_input_index else {
        return input_label.to_string();
    };
    let matte_rgba = format!("[media_overlay_blur{overlay_idx}_matte_rgba{sample_idx}]");
    let matte_scaled = format!("[media_overlay_blur{overlay_idx}_matte_scaled{sample_idx}]");
    let matte_ref = format!("[media_overlay_blur{overlay_idx}_matte_ref{sample_idx}]");
    let matte_label = format!("[media_overlay_blur{overlay_idx}_matte{sample_idx}]");
    let (matte_source_filter, matte_stream_label) = overlay_matte_source_filter(
        overlay,
        matte_input_index,
        "[media_overlay_blur_matte_gray",
        overlay_idx,
        Some(sample_idx),
    );
    filter.push_str(&format!(
        "{input_label}format=rgba{matte_rgba};\
         {matte_source_filter}\
         {matte_stream_label}{matte_rgba}scale2ref=w=main_w:h=main_h{matte_scaled}{matte_ref};\
         {matte_ref}{matte_scaled}alphamerge=shortest=1{matte_label};"
    ));
    matte_label
}

fn apply_overlay_sample_mask_filter(
    overlay: &VideoOverlayPlan,
    overlay_idx: usize,
    sample_idx: usize,
    filter: &mut String,
    input_label: &str,
) -> String {
    let Some(mask) = overlay.mask.as_ref() else {
        return input_label.to_string();
    };
    let mask_label = format!("[media_overlay_blur{overlay_idx}_mask{sample_idx}]");
    let alpha_expr = overlay_mask_alpha_expr(mask, &format!("(T-{})", overlay.track_start_s));
    filter.push_str(&format!(
        "{input_label}format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{alpha_expr}'{mask_label};"
    ));
    mask_label
}

fn overlay_motion_blur_alpha_expr(overlay: &VideoOverlayPlan, offset: f64, weight: &str) -> String {
    if has_overlay_animation(overlay, "overlay.opacity") {
        let opacity = overlay_animation_value_expr(
            overlay,
            "overlay.opacity",
            "1",
            &offset_time_var("T", offset),
        );
        format!("alpha(X,Y)*({opacity})*{weight}")
    } else {
        format!("alpha(X,Y)*{weight}")
    }
}

fn apply_overlay_blur_filter(
    overlay: &VideoOverlayPlan,
    idx: usize,
    input_label: &str,
) -> (String, String) {
    if !has_overlay_animation(overlay, "overlay.blur") {
        return (String::new(), input_label.to_string());
    }
    let video_label = format!("[media_overlay_blurvideo{idx}]");
    let radius_src_label = format!("[media_overlay_blurradiussrc{idx}]");
    let radius_label = format!("[media_overlay_blurradius{idx}]");
    let out_label = format!("[media_overlay_softblur{idx}]");
    let radius_expr = overlay_blur_radius_expr(overlay, "T");
    let radius = fmt_filter_num(OVERLAY_BLUR_MAX_RADIUS_PX);
    (
        format!(
            "{input_label}format=rgba,split=2{video_label}{radius_src_label};\
             {radius_src_label}format=gray,geq=lum='{radius_expr}'{radius_label};\
             {video_label}{radius_label}varblur=min_r=0:max_r={radius}:planes=15{out_label};"
        ),
        out_label,
    )
}

fn apply_overlay_sample_blur_filter(
    overlay: &VideoOverlayPlan,
    overlay_idx: usize,
    sample_idx: usize,
    filter: &mut String,
    input_label: &str,
    time_var: &str,
) -> String {
    if !has_overlay_animation(overlay, "overlay.blur") {
        return input_label.to_string();
    }
    let video_label = format!("[media_overlay_blur{overlay_idx}_video{sample_idx}]");
    let radius_src_label = format!("[media_overlay_blur{overlay_idx}_radiussrc{sample_idx}]");
    let radius_label = format!("[media_overlay_blur{overlay_idx}_radius{sample_idx}]");
    let out_label = format!("[media_overlay_blur{overlay_idx}_soft{sample_idx}]");
    let radius_expr = overlay_blur_radius_expr(overlay, time_var);
    let radius = fmt_filter_num(OVERLAY_BLUR_MAX_RADIUS_PX);
    filter.push_str(&format!(
        "{input_label}format=rgba,split=2{video_label}{radius_src_label};\
         {radius_src_label}format=gray,geq=lum='{radius_expr}'{radius_label};\
         {video_label}{radius_label}varblur=min_r=0:max_r={radius}:planes=15{out_label};"
    ));
    out_label
}

fn overlay_blur_radius_expr(overlay: &VideoOverlayPlan, time_var: &str) -> String {
    let blur_px = overlay_animation_value_expr(overlay, "overlay.blur", "0", time_var);
    format!(
        "min(max(({blur_px}),0),{})",
        fmt_filter_num(OVERLAY_BLUR_MAX_RADIUS_PX)
    )
}

fn has_overlay_transform_motion(overlay: &VideoOverlayPlan) -> bool {
    has_overlay_animation(overlay, "overlay.position")
        || has_overlay_animation(overlay, "overlay.x")
        || has_overlay_animation(overlay, "overlay.y")
        || has_overlay_animation(overlay, "overlay.scale")
        || has_overlay_animation(overlay, "overlay.rotation_deg")
}

fn offset_time_var(base: &str, offset_s: f64) -> String {
    if offset_s.abs() <= f64::EPSILON {
        return base.to_string();
    }
    if offset_s.is_sign_negative() {
        format!("{base}-{}", fmt_filter_num(offset_s.abs()))
    } else {
        format!("{base}+{}", fmt_filter_num(offset_s))
    }
}

fn overlay_motion_path_expr(
    overlay: &VideoOverlayPlan,
    coordinate: MotionPathCoordinate,
    time_var: &str,
) -> Option<String> {
    overlay
        .animations
        .iter()
        .find(|animation| animation.parameter == "overlay.position")
        .and_then(|animation| {
            let path = animation.motion_path.as_ref()?;
            let local_time_var = format!("({time_var}-{})", overlay.track_start_s);
            Some(motion_path_to_ffmpeg_expr_with_keyframes(
                path,
                &animation.keyframes,
                animation.pre_extrapolation,
                animation.post_extrapolation,
                coordinate,
                &local_time_var,
            ))
        })
}

fn has_overlay_animation(overlay: &VideoOverlayPlan, parameter: &str) -> bool {
    overlay
        .animations
        .iter()
        .any(|animation| animation.parameter == parameter)
}

fn overlay_rotation_deg_expr(overlay: &VideoOverlayPlan, time_var: &str) -> Option<String> {
    if has_overlay_animation(overlay, "overlay.rotation_deg") {
        return Some(overlay_animation_value_expr(
            overlay,
            "overlay.rotation_deg",
            "0",
            time_var,
        ));
    }
    if overlay.rotation_deg.abs() > f64::EPSILON {
        Some(fmt_filter_num(overlay.rotation_deg))
    } else {
        None
    }
}

fn overlay_animation_value_expr(
    overlay: &VideoOverlayPlan,
    parameter: &str,
    fallback: &str,
    time_var: &str,
) -> String {
    overlay
        .animations
        .iter()
        .find(|animation| animation.parameter == parameter)
        .map(|animation| {
            let local_time_var = format!("({time_var}-{})", overlay.track_start_s);
            keyframes_to_ffmpeg_expr_with_extrapolation(
                &animation.keyframes,
                &local_time_var,
                animation.pre_extrapolation,
                animation.post_extrapolation,
            )
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn append_timeline_loudness_filter(
    filter: &mut String,
    audio_label: String,
    loudness_target: Option<LoudnessTargetPlan>,
) -> String {
    let Some(target) = loudness_target else {
        return audio_label;
    };
    if !target.integrated_lufs.is_finite() || target.integrated_lufs >= 0.0 {
        return audio_label;
    }
    let true_peak_db = target
        .true_peak_db
        .filter(|v| v.is_finite() && *v <= 0.0)
        .unwrap_or(-1.5);
    let out = "[mastera]".to_string();
    if !filter.ends_with(';') {
        filter.push(';');
    }
    filter.push_str(&format!(
        "{audio_label}aresample=async=1:first_pts=0,loudnorm=I={}:TP={}:LRA=11{out}",
        fmt_filter_num(target.integrated_lufs),
        fmt_filter_num(true_peak_db),
    ));
    out
}

fn hdr_tonemap_filter(transfer: HdrTransfer) -> &'static str {
    match transfer {
        HdrTransfer::Smpte2084 => {
            "zscale=t=linear:npl=100,format=gbrpf32le,tonemap=tonemap=hable:desat=0,zscale=transfer=bt709:matrix=bt709:primaries=bt709,format=yuv420p"
        }
        HdrTransfer::AribStdB67 => {
            "zscale=t=linear:npl=100,format=gbrpf32le,tonemap=tonemap=hable:desat=0,zscale=transfer=bt709:matrix=bt709:primaries=bt709,format=yuv420p"
        }
    }
}

fn stage_overlay_video_input(
    filter: &mut String,
    input_idx: usize,
    seg: &TimelineSegment,
) -> String {
    let mut video_label = format!("[{input_idx}:v:0]");
    if let Some(transfer) = seg.hdr_transfer {
        let hv = format!("[media_overlay_hdr{input_idx}]");
        filter.push(';');
        filter.push_str(&format!(
            "{video_label}{}{hv}",
            hdr_tonemap_filter(transfer)
        ));
        video_label = hv;
    }
    if let Some(color) = seg.color_correction.as_ref()
        && let Some(chain) = color_filter_chain(color)
    {
        let cv = format!("[media_overlay_cv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{chain}{cv}"));
        video_label = cv;
    }
    let blurred_label = append_clip_blur_filter(
        filter,
        &video_label,
        &format!("media_overlay{input_idx}"),
        seg.blur,
        seg.blur_radius_animation.as_ref(),
    );
    if blurred_label != video_label {
        video_label = blurred_label;
    }
    if let Some(plan) = seg.color_pipeline.as_ref() {
        if plan.has_any_lut() {
            let cv = format!("[media_overlay_cpv{input_idx}]");
            filter.push(';');
            filter.push_str(&color_pipeline_filter_block(
                &video_label,
                &cv,
                &format!("media_overlay_{input_idx}"),
                plan,
            ));
            video_label = cv;
        }
    } else if let Some(lut_path) = seg.lut_path.as_ref() {
        let lv = format!("[media_overlay_lv{input_idx}]");
        filter.push(';');
        filter.push_str(&lut3d_filter_block(
            &video_label,
            &lv,
            &format!("media_overlay_{input_idx}"),
            lut_path,
            seg.lut_interpolation.as_deref(),
            seg.lut_strength,
        ));
        video_label = lv;
    }
    if let Some(chain) = segment_reframe_filter_chain(seg) {
        let rv = format!("[media_overlay_rv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{chain}{rv}"));
        video_label = rv;
    }
    if let Some(warp) = seg.warp.as_ref()
        && let Some(chain) = warp_filter_chain(warp, seg.warp_animation.as_ref(), input_idx)
    {
        let wv = format!("[media_overlay_wv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{chain}{wv}"));
        video_label = wv;
    }
    if let Some(shake) = seg.shake.as_ref()
        && let Some(chain) = animated_shake_filter_chain(
            shake,
            seg.shake_intensity_animation.as_ref(),
            seg.shake_frequency_animation.as_ref(),
        )
    {
        let sv = format!("[media_overlay_shv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{chain}{sv}"));
        video_label = sv;
    }
    if let Some(time_remap) = seg.time_remap.as_ref() {
        let sv = format!("[media_overlay_trv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!(
            "{video_label}setpts='{expr}/TB'{sv}",
            expr = time_remap_setpts_expr(time_remap),
        ));
        video_label = sv;
    } else if let Some(speed) = seg.speed
        && speed_needs_filter(speed)
    {
        let sv = format!("[media_overlay_sv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{}{sv}", speed_video_filter(speed)));
        video_label = sv;
    }
    video_label
}

/// Build the per-segment video / audio entry labels into `filter`.
/// Threads clip-level graph effects in front of the raw stream so
/// downstream filter graph nodes (concat, xfade) read the post-effect
/// labels.
///
/// Order matters: color correction and LUTs run before speed so
/// visual transforms operate in source-frame space. For audio,
/// `atempo` runs before `volume` so the volume gain applies to the
/// time-stretched signal.
///
/// Returns the `(video_label, audio_label)` pair to feed into the
/// next filter graph node.
fn stage_segment_inputs(filter: &mut String, i: usize, seg: &TimelineSegment) -> (String, String) {
    let mut video_label = format!("[{i}:v:0]");
    let mut audio_label = format!("[{i}:a:0]");

    if let Some(transfer) = seg.hdr_transfer {
        let hv = format!("[hdr{i}]");
        filter.push_str(&format!(
            "{video_label}{}{hv};",
            hdr_tonemap_filter(transfer)
        ));
        video_label = hv;
    }

    if let Some(color) = seg.color_correction.as_ref()
        && let Some(chain) = color_filter_chain(color)
    {
        let cv = format!("[cv{i}]");
        filter.push_str(&format!("{video_label}{chain}{cv};"));
        video_label = cv;
    }
    let blurred_label = append_clip_blur_filter(
        filter,
        &video_label,
        &i.to_string(),
        seg.blur,
        seg.blur_radius_animation.as_ref(),
    );
    if blurred_label != video_label {
        video_label = blurred_label;
    }
    if let Some(chroma_key) = seg.chroma_key.as_ref() {
        let kv = format!("[ckv{i}]");
        filter.push_str(&format!(
            "{video_label}{}{kv};",
            chroma_key_filter_chain(chroma_key)
        ));
        video_label = kv;
    }
    if let Some(luma_key) = seg.luma_key {
        let kv = format!("[lkv{i}]");
        filter.push_str(&format!(
            "{video_label}{}{kv};",
            luma_key_filter_chain(luma_key)
        ));
        video_label = kv;
    }
    if let Some(region_blur) = seg.region_blur {
        let rv = format!("[rbv{i}]");
        filter.push_str(&region_blur_filter_block(
            &video_label,
            &rv,
            &i.to_string(),
            region_blur,
        ));
        filter.push(';');
        video_label = rv;
    }

    if let Some(plan) = seg.color_pipeline.as_ref() {
        if plan.has_any_lut() {
            let cv = format!("[cpv{i}]");
            let block = if let Some(mask_input_index) = seg.mask_input_index {
                masked_color_pipeline_filter_block(
                    &video_label,
                    &cv,
                    &i.to_string(),
                    plan,
                    mask_input_index,
                )
                .unwrap_or_else(|| {
                    color_pipeline_filter_block(&video_label, &cv, &i.to_string(), plan)
                })
            } else {
                color_pipeline_filter_block(&video_label, &cv, &i.to_string(), plan)
            };
            filter.push_str(&block);
            filter.push(';');
            video_label = cv;
        }
    } else if let Some(lut_path) = seg.lut_path.as_ref() {
        let lv = format!("[lv{i}]");
        filter.push_str(&lut3d_filter_block(
            &video_label,
            &lv,
            &i.to_string(),
            lut_path,
            seg.lut_interpolation.as_deref(),
            seg.lut_strength,
        ));
        filter.push(';');
        video_label = lv;
    }

    if let Some(chain) = segment_reframe_filter_chain(seg) {
        let rv = format!("[rv{i}]");
        filter.push_str(&format!("{video_label}{chain}{rv};"));
        video_label = rv;
    }
    if let Some(warp) = seg.warp.as_ref()
        && let Some(chain) = warp_filter_chain(warp, seg.warp_animation.as_ref(), i)
    {
        let wv = format!("[wv{i}]");
        filter.push_str(&format!("{video_label}{chain}{wv};"));
        video_label = wv;
    }
    if let Some(shake) = seg.shake.as_ref()
        && let Some(chain) = animated_shake_filter_chain(
            shake,
            seg.shake_intensity_animation.as_ref(),
            seg.shake_frequency_animation.as_ref(),
        )
    {
        let sv = format!("[shv{i}]");
        filter.push_str(&format!("{video_label}{chain}{sv};"));
        video_label = sv;
    }

    // Audio repair runs before speed/fades/gain/loudness on each clip:
    // cleanup -> EQ -> dynamics. Track-level FX are applied after
    // per-track concat.
    if let Some(fx) = seg.audio_fx.as_ref()
        && let Some(chain) = audio_fx_filter_chain(fx)
    {
        let fx_label = format!("[afx{i}]");
        filter.push_str(&format!("{audio_label}{chain}{fx_label};"));
        audio_label = fx_label;
    }

    if let Some(freeze) = seg.freeze {
        let (fv, fa) =
            append_freeze_frame_filters(filter, i, &video_label, &audio_label, seg, freeze);
        video_label = fv;
        audio_label = fa;
    }

    // Speed/time-remap after color/audio cleanup: setpts on video, atempo
    // (possibly chained) on audio. Variable time remap uses an average
    // audio tempo to keep stream duration aligned with the retimed video.
    if let Some(time_remap) = seg.time_remap.as_ref() {
        let sv = format!("[trv{i}]");
        filter.push_str(&format!(
            "{video_label}setpts='{expr}/TB'{sv};",
            expr = time_remap_setpts_expr(time_remap),
        ));
        video_label = sv;

        let factor = segment_speed(seg);
        if (factor - 1.0).abs() > 1e-9 && factor > 0.0 {
            let sa = format!("[tra{i}]");
            let chain = atempo_chain(factor);
            filter.push_str(&format!("{audio_label}{chain}{sa};"));
            audio_label = sa;
        }
    } else if let Some(speed) = seg.speed
        && speed_needs_filter(speed)
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!("{video_label}{}{sv};", speed_video_filter(speed)));
        video_label = sv;

        let sa = format!("[sa{i}]");
        filter.push_str(&format!("{audio_label}{}{sa};", speed_audio_filter(speed)));
        audio_label = sa;
    }

    // Volume next: applies to whatever the audio_label currently
    // points at (raw input or speed-stretched stream). Clip-local
    // automation takes precedence over scalar `awidat.volume`.
    if let Some(automation) = seg.volume_automation.as_ref() {
        let av = format!("[av{i}]");
        filter.push_str(&format!(
            "{audio_label}volume='{}':eval=frame{av};",
            automation.expression
        ));
        audio_label = av;
    } else if let Some(v) = seg.volume
        && (v - 1.0).abs() > 1e-9
    {
        let av = format!("[av{i}]");
        filter.push_str(&format!("{audio_label}volume={v}{av};"));
        audio_label = av;
    }
    (video_label, audio_label)
}

fn chroma_key_filter_chain(plan: &ChromaKeyPlan) -> String {
    format!(
        "chromakey={}:{}:{}",
        plan.key_color,
        fmt_filter_num(plan.similarity),
        fmt_filter_num(plan.blend),
    )
}

fn luma_key_filter_chain(plan: LumaKeyPlan) -> String {
    format!(
        "format=rgba,lumakey=threshold={}:softness={}",
        fmt_filter_num(plan.threshold),
        fmt_filter_num(plan.softness),
    )
}

fn region_blur_filter_block(
    input_label: &str,
    output_label: &str,
    suffix: &str,
    plan: RegionBlurPlan,
) -> String {
    let base = format!("[rb_base{suffix}]");
    let src = format!("[rb_src{suffix}]");
    let crop = format!("[rb_crop{suffix}]");
    let shape_filter = match plan.shape {
        RegionBlurShape::Rect => String::new(),
        RegionBlurShape::Ellipse => ",format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lte(pow((X-W/2)/(W/2)\\,2)+pow((Y-H/2)/(H/2)\\,2)\\,1)\\,255\\,0)'".to_string(),
    };
    format!(
        "{input_label}split=2{base}{src};\
{src}crop=w=iw*{w}:h=ih*{h}:x=iw*{x}:y=ih*{y},boxblur={radius}{shape_filter}{crop};\
{base}{crop}overlay=x=main_w*{x}:y=main_h*{y}{output_label}",
        x = fmt_filter_num(plan.x),
        y = fmt_filter_num(plan.y),
        w = fmt_filter_num(plan.width),
        h = fmt_filter_num(plan.height),
        radius = fmt_filter_num(plan.radius_px),
    )
}

fn append_freeze_frame_filters(
    filter: &mut String,
    i: usize,
    video_label: &str,
    audio_label: &str,
    seg: &TimelineSegment,
    freeze: FreezePlan,
) -> (String, String) {
    let freeze_at = (freeze.freeze_at_source_s - seg.start_s).clamp(0.0, seg.duration_s);
    let frame_end = (freeze_at + (1.0 / 24.0)).min(seg.duration_s);
    let clip_end = seg.duration_s;

    let vpre_src = format!("[fvpre_src{i}]");
    let vhold_src = format!("[fvhold_src{i}]");
    let vpost_src = format!("[fvpost_src{i}]");
    let vpre = format!("[fvpre{i}]");
    let vhold = format!("[fvhold{i}]");
    let vpost = format!("[fvpost{i}]");
    let fv = format!("[fv{i}]");
    filter.push_str(&format!(
        "{video_label}split=3{vpre_src}{vhold_src}{vpost_src};\
{vpre_src}trim=0:{freeze_at},setpts=PTS-STARTPTS{vpre};\
{vhold_src}trim={freeze_at}:{frame_end},setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration={hold}{vhold};\
{vpost_src}trim={freeze_at}:{clip_end},setpts=PTS-STARTPTS{vpost};\
{vpre}{vhold}{vpost}concat=n=3:v=1:a=0{fv};",
        freeze_at = fmt_filter_num(freeze_at),
        frame_end = fmt_filter_num(frame_end),
        hold = fmt_filter_num(freeze.duration_s),
        clip_end = fmt_filter_num(clip_end),
    ));

    let apre_src = format!("[fapre_src{i}]");
    let apost_src = format!("[fapost_src{i}]");
    let apre = format!("[fapre{i}]");
    let silence = format!("[fasilence{i}]");
    let apost = format!("[fapost{i}]");
    let fa = format!("[fa{i}]");
    filter.push_str(&format!(
        "{audio_label}asplit=2{apre_src}{apost_src};\
{apre_src}atrim=0:{freeze_at},asetpts=PTS-STARTPTS{apre};\
anullsrc=channel_layout=stereo:sample_rate=48000,atrim=0:{hold}{silence};\
{apost_src}atrim={freeze_at}:{clip_end},asetpts=PTS-STARTPTS{apost};\
{apre}{silence}{apost}concat=n=3:v=0:a=1{fa};",
        freeze_at = fmt_filter_num(freeze_at),
        hold = fmt_filter_num(freeze.duration_s),
        clip_end = fmt_filter_num(clip_end),
    ));

    (fv, fa)
}

fn audio_fx_filter_chain(plan: &AudioFxPlan) -> Option<String> {
    let mut filters = Vec::new();

    // cleanup
    if plan.adeclick.unwrap_or(false) {
        filters.push("adeclick".to_string());
    }
    if plan.adeclip.unwrap_or(false) {
        filters.push("adeclip".to_string());
    }
    if let Some(model) = filter_string(plan.arnndn_model.as_deref()) {
        filters.push(format!("arnndn=m={}", ffmpeg_filter_value_escape(model)));
    }
    if let Some(freq) = positive(plan.high_pass_hz) {
        filters.push(format!("highpass=f={}", fmt_filter_num(freq)));
    }
    if let Some(freq) = positive(plan.low_pass_hz) {
        filters.push(format!("lowpass=f={}", fmt_filter_num(freq)));
    }
    if let Some(threshold_db) = finite(plan.noise_gate_threshold_db) {
        filters.push(format!(
            "agate=threshold={}:ratio=2:attack=8:release=120",
            fmt_filter_num(db_to_linear(threshold_db).clamp(0.000001, 1.0))
        ));
    }
    if let Some(freq) = positive(plan.hum_notch_hz) {
        filters.push(format!(
            "bandstop=f={}:width_type=h:width=4",
            fmt_filter_num(freq)
        ));
        filters.push(format!(
            "bandstop=f={}:width_type=h:width=4",
            fmt_filter_num(freq * 2.0)
        ));
    }

    // stereo image
    if let Some(pan) = unit_interval(plan.pan) {
        filters.push(stereo_balance_filter(pan));
    }
    if let Some(balance) = unit_interval(plan.balance) {
        filters.push(stereo_balance_filter(balance));
    }

    // EQ
    for band in &plan.eq_bands {
        if band.freq_hz.is_finite() && band.freq_hz > 0.0 && band.gain_db.is_finite() {
            let width = band.width_hz.filter(|v| v.is_finite() && *v > 0.0);
            filters.push(format!(
                "equalizer=f={}:width_type=h:width={}:g={}",
                fmt_filter_num(band.freq_hz),
                fmt_filter_num(width.unwrap_or(120.0)),
                fmt_filter_num(band.gain_db),
            ));
        }
    }
    if let Some(freq) = positive(plan.de_ess_hz) {
        let reduction = finite(plan.de_ess_reduction_db).unwrap_or(4.0).abs();
        filters.push(format!(
            "equalizer=f={}:width_type=h:width=2500:g=-{}",
            fmt_filter_num(freq),
            fmt_filter_num(reduction),
        ));
    }

    // dynamics and final loudness
    if let Some(threshold) = finite(plan.compressor_threshold_db) {
        let ratio = finite(plan.compressor_ratio).unwrap_or(3.0).max(1.0);
        filters.push(format!(
            "acompressor=threshold={}dB:ratio={}:attack=5:release=80",
            fmt_filter_num(threshold),
            fmt_filter_num(ratio),
        ));
    }
    if let Some(limit_db) = finite(plan.limiter_limit_db) {
        filters.push(format!(
            "alimiter=limit={}",
            fmt_filter_num(db_to_linear(limit_db).clamp(0.000001, 1.0))
        ));
    }
    if let Some(i) = finite(plan.loudnorm_i) {
        let tp = finite(plan.loudnorm_tp).unwrap_or(-1.5);
        filters.push(format!(
            "loudnorm=I={}:TP={}:LRA=11",
            fmt_filter_num(i),
            fmt_filter_num(tp),
        ));
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters.join(","))
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite())
}

fn unit_interval(value: Option<f64>) -> Option<f64> {
    finite(value).map(|v| v.clamp(-1.0, 1.0))
}

fn positive(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0)
}

fn filter_string(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn stereo_balance_filter(value: f64) -> String {
    let value = value.clamp(-1.0, 1.0);
    let left_gain = if value > 0.0 { 1.0 - value } else { 1.0 };
    let right_gain = if value < 0.0 { 1.0 + value } else { 1.0 };
    format!(
        "pan=stereo|c0={}*FL|c1={}*FR",
        fmt_filter_num(left_gain),
        fmt_filter_num(right_gain)
    )
}

fn ffmpeg_filter_value_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | ':' | '\'' | ',' | ';' | '[' | ']') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn db_to_linear(db: f64) -> f64 {
    10_f64.powf(db / 20.0)
}

/// Flat-chain grade preview for a single clip, suitable for
/// FFmpeg's `-vf` argument when extracting one frame.
///
/// Returned `vf` is a comma-separated filter chain (no labels) that
/// can be prefixed onto a `scale=...` chain. When a clip's effects
/// require multi-segment labeled filtergraphs (e.g.
/// `awidat.lut.strength < 1.0`, multi-stage `color_pipeline`) those
/// stages are recorded in `skipped` rather than emitted — the
/// preview falls back to a partial grade and the caller can warn
/// the user. Full-strength single-LUT cases are the common path
/// and are emitted faithfully.
#[derive(Debug, Clone, Default)]
pub struct ClipGradeChain {
    /// Comma-separated filter chain or `None` if nothing applies.
    pub vf: Option<String>,
    /// Human-readable reasons certain stages were skipped.
    pub skipped: Vec<String>,
}

/// Labeled `-filter_complex` graph that applies the full grade for
/// a single clip — color correction + the legacy `awidat.lut` or
/// the full `awidat.color_pipeline` chain — without dropping any
/// stage. Strength<1 LUTs and multi-stage pipelines lower cleanly.
///
/// The returned `graph` (when `Some`) uses `[in]` for the source
/// input label and `[grade_out]` for the final output label so
/// callers can splice their own scale / output transforms in after
/// and route `-map [grade_out]` (or post-scale) on the encoder side.
#[derive(Debug, Clone, Default)]
pub struct ClipGradePreview {
    /// Full filtergraph or `None` when the clip has no graded
    /// effects (color correction, LUT, or color_pipeline).
    pub graph: Option<String>,
    /// Soft diagnostics surfaced when something couldn't resolve
    /// (e.g. a `color_pipeline.look_lut` path that didn't exist).
    pub skipped: Vec<String>,
}

/// Build a `-filter_complex` graph that reproduces the full grade
/// the timeline render would apply to `clip`, preserving
/// strength<1 (`split → lut3d → blend`) and multi-stage
/// color_pipeline chains exactly. Input label is `[in]`, output is
/// `[grade_out]`. Returns `graph: None` when the clip has no
/// graded effects.
pub fn build_clip_preview_filtergraph(
    clip: &awidat_proto::otio::Clip,
    project_root: &Path,
) -> ClipGradePreview {
    const SUFFIX: &str = "preview";
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::from("[in]");
    let mut skipped: Vec<String> = Vec::new();

    // HDR tonemap mirrors what the timeline render does first
    // (`stage_segment_inputs` runs `hdr_tonemap_filter` before
    // color correction). Without this, view_frame previews of
    // PQ/HLG sources would skip the linearization step and the
    // downstream LUT chain would see raw PQ/HLG pixels — that's
    // the opposite of what render emits.
    if let Some(transfer) = read_hdr_transfer(clip) {
        let next = format!("[hdr_{SUFFIX}]");
        segments.push(format!("{current}{}{next}", hdr_tonemap_filter(transfer)));
        current = next;
    }

    if let Some(plan) = read_color_correction(clip)
        && let Some(chain) = color_filter_chain(&plan)
    {
        let next = format!("[cc_{SUFFIX}]");
        segments.push(format!("{current}{chain}{next}"));
        current = next;
    }

    let pipeline_present = clip
        .effects
        .iter()
        .any(|e| e.effect_name == "awidat.color_pipeline");

    if pipeline_present {
        match read_color_pipeline(clip, project_root) {
            Ok(Some(plan)) => {
                let needs_chain = plan.has_any_lut() || auto_pre_lut_zscale(&plan).is_some();
                if needs_chain {
                    let next = format!("[cp_{SUFFIX}_out]");
                    segments.push(color_pipeline_filter_block(&current, &next, SUFFIX, &plan));
                    current = next;
                }
            }
            Ok(None) => {}
            Err(err) => skipped.push(format!("color_pipeline failed to resolve: {err}")),
        }
    } else if let Some(lut_path) =
        read_effect_string(clip, "awidat.lut", "lut_path").map(|p| project_root.join(p))
    {
        let interpolation = read_lut_interpolation(clip);
        let strength = read_effect_number(clip, "awidat.lut", "strength")
            .filter(|s| s.is_finite() && (0.0..=1.0).contains(s));
        let next = format!("[lut_{SUFFIX}_out]");
        segments.push(lut3d_filter_block(
            &current,
            &next,
            SUFFIX,
            &lut_path,
            interpolation.as_deref(),
            strength,
        ));
        current = next;
    }

    if segments.is_empty() {
        return ClipGradePreview {
            graph: None,
            skipped,
        };
    }

    // Rewrite the last sub-chain's output label to the canonical
    // `[grade_out]` so the caller has a stable label to splice the
    // scale stage onto.
    let final_label = "[grade_out]";
    if let Some(last) = segments.last_mut() {
        let new_last = last.replacen(current.as_str(), final_label, 1);
        *last = new_last;
    }

    ClipGradePreview {
        graph: Some(segments.join(";")),
        skipped,
    }
}

/// Build a flat preview-grade filter chain for `clip`, joining
/// (when present): color correction, the legacy `awidat.lut` at
/// full strength, and the `color_pipeline` look LUT at full
/// strength. Multi-stage pipelines and partial-strength LUTs are
/// recorded in `skipped`.
pub fn build_clip_grade_chain(
    clip: &awidat_proto::otio::Clip,
    project_root: &Path,
) -> ClipGradeChain {
    let mut filters: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    if let Some(plan) = read_color_correction(clip)
        && let Some(chain) = color_filter_chain(&plan)
    {
        filters.push(chain);
    }

    let pipeline_present = clip
        .effects
        .iter()
        .any(|e| e.effect_name == "awidat.color_pipeline");

    if pipeline_present {
        match read_color_pipeline(clip, project_root) {
            Ok(Some(plan)) => {
                if plan.input_transform_lut.is_some()
                    || plan.shaper_lut.is_some()
                    || plan.output_transform_lut.is_some()
                {
                    skipped.push(
                        "color_pipeline has multi-stage chain (shaper/IDT/ODT); only look_lut is included in preview".into(),
                    );
                }
                if let Some(look) = plan.look_lut.as_ref() {
                    let s = plan.look_strength.unwrap_or(1.0);
                    if s >= 1.0 - 1e-9 {
                        filters.push(lut3d_filter(look, plan.look_interpolation.as_deref()));
                    } else {
                        skipped.push(format!(
                            "color_pipeline look_strength {s} < 1.0 needs split/blend; preview drops blend"
                        ));
                        filters.push(lut3d_filter(look, plan.look_interpolation.as_deref()));
                    }
                }
            }
            Ok(None) => {}
            Err(err) => skipped.push(format!("color_pipeline failed to resolve: {err}")),
        }
    } else if let Some(lut_path) =
        read_effect_string(clip, "awidat.lut", "lut_path").map(|p| project_root.join(p))
    {
        let interpolation = read_lut_interpolation(clip);
        let strength = read_effect_number(clip, "awidat.lut", "strength")
            .filter(|s| s.is_finite() && (0.0..=1.0).contains(s));
        if let Some(s) = strength
            && s < 1.0 - 1e-9
        {
            skipped.push(format!(
                "awidat.lut strength {s} < 1.0 needs split/blend; preview applies LUT at full strength"
            ));
        }
        filters.push(lut3d_filter(&lut_path, interpolation.as_deref()));
    }

    if filters.is_empty() {
        ClipGradeChain { vf: None, skipped }
    } else {
        ClipGradeChain {
            vf: Some(filters.join(",")),
            skipped,
        }
    }
}

fn color_filter_chain(plan: &ColorCorrectionPlan) -> Option<String> {
    let mut filters = Vec::new();
    let exposure = plan.exposure_ev.unwrap_or(0.0);
    let contrast = plan.contrast.unwrap_or(1.0);
    let saturation = plan.saturation.unwrap_or(1.0);
    if exposure.abs() > 1e-9 || (contrast - 1.0).abs() > 1e-9 || (saturation - 1.0).abs() > 1e-9 {
        let shadows = plan.shadows.unwrap_or(0.0);
        let highlights = plan.highlights.unwrap_or(0.0);
        let brightness = (exposure * 0.14 + shadows * 0.06 + highlights * 0.03).clamp(-1.0, 1.0);
        filters.push(format!(
            "eq=brightness={}:contrast={}:saturation={}",
            fmt_filter_num(brightness),
            fmt_filter_num(contrast),
            fmt_filter_num(saturation),
        ));
    }

    let temperature = plan.temperature.unwrap_or(0.0);
    let tint = plan.tint.unwrap_or(0.0);
    let shadows = plan.shadows.unwrap_or(0.0);
    let highlights = plan.highlights.unwrap_or(0.0);
    if shadows.abs() > 1e-9 || highlights.abs() > 1e-9 {
        let shadow_mid = (0.25 + shadows * 0.16).clamp(0.08, 0.45);
        let highlight_mid = (0.75 + highlights * 0.16).clamp(0.55, 0.92);
        filters.push(format!(
            "curves=all='0/0 0.25/{} 0.75/{} 1/1'",
            fmt_filter_num(shadow_mid),
            fmt_filter_num(highlight_mid),
        ));
    }

    if temperature.abs() > 1e-9 || tint.abs() > 1e-9 {
        let rs = (temperature * 0.1 + tint * 0.045).clamp(-1.0, 1.0);
        let gs = (-tint * 0.09).clamp(-1.0, 1.0);
        let bs = (-temperature * 0.1 + tint * 0.045).clamp(-1.0, 1.0);
        let rh = (temperature * 0.07 + tint * 0.035).clamp(-1.0, 1.0);
        let gh = (-tint * 0.07).clamp(-1.0, 1.0);
        let bh = (-temperature * 0.07 + tint * 0.035).clamp(-1.0, 1.0);
        filters.push(format!(
            "colorbalance=rs={}:gs={}:bs={}:rh={}:gh={}:bh={}",
            fmt_filter_num(rs),
            fmt_filter_num(gs),
            fmt_filter_num(bs),
            fmt_filter_num(rh),
            fmt_filter_num(gh),
            fmt_filter_num(bh),
        ));
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters.join(","))
    }
}

fn blur_filter_chain(plan: BlurPlan) -> Option<String> {
    if !plan.radius_px.is_finite() || plan.radius_px <= 0.0 {
        return None;
    }
    Some(format!(
        "gblur=sigma={}",
        fmt_filter_num(plan.radius_px.clamp(0.0, 100.0))
    ))
}

fn append_clip_blur_filter(
    filter: &mut String,
    input_label: &str,
    suffix: &str,
    static_blur: Option<BlurPlan>,
    radius_animation: Option<&RenderParameterAnimation>,
) -> String {
    if let Some(animation) = radius_animation {
        if !filter.is_empty() && !filter.ends_with(';') {
            filter.push(';');
        }
        let video_label = format!("[clip_blur{suffix}_video]");
        let radius_src_label = format!("[clip_blur{suffix}_radiussrc]");
        let radius_label = format!("[clip_blur{suffix}_radius]");
        let out_label = format!("[clip_blur{suffix}_out]");
        let radius_expr = clip_blur_radius_expr(animation, "T");
        filter.push_str(&format!(
            "{input_label}split=2{video_label}{radius_src_label};\
             {radius_src_label}format=gray,geq=lum='{radius_expr}'{radius_label};\
             {video_label}{radius_label}varblur=min_r=0:max_r=100:planes=15{out_label};",
        ));
        return out_label;
    }
    if let Some(blur) = static_blur
        && let Some(chain) = blur_filter_chain(blur)
    {
        if !filter.is_empty() && !filter.ends_with(';') {
            filter.push(';');
        }
        let out_label = format!("[blv{suffix}]");
        filter.push_str(&format!("{input_label}{chain}{out_label};"));
        return out_label;
    }
    input_label.to_string()
}

fn clip_blur_radius_expr(animation: &RenderParameterAnimation, time_var: &str) -> String {
    let blur_px = keyframes_to_ffmpeg_expr_with_extrapolation(
        &animation.keyframes,
        time_var,
        animation.pre_extrapolation,
        animation.post_extrapolation,
    );
    format!("min(max(({blur_px}),0),100)")
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

fn reframe_filter_chain(plan: &ReframePlan) -> Option<String> {
    if !plan.zoom.is_finite() || plan.zoom <= 1.0 {
        return None;
    }
    let zoom = plan.zoom.clamp(1.0, 3.0);
    let x = ((plan.x.clamp(-1.0, 1.0) + 1.0) / 2.0).clamp(0.0, 1.0);
    let y = ((plan.y.clamp(-1.0, 1.0) + 1.0) / 2.0).clamp(0.0, 1.0);
    Some(format!(
        "scale=ceil(iw*{zoom}/2)*2:ceil(ih*{zoom}/2)*2,\
         crop=iw/{zoom}:ih/{zoom}:x=(in_w-out_w)*{x}:y=(in_h-out_h)*{y}",
        zoom = fmt_filter_num(zoom),
        x = fmt_filter_num(x),
        y = fmt_filter_num(y),
    ))
}

fn segment_reframe_filter_chain(seg: &TimelineSegment) -> Option<String> {
    seg.reframe_path
        .as_ref()
        .and_then(reframe_path_filter_chain)
        .or_else(|| seg.reframe.as_ref().and_then(reframe_filter_chain))
}

fn reframe_path_filter_chain(plan: &ReframePathPlan) -> Option<String> {
    let first = plan.keyframes.first()?;
    let zoom = first.scale.clamp(1.0, 8.0);
    if !zoom.is_finite() || zoom <= 1.0 {
        return None;
    }
    let x_expr = reframe_path_axis_expr(plan, |keyframe| keyframe.center_x);
    let y_expr = reframe_path_axis_expr(plan, |keyframe| keyframe.center_y);
    Some(format!(
        "scale=ceil(iw*{zoom}/2)*2:ceil(ih*{zoom}/2)*2:eval=frame,\
         crop=iw/{zoom}:ih/{zoom}:x=(in_w-out_w)*({x_expr}):y=(in_h-out_h)*({y_expr})",
        zoom = fmt_filter_num(zoom),
    ))
}

fn reframe_path_axis_expr(
    plan: &ReframePathPlan,
    value: fn(&ReframePathKeyframePlan) -> f64,
) -> String {
    let keyframes = plan
        .keyframes
        .iter()
        .map(|keyframe| {
            awidat_proto::professional::Keyframe::linear(
                keyframe.time_s,
                value(keyframe).clamp(0.0, 1.0),
            )
        })
        .collect::<Vec<_>>();
    keyframes_to_ffmpeg_expr_with_extrapolation(
        &keyframes,
        "t",
        awidat_proto::professional::ExtrapolationMode::Hold,
        awidat_proto::professional::ExtrapolationMode::Hold,
    )
}

fn warp_filter_chain(
    plan: &WarpPlan,
    animation: Option<&WarpAnimationPlan>,
    segment_index: usize,
) -> Option<String> {
    if !plan.k1.is_finite()
        || !plan.k2.is_finite()
        || !plan.center_x.is_finite()
        || !plan.center_y.is_finite()
    {
        return None;
    }
    let k1 = initial_animation_value(animation.and_then(|plan| plan.k1.as_ref()))
        .unwrap_or(plan.k1)
        .clamp(-1.0, 1.0);
    let k2 = initial_animation_value(animation.and_then(|plan| plan.k2.as_ref()))
        .unwrap_or(plan.k2)
        .clamp(-1.0, 1.0);
    let center_x = initial_animation_value(animation.and_then(|plan| plan.center_x.as_ref()))
        .unwrap_or(plan.center_x)
        .clamp(0.0, 1.0);
    let center_y = initial_animation_value(animation.and_then(|plan| plan.center_y.as_ref()))
        .unwrap_or(plan.center_y)
        .clamp(0.0, 1.0);
    if k1.abs() <= 1e-9 && k2.abs() <= 1e-9 {
        return None;
    }
    if let Some(animation) = animation {
        let filter_id = format!("warp{segment_index}");
        let commands = warp_sendcmd_commands(animation, &filter_id)?;
        let lens = format!(
            "lenscorrection@{filter_id}=cx={}:cy={}:k1={}:k2={}:i=bilinear",
            fmt_filter_num(center_x),
            fmt_filter_num(center_y),
            fmt_filter_num(k1),
            fmt_filter_num(k2),
        );
        return Some(format!("sendcmd=c='{commands}',{lens}"));
    }
    Some(format!(
        "lenscorrection=cx={}:cy={}:k1={}:k2={}:i=bilinear",
        fmt_filter_num(center_x),
        fmt_filter_num(center_y),
        fmt_filter_num(k1),
        fmt_filter_num(k2),
    ))
}

fn warp_sendcmd_commands(animation: &WarpAnimationPlan, filter_id: &str) -> Option<String> {
    let mut commands = Vec::new();
    append_warp_parameter_commands(
        &mut commands,
        filter_id,
        "k1",
        animation.k1.as_ref(),
        -1.0,
        1.0,
    );
    append_warp_parameter_commands(
        &mut commands,
        filter_id,
        "k2",
        animation.k2.as_ref(),
        -1.0,
        1.0,
    );
    append_warp_parameter_commands(
        &mut commands,
        filter_id,
        "cx",
        animation.center_x.as_ref(),
        0.0,
        1.0,
    );
    append_warp_parameter_commands(
        &mut commands,
        filter_id,
        "cy",
        animation.center_y.as_ref(),
        0.0,
        1.0,
    );
    (!commands.is_empty()).then(|| commands.join(";"))
}

fn append_warp_parameter_commands(
    commands: &mut Vec<String>,
    filter_id: &str,
    command: &str,
    animation: Option<&RenderParameterAnimation>,
    min: f64,
    max: f64,
) {
    let Some(animation) = animation else {
        return;
    };
    commands.extend(
        warp_parameter_command_samples(animation)
            .into_iter()
            .filter_map(|time_s| {
                let value = evaluate_keyframes_with_modes(
                    &animation.keyframes,
                    time_s,
                    animation.pre_extrapolation,
                    animation.post_extrapolation,
                )?;
                value.is_finite().then(|| {
                    format!(
                        "{} {filter_id} {command} {}",
                        fmt_filter_num(time_s.max(0.0)),
                        fmt_filter_num(value.clamp(min, max))
                    )
                })
            }),
    );
}

fn warp_parameter_command_samples(animation: &RenderParameterAnimation) -> Vec<f64> {
    const SAMPLES_PER_SECOND: f64 = 16.0;
    const MAX_SAMPLES_PER_SEGMENT: usize = 64;
    let mut samples = Vec::new();
    for pair in animation.keyframes.windows(2) {
        let start = pair[0].time_s;
        let end = pair[1].time_s;
        if !start.is_finite() || !end.is_finite() {
            continue;
        }
        samples.push(start.max(0.0));
        if end <= start {
            continue;
        }
        let steps = ((end - start) * SAMPLES_PER_SECOND).ceil() as usize;
        let steps = steps.clamp(1, MAX_SAMPLES_PER_SEGMENT);
        for index in 1..steps {
            let time_s = start + (end - start) * index as f64 / steps as f64;
            if time_s > start && time_s < end {
                samples.push(time_s.max(0.0));
            }
        }
    }
    if let Some(last) = animation
        .keyframes
        .last()
        .filter(|keyframe| keyframe.time_s.is_finite())
    {
        samples.push(last.time_s.max(0.0));
    }
    samples.sort_by(f64::total_cmp);
    samples.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);
    samples
}

fn initial_animation_value(animation: Option<&RenderParameterAnimation>) -> Option<f64> {
    animation?
        .keyframes
        .iter()
        .find(|keyframe| keyframe.value.is_finite())
        .map(|keyframe| keyframe.value)
}

fn animated_shake_filter_chain(
    plan: &ShakePlan,
    intensity_animation: Option<&RenderParameterAnimation>,
    frequency_animation: Option<&RenderParameterAnimation>,
) -> Option<String> {
    if !plan.intensity_px.is_finite()
        || !plan.frequency_hz.is_finite()
        || plan.intensity_px <= 0.0
        || plan.frequency_hz <= 0.0
    {
        return None;
    }
    let intensity = max_animation_value(intensity_animation).unwrap_or(plan.intensity_px);
    let intensity = intensity.clamp(0.0, 200.0);
    let pad = intensity.ceil() as u32;
    if pad == 0 {
        return None;
    }
    let intensity_expr = intensity_animation
        .map(|animation| {
            format!(
                "min(max(({}),0),200)",
                keyframes_to_ffmpeg_expr_with_extrapolation(
                    &animation.keyframes,
                    "t",
                    animation.pre_extrapolation,
                    animation.post_extrapolation,
                )
            )
        })
        .unwrap_or_else(|| fmt_filter_num(plan.intensity_px.clamp(0.0, 200.0)));
    let frequency_expr = frequency_animation
        .map(|animation| {
            format!(
                "min(max(({}),0.1),60)",
                keyframes_to_ffmpeg_expr_with_extrapolation(
                    &animation.keyframes,
                    "t",
                    animation.pre_extrapolation,
                    animation.post_extrapolation,
                )
            )
        })
        .unwrap_or_else(|| fmt_filter_num(plan.frequency_hz.clamp(0.1, 60.0)));
    let y_frequency_expr = if frequency_animation.is_some() {
        format!("({frequency_expr})*0.73")
    } else {
        fmt_filter_num(plan.frequency_hz.clamp(0.1, 60.0) * 0.73)
    };
    let x_expr = format!("{intensity_expr}*sin(2*PI*{frequency_expr}*t)");
    let y_expr = format!("{intensity_expr}*cos(2*PI*{y_frequency_expr}*t)");
    let crop_margin = pad * 2;
    Some(format!(
        "pad=iw+{crop_margin}:ih+{crop_margin}:{pad}:{pad},\
         crop=w=iw-{crop_margin}:h=ih-{crop_margin}:x={pad}+{x_expr}:y={pad}+{y_expr}",
    ))
}

fn max_animation_value(animation: Option<&RenderParameterAnimation>) -> Option<f64> {
    animation?
        .keyframes
        .iter()
        .filter_map(|keyframe| keyframe.value.is_finite().then_some(keyframe.value))
        .max_by(f64::total_cmp)
}

fn filter_escape_single_quoted(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\'' => "\\'".chars().collect::<Vec<_>>(),
            ':' => "\\:".chars().collect::<Vec<_>>(),
            ',' => "\\,".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

/// Map a delivery color-space id (as carried on
/// `awidat.color_pipeline.output_space`) to the four FFmpeg encoder
/// tags needed to round-trip the intent in the output container:
/// `-color_primaries -color_trc -colorspace -color_range`. Returns
/// `None` for spaces awidat doesn't currently tag (Log encodings —
/// nobody delivers those — and `scene_linear`, which has no
/// canonical container-tag triple).
fn output_space_color_tags(output_space: &str) -> Option<[&'static str; 4]> {
    // primaries, trc, matrix, range.
    let tags: [&'static str; 4] = match output_space {
        "rec709_g24" => ["bt709", "bt709", "bt709", "tv"],
        "srgb" => ["bt709", "iec61966-2-1", "bt709", "pc"],
        "dci_p3" => ["smpte432", "bt709", "bt709", "tv"],
        "pq_rec2020" => ["bt2020", "smpte2084", "bt2020nc", "tv"],
        "hlg_rec2020" => ["bt2020", "arib-std-b67", "bt2020nc", "tv"],
        _ => return None,
    };
    Some(tags)
}

/// Pick a single delivery `output_space` for the whole render by
/// taking the first segment that declares one. The agent is
/// expected to set `output_space` consistently across clips when it
/// cares; mixed values are not reconciled here.
fn render_output_space(segs: &[TimelineSegment]) -> Option<&str> {
    segs.iter()
        .filter_map(|s| s.color_pipeline.as_ref())
        .find_map(|p| p.output_space.as_deref())
}

/// Build the `-color_primaries / -color_trc / -colorspace /
/// -color_range` argv chunk for the render's delivery space, or an
/// empty vec when no tags apply.
fn output_color_tag_argv(segs: &[TimelineSegment]) -> Vec<String> {
    let Some(space) = render_output_space(segs) else {
        return Vec::new();
    };
    let Some([primaries, trc, matrix, range]) = output_space_color_tags(space) else {
        return Vec::new();
    };
    vec![
        "-color_primaries".into(),
        primaries.into(),
        "-color_trc".into(),
        trc.into(),
        "-colorspace".into(),
        matrix.into(),
        "-color_range".into(),
        range.into(),
    ]
}

fn lut3d_filter(lut_path: &Path, interpolation: Option<&str>) -> String {
    let path = filter_escape_single_quoted(&lut_path.to_string_lossy());
    match interpolation {
        Some(interpolation) => format!("lut3d=file='{path}':interp={interpolation}"),
        None => format!("lut3d=file='{path}'"),
    }
}

/// FFmpeg `lut1d=file=...` filter for shaper LUTs and 1D output
/// transforms. Used by [`color_pipeline_filter_block`] to apply a
/// 1D shaper (e.g. log → working space) before the look LUT.
fn lut1d_filter(lut_path: &Path) -> String {
    let path = filter_escape_single_quoted(&lut_path.to_string_lossy());
    format!("lut1d=file='{path}'")
}

/// Map a stable color-space id to FFmpeg `zscale` parameters that
/// place pixels in that space. Returns `None` for spaces zscale
/// can't represent — camera Log encodings need a shaper LUT for
/// linearization, not a transfer-curve swap.
///
/// The DCI-P3 mapping is a compromise: zscale doesn't have a true
/// "DCI-P3 gamma 2.6" preset, so we use `transfer=bt709` with
/// P3-D65 primaries — close enough for the apply-time normalize
/// case the agent uses to feed a P3-authored LUT; the actual
/// gamma 2.6 encoding lives in the LUT itself.
/// Stable (transfer, primaries, matrix) triple for each color space
/// awidat can normalize via FFmpeg's `zscale` filter. Returns `None`
/// for camera Log encodings — those need a shaper LUT for
/// linearization, not a transfer-curve swap.
///
/// The DCI-P3 mapping is a compromise: zscale doesn't have a true
/// "DCI-P3 gamma 2.6" preset, so we use `transfer=bt709` with
/// P3-D65 primaries — close enough for the apply-time normalize
/// case the agent uses to feed a P3-authored LUT; the actual
/// gamma 2.6 encoding lives in the LUT itself.
fn zscale_triple_for_space(space: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match space {
        "rec709_g24" => Some(("bt709", "bt709", "bt709")),
        "srgb" => Some(("iec61966-2-1", "bt709", "bt709")),
        "dci_p3" => Some(("bt709", "smpte432", "bt709")),
        "scene_linear" => Some(("linear", "bt709", "bt709")),
        "pq_rec2020" => Some(("smpte2084", "bt2020", "bt2020nc")),
        "hlg_rec2020" => Some(("arib-std-b67", "bt2020", "bt2020nc")),
        _ => None,
    }
}

/// Output-side `zscale=transfer=…:primaries=…:matrix=…` parameters
/// for the named space. Returns `None` for spaces zscale can't
/// represent — see [`zscale_triple_for_space`] for the underlying
/// mapping.
fn zscale_params_for_space(space: &str) -> Option<String> {
    let (transfer, primaries, matrix) = zscale_triple_for_space(space)?;
    Some(format!(
        "transfer={transfer}:primaries={primaries}:matrix={matrix}"
    ))
}

/// Input-side `zscale=transferin=…:primariesin=…:matrixin=…`
/// parameters for the named space. Used to tag untagged source
/// streams so `zscale` can compute the path between the input
/// space and the target. FFmpeg fails with
/// "no path between colorspaces" when the input is untagged and
/// only output params are supplied.
fn zscale_input_params_for_space(space: &str) -> Option<String> {
    let (transfer, primaries, matrix) = zscale_triple_for_space(space)?;
    Some(format!(
        "transferin={transfer}:primariesin={primaries}:matrixin={matrix}"
    ))
}

/// Decide whether render should auto-insert a `zscale=` filter to
/// normalize the source pixels into the look LUT's declared input
/// space. Returns the full `zscale=` parameter string (both input
/// AND output side) so untagged sources still resolve, or `None`
/// when no auto-conversion applies (matching spaces, agent-provided
/// shaper/IDT, or an unsupported space — typically a camera Log
/// encoding).
fn auto_pre_lut_zscale(plan: &ColorPipelinePlan) -> Option<String> {
    // Agent owns the chain whenever they declared one of these.
    if plan.shaper_lut.is_some() || plan.input_transform_lut.is_some() {
        return None;
    }
    // Only triggers when there's actually a look LUT to feed.
    plan.look_lut.as_ref()?;
    let lut_space = plan.lut_input_space.as_deref()?;
    if plan.clip_input_space == lut_space {
        return None;
    }
    // Both spaces must be zscale-representable. If either is a Log
    // encoding (or anything else without a `zscale_triple_for_space`
    // mapping), we don't auto-normalize — that would silently lie
    // to the LUT. The render limitation surfaces the gap instead.
    let in_params = zscale_input_params_for_space(&plan.clip_input_space)?;
    let out_params = zscale_params_for_space(lut_space)?;
    Some(format!("{in_params}:{out_params}"))
}

/// Build the filter-graph block for an `awidat.color_pipeline` plan,
/// chaining each populated slot in order:
/// input_transform_lut → shaper_lut → look_lut (with look_strength)
/// → output_transform_lut. Returns the entire block including
/// `[in_label]` and `[out_label]` and any intermediate `;`
/// separators between sub-chains. Callers append a trailing `;`
/// before the next stage.
///
/// If no LUT slot is populated, emits a single `null` filter so the
/// caller always gets a chain that terminates at `out_label`.
///
/// `suffix` is a per-segment uniquifier used to avoid label
/// collisions across the wider graph.
fn color_pipeline_filter_block(
    in_label: &str,
    out_label: &str,
    suffix: &str,
    plan: &ColorPipelinePlan,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut current = in_label.to_string();

    if let Some(p) = &plan.input_transform_lut {
        let next = format!("[cp_idt_{suffix}]");
        segments.push(format!("{current}{}{next}", lut3d_filter(p, None)));
        current = next;
    }
    if let Some(p) = &plan.shaper_lut {
        let next = format!("[cp_shaper_{suffix}]");
        segments.push(format!("{current}{}{next}", lut1d_filter(p)));
        current = next;
    }
    // When the agent didn't provide a shaper / IDT but the clip's
    // input space differs from the LUT's declared input space, fall
    // back to FFmpeg's `zscale` filter for display-referred pairs
    // (per [[lut-color-space-rule]]: apply each LUT in its declared
    // input space). Log encodings without a shaper still get no
    // pre-transform — the lack of shaper is surfaced separately as
    // a `log_clip_without_shaper` render limitation.
    if let Some(zscale_params) = auto_pre_lut_zscale(plan) {
        let next = format!("[cp_norm_{suffix}]");
        segments.push(format!("{current}zscale={zscale_params}{next}"));
        current = next;
    }
    if let Some(p) = &plan.look_lut {
        let next = format!("[cp_look_{suffix}]");
        let block = lut3d_filter_block(
            &current,
            &next,
            &format!("cp_look_{suffix}"),
            p,
            plan.look_interpolation.as_deref(),
            plan.look_strength,
        );
        segments.push(block);
        current = next;
    }
    if let Some(p) = &plan.output_transform_lut {
        let next = format!("[cp_odt_{suffix}]");
        segments.push(format!("{current}{}{next}", lut3d_filter(p, None)));
        current = next;
    }

    if segments.is_empty() {
        // Metadata-only pipeline (e.g., agent set output_space but no
        // LUTs). Emit a no-op `null` so the chain terminates at
        // `out_label` regardless.
        return format!("{in_label}null{out_label}");
    }

    // Rewrite the last sub-chain's output label so it points at the
    // caller's `out_label` rather than the intermediate label we
    // generated for it.
    if let Some(last) = segments.last_mut() {
        let new_last = last.replacen(current.as_str(), out_label, 1);
        *last = new_last;
    }

    segments.join(";")
}

fn masked_color_pipeline_filter_block(
    in_label: &str,
    out_label: &str,
    suffix: &str,
    plan: &ColorPipelinePlan,
    mask_input_index: usize,
) -> Option<String> {
    let look_lut = plan.look_lut.as_ref()?;
    if !is_renderable_masked_color_pipeline(plan) {
        return None;
    }
    let orig = format!("[cp_mask_orig_{suffix}]");
    let to_lut = format!("[cp_mask_lut_in_{suffix}]");
    let lutted = format!("[cp_mask_lutted_{suffix}]");
    let mask_loop = format!("[cp_mask_loop_{suffix}]");
    let mask_scaled = format!("[cp_mask_scaled_{suffix}]");
    let lut_ref = format!("[cp_mask_lut_ref_{suffix}]");
    let lut_alpha = format!("[cp_mask_alpha_{suffix}]");
    Some(format!(
        "{in_label}split{orig}{to_lut};\
         {to_lut}{}{lutted};\
         [{mask_input_index}:v:0]format=gray,loop=-1:1{mask_loop};\
         {mask_loop}{lutted}scale2ref=w=main_w:h=main_h{mask_scaled}{lut_ref};\
         {lut_ref}{mask_scaled}alphamerge{lut_alpha};\
         {orig}{lut_alpha}overlay=format=auto{out_label}",
        lut3d_filter(look_lut, plan.look_interpolation.as_deref()),
    ))
}

/// Build the filter-graph chain segment that applies a LUT to a labeled
/// video stream, including the multi-filter `split → lut3d → blend`
/// block when strength is below 1.0. Returns the full string starting
/// with `in_label` and ending with `out_label`, with `;` separators
/// between intermediate sub-chains. The caller appends a trailing `;`
/// before the next filter.
///
/// `suffix` is a per-segment uniquifier (e.g. `"0"` for segment 0 or
/// `"media_overlay_3"`) used to avoid label collisions across the
/// graph.
fn lut3d_filter_block(
    in_label: &str,
    out_label: &str,
    suffix: &str,
    lut_path: &Path,
    interpolation: Option<&str>,
    strength: Option<f64>,
) -> String {
    let core = lut3d_filter(lut_path, interpolation);
    match strength {
        Some(s) if s.is_finite() && s < 1.0 - 1e-9 => {
            // Clamp away from 0 too — 0 means "bypass the LUT" but we
            // still emit the filter so the graph topology is stable
            // across re-renders; `blend=all_opacity=0` short-circuits
            // back to the original.
            let s = s.clamp(0.0, 1.0);
            let pre = format!("[lut_pre_{suffix}]");
            let lin = format!("[lut_in_{suffix}]");
            let lout = format!("[lut_out_{suffix}]");
            // FFmpeg's `blend` filter treats the FIRST input as the
            // "top" layer and applies `all_opacity` to it:
            //   output = top * all_opacity + bottom * (1 - all_opacity)
            // We want `strength` to mean "fraction of the LUT to
            // keep" — strength=1 → full LUT, strength=0 → bypass.
            // So the LUT-applied stream is FIRST (`lout`), with the
            // un-graded original as the bottom (`pre`).
            format!(
                "{in_label}split{pre}{lin};{lin}{core}{lout};{lout}{pre}blend=all_opacity={s}{out_label}",
                s = fmt_filter_num(s),
            )
        }
        _ => format!("{in_label}{core}{out_label}"),
    }
}

/// Length of the alpha / slide ramp for fade and slide animations,
/// in seconds. Half a second feels intentional without lingering.
const ANIMATION_RAMP_S: f64 = 0.5;
const HARD_CUT_AUDIO_FADE_S: f64 = 0.03;

/// Build one ffmpeg `drawtext=...` filter from a title plan.
///
/// Position uses proportional y= expressions so titles survive
/// resolution changes:
///   - top    → `y=h*0.05`
///   - center → `y=(h-text_h)/2`
///   - bottom → `y=h*0.85`
///     (or a higher safe band when a broadcast overlay owns the
///     lower-third area)
///
/// Animations modulate alpha (fade) or x/y (slide) via piecewise
/// expressions; see [`apply_title_animation`] for the math. The
/// `enable=between(t,start,end)` window still bounds the title
/// regardless of animation — fade-out reaches alpha 0 right at
/// `end`, slide-out reaches off-screen right at `end`.
///
/// `text_h` and `text_w` are drawtext-evaluated dimensions of the
/// rendered text; `h` and `w` are the frame dimensions.
fn format_drawtext_filter(
    t: &TitlePlan,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    if !t.word_timings.is_empty() {
        return format_word_timed_drawtext_filters(t, broadcast_overlay);
    }
    if !t.rich_segments.is_empty() && t.reveal == TextReveal::None {
        return format_rich_drawtext_filters(t, broadcast_overlay);
    }
    if t.reveal != TextReveal::None {
        return format_revealed_drawtext_filters(t, broadcast_overlay);
    }
    format_drawtext_filter_with_text(t, &t.text, t.start_s, t.end_s, broadcast_overlay)
}

fn format_drawtext_filter_with_text(
    t: &TitlePlan,
    text: &str,
    start_s: f64,
    end_s: f64,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let wrapped_text = auto_wrap_title_text(text, t.font_size);
    let escaped_text = drawtext_escape(&wrapped_text);
    let resting_y = match t.position {
        TitlePosition::Top => "h*0.05".to_string(),
        TitlePosition::Center => "(h-text_h)/2".to_string(),
        TitlePosition::Bottom => title_bottom_y(t, broadcast_overlay),
    };
    let resting_x = "(w-text_w)/2".to_string();
    let weight_attr = title_weight_drawtext_attr(t.font_weight);
    let fontfile = pick_fontfile_attr();
    let anim = apply_title_animation(t, &resting_x, &resting_y);
    let alpha = if has_title_animation(t, "title.opacity") {
        format!(
            ":alpha='{}'",
            title_animation_value_expr(t, "title.opacity", "1")
        )
    } else {
        anim.alpha
    };
    let x = if has_title_animation(t, "title.position") {
        format!(
            "({})+w*({})",
            anim.x,
            title_motion_path_expr(t, MotionPathCoordinate::X).unwrap_or_else(|| "0".to_string())
        )
    } else if has_title_animation(t, "title.x") {
        format!(
            "({})+w*({})",
            anim.x,
            title_animation_value_expr(t, "title.x", "0")
        )
    } else {
        anim.x
    };
    let y = if has_title_animation(t, "title.position") {
        format!(
            "({})+h*({})",
            anim.y,
            title_motion_path_expr(t, MotionPathCoordinate::Y).unwrap_or_else(|| "0".to_string())
        )
    } else if has_title_animation(t, "title.y") {
        format!(
            "({})+h*({})",
            anim.y,
            title_animation_value_expr(t, "title.y", "0")
        )
    } else {
        anim.y
    };
    let size = if has_title_animation(t, "title.font_size") {
        title_animation_value_expr(t, "title.font_size", &t.font_size.to_string())
    } else {
        t.font_size.to_string()
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{weight}\
         :x={x}:y={y}{alpha}:enable='between(t\\,{start}\\,{end})'",
        text = escaped_text,
        font = fontfile,
        size = size,
        color = t.color,
        weight = weight_attr,
        x = x,
        y = y,
        alpha = alpha,
        start = start_s,
        end = end_s,
    )
}

fn title_weight_drawtext_attr(font_weight: TitleWeight) -> &'static str {
    match font_weight {
        TitleWeight::Normal => "",
        // ffmpeg drawtext doesn't have a `font_weight=` flag. Without a
        // custom bold font bundle, approximate bold by stroking the text.
        TitleWeight::Bold => ":borderw=2",
    }
}

fn format_rich_drawtext_filters(
    title: &TitlePlan,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let full_text = title
        .rich_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    if full_text.is_empty() {
        return String::new();
    }
    let total_width = approximate_text_width_px(&full_text, title.font_size);
    let mut offset = 0.0_f64;
    title
        .rich_segments
        .iter()
        .filter(|segment| !segment.text.is_empty())
        .map(|segment| {
            let run_width = approximate_text_width_px(&segment.text, title.font_size);
            let filter =
                format_rich_drawtext_run(title, segment, total_width, offset, broadcast_overlay);
            offset += run_width;
            filter
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_rich_drawtext_run(
    title: &TitlePlan,
    segment: &RichTextSegment,
    total_width_px: f64,
    offset_px: f64,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let mut run = title.clone();
    run.text = segment.text.clone();
    run.color = segment.color.clone().unwrap_or_else(|| title.color.clone());
    run.font_weight = segment.font_weight.unwrap_or(title.font_weight);
    run.rich_segments.clear();
    let base_x = format!(
        "(w-{total})/2+{offset}",
        total = fmt_px(total_width_px),
        offset = fmt_px(offset_px)
    );
    format_drawtext_filter_with_position(&run, &segment.text, &base_x, broadcast_overlay)
}

fn format_drawtext_filter_with_position(
    t: &TitlePlan,
    text: &str,
    resting_x: &str,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let wrapped_text = auto_wrap_title_text(text, t.font_size);
    let escaped_text = drawtext_escape(&wrapped_text);
    let resting_y = match t.position {
        TitlePosition::Top => "h*0.05".to_string(),
        TitlePosition::Center => "(h-text_h)/2".to_string(),
        TitlePosition::Bottom => title_bottom_y(t, broadcast_overlay),
    };
    let weight_attr = title_weight_drawtext_attr(t.font_weight);
    let fontfile = pick_fontfile_attr();
    let anim = apply_title_animation(t, resting_x, &resting_y);
    let alpha = if has_title_animation(t, "title.opacity") {
        format!(
            ":alpha='{}'",
            title_animation_value_expr(t, "title.opacity", "1")
        )
    } else {
        anim.alpha
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{weight}\
         :x={x}:y={y}{alpha}:enable='between(t\\,{start}\\,{end})'",
        text = escaped_text,
        font = fontfile,
        size = t.font_size,
        color = t.color,
        weight = weight_attr,
        x = anim.x,
        y = anim.y,
        alpha = alpha,
        start = t.start_s,
        end = t.end_s,
    )
}

fn approximate_text_width_px(text: &str, font_size: u32) -> f64 {
    text.graphemes(true).map(glyph_width_factor).sum::<f64>() * font_size as f64
}

fn auto_wrap_title_text(text: &str, font_size: u32) -> String {
    if text.contains('\n') {
        return text.to_string();
    }
    let max_chars = title_wrap_max_chars(font_size);
    if grapheme_count(text) <= max_chars {
        return text.to_string();
    }
    wrap_text_by_words(text, max_chars)
}

fn title_wrap_max_chars(font_size: u32) -> usize {
    const TITLE_SAFE_WIDTH_PX: f64 = 1500.0;
    let average_glyph_width = (font_size.max(1) as f64 * 0.55).max(1.0);
    (TITLE_SAFE_WIDTH_PX / average_glyph_width).floor().max(8.0) as usize
}

fn wrap_text_by_words(text: &str, max_chars: usize) -> String {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut pending_space = false;
    for token in text.split_word_bounds() {
        if token.chars().all(char::is_whitespace) {
            pending_space |= !current.is_empty();
            continue;
        }
        append_wrapped_token(&mut lines, &mut current, token, pending_space, max_chars);
        pending_space = false;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        text.to_string()
    } else {
        lines.join("\n")
    }
}

fn append_wrapped_token(
    lines: &mut Vec<String>,
    current: &mut String,
    token: &str,
    pending_space: bool,
    max_chars: usize,
) {
    let token_len = grapheme_count(token);
    if token_len > max_chars {
        if !current.is_empty() {
            lines.push(std::mem::take(current));
        }
        push_grapheme_chunks(lines, current, token, max_chars);
        return;
    }

    let space_len = usize::from(pending_space && !current.is_empty());
    let next_len = grapheme_count(current) + space_len + token_len;
    if current.is_empty() || next_len <= max_chars {
        if space_len == 1 {
            current.push(' ');
        }
        current.push_str(token);
    } else {
        lines.push(std::mem::take(current));
        current.push_str(token);
    }
}

fn push_grapheme_chunks(
    lines: &mut Vec<String>,
    current: &mut String,
    token: &str,
    max_chars: usize,
) {
    for grapheme in token.graphemes(true) {
        if grapheme_count(current) >= max_chars {
            lines.push(std::mem::take(current));
        }
        current.push_str(grapheme);
    }
}

fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

fn glyph_width_factor(grapheme: &str) -> f64 {
    let Some(ch) = grapheme.chars().next() else {
        return 0.0;
    };
    if is_ascii_narrow(ch) {
        0.28
    } else if is_ascii_wide(ch) {
        0.78
    } else if ch.is_ascii_whitespace() {
        0.33
    } else if ch.is_ascii_punctuation() {
        0.35
    } else if ch.is_ascii_alphanumeric() {
        0.55
    } else if is_cjk(ch) || grapheme.chars().count() > 1 {
        1.0
    } else {
        0.65
    }
}

fn is_ascii_narrow(ch: char) -> bool {
    matches!(ch, 'i' | 'j' | 'l' | 'I' | '!' | '|' | '\'' | '`')
}

fn is_ascii_wide(ch: char) -> bool {
    matches!(ch, 'm' | 'w' | 'M' | 'W' | '@' | '#' | '%' | '&')
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

fn title_motion_path_expr(title: &TitlePlan, coordinate: MotionPathCoordinate) -> Option<String> {
    title
        .animations
        .iter()
        .find(|animation| animation.parameter == "title.position")
        .and_then(|animation| {
            let path = animation.motion_path.as_ref()?;
            let local_time_var = format!("(t-{})", title.start_s);
            Some(motion_path_to_ffmpeg_expr_with_keyframes(
                path,
                &animation.keyframes,
                animation.pre_extrapolation,
                animation.post_extrapolation,
                coordinate,
                &local_time_var,
            ))
        })
}

fn format_revealed_drawtext_filters(
    t: &TitlePlan,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let steps = reveal_steps(&t.text, t.reveal);
    if steps.is_empty() {
        return String::new();
    }
    let duration = (t.end_s - t.start_s).max(0.0);
    let step_duration = duration / steps.len() as f64;
    steps
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let start_s = t.start_s + step_duration * index as f64;
            let end_s = if index + 1 == steps.len() {
                t.end_s
            } else {
                t.start_s + step_duration * (index + 1) as f64
            };
            format_drawtext_filter_with_text(t, text, start_s, end_s, broadcast_overlay)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_word_timed_drawtext_filters(
    t: &TitlePlan,
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
) -> String {
    let steps = word_timed_reveal_steps(t);
    steps
        .iter()
        .map(|step| {
            format_drawtext_filter_with_text(
                t,
                &step.text,
                step.start_s,
                step.end_s,
                broadcast_overlay,
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

struct WordTimedRevealStep {
    text: String,
    start_s: f64,
    end_s: f64,
}

fn word_timed_reveal_steps(t: &TitlePlan) -> Vec<WordTimedRevealStep> {
    let valid_words = t
        .word_timings
        .iter()
        .filter(|word| {
            !word.text.trim().is_empty()
                && word.start_s.is_finite()
                && word.end_s.is_finite()
                && word.start_s >= t.start_s
                && word.start_s < t.end_s
                && word.end_s >= word.start_s
        })
        .collect::<Vec<_>>();
    if valid_words.is_empty() {
        return Vec::new();
    }

    let mut steps = Vec::with_capacity(valid_words.len());
    let mut prefix_words = Vec::with_capacity(valid_words.len());
    for (index, word) in valid_words.iter().enumerate() {
        prefix_words.push(word.text.trim());
        let start_s = word.start_s.max(t.start_s);
        let end_s = valid_words
            .get(index + 1)
            .map(|next| next.start_s.min(t.end_s))
            .unwrap_or(t.end_s);
        if end_s <= start_s {
            continue;
        }
        steps.push(WordTimedRevealStep {
            text: prefix_words.join(" "),
            start_s,
            end_s,
        });
    }
    steps
}

fn reveal_steps(text: &str, reveal: TextReveal) -> Vec<String> {
    match reveal {
        TextReveal::None => vec![text.to_string()],
        TextReveal::Typewriter => UnicodeSegmentation::grapheme_indices(text, true)
            .map(|(index, grapheme)| {
                let end = index + grapheme.len();
                text[..end].to_string()
            })
            .collect(),
        TextReveal::Word => {
            let mut steps = Vec::new();
            let mut end = 0;
            let mut saw_word = false;
            for grapheme in UnicodeSegmentation::graphemes(text, true) {
                end += grapheme.len();
                if grapheme.chars().all(char::is_whitespace) {
                    saw_word = false;
                    continue;
                }
                saw_word = true;
                let next_is_boundary = text[end..]
                    .chars()
                    .next()
                    .map(char::is_whitespace)
                    .unwrap_or(true);
                if next_is_boundary {
                    steps.push(text[..end].to_string());
                }
            }
            if steps.is_empty() && saw_word {
                steps.push(text.to_string());
            }
            steps
        }
        TextReveal::Line => {
            let mut steps = Vec::new();
            let mut end = 0;
            for line in text.split_inclusive('\n') {
                end += line.len();
                steps.push(text[..end].to_string());
            }
            if steps.is_empty() && !text.is_empty() {
                steps.push(text.to_string());
            }
            steps
        }
    }
}

fn has_title_animation(title: &TitlePlan, parameter: &str) -> bool {
    title
        .animations
        .iter()
        .any(|animation| animation.parameter == parameter)
}

fn title_animation_value_expr(title: &TitlePlan, parameter: &str, fallback: &str) -> String {
    title
        .animations
        .iter()
        .find(|animation| animation.parameter == parameter)
        .map(|animation| {
            let local_time_var = format!("(t-{})", title.start_s);
            keyframes_to_ffmpeg_expr_with_extrapolation(
                &animation.keyframes,
                &local_time_var,
                animation.pre_extrapolation,
                animation.post_extrapolation,
            )
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn title_bottom_y(title: &TitlePlan, broadcast_overlay: Option<&BroadcastOverlayPlan>) -> String {
    let Some(overlay) = broadcast_overlay else {
        if title.safe_area.as_deref() == Some("mobile") {
            return "h*0.75".to_string();
        }
        return "h*0.85".to_string();
    };
    if !overlay.config.enabled {
        if title.safe_area.as_deref() == Some("mobile") {
            return "h*0.75".to_string();
        }
        return "h*0.85".to_string();
    }
    if overlay.config.short_form_mode {
        return "h*0.75".to_string();
    }
    // Long-form broadcast overlays reserve the lower frame for host
    // name bars and the ticker. Keep generic bottom titles in the
    // lower-middle safe band instead of drawing them into that chrome.
    "h*0.56".to_string()
}

fn broadcast_overlay_owns_program_titles(broadcast_overlay: Option<&BroadcastOverlayPlan>) -> bool {
    broadcast_overlay
        .is_some_and(|overlay| overlay.config.enabled && !overlay.config.short_form_mode)
}

fn format_broadcast_overlay_filters(overlay: &BroadcastOverlayPlan) -> Vec<String> {
    let c = &overlay.config;
    let st = &c.style;
    let mut parts = Vec::new();
    let navy = normalize_hex(&st.dark_navy_hex);
    let gold = normalize_hex(&st.gold_hex);
    let gold_light = normalize_hex(&st.gold_light_hex);
    let cyan = normalize_hex(&st.cyan_hex);
    let show = if c.show_name.trim().is_empty() {
        "BROADCAST".to_string()
    } else {
        c.show_name.to_uppercase()
    };

    if c.short_form_mode {
        return format_short_form_brand_bar(c, &show, &navy, &gold);
    }

    if !c.episode_title.trim().is_empty() {
        parts.extend(format_episode_title_card(c, st, &navy, &gold, &cyan));
    }
    if !c.host_a.name.trim().is_empty() || !c.host_b.name.trim().is_empty() {
        parts.extend(format_host_name_bar(c, st, &navy, &gold));
        parts.extend(format_host_intro_strip(c, st, &gold, &gold_light, &navy));
    }
    parts.extend(format_smart_ticker(c, st, &show, &navy, &gold, &cyan));
    if !c.chapters.is_empty() {
        parts.extend(format_chapter_cards(c, st, &navy, &gold));
    }
    parts
}

fn broadcast_ticker_entries(
    c: &BroadcastOverlayConfig,
) -> &[awidat_proto::awidat_meta::BroadcastTimedEntry] {
    if c.topics.is_empty() {
        &c.chapters
    } else {
        &c.topics
    }
}

fn format_short_form_brand_bar(
    c: &BroadcastOverlayConfig,
    show: &str,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let display = if !c.show_name.trim().is_empty() {
        c.show_name.to_uppercase()
    } else if !c.episode_title.trim().is_empty() {
        c.episode_title.to_uppercase()
    } else {
        show.to_string()
    };
    vec![
        format!("drawbox=x=0:y=ih*0.9583:w=iw:h=ih*0.0417:color={navy}@0.90:t=fill"),
        format!("drawbox=x=0:y=ih*0.9583:w=iw:h=3:color={gold}@1:t=fill"),
        broadcast_drawtext(
            &display,
            "(w-text_w)/2",
            "h*0.983-text_h/2",
            32,
            gold,
            None,
            None,
            true,
        )
        .replace(":enable='between(t\\,0\\,0)'", ""),
    ]
}

fn format_episode_title_card(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
    cyan: &str,
) -> Vec<String> {
    let start = 0.0;
    let end = st.title_visible_end;
    let x = "iw*0.26";
    let y = "ih*0.46";
    let w = "iw*0.48";
    let h = "ih*0.19";
    let alpha = broadcast_title_alpha(st);
    let mut out = vec![
        format!(
            "drawbox=x={x}:y={y}:w={w}:h={h}:color={navy}@0.88:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        format!(
            "drawbox=x={x}:y={y}:w={w}:h=4:color={gold}@1:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        format!(
            "drawbox=x={x}:y=ih*0.65-2:w={w}:h=2:color={cyan}@0.27:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        broadcast_drawtext(
            "EPISODE",
            "w*0.29",
            "h*0.50",
            18,
            gold,
            Some(&alpha),
            Some((start, end)),
            false,
        ),
        broadcast_drawtext(
            &c.episode_title.to_uppercase(),
            "w*0.29",
            "h*0.545",
            55,
            "#FFFFFF",
            Some(&alpha),
            Some((start, end)),
            true,
        ),
    ];
    if !c.episode_subtitle.trim().is_empty() {
        out.push(broadcast_drawtext_body(
            &c.episode_subtitle,
            "w*0.29",
            "h*0.60",
            23,
            "#CBD5E1",
            Some(&alpha),
            Some((start, end)),
            false,
        ));
    }
    out
}

fn format_host_name_bar(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let name_h = broadcast_ref_to_1080_px(st.name_bar_height);
    let ticker_h = broadcast_ref_to_1080_px(st.ticker_height);
    let y = format!("ih-{}", fmt_px(name_h + ticker_h));
    let divider_h = broadcast_ref_to_1080_px(114.0);
    let divider_y = format!("ih-{}", fmt_px(ticker_h + (name_h + divider_h) / 2.0));
    let name_text_y = format!("h-{}", fmt_px(ticker_h + name_h / 2.0 + 16.0));
    let title_text_y = format!("h-{}", fmt_px(ticker_h + name_h / 2.0 + 11.0));
    let enable = format!(
        "not(between(t\\,{s}\\,{e}))",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let mut out = vec![
        format!(
            "drawbox=x=0:y={y}:w=iw:h={h}:color={navy}@0.92:t=fill:enable='{enable}'",
            h = fmt_px(name_h)
        ),
        format!("drawbox=x=0:y={y}:w=iw:h=2:color={gold}@0.55:t=fill:enable='{enable}'"),
        format!(
            "drawbox=x=iw/2:y={divider_y}:w=1:h={divider_h}:color={gold}@0.35:t=fill:enable='{enable}'",
            divider_h = fmt_px(divider_h)
        ),
    ];
    out.extend(format_host_inline_text(
        &c.host_a,
        HostTextSide::Left,
        &name_text_y,
        &title_text_y,
        gold,
        &enable,
    ));
    out.extend(format_host_inline_text(
        &c.host_b,
        HostTextSide::Right,
        &name_text_y,
        &title_text_y,
        gold,
        &enable,
    ));
    out
}

enum HostTextSide {
    Left,
    Right,
}

fn format_host_inline_text(
    host: &BroadcastHost,
    side: HostTextSide,
    name_y: &str,
    title_y: &str,
    gold: &str,
    enable: &str,
) -> Vec<String> {
    if host.name.trim().is_empty() {
        return Vec::new();
    }
    let name = host.name.to_uppercase();
    let title = host.title.to_uppercase();
    let gap = 8.0;
    let margin = 28.0;
    let name_size = 32;
    let title_size = 23;
    let mut out = Vec::new();
    if title.trim().is_empty() {
        let x = match side {
            HostTextSide::Left => fmt_px(margin),
            HostTextSide::Right => format!("w-text_w-{}", fmt_px(margin)),
        };
        out.push(
            broadcast_drawtext(&name, &x, name_y, name_size, "#FFFFFF", None, None, true).replace(
                ":enable='between(t\\,0\\,0)'",
                &format!(":enable='{enable}'"),
            ),
        );
        return out;
    }

    let name_w = estimated_broadcast_text_width(&name, name_size);
    let title_w = estimated_broadcast_text_width(&title, title_size);
    let (name_x, title_x) = match side {
        HostTextSide::Left => (fmt_px(margin), fmt_px(margin + name_w + gap)),
        HostTextSide::Right => (
            format!("w-{}", fmt_px(margin + title_w + gap + name_w)),
            format!("w-{}", fmt_px(margin + title_w)),
        ),
    };
    out.push(
        broadcast_drawtext(
            &name, &name_x, name_y, name_size, "#FFFFFF", None, None, true,
        )
        .replace(
            ":enable='between(t\\,0\\,0)'",
            &format!(":enable='{enable}'"),
        ),
    );
    out.push(
        broadcast_drawtext(
            &title, &title_x, title_y, title_size, gold, None, None, true,
        )
        .replace(
            ":enable='between(t\\,0\\,0)'",
            &format!(":enable='{enable}'"),
        ),
    );
    out
}

fn format_smart_ticker(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    show: &str,
    navy: &str,
    gold: &str,
    cyan: &str,
) -> Vec<String> {
    let enable = format!(
        "not(between(t\\,{s}\\,{e}))",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let ticker_h = broadcast_ref_to_1080_px(st.ticker_height);
    let ticker_y = format!("ih-{}", fmt_px(ticker_h));
    let label_w = broadcast_ref_to_1080_px(680.0);
    let content_x = label_w + broadcast_ref_to_1080_px(48.0);
    let topic_badge_x = content_x;
    let topic_badge_font_size = 23;
    let topic_badge_pad_x = broadcast_ref_to_1080_px(24.0);
    let topic_badge_w = estimated_condensed_text_width("NOW DISCUSSING", topic_badge_font_size)
        + topic_badge_pad_x * 2.0;
    let topic_badge_h = f64::from(topic_badge_font_size) + broadcast_ref_to_1080_px(16.0);
    let topic_text_x = topic_badge_x + topic_badge_w + broadcast_ref_to_1080_px(28.0);
    let topic_badge_y = format!("ih-{}", fmt_px(ticker_h / 2.0 + topic_badge_h / 2.0));
    let topic_badge_text_y = format!("h-{}", fmt_px(ticker_h / 2.0 + 14.0));
    let ticker_text_y = format!("h-{}", fmt_px(ticker_h / 2.0 + 20.0));
    let label_text_y = format!("h-{}", fmt_px(ticker_h / 2.0 + 16.0));
    let cycle = st.ticker_sponsor_duration
        + st.ticker_fade_duration
        + st.ticker_topic_duration
        + st.ticker_fade_duration;
    let entries = broadcast_ticker_entries(c);
    let first_entry_time = entries
        .iter()
        .map(|t| t.time_seconds)
        .min_by(f64::total_cmp);
    let sponsor_phase_enable = if let Some(first_topic) = first_entry_time {
        format!(
            "(lt(t\\,{first_topic})+lt(mod(t\\,{cycle})\\,{sponsor})+gte(mod(t\\,{cycle})\\,{topic_end}))",
            sponsor = st.ticker_sponsor_duration,
            topic_end = cycle - st.ticker_fade_duration
        )
    } else {
        "1".to_string()
    };
    let topic_fade_start = (st.ticker_sponsor_duration - st.ticker_fade_duration).max(0.0);
    let topic_hold_end = st.ticker_sponsor_duration + st.ticker_topic_duration;
    let sponsor_alpha = first_entry_time
        .filter(|_| st.ticker_fade_duration > 0.0)
        .map(|first_topic| {
            format!(
                "if(lt(t\\,{first_topic})\\,1\\,if(lt(mod(t\\,{cycle})\\,{topic_fade_start})\\,1\\,if(lt(mod(t\\,{cycle})\\,{sponsor})\\,({sponsor}-mod(t\\,{cycle}))/{fade}\\,if(lt(mod(t\\,{cycle})\\,{topic_hold_end})\\,0\\,(mod(t\\,{cycle})-{topic_hold_end})/{fade}))))",
                sponsor = st.ticker_sponsor_duration,
                fade = st.ticker_fade_duration,
            )
        });
    let topic_alpha = (st.ticker_fade_duration > 0.0).then(|| {
        format!(
            "if(lt(mod(t\\,{cycle})\\,{sponsor})\\,(mod(t\\,{cycle})-{topic_fade_start})/{fade}\\,if(lt(mod(t\\,{cycle})\\,{topic_hold_end})\\,1\\,({cycle}-mod(t\\,{cycle}))/{fade}))",
            sponsor = st.ticker_sponsor_duration,
            fade = st.ticker_fade_duration,
        )
    });
    let sponsor_enable = format!(
        "(not(between(t\\,{s}\\,{e}))*{sponsor_phase_enable})",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let sponsor_items = if c.sponsors.is_empty() {
        vec![show.to_string()]
    } else {
        c.sponsors
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
    };
    let mut out = vec![
        format!(
            "drawbox=x=0:y={ticker_y}:w=iw:h={ticker_h}:color={navy}@1:t=fill:enable='{enable}'",
            ticker_h = fmt_px(ticker_h)
        ),
        format!("drawbox=x=0:y={ticker_y}:w=iw:h=3:color={gold}@1:t=fill:enable='{enable}'"),
    ];
    if !entries.is_empty() {
        for topic in entries {
            let start = topic.time_seconds;
            let end = start + st.ticker_topic_duration;
            let next_topic = entries
                .iter()
                .filter_map(|candidate| {
                    (candidate.time_seconds > start).then_some(candidate.time_seconds)
                })
                .min_by(f64::total_cmp)
                .unwrap_or(1.0e12);
            let topic_enable = format!(
                "(not(between(t\\,{host_s}\\,{host_e}))*gte(t\\,{start})*lt(t\\,{next_topic})*gte(mod(t\\,{cycle})\\,{topic_s})*lt(mod(t\\,{cycle})\\,{topic_e}))",
                host_s = st.host_intro_start,
                host_e = st.host_intro_end,
                topic_s = topic_fade_start,
                topic_e = cycle,
            );
            out.push(format!(
                "drawbox=x={x}:y={y}:w={w}:h={h}:color={cyan}@1:t=fill:enable='between(t\\,{start}\\,{end})'",
                x = fmt_px(topic_badge_x),
                y = topic_badge_y,
                w = fmt_px(topic_badge_w),
                h = fmt_px(topic_badge_h),
            ).replace(&format!("between(t\\,{start}\\,{end})"), &topic_enable));
            out.push(
                broadcast_drawtext(
                    "NOW DISCUSSING",
                    &fmt_px(topic_badge_x + topic_badge_pad_x),
                    &topic_badge_text_y,
                    topic_badge_font_size,
                    navy,
                    topic_alpha.as_deref(),
                    None,
                    true,
                )
                .replace(
                    ":enable='between(t\\,0\\,0)'",
                    &format!(":enable='{topic_enable}'"),
                ),
            );
            out.push(
                broadcast_drawtext_body(
                    &topic.text,
                    &fmt_px(topic_text_x),
                    &ticker_text_y,
                    33,
                    "#FFFFFF",
                    topic_alpha.as_deref(),
                    None,
                    false,
                )
                .replace(
                    ":enable='between(t\\,0\\,0)'",
                    &format!(":enable='{topic_enable}'"),
                ),
            );
        }
    }
    if sponsor_items.iter().any(|item| !item.trim().is_empty()) {
        out.extend(format_sponsor_marquee_filters(
            &sponsor_items,
            content_x,
            &ticker_text_y,
            sponsor_alpha.as_deref(),
            &sponsor_enable,
        ));
    }
    // Match the reference overlay renderer: ticker/topic text can
    // move continuously, but the branded gold label owns the left
    // lane and masks anything passing underneath it.
    out.push(format!(
        "drawbox=x=0:y={ticker_y}:w={label_w}:h={ticker_h}:color={gold}@1:t=fill:enable='{enable}'",
        label_w = fmt_px(label_w),
        ticker_h = fmt_px(ticker_h)
    ));
    out.push(
        broadcast_drawtext(
            show,
            &format!("({}-text_w)/2", fmt_px(label_w)),
            &label_text_y,
            26,
            navy,
            None,
            None,
            true,
        )
        .replace(
            ":enable='between(t\\,0\\,0)'",
            &format!(":enable='{enable}'"),
        ),
    );
    out
}

fn format_sponsor_marquee_filters(
    sponsors: &[String],
    content_x: f64,
    y: &str,
    alpha: Option<&str>,
    enable: &str,
) -> Vec<String> {
    let text_size = 32;
    let diamond_size = 42;
    let diamond_gap = 36.0;
    let item_gap = 36.0;
    let diamond_y = format!("{y}-4");
    let mut offsets = Vec::new();
    let mut cursor = 0.0;
    for sponsor in sponsors {
        let text_w = estimated_condensed_text_width(sponsor, text_size);
        offsets.push((sponsor.as_str(), cursor, text_w));
        cursor += text_w + diamond_gap;
        offsets.push(("◆", cursor, estimated_diamond_width(diamond_size)));
        cursor += estimated_diamond_width(diamond_size) + item_gap;
    }
    let loop_w = cursor.max(1.0);
    let mut out = Vec::new();
    for repeat in 0..3 {
        let repeat_offset = loop_w * f64::from(repeat);
        for (text, offset, _) in &offsets {
            let x = format!(
                "{}+{}-mod(t*70\\,{})",
                fmt_px(content_x),
                fmt_px(repeat_offset + offset),
                fmt_px(loop_w)
            );
            let filter = if *text == "◆" {
                broadcast_drawtext_sponsor(
                    text,
                    &x,
                    &diamond_y,
                    diamond_size,
                    "#CBD5E1",
                    alpha,
                    None,
                    false,
                )
            } else {
                broadcast_drawtext(text, &x, y, text_size, "#CBD5E1", alpha, None, true)
            };
            out.push(filter.replace(
                ":enable='between(t\\,0\\,0)'",
                &format!(":enable='{enable}'"),
            ));
        }
    }
    out
}

fn format_host_intro_strip(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    gold: &str,
    gold_light: &str,
    navy: &str,
) -> Vec<String> {
    let start = st.host_intro_start;
    let end = st.host_intro_end;
    let enable = format!("between(t\\,{start}\\,{end})");
    let host_h = broadcast_ref_to_1080_px(st.host_strip_height);
    let host_y = format!("ih-{}", fmt_px(host_h));
    let divider_h = broadcast_ref_to_1080_px(110.0);
    let divider_y = format!("ih-{}", fmt_px((host_h + divider_h) / 2.0));
    let name_y = format!("h-{}", fmt_px(host_h / 2.0 - 10.0));
    let title_y = format!("h-{}", fmt_px(host_h / 2.0 + 50.0));
    let host_a_text_x = fmt_px(broadcast_ref_to_1080_px(60.0 + 260.0 + 28.0));
    let host_b_text_x = format!(
        "w-text_w-{}",
        fmt_px(broadcast_ref_to_1080_px(60.0 + 260.0 + 28.0))
    );
    let mut out = vec![
        format!(
            "drawbox=x=0:y={host_y}:w=iw/2:h={host_h}:color={gold}@1:t=fill:enable='{enable}'",
            host_h = fmt_px(host_h)
        ),
        format!(
            "drawbox=x=iw/2:y={host_y}:w=iw/2:h={host_h}:color={gold_light}@1:t=fill:enable='{enable}'",
            host_h = fmt_px(host_h)
        ),
        format!(
            "drawbox=x=iw/2:y={divider_y}:w=2:h={divider_h}:color={navy}@0.2:t=fill:enable='{enable}'",
            divider_h = fmt_px(divider_h)
        ),
    ];
    out.extend(format_host_intro_host(
        &c.host_a,
        &host_a_text_x,
        &name_y,
        &title_y,
        navy,
        start,
        end,
    ));
    out.extend(format_host_intro_host(
        &c.host_b,
        &host_b_text_x,
        &name_y,
        &title_y,
        navy,
        start,
        end,
    ));
    out
}

fn format_host_intro_host(
    host: &BroadcastHost,
    x: &str,
    name_y: &str,
    title_y: &str,
    navy: &str,
    start: f64,
    end: f64,
) -> Vec<String> {
    if host.name.trim().is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.push(broadcast_drawtext(
        &host.name.to_uppercase(),
        x,
        name_y,
        50,
        navy,
        None,
        Some((start, end)),
        true,
    ));
    if !host.title.trim().is_empty() {
        out.push(broadcast_drawtext(
            &host.title.to_uppercase(),
            x,
            title_y,
            29,
            navy,
            None,
            Some((start, end)),
            true,
        ));
    }
    out
}

fn format_chapter_cards(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, chapter) in c.chapters.iter().enumerate() {
        let start = chapter.time_seconds.max(st.title_visible_end);
        let end = start + st.chapter_display_duration;
        out.push(format!("drawbox=x=iw*0.30:y=ih*0.38:w=80:h=50:color={gold}@1:t=fill:enable='between(t\\,{start}\\,{end})'"));
        out.push(format!("drawbox=x=iw*0.30+80:y=ih*0.38:w=iw*0.40:h=50:color={navy}@0.88:t=fill:enable='between(t\\,{start}\\,{end})'"));
        out.push(broadcast_drawtext(
            &(idx + 1).to_string(),
            "w*0.30+32",
            "h*0.38+36",
            36,
            navy,
            None,
            Some((start, end)),
            true,
        ));
        out.push(broadcast_drawtext(
            &chapter.text.to_uppercase(),
            "w*0.30+96",
            "h*0.38+36",
            36,
            "#FFFFFF",
            None,
            Some((start, end)),
            true,
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn broadcast_drawtext(
    text: &str,
    x: &str,
    y: &str,
    size: u32,
    color: &str,
    alpha: Option<&str>,
    window: Option<(f64, f64)>,
    bold: bool,
) -> String {
    let fontfile = pick_fontfile_attr();
    let border = if bold { ":borderw=1" } else { "" };
    let alpha = alpha.map(|a| format!(":alpha='{a}'")).unwrap_or_default();
    let enable = if let Some((start, end)) = window {
        format!(":enable='between(t\\,{start}\\,{end})'")
    } else {
        ":enable='between(t\\,0\\,0)'".into()
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{border}:x={x}:y={y}{alpha}{enable}",
        text = drawtext_escape(text),
        font = fontfile,
    )
}

#[allow(clippy::too_many_arguments)]
fn broadcast_drawtext_body(
    text: &str,
    x: &str,
    y: &str,
    size: u32,
    color: &str,
    alpha: Option<&str>,
    window: Option<(f64, f64)>,
    bold: bool,
) -> String {
    let fontfile = pick_body_fontfile_attr();
    let border = if bold { ":borderw=1" } else { "" };
    let alpha = alpha.map(|a| format!(":alpha='{a}'")).unwrap_or_default();
    let enable = if let Some((start, end)) = window {
        format!(":enable='between(t\\,{start}\\,{end})'")
    } else {
        ":enable='between(t\\,0\\,0)'".into()
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{border}:x={x}:y={y}{alpha}{enable}",
        text = drawtext_escape(text),
        font = fontfile,
    )
}

#[allow(clippy::too_many_arguments)]
fn broadcast_drawtext_sponsor(
    text: &str,
    x: &str,
    y: &str,
    size: u32,
    color: &str,
    alpha: Option<&str>,
    window: Option<(f64, f64)>,
    bold: bool,
) -> String {
    let fontfile = pick_sponsor_fontfile_attr();
    let border = if bold { ":borderw=1" } else { "" };
    let alpha = alpha.map(|a| format!(":alpha='{a}'")).unwrap_or_default();
    let enable = if let Some((start, end)) = window {
        format!(":enable='between(t\\,{start}\\,{end})'")
    } else {
        ":enable='between(t\\,0\\,0)'".into()
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{border}:x={x}:y={y}{alpha}{enable}",
        text = drawtext_escape(text),
        font = fontfile,
    )
}

fn broadcast_title_alpha(st: &BroadcastOverlayStyle) -> String {
    format!(
        "if(lt(t\\,{fade_in})\\,t/{fade_in}\\,if(lt(t\\,{fade_out})\\,1\\,({end}-t)/({end}-{fade_out})))",
        fade_in = st.title_fade_in_end,
        fade_out = st.title_fade_out_start,
        end = st.title_visible_end,
    )
}

fn estimated_condensed_text_width(text: &str, font_size: u32) -> f64 {
    let weighted_chars = text
        .chars()
        .map(|ch| if ch.is_whitespace() { 0.35 } else { 0.57 })
        .sum::<f64>();
    weighted_chars * f64::from(font_size)
}

fn estimated_broadcast_text_width(text: &str, font_size: u32) -> f64 {
    estimated_condensed_text_width(text, font_size)
}

fn estimated_diamond_width(font_size: u32) -> f64 {
    f64::from(font_size) * 0.72
}

fn normalize_hex(value: &str) -> String {
    if value.starts_with('#') {
        value.to_string()
    } else {
        format!("#{value}")
    }
}

fn broadcast_ref_to_1080_px(value: f64) -> f64 {
    value * 0.5
}

fn fmt_px(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.001 {
        format!("{rounded:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn format_broadcast_overlay_graph(
    in_label: &str,
    out_label: &str,
    overlay: &BroadcastOverlayPlan,
    parts: &[String],
) -> String {
    let asset_overlays = broadcast_asset_overlays(overlay);
    if asset_overlays.is_empty() {
        let mut graph = String::new();
        graph.push_str(in_label);
        graph.push_str(&parts.join(","));
        graph.push_str(out_label);
        return graph;
    }

    let mut graph = String::new();
    let mut current = "[broadcast_base]".to_string();
    graph.push_str(in_label);
    graph.push_str(&parts.join(","));
    graph.push_str(&current);
    for (idx, asset) in asset_overlays.iter().enumerate() {
        let photo_label = format!("[broadcast_asset{idx}]");
        let next = if idx + 1 == asset_overlays.len() {
            out_label.to_string()
        } else {
            format!("[broadcast_asset_v{idx}]")
        };
        graph.push(';');
        graph.push_str(&format!(
            "movie='{path}',scale={size}:{size}:force_original_aspect_ratio=increase,crop={size}:{size},format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lte((X-W/2)*(X-W/2)+(Y-H/2)*(Y-H/2)\\,(W/2)*(W/2))\\,255\\,0)'{photo};{current}{photo}overlay=x={x}:y={y}:enable='between(t\\,{start}\\,{end})'{next}",
            path = filter_escape_single_quoted(&asset.path.to_string_lossy()),
            size = asset.size,
            photo = photo_label,
            current = current,
            x = asset.x,
            y = asset.y,
            start = asset.start_s,
            end = asset.end_s,
            next = next,
        ));
        current = next;
    }
    graph
}

struct BroadcastAssetOverlay {
    path: PathBuf,
    x: String,
    y: String,
    size: u32,
    start_s: f64,
    end_s: f64,
}

fn broadcast_asset_overlays(overlay: &BroadcastOverlayPlan) -> Vec<BroadcastAssetOverlay> {
    let c = &overlay.config;
    let st = &c.style;
    let mut out = Vec::new();
    if c.short_form_mode {
        if let Some(path) = resolve_project_overlay_asset(overlay, c.brand_logo_path.as_deref()) {
            out.push(BroadcastAssetOverlay {
                path,
                x: "32".into(),
                y: "main_h-70".into(),
                size: 52,
                start_s: 0.0,
                end_s: 1.0e9,
            });
        }
        return out;
    }
    let photo_size = broadcast_ref_to_1080_px(260.0).round() as u32;
    let host_h = broadcast_ref_to_1080_px(st.host_strip_height);
    let photo_y = format!("main_h-{}", fmt_px((host_h + f64::from(photo_size)) / 2.0));
    if let Some(path) = resolve_project_overlay_asset(overlay, c.host_a.photo_path.as_deref()) {
        out.push(BroadcastAssetOverlay {
            path,
            x: "30".into(),
            y: photo_y.clone(),
            size: photo_size,
            start_s: st.host_intro_start,
            end_s: st.host_intro_end,
        });
    }
    if let Some(path) = resolve_project_overlay_asset(overlay, c.host_b.photo_path.as_deref()) {
        out.push(BroadcastAssetOverlay {
            path,
            x: "main_w-160".into(),
            y: photo_y,
            size: photo_size,
            start_s: st.host_intro_start,
            end_s: st.host_intro_end,
        });
    }
    out
}

fn resolve_project_overlay_asset(
    overlay: &BroadcastOverlayPlan,
    rel_path: Option<&str>,
) -> Option<PathBuf> {
    let rel_path = rel_path?;
    if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.split('/').any(|p| p == "..") {
        return None;
    }
    let path = overlay.project_root.join(rel_path);
    path.is_file().then_some(path)
}

/// Per-animation x / y / alpha expressions. `x` and `y` always have
/// the resting position as the default; `alpha` is empty when the
/// animation doesn't fade. Fade ramps live in `alpha`; slide ramps
/// live in `x` or `y` depending on the title's resting position.
struct AnimatedExpressions {
    x: String,
    y: String,
    /// Either empty or `:alpha='<expr>'` ready to splice in.
    alpha: String,
}

/// Build the animation expressions for a title. Pure function so it
/// can be unit-tested without spinning up ffmpeg.
fn apply_title_animation(t: &TitlePlan, resting_x: &str, resting_y: &str) -> AnimatedExpressions {
    let start = t.start_s;
    let end = t.end_s;
    let ramp = ANIMATION_RAMP_S;
    let fade_in_end = start + ramp;
    let fade_out_start = (end - ramp).max(start);

    match t.animation {
        TitleAnimation::None => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            alpha: String::new(),
        },
        TitleAnimation::FadeIn => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // Linear ramp 0→1 over [start, start+ramp]; 1 thereafter.
            // `if(lt(t,A), B, C)` evaluates B when t<A, else C.
            alpha: format!(":alpha='if(lt(t\\,{fade_in_end})\\,(t-{start})/{ramp}\\,1)'"),
        },
        TitleAnimation::FadeOut => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // 1 until [end-ramp, end], then ramp 1→0.
            alpha: format!(":alpha='if(lt(t\\,{fade_out_start})\\,1\\,({end}-t)/{ramp})'"),
        },
        TitleAnimation::FadeInOut => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // Three pieces: ramp in, plateau, ramp out.
            alpha: format!(
                ":alpha='if(lt(t\\,{fade_in_end})\\,(t-{start})/{ramp}\\,if(lt(t\\,{fade_out_start})\\,1\\,({end}-t)/{ramp}))'"
            ),
        },
        TitleAnimation::SlideIn => slide_expressions(
            t.position,
            resting_x,
            resting_y,
            SlideDirection::In,
            start,
            end,
            ramp,
        ),
        TitleAnimation::SlideOut => slide_expressions(
            t.position,
            resting_x,
            resting_y,
            SlideDirection::Out,
            start,
            end,
            ramp,
        ),
    }
}

#[derive(Clone, Copy)]
enum SlideDirection {
    In,
    Out,
}

/// Build x / y expressions for a slide animation. The slide direction
/// follows the title's resting position:
///
///   - `Top` slides in from above (y goes from `-text_h` to resting).
///   - `Bottom` slides in from below (y from `h` to resting).
///   - `Center` slides in from the right (x from `w` to resting).
///
/// Slide-out reverses each interpolation.
fn slide_expressions(
    position: TitlePosition,
    resting_x: &str,
    resting_y: &str,
    dir: SlideDirection,
    start: f64,
    end: f64,
    ramp: f64,
) -> AnimatedExpressions {
    let ramp_in_end = start + ramp;
    let ramp_out_start = (end - ramp).max(start);
    // For Top/Bottom we slide along Y; for Center we slide along X.
    let slide_along_y = matches!(position, TitlePosition::Top | TitlePosition::Bottom);
    let off_y = match position {
        TitlePosition::Top => "-text_h".to_string(),
        TitlePosition::Bottom => "h".to_string(),
        TitlePosition::Center => resting_y.to_string(),
    };
    let off_x = match position {
        TitlePosition::Center => "w".to_string(),
        _ => resting_x.to_string(),
    };

    // Linear interp helper: write an `if(lt(t,A), <off>+<rest-off>*progress, <rest>)`-style
    // expression. We build it as ffmpeg-string.
    let (x_expr, y_expr) = match (dir, slide_along_y) {
        (SlideDirection::In, true) => (
            resting_x.to_string(),
            // y(t) = off_y + (resting - off_y) * progress, where
            // progress = (t-start)/ramp clamped to 1.
            format!(
                "if(lt(t\\,{ramp_in_end})\\,{off_y}+({resting_y}-({off_y}))*(t-{start})/{ramp}\\,{resting_y})"
            ),
        ),
        (SlideDirection::In, false) => (
            format!(
                "if(lt(t\\,{ramp_in_end})\\,{off_x}+({resting_x}-({off_x}))*(t-{start})/{ramp}\\,{resting_x})"
            ),
            resting_y.to_string(),
        ),
        (SlideDirection::Out, true) => (
            resting_x.to_string(),
            // y(t) = resting until ramp_out_start, then linear to off_y.
            format!(
                "if(lt(t\\,{ramp_out_start})\\,{resting_y}\\,{resting_y}+({off_y}-({resting_y}))*(t-{ramp_out_start})/{ramp})"
            ),
        ),
        (SlideDirection::Out, false) => (
            format!(
                "if(lt(t\\,{ramp_out_start})\\,{resting_x}\\,{resting_x}+({off_x}-({resting_x}))*(t-{ramp_out_start})/{ramp})"
            ),
            resting_y.to_string(),
        ),
    };

    AnimatedExpressions {
        x: x_expr,
        y: y_expr,
        alpha: String::new(),
    }
}

/// Escape characters drawtext treats as special inside `text='...'`.
/// drawtext uses `\` to escape `:`, `'`, `\`, and `,` — we don't
/// support newlines (single-line titles in v1).
fn drawtext_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '\'' => "\\'".to_string(),
            ':' => "\\:".to_string(),
            ',' => "\\,".to_string(),
            '\n' => "\\n".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Best-effort condensed font lookup for broadcast labels. Returns
/// either an empty string or a `:fontfile=<path>` segment ready to
/// splice into the filter args.
fn pick_fontfile_attr() -> String {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Avenir Next Condensed.ttc",
        "/System/Library/Fonts/Supplemental/Arial Narrow Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial Narrow.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    for path in CANDIDATES {
        if std::path::Path::new(path).is_file() {
            return format!(":fontfile={path}");
        }
    }
    String::new()
}

/// Best-effort body font lookup for readable ticker topics and
/// subtitles. The reference renderer keeps these non-condensed while
/// the surrounding broadcast chrome stays condensed.
fn pick_body_fontfile_attr() -> String {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Avenir Next.ttc",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    for path in CANDIDATES {
        if std::path::Path::new(path).is_file() {
            return format!(":fontfile={path}");
        }
    }
    String::new()
}

/// Sponsor ticker text needs broad Unicode coverage for the diamond
/// separator. Some condensed system fonts render `◆` as a missing-glyph
/// box, which is worse than using a slightly less condensed face here.
fn pick_sponsor_fontfile_attr() -> String {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Avenir Next.ttc",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    for path in CANDIDATES {
        if std::path::Path::new(path).is_file() {
            return format!(":fontfile={path}");
        }
    }
    String::new()
}

/// Effective on-timeline duration of a segment, accounting for any
/// `awidat.time_remap` curve or `awidat.speed` factor. A 4s clip at
/// 2× plays in 2s; at 0.5× it plays in 8s.
fn effective_duration(seg: &TimelineSegment) -> f64 {
    let freeze_duration = seg.freeze.map(|freeze| freeze.duration_s).unwrap_or(0.0);
    if let Some(time_remap) = seg.time_remap.as_ref() {
        time_remap.timeline_time_at_source(seg.duration_s) + freeze_duration
    } else {
        (seg.duration_s + freeze_duration) / segment_speed(seg)
    }
}

fn visible_effective_duration(seg: &TimelineSegment) -> f64 {
    (effective_duration(seg) - seg.pre_handle_s - seg.post_handle_s).max(0.0)
}

fn segment_speed(seg: &TimelineSegment) -> f64 {
    if let Some(time_remap) = seg.time_remap.as_ref() {
        let timeline_duration = time_remap.timeline_time_at_source(seg.duration_s);
        if timeline_duration > 0.0 {
            return seg.duration_s / timeline_duration;
        }
    }
    match seg.speed {
        Some(speed) if speed.factor > 0.0 => speed.factor,
        _ => 1.0,
    }
}

fn speed_needs_filter(speed: SpeedPlan) -> bool {
    speed.reverse || ((speed.factor - 1.0).abs() > 1e-9 && speed.factor > 0.0)
}

fn speed_video_filter(speed: SpeedPlan) -> String {
    let mut filter = format!("setpts={}*PTS", 1.0 / speed.factor);
    match speed.frame_blending {
        FrameBlending::Nearest => {}
        FrameBlending::Blend => {
            filter.push_str(",tblend=all_mode=average,framerate=fps=30000/1001");
        }
        FrameBlending::Flow => {
            filter.push_str(",minterpolate=mi_mode=mci:mc_mode=aobmc:vsbmc=1:fps=30000/1001");
        }
    }
    if speed.reverse {
        let setpts = filter;
        format!("reverse,{setpts}")
    } else {
        filter
    }
}

fn speed_audio_filter(speed: SpeedPlan) -> String {
    let retime = if speed.maintain_pitch {
        atempo_chain(speed.factor)
    } else {
        format!(
            "asetrate=sample_rate*{},aresample=sample_rate",
            fmt_filter_num(speed.factor)
        )
    };
    if speed.reverse {
        format!("areverse,{retime}")
    } else {
        retime
    }
}

fn time_remap_setpts_expr(plan: &TimeRemapPlan) -> String {
    let time_var = "(PTS-STARTPTS)*TB";
    let last = plan.points.len() - 1;
    let mut expr = time_remap_segment_expr(plan.points[last - 1], plan.points[last], time_var);
    for idx in (0..(last - 1)).rev() {
        let segment_expr =
            time_remap_segment_expr(plan.points[idx], plan.points[idx + 1], time_var);
        expr = format!(
            "if(lte({time_var}\\,{end})\\,{segment_expr}\\,{expr})",
            end = fmt_filter_num(plan.points[idx + 1].source_time_s),
        );
    }
    expr
}

/// Build the `setpts` expression for a TimeRemap transition window
/// positioned at `window_start_s` seconds inside the input stream.
///
/// The expression keeps frames outside the window at identity (so
/// `xfade` still sees correctly-timed surrounding content) and warps
/// frames inside the window through the piecewise-linear samples
/// produced by the proto helper. The returned string is the bare
/// expression — callers wrap it in `setpts='<expr>/TB'`.
///
/// `window_start_s` may be `0.0` for the incoming clip's head retime
/// or `outgoing_duration - transition_duration` for the outgoing
/// tail retime.
fn time_remap_transition_setpts_expr(
    curve: &transitions::TimeRemapSetptsCurve,
    window_start_s: f64,
) -> String {
    let time_var = "(PTS-STARTPTS)*TB";
    let samples = &curve.samples;
    // Build piecewise expression segment-by-segment, mirroring the
    // shape of `time_remap_setpts_expr`. The final fallback branch is
    // the last segment's linear extrapolation, so frames past the
    // window land at `window_start_s + samples.last().t_out`.
    let last_idx = samples.len() - 1;
    let last_pair = (samples[last_idx - 1], samples[last_idx]);
    let mut expr =
        time_remap_window_segment_expr(last_pair.0, last_pair.1, window_start_s, time_var);
    for i in (0..last_idx - 1).rev() {
        let pair = (samples[i], samples[i + 1]);
        let segment_expr = time_remap_window_segment_expr(pair.0, pair.1, window_start_s, time_var);
        let end_t_in = window_start_s + samples[i + 1].0;
        expr = format!(
            "if(lte({time_var}\\,{end})\\,{segment_expr}\\,{expr})",
            end = fmt_filter_num(end_t_in),
        );
    }
    // Outside the window (before window_start_s), frames pass through
    // at identity so the surrounding clip plays at realtime.
    if window_start_s > 0.0 {
        expr = format!(
            "if(lt({time_var}\\,{start})\\,{time_var}\\,{expr})",
            start = fmt_filter_num(window_start_s),
        );
    }
    expr
}

fn time_remap_window_segment_expr(
    start: (f64, f64),
    end: (f64, f64),
    window_start_s: f64,
    time_var: &str,
) -> String {
    let source_span = end.0 - start.0;
    let timeline_span = end.1 - start.1;
    let slope = if source_span.abs() > 1e-12 {
        timeline_span / source_span
    } else {
        1.0
    };
    let start_t_in = window_start_s + start.0;
    let start_t_out = window_start_s + start.1;
    format!(
        "{}+({time_var}-{})*{}",
        fmt_filter_num(start_t_out),
        fmt_filter_num(start_t_in),
        fmt_filter_num(slope),
    )
}

fn time_remap_segment_expr(start: TimeRemapPoint, end: TimeRemapPoint, time_var: &str) -> String {
    let source_span = end.source_time_s - start.source_time_s;
    let timeline_span = end.timeline_time_s - start.timeline_time_s;
    let slope = timeline_span / source_span;
    format!(
        "{}+({time_var}-{})*{}",
        fmt_filter_num(start.timeline_time_s),
        fmt_filter_num(start.source_time_s),
        fmt_filter_num(slope)
    )
}

/// Decompose a speed factor into a chain of `atempo=` calls, each
/// in atempo's per-instance legal range `[0.5, 2.0]`. Returns a
/// string like `atempo=2.0,atempo=2.0` for factor=4, or `atempo=0.5,
/// atempo=0.6` for factor=0.3. Caller is responsible for prepending
/// the input label and appending the output label.
fn atempo_chain(factor: f64) -> String {
    // atempo's legal range is [0.5, 2.0] per filter instance.
    // - factor >= 0.5 && factor <= 2.0 → single atempo=<factor>.
    // - factor > 2.0 → chain atempo=2.0 stages until product >= factor,
    //   then a remainder.
    // - factor < 0.5 → chain atempo=0.5 stages until product <= factor,
    //   then a remainder.
    if (0.5..=2.0).contains(&factor) {
        return format!("atempo={factor}");
    }
    let mut stages = Vec::<f64>::new();
    let mut remaining = factor;
    if factor > 2.0 {
        while remaining > 2.0 {
            stages.push(2.0);
            remaining /= 2.0;
        }
    } else {
        while remaining < 0.5 {
            stages.push(0.5);
            remaining /= 0.5;
        }
    }
    stages.push(remaining);
    stages
        .into_iter()
        .map(|s| format!("atempo={s}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Map an OTIO transition kind or Awidat transition id to an ffmpeg
/// `xfade=transition=` name. Project-level render planning validates
/// this earlier; direct test/helper callers get an intentionally
/// invalid token for unsupported values instead of a wrong fallback.
fn map_transition_kind(kind: &str) -> String {
    transitions::resolve_ffmpeg_xfade(kind)
        .ok()
        .flatten()
        .unwrap_or("__unsupported_awidat_transition__")
        .to_string()
}

fn map_transition_plan_kind(transition: &TransitionPlan) -> String {
    transition
        .composition
        .as_ref()
        .and_then(transitions::resolve_composition_ffmpeg_xfade)
        .map(str::to_string)
        .unwrap_or_else(|| map_transition_kind(&transition.kind))
}

/// Build the ffmpeg argv that concats `segs` into `output_path` with
/// a single re-encode. The re-encode kills the DTS-seam scratch that
/// stream-copy concat produces at non-keyframe-aligned cut points.
/// libx264 medium preset / CRF 20, AAC 192k — universal compatibility.
///
/// Internally delegates the filter-graph construction to
/// [`FilterPlanner`] (Step 14.4 extraction); behaviour is byte-
/// identical to the prior monolithic builder. Callers wanting
/// transitions should use [`build_timeline_argv_with_transitions`]
/// (Step 14.5).
pub fn build_timeline_argv(segs: &[TimelineSegment], output_path: &Path) -> Vec<String> {
    build_timeline_argv_with_transitions(segs, &[], output_path)
}

/// Like [`build_timeline_argv`] but accepts a transitions slice that
/// gets composed into the filter graph. Wraps
/// [`build_timeline_argv_full`] with no titles — preserved for
/// callers that don't need title awareness.
pub fn build_timeline_argv_with_transitions(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_full(segs, transitions, &[], &[], None, None, None, output_path)
}

/// Like [`build_timeline_argv_with_transitions`] but also takes a
/// titles slice. Each [`TitlePlan`] becomes a `drawtext=` filter
/// chained off the master video output of the segment + transition
/// graph; alpha / x / y expressions handle title animations.
#[allow(clippy::too_many_arguments)]
pub fn build_timeline_argv_full(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_full_with_annotations(
        segs,
        transitions,
        video_overlays,
        &[],
        titles,
        broadcast_overlay,
        browser_broadcast_overlay,
        loudness_target,
        output_path,
    )
}

/// Build FFmpeg arguments with media overlays, annotation overlays, titles,
/// broadcast overlay, and optional final loudness processing.
#[allow(clippy::too_many_arguments)]
pub fn build_timeline_argv_full_with_annotations(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    annotations: &[AnnotationPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_full_with_annotations_and_ass(
        segs,
        transitions,
        video_overlays,
        annotations,
        titles,
        broadcast_overlay,
        browser_broadcast_overlay,
        loudness_target,
        output_path,
        None,
    )
}

/// Same as [`build_timeline_argv_full_with_annotations`] but with an
/// extra optional `ass_workdir`: when supplied, captions carrying
/// per-word timings are written as `.ass` files there and rendered
/// through the libass `subtitles=` filter instead of comma-chained
/// drawtext. Kept crate-private so the public surface stays the
/// drawtext-only legacy signature.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_timeline_argv_full_with_annotations_and_ass(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    annotations: &[AnnotationPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    output_path: &Path,
    ass_workdir: Option<&Path>,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    let first_matte_input =
        segs.len() + video_overlays.len() + usize::from(browser_broadcast_overlay.is_some());
    let video_overlays = assign_overlay_matte_input_indices(video_overlays, first_matte_input);
    let first_mask_input = first_matte_input + renderable_overlay_matte_count(&video_overlays);
    let segs = assign_mask_input_indices(segs, first_mask_input);
    for s in &segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    for overlay in &video_overlays {
        append_video_overlay_input_args(&mut argv, &overlay.segment);
    }
    if let Some(path) = browser_broadcast_overlay {
        argv.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    }
    for overlay in &video_overlays {
        if let Some(matte_source) = overlay.matte_source.as_ref() {
            argv.extend(["-i".into(), matte_source.to_string_lossy().into_owned()]);
        }
    }
    for segment in &segs {
        if let Some(mask_source) = renderable_mask_source(segment) {
            argv.extend(["-i".into(), mask_source.to_string_lossy().into_owned()]);
        }
    }
    let base = FilterPlanner::new(&segs, transitions).plan();
    let media = append_video_overlays(base, &video_overlays, segs.len());
    let annotated = append_annotations(media, annotations);
    let titles = if broadcast_overlay_owns_program_titles(broadcast_overlay) {
        &[]
    } else {
        titles
    };
    let ffmpeg_broadcast_overlay = browser_broadcast_overlay
        .is_none()
        .then_some(broadcast_overlay)
        .flatten();
    let mut planner = FilterPlanner::with_titles_and_broadcast_overlay(
        &[],
        &[],
        titles,
        ffmpeg_broadcast_overlay,
    );
    if let Some(dir) = ass_workdir {
        planner = planner.with_ass_workdir(dir.to_path_buf());
    }
    let plan = planner.decorate_video_filter(annotated.filter_complex, annotated.video_out_label);
    let mut filter_complex = plan.filter_complex;
    let video_out_label = if browser_broadcast_overlay.is_some() {
        let overlay_input = segs.len() + video_overlays.len();
        let out = "[browser_broadcast_v]".to_string();
        filter_complex.push_str(&format!(
            "{}[{overlay_input}:v:0]format=rgba[browser_broadcast_overlay];{}[browser_broadcast_overlay]overlay=x=0:y=0:format=auto{out};",
            if filter_complex.ends_with(';') || filter_complex.is_empty() { "" } else { ";" },
            plan.video_out_label,
        ));
        out
    } else {
        plan.video_out_label
    };
    let audio_out_label = append_timeline_loudness_filter(
        &mut filter_complex,
        annotated.audio_out_label,
        loudness_target,
    );
    argv.extend([
        "-filter_complex".into(),
        filter_complex,
        "-map".into(),
        video_out_label,
        "-map".into(),
        audio_out_label,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);
    argv.extend(output_color_tag_argv(&segs));
    argv.extend([
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

fn append_video_overlay_input_args(argv: &mut Vec<String>, segment: &TimelineSegment) {
    if still_graphic_asset(&segment.asset_path) {
        argv.extend([
            "-loop".into(),
            "1".into(),
            "-t".into(),
            format!("{}", segment.duration_s),
            "-i".into(),
            segment.asset_path.to_string_lossy().into_owned(),
        ]);
    } else {
        argv.extend([
            "-ss".into(),
            format!("{}", segment.start_s),
            "-t".into(),
            format!("{}", segment.duration_s),
            "-i".into(),
            segment.asset_path.to_string_lossy().into_owned(),
        ]);
    }
}

fn still_graphic_asset(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "svg" | "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp"
            )
        })
        .unwrap_or(false)
}

/// Build an ffmpeg argv for explicit audio-track mixing plus video timeline rendering.
/// Build an ffmpeg argv for timelines with first-class audio tracks.
/// Video streams are rendered video-only and final audio is mixed from
/// explicit audio-track plans.
#[allow(clippy::too_many_arguments)]
pub fn build_timeline_argv_with_audio_tracks(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    audio_tracks: &[AudioTrackPlan],
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_with_audio_tracks_and_annotations(
        segs,
        transitions,
        video_overlays,
        &[],
        titles,
        broadcast_overlay,
        browser_broadcast_overlay,
        loudness_target,
        audio_tracks,
        output_path,
    )
}

/// Build FFmpeg arguments for explicit audio-track timelines with media
/// overlays, annotation overlays, titles, and optional loudness processing.
#[allow(clippy::too_many_arguments)]
pub fn build_timeline_argv_with_audio_tracks_and_annotations(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    annotations: &[AnnotationPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    audio_tracks: &[AudioTrackPlan],
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_with_audio_tracks_and_annotations_and_ass(
        segs,
        transitions,
        video_overlays,
        annotations,
        titles,
        broadcast_overlay,
        browser_broadcast_overlay,
        loudness_target,
        audio_tracks,
        output_path,
        None,
    )
}

/// libass-aware variant of
/// [`build_timeline_argv_with_audio_tracks_and_annotations`]. When
/// `ass_workdir` is `Some`, eligible captions burn in via the
/// libass `subtitles=` filter. Otherwise the function is identical
/// to the public drawtext-only signature.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_timeline_argv_with_audio_tracks_and_annotations_and_ass(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    video_overlays: &[VideoOverlayPlan],
    annotations: &[AnnotationPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    browser_broadcast_overlay: Option<&Path>,
    loudness_target: Option<LoudnessTargetPlan>,
    audio_tracks: &[AudioTrackPlan],
    output_path: &Path,
    ass_workdir: Option<&Path>,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    let first_matte_input =
        segs.len() + video_overlays.len() + usize::from(browser_broadcast_overlay.is_some());
    let video_overlays = assign_overlay_matte_input_indices(video_overlays, first_matte_input);
    let first_mask_input = first_matte_input + renderable_overlay_matte_count(&video_overlays);
    let segs = assign_mask_input_indices(segs, first_mask_input);
    for s in &segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    for overlay in &video_overlays {
        append_video_overlay_input_args(&mut argv, &overlay.segment);
    }
    if let Some(path) = browser_broadcast_overlay {
        argv.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    }
    for overlay in &video_overlays {
        if let Some(matte_source) = overlay.matte_source.as_ref() {
            argv.extend(["-i".into(), matte_source.to_string_lossy().into_owned()]);
        }
    }
    for segment in &segs {
        if let Some(mask_source) = renderable_mask_source(segment) {
            argv.extend(["-i".into(), mask_source.to_string_lossy().into_owned()]);
        }
    }
    for track in audio_tracks {
        for item in &track.items {
            let AudioTrackItemPlan::Clip(c) = item else {
                continue;
            };
            argv.extend([
                "-ss".into(),
                format!("{}", c.start_s),
                "-t".into(),
                format!("{}", c.duration_s),
                "-i".into(),
                c.asset_path.to_string_lossy().into_owned(),
            ]);
        }
    }

    let fallback_video_duration = audio_tracks
        .iter()
        .map(audio_track_duration)
        .fold(0.1_f64, f64::max);
    let mut filter = plan_video_only_filter(&segs, transitions, fallback_video_duration);
    let base_video_label = "[vonly]";
    let media = append_video_overlays(
        FilterPlan {
            filter_complex: filter,
            video_out_label: base_video_label.to_string(),
            audio_out_label: String::new(),
        },
        &video_overlays,
        segs.len(),
    );
    let annotated = append_annotations(media, annotations);
    filter = annotated.filter_complex;
    let mut video_label = annotated.video_out_label;
    let titles = if broadcast_overlay_owns_program_titles(broadcast_overlay) {
        &[]
    } else {
        titles
    };
    let ffmpeg_broadcast_overlay = browser_broadcast_overlay
        .is_none()
        .then_some(broadcast_overlay)
        .flatten();
    if !titles.is_empty() || ffmpeg_broadcast_overlay.is_some() {
        let mut planner = FilterPlanner::with_titles_and_broadcast_overlay(
            &[],
            &[],
            titles,
            ffmpeg_broadcast_overlay,
        );
        if let Some(dir) = ass_workdir {
            planner = planner.with_ass_workdir(dir.to_path_buf());
        }
        let decorated = planner.decorate_video_filter(filter, video_label.clone());
        filter = decorated.filter_complex;
        video_label = decorated.video_out_label;
    }

    if browser_broadcast_overlay.is_some() {
        let overlay_input = segs.len() + video_overlays.len();
        let out = "[browser_broadcast_v]".to_string();
        filter.push_str(&format!(
            "{}[{overlay_input}:v:0]format=rgba[browser_broadcast_overlay];{video_label}[browser_broadcast_overlay]overlay=x=0:y=0:format=auto{out};",
            if filter.ends_with(';') || filter.is_empty() { "" } else { ";" },
        ));
        video_label = out;
    }

    let mut next_input = segs.len()
        + video_overlays.len()
        + usize::from(browser_broadcast_overlay.is_some())
        + renderable_overlay_matte_count(&video_overlays)
        + renderable_mask_count(&segs);
    let audio_label = plan_audio_mix_filter(&mut filter, audio_tracks, &mut next_input);
    let audio_label = append_timeline_loudness_filter(&mut filter, audio_label, loudness_target);
    argv.extend([
        "-filter_complex".into(),
        filter,
        "-map".into(),
        video_label,
        "-map".into(),
        audio_label,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);
    argv.extend(output_color_tag_argv(&segs));
    argv.extend([
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

fn plan_video_only_filter(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    fallback_duration_s: f64,
) -> String {
    let mut filter = String::new();
    if segs.is_empty() {
        filter.push_str(&format!(
            "color=c=black:s=1280x720:r=30:d={fallback_duration_s}[vonly];"
        ));
        return filter;
    }
    let video_inputs: Vec<String> = (0..segs.len())
        .map(|i| stage_segment_video_input(&mut filter, i, &segs[i]))
        .collect();
    if transitions.is_empty() {
        for v in &video_inputs {
            filter.push_str(v);
        }
        filter.push_str(&format!("concat=n={}:v=1:a=0[vonly];", video_inputs.len()));
        return filter;
    }

    // Explicit audio projects still preserve visual transition chains;
    // audio transitions are owned by the audio tracks.
    let transition_after = transition_edge_map(segs.len(), transitions);
    let mut concat_inputs = Vec::new();
    let mut i = 0;
    let mut transition_id = 0;
    while i < segs.len() {
        let mut current_v = video_inputs[i].clone();
        let mut current_duration = effective_duration(&segs[i]);
        let mut group_end = i;
        while group_end + 1 < segs.len() {
            let Some(t) = transition_after[group_end] else {
                break;
            };
            let next = group_end + 1;
            let label = format!("[xv{transition_id}]");
            let offset = (current_duration - t.duration_s).max(0.0);
            filter.push_str(&format!(
                "{}{}xfade=transition={}:duration={}:offset={}{};",
                current_v,
                video_inputs[next],
                map_transition_plan_kind(t),
                t.duration_s,
                offset,
                label
            ));
            current_v = label;
            current_duration = current_duration + effective_duration(&segs[next]) - t.duration_s;
            transition_id += 1;
            group_end = next;
        }
        concat_inputs.push(current_v);
        i = group_end + 1;
    }
    for v in &concat_inputs {
        filter.push_str(v);
    }
    filter.push_str(&format!("concat=n={}:v=1:a=0[vonly];", concat_inputs.len()));
    filter
}

fn audio_track_duration(track: &AudioTrackPlan) -> f64 {
    track
        .items
        .iter()
        .map(|item| match item {
            AudioTrackItemPlan::Clip(c) => {
                if let Some(speed) = c.speed
                    && speed.factor > 0.0
                {
                    return c.duration_s / speed.factor;
                }
                c.duration_s
            }
            AudioTrackItemPlan::Gap { duration_s } => *duration_s,
        })
        .sum()
}

fn stage_segment_video_input(filter: &mut String, i: usize, seg: &TimelineSegment) -> String {
    let mut video_label = format!("[{i}:v:0]");
    if let Some(color) = seg.color_correction.as_ref()
        && let Some(chain) = color_filter_chain(color)
    {
        let cv = format!("[cv{i}]");
        filter.push_str(&format!("{video_label}{chain}{cv};"));
        video_label = cv;
    }
    let blurred_label = append_clip_blur_filter(
        filter,
        &video_label,
        &i.to_string(),
        seg.blur,
        seg.blur_radius_animation.as_ref(),
    );
    if blurred_label != video_label {
        video_label = blurred_label;
    }
    if let Some(plan) = seg.color_pipeline.as_ref() {
        if plan.has_any_lut() {
            let cv = format!("[cpv{i}]");
            let block = if let Some(mask_input_index) = seg.mask_input_index {
                masked_color_pipeline_filter_block(
                    &video_label,
                    &cv,
                    &i.to_string(),
                    plan,
                    mask_input_index,
                )
                .unwrap_or_else(|| {
                    color_pipeline_filter_block(&video_label, &cv, &i.to_string(), plan)
                })
            } else {
                color_pipeline_filter_block(&video_label, &cv, &i.to_string(), plan)
            };
            filter.push_str(&block);
            filter.push(';');
            video_label = cv;
        }
    } else if let Some(lut_path) = seg.lut_path.as_ref() {
        let lv = format!("[lv{i}]");
        filter.push_str(&lut3d_filter_block(
            &video_label,
            &lv,
            &i.to_string(),
            lut_path,
            seg.lut_interpolation.as_deref(),
            seg.lut_strength,
        ));
        filter.push(';');
        video_label = lv;
    }
    if let Some(chain) = segment_reframe_filter_chain(seg) {
        let rv = format!("[rv{i}]");
        filter.push_str(&format!("{video_label}{chain}{rv};"));
        video_label = rv;
    }
    if let Some(warp) = seg.warp.as_ref()
        && let Some(chain) = warp_filter_chain(warp, seg.warp_animation.as_ref(), i)
    {
        let wv = format!("[wv{i}]");
        filter.push_str(&format!("{video_label}{chain}{wv};"));
        video_label = wv;
    }
    if let Some(shake) = seg.shake.as_ref()
        && let Some(chain) = animated_shake_filter_chain(
            shake,
            seg.shake_intensity_animation.as_ref(),
            seg.shake_frequency_animation.as_ref(),
        )
    {
        let sv = format!("[shv{i}]");
        filter.push_str(&format!("{video_label}{chain}{sv};"));
        video_label = sv;
    }
    if let Some(speed) = seg.speed
        && speed_needs_filter(speed)
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!("{video_label}{}{sv};", speed_video_filter(speed)));
        video_label = sv;
    }
    video_label
}

fn plan_audio_mix_filter(
    filter: &mut String,
    audio_tracks: &[AudioTrackPlan],
    next_input: &mut usize,
) -> String {
    let solo_active = audio_tracks.iter().any(|t| t.solo);
    let mut audible = Vec::<(&AudioTrackPlan, String)>::new();
    for (track_index, track) in audio_tracks.iter().enumerate() {
        if track.muted || (solo_active && !track.solo) || track.items.is_empty() {
            continue;
        }
        let label = plan_one_audio_track(filter, track_index, track, next_input);
        audible.push((track, label));
    }
    if audible.is_empty() {
        filter.push_str("anullsrc=r=48000:cl=stereo:d=0.1[finala];");
        return "[finala]".into();
    }

    let ducking_needed = audible.iter().any(|(t, _)| {
        t.role != "dialogue" && t.ducking.as_ref().map(|d| d.enabled).unwrap_or(false)
    });
    let mut audible_labels: Vec<String> = audible.iter().map(|(_, label)| label.clone()).collect();
    let mut dialogue_labels = Vec::new();
    if ducking_needed {
        for (idx, (track, label)) in audible.iter().enumerate() {
            if track.role == "dialogue" {
                let main = format!("[dlgmain{idx}]");
                let sc = format!("[dlgsc{idx}]");
                filter.push_str(&format!("{label}asplit=2{main}{sc};"));
                audible_labels[idx] = main;
                dialogue_labels.push(sc);
            }
        }
    }
    let sidechain_label = if dialogue_labels.is_empty() {
        None
    } else if dialogue_labels.len() == 1 {
        Some(dialogue_labels[0].clone())
    } else {
        for l in &dialogue_labels {
            filter.push_str(l);
        }
        filter.push_str(&format!(
            "amix=inputs={}:duration=longest[ducksc];",
            dialogue_labels.len()
        ));
        Some("[ducksc]".into())
    };

    let mut mix_labels = Vec::new();
    for (idx, (track, _)) in audible.iter().enumerate() {
        let label = &audible_labels[idx];
        if let (Some(duck), Some(sidechain)) = (track.ducking.as_ref(), sidechain_label.as_ref())
            && duck.enabled
            && track.role != "dialogue"
        {
            let out = format!("[ducked{idx}]");
            let ratio = ducking_sidechain_ratio(duck.amount_db);
            filter.push_str(&format!(
                "{label}{sidechain}sidechaincompress=threshold=0.05:ratio={}:attack={}:release={}{};",
                fmt_filter_num(ratio),
                fmt_filter_num(duck.attack_ms),
                fmt_filter_num(duck.release_ms),
                out
            ));
            mix_labels.push(out);
            continue;
        }
        mix_labels.push(label.to_string());
    }
    for l in &mix_labels {
        filter.push_str(l);
    }
    filter.push_str(&format!(
        "amix=inputs={}:duration=longest:dropout_transition=0[finala];",
        mix_labels.len()
    ));
    "[finala]".into()
}

fn ducking_sidechain_ratio(amount_db: f64) -> f64 {
    let reduction_db = if amount_db.is_finite() {
        amount_db.abs().clamp(0.0, 60.0)
    } else {
        12.0
    };
    1.0 + reduction_db * (7.0 / 12.0)
}

fn plan_one_audio_track(
    filter: &mut String,
    track_index: usize,
    track: &AudioTrackPlan,
    next_input: &mut usize,
) -> String {
    let mut item_labels = Vec::new();
    for (item_index, item) in track.items.iter().enumerate() {
        match item {
            AudioTrackItemPlan::Gap { duration_s } => {
                let label = format!("[agap{track_index}_{item_index}]");
                filter.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo:d={duration_s}{label};"
                ));
                item_labels.push(label);
            }
            AudioTrackItemPlan::Clip(clip) => {
                let input = *next_input;
                *next_input += 1;
                let mut label = format!("[{input}:a:0]");
                let trimmed = format!("[atrim{track_index}_{item_index}]");
                let source_end = clip.start_s + clip.duration_s;
                filter.push_str(&format!(
                    "{label}atrim={}:{},asetpts=PTS-STARTPTS{};",
                    clip.start_s, source_end, trimmed
                ));
                label = trimmed;
                if let Some(fx) = clip.audio_fx.as_ref()
                    && let Some(chain) = audio_fx_filter_chain(fx)
                {
                    let fx_label = format!("[acfx{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}{chain}{fx_label};"));
                    label = fx_label;
                }
                if let Some(speed) = clip.speed
                    && speed_needs_filter(speed)
                {
                    let sped = format!("[aspeed{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}{}{sped};", speed_audio_filter(speed)));
                    label = sped;
                }
                if let Some(automation) = clip.volume_automation.as_ref() {
                    let vol = format!("[avol{track_index}_{item_index}]");
                    filter.push_str(&format!(
                        "{label}volume='{}':eval=frame{vol};",
                        automation.expression
                    ));
                    label = vol;
                } else if let Some(v) = clip.volume
                    && (v - 1.0).abs() > 1e-9
                {
                    let vol = format!("[avol{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}volume={v}{vol};"));
                    label = vol;
                }
                if let Some(fade_in) = clip.fade_in_s
                    && fade_in > 0.0
                {
                    let fade = format!("[afi{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}afade=t=in:st=0:d={fade_in}{fade};"));
                    label = fade;
                }
                if let Some(fade_out) = clip.fade_out_s
                    && fade_out > 0.0
                {
                    let fade = format!("[afo{track_index}_{item_index}]");
                    let st = (clip.duration_s - fade_out).max(0.0);
                    filter.push_str(&format!("{label}afade=t=out:st={st}:d={fade_out}{fade};"));
                    label = fade;
                }
                item_labels.push(label);
            }
        }
    }
    for l in &item_labels {
        filter.push_str(l);
    }
    let track_label = format!("[atrack{track_index}]");
    filter.push_str(&format!(
        "concat=n={}:v=0:a=1{};",
        item_labels.len(),
        track_label
    ));
    let mut track_label = track_label;
    if let Some(fx) = track.audio_fx.as_ref()
        && let Some(chain) = audio_fx_filter_chain(fx)
    {
        let out = format!("[atrackfx{track_index}]");
        filter.push_str(&format!("{track_label}{chain}{out};"));
        track_label = out;
    }
    if let Some(automation) = track.volume_automation.as_ref() {
        let out = format!("[atrackauto{track_index}]");
        filter.push_str(&format!(
            "{track_label}volume='{}':eval=frame{out};",
            automation.expression
        ));
        track_label = out;
    }
    if track.volume_automation.is_none() && (track.volume - 1.0).abs() > 1e-9 {
        let out = format!("[atrackv{track_index}]");
        filter.push_str(&format!("{track_label}volume={}{};", track.volume, out));
        out
    } else {
        track_label
    }
}

/// One-call helper: walk OTIO, build the spec, return it. The output
/// path is `<project_root>/renders/timeline-<HHMMSS>.mp4` — same
/// naming `start_render scope=timeline` uses, so the agent and the
/// desktop produce indistinguishable artifacts.
pub fn build_timeline_render_spec(
    project_root: &Path,
) -> Result<RenderJobSpec, RenderTimelineError> {
    build_timeline_render_spec_inner(project_root, None)
}

/// Build a timeline render spec clipped to one guide-track marker range.
pub fn build_timeline_section_render_spec(
    project_root: &Path,
    guide_track_id: &str,
    marker_id: &str,
) -> Result<RenderJobSpec, RenderTimelineError> {
    let section = resolve_timeline_section(project_root, guide_track_id, marker_id)?;
    build_timeline_render_spec_inner(project_root, Some(section))
}

fn build_timeline_render_spec_inner(
    project_root: &Path,
    section: Option<TimelineSectionRange>,
) -> Result<RenderJobSpec, RenderTimelineError> {
    let (
        segs,
        transitions,
        video_overlays,
        titles,
        annotations,
        broadcast_overlay,
        audio_tracks,
        loudness_target,
        render_limitations,
    ) = collect_timeline_full_plan(project_root)?;
    if segs.is_empty() && audio_tracks.is_empty() {
        return Err(RenderTimelineError::EmptyTimeline);
    }
    // Total duration is the sum of each segment's visible effective
    // duration. Centered transitions extend source handles before
    // and after clips for xfade inputs, but they do not shorten the
    // master timeline the way the old phase-one overlap model did.
    let total_duration_s: f64 = if segs.is_empty() {
        audio_tracks
            .iter()
            .map(audio_track_duration)
            .fold(0.0_f64, f64::max)
    } else {
        segs.iter().map(visible_effective_duration).sum()
    };
    let renders_dir = project_root.join("renders");
    fs::create_dir_all(&renders_dir)
        .map_err(|e| RenderTimelineError::BroadcastOverlayRender(e.to_string()))?;
    let timestamp = Utc::now().format("%H%M%S");
    let output_path = match section.as_ref() {
        Some(section) => renders_dir.join(format!(
            "timeline-section-{}-{timestamp}.mp4",
            safe_filename_slug(&section.marker_id)
        )),
        None => renders_dir.join(format!("timeline-{timestamp}.mp4")),
    };
    let input_paths = render_input_paths(&segs, &video_overlays, &audio_tracks);
    validate_render_output_path(
        project_root,
        &output_path,
        &input_paths,
        &[],
        OutputPathPolicy::default(),
    )
    .map_err(|e| {
        RenderTimelineError::BroadcastOverlayRender(format!("output path preflight failed: {e}"))
    })?;
    let browser_broadcast_overlay = if let Some(overlay) = broadcast_overlay.as_ref()
        && overlay.config.enabled
        && !overlay.config.short_form_mode
    {
        Some(prepare_browser_broadcast_overlay_video(
            overlay,
            total_duration_s,
            &renders_dir,
            &timestamp.to_string(),
        )?)
    } else {
        None
    };
    // libass burn-in path: when any caption carries word timings we
    // materialize `.ass` files under <renders>/.ass/<timestamp>/ so
    // the ffmpeg `subtitles=` filter can find them. Drawtext stays
    // the default for everything else.
    let ass_workdir = if titles.iter().any(crate::ass::is_libass_eligible) {
        let dir = renders_dir.join(".ass").join(timestamp.to_string());
        if let Err(err) = fs::create_dir_all(&dir) {
            tracing::warn!(
                ?err,
                dir = %dir.display(),
                "build_timeline_render_spec: failed to create ASS workdir; captions will fall back to drawtext",
            );
            None
        } else {
            Some(dir)
        }
    } else {
        None
    };
    let mut argv = if audio_tracks.is_empty() {
        build_timeline_argv_full_with_annotations_and_ass(
            &segs,
            &transitions,
            &video_overlays,
            &annotations,
            &titles,
            broadcast_overlay.as_ref(),
            browser_broadcast_overlay.as_deref(),
            loudness_target,
            &output_path,
            ass_workdir.as_deref(),
        )
    } else {
        build_timeline_argv_with_audio_tracks_and_annotations_and_ass(
            &segs,
            &transitions,
            &video_overlays,
            &annotations,
            &titles,
            broadcast_overlay.as_ref(),
            browser_broadcast_overlay.as_deref(),
            loudness_target,
            &audio_tracks,
            &output_path,
            ass_workdir.as_deref(),
        )
    };
    if let Some(section) = section {
        trim_render_argv_to_section(&mut argv, section.start_s, section.duration_s);
        return Ok(RenderJobSpec {
            args: argv,
            total_duration_s: Some(section.duration_s),
            cwd: Some(project_root.to_path_buf()),
            output_path,
            limitations: render_limitations,
        });
    }
    Ok(RenderJobSpec {
        args: argv,
        total_duration_s: Some(total_duration_s),
        cwd: Some(project_root.to_path_buf()),
        output_path,
        limitations: render_limitations,
    })
}

fn resolve_timeline_section(
    project_root: &Path,
    guide_track_id: &str,
    marker_id: &str,
) -> Result<TimelineSectionRange, RenderTimelineError> {
    let otio_path = project_root.join(files::OTIO);
    if !otio_path.exists() {
        return Err(RenderTimelineError::NoOtio(otio_path));
    }
    let mut warnings = Vec::new();
    let timeline = read_otio_timeline(&otio_path, &mut warnings).map_err(|e| {
        RenderTimelineError::OtioParse {
            message: e.to_string(),
        }
    })?;
    let Some(metadata) = timeline.metadata.awidat.as_ref() else {
        return Err(RenderTimelineError::GuideSectionNotFound {
            guide_track_id: guide_track_id.into(),
            marker_id: marker_id.into(),
        });
    };
    let Some(guide_track) = metadata
        .guide_tracks
        .iter()
        .find(|track| track.id == guide_track_id)
    else {
        return Err(RenderTimelineError::GuideSectionNotFound {
            guide_track_id: guide_track_id.into(),
            marker_id: marker_id.into(),
        });
    };
    let Some(marker) = guide_track
        .markers
        .iter()
        .find(|marker| marker.id == marker_id)
    else {
        return Err(RenderTimelineError::GuideSectionNotFound {
            guide_track_id: guide_track_id.into(),
            marker_id: marker_id.into(),
        });
    };
    let Some(duration_s) = marker.duration_s else {
        return Err(RenderTimelineError::GuideSectionMissingDuration {
            guide_track_id: guide_track_id.into(),
            marker_id: marker_id.into(),
        });
    };
    if !marker.time_s.is_finite()
        || marker.time_s < 0.0
        || !duration_s.is_finite()
        || duration_s <= 0.0
    {
        return Err(RenderTimelineError::GuideSectionMissingDuration {
            guide_track_id: guide_track_id.into(),
            marker_id: marker_id.into(),
        });
    }

    Ok(TimelineSectionRange {
        marker_id: marker_id.into(),
        start_s: marker.time_s,
        duration_s,
    })
}

fn trim_render_argv_to_section(argv: &mut Vec<String>, start_s: f64, duration_s: f64) {
    let output_index = argv.len().saturating_sub(1);
    argv.splice(
        output_index..output_index,
        [
            "-ss".to_string(),
            format!("{start_s}"),
            "-t".to_string(),
            format!("{duration_s}"),
        ],
    );
}

fn safe_filename_slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "section".into()
    } else {
        slug.into()
    }
}

fn render_input_paths(
    segs: &[TimelineSegment],
    video_overlays: &[VideoOverlayPlan],
    audio_tracks: &[AudioTrackPlan],
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.extend(segs.iter().map(|segment| segment.asset_path.clone()));
    paths.extend(
        video_overlays
            .iter()
            .map(|overlay| overlay.segment.asset_path.clone()),
    );
    paths.extend(
        segs.iter()
            .filter_map(renderable_mask_source)
            .map(Path::to_path_buf),
    );
    for track in audio_tracks {
        paths.extend(track.items.iter().filter_map(|item| match item {
            AudioTrackItemPlan::Clip(clip) => Some(clip.asset_path.clone()),
            AudioTrackItemPlan::Gap { .. } => None,
        }));
    }
    paths
}

fn prepare_browser_broadcast_overlay_video(
    overlay: &BroadcastOverlayPlan,
    duration_s: f64,
    renders_dir: &Path,
    timestamp: &str,
) -> Result<PathBuf, RenderTimelineError> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            RenderTimelineError::BroadcastOverlayRender(
                "could not resolve repository root".to_string(),
            )
        })?;
    let script = repo_root
        .join("apps")
        .join("desktop")
        .join("scripts")
        .join("render-broadcast-overlay.mjs");
    if !script.exists() {
        return Err(RenderTimelineError::BroadcastOverlayRender(format!(
            "missing overlay renderer script at {}",
            script.display()
        )));
    }
    let output = renders_dir.join(format!("broadcast-overlay-{timestamp}.mov"));
    let config_json = serde_json::to_string(&overlay.config)
        .map_err(|e| RenderTimelineError::BroadcastOverlayRender(e.to_string()))?;
    let status = Command::new("node")
        .arg(&script)
        .arg("--config")
        .arg(config_json)
        .arg("--project-root")
        .arg(&overlay.project_root)
        .arg("--duration")
        .arg(format!("{duration_s}"))
        .arg("--output")
        .arg(&output)
        .arg("--width")
        .arg("1920")
        .arg("--height")
        .arg("1080")
        .arg("--fps")
        .arg("30")
        .current_dir(repo_root.join("apps").join("desktop"))
        .status()
        .map_err(|e| RenderTimelineError::BroadcastOverlayRender(e.to_string()))?;
    if !status.success() {
        return Err(RenderTimelineError::BroadcastOverlayRender(format!(
            "overlay renderer exited with {status}"
        )));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
    };
    use awidat_proto::professional::{
        BezierHandles, CompositionGraph, CompositionNode, CompositionNodeType, Easing,
        ExtrapolationMode, Keyframe, KeyframeInterpolation, MaskKeyframe, MaskOperation,
        MaskSidecar, MatteSidecar, ParameterAnimation, ReframeKeyframe, ReframePath,
        ReframeSmoothing, TrackKind as ProfessionalTrackKind, TrackSample, TrackSidecar,
        TrackingPackage,
    };
    use std::fs;

    fn write_fixture_project(dir: &Path) -> PathBuf {
        let asset_rel = "raw/x.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();
        let mut clip = Clip::empty("c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_nested_stack(dir: &Path) -> PathBuf {
        let asset_rel = "raw/nested.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();
        let mut clip = Clip::empty("nested-c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(1.0 * 24.0, 24.0),
            RationalTime::new(2.5 * 24.0, 24.0),
        ));
        let mut nested_track = Track::empty("Nested V1", TrackKind::Video);
        nested_track.children.push(TrackChild::Clip(clip));
        let mut nested_stack = Stack::empty("nested-stack");
        nested_stack.children.push(StackChild::Track(nested_track));
        let mut base_track = Track::empty("V1", TrackKind::Video);
        base_track.children.push(TrackChild::Stack(nested_stack));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(base_track));
        tl.tracks = stack;
        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_reframe_path(dir: &Path) -> PathBuf {
        let otio_path = write_fixture_project(dir);
        let mut tl = read_otio_timeline(&otio_path, &mut Vec::new()).expect("fixture should parse");
        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata.tracking_package = Some(TrackingPackage {
            reframe_paths: vec![ReframePath {
                id: "reframe-c1-9-16".into(),
                clip_id: "c1".into(),
                aspect_ratio: "9:16".into(),
                source_width: 1920,
                source_height: 1080,
                target_width: 1080,
                target_height: 1920,
                keyframes: vec![
                    ReframeKeyframe {
                        time_s: 0.0,
                        center: [0.30, 0.35],
                        scale: 3.1604938271604937,
                        confidence: Some(0.91),
                    },
                    ReframeKeyframe {
                        time_s: 1.0,
                        center: [0.60, 0.45],
                        scale: 3.1604938271604937,
                        confidence: Some(0.89),
                    },
                ],
                smoothing: ReframeSmoothing::None,
                evidence_track_id: Some("speaker-face".into()),
                safe_area: Some("mobile".into()),
            }],
            ..TrackingPackage::default()
        });
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_guide_section(dir: &Path) -> PathBuf {
        let otio_path = write_fixture_project(dir);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata
            .guide_tracks
            .push(awidat_proto::awidat_meta::GuideTrack {
                id: "deliverables".into(),
                label: "Deliverables".into(),
                markers: vec![awidat_proto::awidat_meta::TimelineMarker {
                    id: "cold-open".into(),
                    label: "Cold open".into(),
                    time_s: 0.5,
                    duration_s: Some(1.25),
                    category: Some(awidat_proto::awidat_meta::TimelineMarkerCategory::ExportRange),
                    ..Default::default()
                }],
                ..Default::default()
            });
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_time_remap(dir: &Path) -> PathBuf {
        let otio_path = write_fixture_project(dir);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let StackChild::Track(track) = &mut tl.tracks.children[0] else {
            panic!("expected video track");
        };
        let TrackChild::Clip(clip) = &mut track.children[0] else {
            panic!("expected fixture clip");
        };
        let mut effect = Effect::new("awidat.time_remap");
        effect.metadata.insert(
            "curve".into(),
            serde_json::json!([
                {"source_time_s": 0.0, "timeline_time_s": 0.0},
                {"source_time_s": 1.0, "timeline_time_s": 2.5},
                {"source_time_s": 2.0, "timeline_time_s": 3.5}
            ]),
        );
        clip.effects.push(effect);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    /// Fixture with a freeze-plateau time_remap curve: source advances from
    /// 1s -> 2s while the timeline output stays flat at 1s. The validator must
    /// accept zero-slope on the OUTPUT axis (freeze) while continuing to reject
    /// zero-slope on the SOURCE axis (reverse).
    fn write_fixture_project_with_time_remap_freeze_plateau(dir: &Path) -> PathBuf {
        let asset_rel = "raw/x.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();
        let mut clip = Clip::empty("c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        // Source range covers 0..3s so the curve's source axis (0,1,2,3) fits.
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(3.0 * 24.0, 24.0),
        ));
        let mut effect = Effect::new("awidat.time_remap");
        effect.metadata.insert(
            "curve".into(),
            serde_json::json!([
                {"source_time_s": 0.0, "timeline_time_s": 0.0},
                {"source_time_s": 1.0, "timeline_time_s": 1.0},
                {"source_time_s": 2.0, "timeline_time_s": 1.0},
                {"source_time_s": 3.0, "timeline_time_s": 2.0}
            ]),
        );
        clip.effects.push(effect);
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_overlay_animation(
        dir: &Path,
        animation: ParameterAnimation,
    ) -> PathBuf {
        let base_asset_rel = "raw/base.mp4";
        let overlay_asset_rel = "raw/overlay.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(base_asset_rel), b"stub").unwrap();
        fs::write(dir.join(overlay_asset_rel), b"stub").unwrap();

        let mut base_clip = Clip::empty("base".to_string());
        base_clip.media_reference =
            MediaReference::External(ExternalReference::new(base_asset_rel));
        base_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));

        let mut overlay_clip = Clip::empty("clip-a".to_string());
        overlay_clip.media_reference =
            MediaReference::External(ExternalReference::new(overlay_asset_rel));
        overlay_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));

        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(base_clip));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(overlay_clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(v1));
        stack.children.push(StackChild::Track(v2));
        tl.tracks = stack;
        tl.metadata
            .awidat
            .as_mut()
            .unwrap()
            .parameter_animations
            .push(animation);

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_oversized_motion_blur_shutter(dir: &Path) -> PathBuf {
        let animation = ParameterAnimation {
            id: "move-overlay".into(),
            target: awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-a".into(),
                parameter: "overlay.x".into(),
            },
            keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 0.2)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };
        let otio_path = write_fixture_project_with_overlay_animation(dir, animation);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let StackChild::Track(track) = &mut tl.tracks.children[1] else {
            panic!("fixture second child should be overlay track");
        };
        let TrackChild::Clip(clip) = &mut track.children[0] else {
            panic!("fixture overlay child should be clip");
        };
        let mut effect = Effect::new("awidat.video_overlay");
        effect
            .metadata
            .insert("motion_blur".into(), serde_json::json!(true));
        effect
            .metadata
            .insert("motion_blur_shutter_s".into(), serde_json::json!(0.5));
        clip.effects.push(effect);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_tracker_bound_overlay(dir: &Path) -> PathBuf {
        let base_asset_rel = "raw/base.mp4";
        let overlay_asset_rel = "raw/overlay.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(base_asset_rel), b"stub").unwrap();
        fs::write(dir.join(overlay_asset_rel), b"stub").unwrap();

        let mut base_clip = Clip::empty("base".to_string());
        base_clip.media_reference =
            MediaReference::External(ExternalReference::new(base_asset_rel));
        base_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut overlay_clip = Clip::empty("clip-a".to_string());
        overlay_clip.media_reference =
            MediaReference::External(ExternalReference::new(overlay_asset_rel));
        overlay_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(base_clip));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(overlay_clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(v1));
        stack.children.push(StackChild::Track(v2));
        tl.tracks = stack;

        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata.tracking_package = Some(TrackingPackage {
            tracks: vec![TrackSidecar {
                id: "speaker-face".into(),
                asset_id: "clip-a".into(),
                kind: ProfessionalTrackKind::Point,
                samples: vec![
                    TrackSample {
                        frame: 0,
                        points: vec![[0.25, 0.75]],
                        confidence: Some(0.95),
                    },
                    TrackSample {
                        frame: 60,
                        points: vec![[0.40, 0.60]],
                        confidence: Some(0.90),
                    },
                ],
                confidence: Some(0.925),
                ..TrackSidecar::default()
            }],
            ..TrackingPackage::default()
        });
        metadata.composition_graphs.push(CompositionGraph {
            id: "tracked-overlay".into(),
            nodes: vec![CompositionNode {
                id: "bind-x".into(),
                node_type: CompositionNodeType::TrackerBind,
                params: [
                    ("track_id".to_string(), serde_json::json!("speaker-face")),
                    ("target_clip_id".to_string(), serde_json::json!("clip-a")),
                    (
                        "target_parameter".to_string(),
                        serde_json::json!("overlay.x"),
                    ),
                    ("channel".to_string(), serde_json::json!("x")),
                ]
                .into_iter()
                .collect(),
            }],
            ..CompositionGraph::default()
        });

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_short_tracker_bound_overlay(dir: &Path) -> PathBuf {
        let otio_path = write_fixture_project_with_tracker_bound_overlay(dir);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let samples = &mut tl
            .metadata
            .awidat
            .as_mut()
            .unwrap()
            .tracking_package
            .as_mut()
            .unwrap()
            .tracks[0]
            .samples;
        samples[1].frame = 30;
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_corner_pin_overlay(dir: &Path) -> PathBuf {
        let base_asset_rel = "raw/base.mp4";
        let overlay_asset_rel = "raw/screen-replacement.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(base_asset_rel), b"stub").unwrap();
        fs::write(dir.join(overlay_asset_rel), b"stub").unwrap();

        let mut base_clip = Clip::empty("base".to_string());
        base_clip.media_reference =
            MediaReference::External(ExternalReference::new(base_asset_rel));
        base_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut overlay_clip = Clip::empty("replacement-screen".to_string());
        overlay_clip.media_reference =
            MediaReference::External(ExternalReference::new(overlay_asset_rel));
        overlay_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(base_clip));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(overlay_clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(v1));
        stack.children.push(StackChild::Track(v2));
        tl.tracks = stack;

        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata.tracking_package = Some(TrackingPackage {
            tracks: vec![TrackSidecar {
                id: "screen-surface".into(),
                asset_id: "base".into(),
                kind: ProfessionalTrackKind::Surface,
                samples: vec![
                    TrackSample {
                        frame: 0,
                        points: vec![[0.10, 0.20], [0.90, 0.18], [0.86, 0.82], [0.14, 0.80]],
                        confidence: Some(0.91),
                    },
                    TrackSample {
                        frame: 30,
                        points: vec![[0.12, 0.22], [0.88, 0.20], [0.84, 0.84], [0.16, 0.82]],
                        confidence: Some(0.88),
                    },
                ],
                confidence: Some(0.895),
                ..TrackSidecar::default()
            }],
            ..TrackingPackage::default()
        });
        metadata.composition_graphs.push(CompositionGraph {
            id: "screen-replace".into(),
            nodes: vec![CompositionNode {
                id: "pin-screen".into(),
                node_type: CompositionNodeType::CornerPin,
                params: [
                    ("track_id".to_string(), serde_json::json!("screen-surface")),
                    (
                        "target_clip_id".to_string(),
                        serde_json::json!("replacement-screen"),
                    ),
                ]
                .into_iter()
                .collect(),
            }],
            ..CompositionGraph::default()
        });

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_matte_overlay(dir: &Path) -> PathBuf {
        let base_asset_rel = "raw/base.mp4";
        let overlay_asset_rel = "raw/subject.mp4";
        let matte_asset_rel = "raw/subject-alpha.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(base_asset_rel), b"stub").unwrap();
        fs::write(dir.join(overlay_asset_rel), b"stub").unwrap();
        fs::write(dir.join(matte_asset_rel), b"stub").unwrap();

        let mut base_clip = Clip::empty("base".to_string());
        base_clip.media_reference =
            MediaReference::External(ExternalReference::new(base_asset_rel));
        base_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut overlay_clip = Clip::empty("subject".to_string());
        overlay_clip.media_reference =
            MediaReference::External(ExternalReference::new(overlay_asset_rel));
        overlay_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(base_clip));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(overlay_clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(v1));
        stack.children.push(StackChild::Track(v2));
        tl.tracks = stack;

        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata.tracking_package = Some(TrackingPackage {
            mattes: vec![MatteSidecar {
                id: "matte-subject".into(),
                alpha_source: matte_asset_rel.into(),
                confidence: Some(0.93),
                ..MatteSidecar::default()
            }],
            ..TrackingPackage::default()
        });
        metadata.composition_graphs.push(CompositionGraph {
            id: "subject-matte".into(),
            nodes: vec![CompositionNode {
                id: "apply-matte".into(),
                node_type: CompositionNodeType::Matte,
                params: [
                    ("matte_id".to_string(), serde_json::json!("matte-subject")),
                    ("target_clip_id".to_string(), serde_json::json!("subject")),
                ]
                .into_iter()
                .collect(),
            }],
            ..CompositionGraph::default()
        });

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_mask_overlay(dir: &Path) -> PathBuf {
        let base_asset_rel = "raw/base.mp4";
        let overlay_asset_rel = "raw/callout.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(base_asset_rel), b"stub").unwrap();
        fs::write(dir.join(overlay_asset_rel), b"stub").unwrap();

        let mut base_clip = Clip::empty("base".to_string());
        base_clip.media_reference =
            MediaReference::External(ExternalReference::new(base_asset_rel));
        base_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut overlay_clip = Clip::empty("callout".to_string());
        overlay_clip.media_reference =
            MediaReference::External(ExternalReference::new(overlay_asset_rel));
        overlay_clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 30.0),
            RationalTime::new(2.0 * 30.0, 30.0),
        ));

        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(base_clip));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(overlay_clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(v1));
        stack.children.push(StackChild::Track(v2));
        tl.tracks = stack;

        let metadata = tl.metadata.awidat.as_mut().unwrap();
        metadata.tracking_package = Some(TrackingPackage {
            masks: vec![MaskSidecar {
                id: "mask-callout".into(),
                attached_clip_id: Some("callout".into()),
                operation: MaskOperation::Add,
                keyframes: vec![MaskKeyframe {
                    time_s: 0.0,
                    points: vec![[0.20, 0.25], [0.70, 0.25], [0.70, 0.80], [0.20, 0.80]],
                    feather: 0.0,
                    opacity: 0.75,
                }],
                ..MaskSidecar::default()
            }],
            ..TrackingPackage::default()
        });
        metadata.composition_graphs.push(CompositionGraph {
            id: "callout-mask".into(),
            nodes: vec![CompositionNode {
                id: "apply-mask".into(),
                node_type: CompositionNodeType::Mask,
                params: [
                    ("mask_id".to_string(), serde_json::json!("mask-callout")),
                    ("target_clip_id".to_string(), serde_json::json!("callout")),
                ]
                .into_iter()
                .collect(),
            }],
            ..CompositionGraph::default()
        });

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_animated_mask_overlay(dir: &Path) -> PathBuf {
        let otio_path = write_fixture_project_with_mask_overlay(dir);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let mask = tl
            .metadata
            .awidat
            .as_mut()
            .unwrap()
            .tracking_package
            .as_mut()
            .unwrap()
            .masks
            .first_mut()
            .unwrap();
        mask.keyframes.push(MaskKeyframe {
            time_s: 1.0,
            points: vec![[0.30, 0.35], [0.80, 0.35], [0.80, 0.90], [0.30, 0.90]],
            feather: 0.0,
            opacity: 0.4,
        });
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_base_animation(
        dir: &Path,
        animation: ParameterAnimation,
    ) -> PathBuf {
        let otio_path = write_fixture_project(dir);
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        tl.metadata
            .awidat
            .as_mut()
            .unwrap()
            .parameter_animations
            .push(animation);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    fn write_fixture_project_with_audio_track_animation(
        dir: &Path,
        animation: ParameterAnimation,
    ) -> PathBuf {
        let asset_rel = "raw/music.wav";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();

        let mut clip = Clip::empty("music-clip".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 48_000.0),
            RationalTime::new(2.0 * 48_000.0, 48_000.0),
        ));

        let mut track = Track::empty("Music", TrackKind::Audio);
        track.children.push(TrackChild::Clip(clip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        tl.metadata
            .awidat
            .as_mut()
            .unwrap()
            .parameter_animations
            .push(animation);

        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    #[test]
    fn no_otio_returns_no_otio_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::NoOtio(_)));
    }

    #[test]
    fn empty_otio_returns_empty_timeline() {
        let dir = tempfile::tempdir().unwrap();
        // Init an OTIO file with no tracks.
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::EmptyTimeline));
    }

    #[test]
    fn fixture_project_produces_concat_argv() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        let spec = build_timeline_render_spec(dir.path()).unwrap();
        assert!(spec.total_duration_s.unwrap() > 1.9);
        // Concat filter present, libx264 (re-encode, not stream-copy).
        let cmd = spec.args.join(" ");
        assert!(cmd.contains("concat=n=1:v=1:a=1"));
        assert!(cmd.contains("libx264"));
        assert!(!cmd.contains(" copy "));
        // Output under renders/ with timeline-<HHMMSS>.mp4 naming.
        assert!(spec.output_path.starts_with(dir.path().join("renders")));
        assert!(
            spec.output_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("timeline-")
        );
    }

    #[test]
    fn nested_stack_clip_is_collected_for_timeline_render() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_nested_stack(dir.path());

        let segments = collect_timeline_segments(dir.path()).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].asset_path, dir.path().join("raw/nested.mp4"));
        assert!((segments[0].start_s - 1.0).abs() < 0.001);
        assert!((segments[0].duration_s - 2.5).abs() < 0.001);
    }

    #[test]
    fn timeline_render_spec_lowers_time_remap_curve_to_setpts() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_time_remap(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert_eq!(spec.total_duration_s, Some(3.5));
        assert!(
            filter.contains("setpts='if(lte((PTS-STARTPTS)*TB\\,1)"),
            "time remap should lower to a piecewise setpts expression: {filter}"
        );
        assert!(
            filter.contains("[0:a:0]atempo="),
            "time remap should keep audio duration aligned with the remapped video: {filter}"
        );
    }

    /// A freeze plateau in a time_remap curve has zero slope on the
    /// timeline axis (output stays constant while source advances). The
    /// validator must accept this — it expresses a held frame, not a
    /// reverse — and the renderer must still emit a valid setpts graph.
    #[test]
    fn timeline_render_spec_accepts_time_remap_freeze_plateau() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_time_remap_freeze_plateau(dir.path());

        let spec = build_timeline_render_spec(dir.path())
            .expect("freeze-plateau curve must validate as a legal time remap");
        let filter = filter_complex_from_argv(&spec.args);

        // Plateau spans timeline output 1.0..1.0, then ramps to 2.0 — total 2.0s.
        assert_eq!(spec.total_duration_s, Some(2.0));
        assert!(
            filter.contains("setpts="),
            "freeze-plateau curve should still lower to setpts: {filter}"
        );
    }

    /// Zero-slope on the SOURCE axis encodes a reverse (or duplicate sample) —
    /// the renderer cannot express that with a single-valued setpts mapping,
    /// so the validator must keep rejecting it even after the output-axis
    /// relaxation that allows freeze plateaus.
    #[test]
    fn timeline_render_spec_rejects_time_remap_zero_source_slope() {
        let dir = tempfile::tempdir().unwrap();
        let otio_path = write_fixture_project(dir.path());
        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        let StackChild::Track(track) = &mut tl.tracks.children[0] else {
            panic!("expected video track");
        };
        let TrackChild::Clip(clip) = &mut track.children[0] else {
            panic!("expected fixture clip");
        };
        let mut effect = Effect::new("awidat.time_remap");
        effect.metadata.insert(
            "curve".into(),
            serde_json::json!([
                {"source_time_s": 0.0, "timeline_time_s": 0.0},
                {"source_time_s": 1.0, "timeline_time_s": 1.0},
                // Source axis stays at 1.0 while timeline advances — illegal.
                {"source_time_s": 1.0, "timeline_time_s": 2.0}
            ]),
        );
        clip.effects.push(effect);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

        let err = build_timeline_render_spec(dir.path())
            .expect_err("zero-slope on source axis must remain rejected");
        let message = format!("{err:?}");
        assert!(
            message.contains("source_time_s"),
            "rejection should call out the source axis: {message}"
        );
    }

    #[test]
    fn timeline_render_spec_applies_tracking_reframe_path_to_matching_clip() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_reframe_path(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("scale=ceil(iw*3.160494/2)*2"),
            "missing subject reframe scale: {filter}"
        );
        assert!(
            filter.contains("crop=iw/3.160494:ih/3.160494"),
            "missing subject reframe crop: {filter}"
        );
        assert!(
            filter.contains("x=(in_w-out_w)*(if(lt(t\\,0)"),
            "missing dynamic x center expression: {filter}"
        );
        assert!(
            filter.contains("0.3+(0.6-0.3)*"),
            "missing x keyframe interpolation: {filter}"
        );
        assert!(
            filter.contains("0.35+(0.45-0.35)*"),
            "missing y keyframe interpolation: {filter}"
        );
    }

    #[test]
    fn timeline_render_spec_lowers_tracker_bind_graph_to_overlay_animation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_tracker_bound_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("main_w*(if(lt((t-0)\\,0)\\,0.25")
                && filter.contains("0.25+(0.4-0.25)"),
            "tracker_bind graph should generate overlay.x animation keyframes from track samples: {filter}"
        );
        assert!(
            spec.limitations.is_empty(),
            "render should consume graph-derived tracker animations without limitations: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn timeline_render_spec_warns_when_tracker_bind_coverage_is_short() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_short_tracker_bound_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();

        assert!(
            spec.limitations.iter().any(|limitation| {
                limitation.kind == "tracker_bind_coverage_shortfall"
                    && limitation.animation_id.as_deref() == Some("tracker-bind-bind-x")
                    && limitation.clip_id.as_deref() == Some("clip-a")
                    && limitation.parameter.as_deref() == Some("overlay.x")
                    && limitation.message.contains("Hold extrapolation")
            }),
            "short tracker coverage should surface a render limitation: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn timeline_render_spec_lowers_corner_pin_graph_to_overlay_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_corner_pin_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("perspective=x0='if(lte(n\\,0)\\,192")
                && filter.contains(":eval=frame[media_overlay_corner_pin0]"),
            "corner_pin graph should lower into the target overlay filter branch: {filter}"
        );
        assert!(
            filter.contains("[media_overlay_ref0][media_overlay_corner_pin0]overlay="),
            "corner-pinned replacement should composite after perspective lowering: {filter}"
        );
        assert!(
            spec.limitations.is_empty(),
            "render should consume graph-derived corner-pin lowerings without limitations: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn timeline_render_spec_lowers_matte_graph_to_overlay_alpha() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_matte_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            spec.args
                .windows(2)
                .any(|w| w[0] == "-i" && w[1].ends_with("raw/subject-alpha.mp4")),
            "matte alpha source should be added as a render input: {:?}",
            spec.args
        );
        assert!(
            !filter.contains("loop=-1:1[media_overlay_matte_loop0]")
                && filter.contains("format=gray[media_overlay_matte_gray0]")
                && filter.contains("alphamerge=shortest=1[media_overlay_matte0]"),
            "video matte graph should stream the matte instead of freezing frame 0: {filter}"
        );
        assert!(
            filter.contains("[media_overlay_ref0][media_overlay_matte0]overlay="),
            "matte-applied overlay should feed the compositing stage: {filter}"
        );
        assert!(
            spec.limitations.is_empty(),
            "render should consume graph-derived matte lowerings without limitations: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn oversized_motion_blur_shutter_surfaces_clamp_limitation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_oversized_motion_blur_shutter(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("t-0.1") && filter.contains("t+0.1"),
            "oversized shutter should be clamped to the renderable max in the filter: {filter}"
        );
        assert!(
            spec.limitations.iter().any(|limitation| {
                limitation.kind == "motion_blur_shutter_clamped"
                    && limitation.clip_id.as_deref() == Some("clip-a")
                    && limitation.parameter.as_deref() == Some("overlay.motion_blur_shutter_s")
                    && limitation.message.contains("0.5")
                    && limitation.message.contains("0.2")
            }),
            "clamped shutter should surface a limitation: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn timeline_render_spec_lowers_mask_graph_to_overlay_alpha_gate() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_mask_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("[media_overlay_scaled0]format=rgba,geq=")
                && filter.contains("between(X\\,W*0.2\\,W*0.7)")
                && filter.contains("between(Y\\,H*0.25\\,H*0.8)")
                && filter.contains("[media_overlay_mask0]"),
            "mask graph should lower to an overlay alpha gate from the mask bounds: {filter}"
        );
        assert!(
            filter.contains("[media_overlay_ref0][media_overlay_mask0]overlay="),
            "masked overlay should feed the compositing stage: {filter}"
        );
        assert!(
            spec.limitations.is_empty(),
            "render should consume graph-derived mask lowerings without limitations: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn timeline_render_spec_lowers_mask_keyframes_to_alpha_gate_animation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_animated_mask_overlay(dir.path());

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(
            filter.contains("between(X\\,W*if(lt((T-0)\\,0)")
                && filter.contains("0.2+(0.3-0.2)")
                && filter.contains("between(Y\\,H*if(lt((T-0)\\,0)")
                && filter.contains("0.25+(0.35-0.25)")
                && filter.contains("0.75+(0.4-0.75)"),
            "mask keyframes should animate bounds and opacity in the alpha gate: {filter}"
        );
        assert!(
            spec.limitations.is_empty(),
            "animated mask lowerings should not surface limitations: {:?}",
            spec.limitations
        );
    }

    #[test]
    fn guide_section_render_spec_trims_timeline_output_to_marker_range() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_guide_section(dir.path());

        let spec =
            build_timeline_section_render_spec(dir.path(), "deliverables", "cold-open").unwrap();
        let cmd = spec.args.join(" ");

        assert_eq!(spec.total_duration_s, Some(1.25));
        assert!(spec.output_path.starts_with(dir.path().join("renders")));
        assert!(
            spec.output_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("timeline-section-cold-open-")
        );
        assert!(cmd.contains("-ss 0.5"));
        assert!(cmd.contains("-t 1.25"));
        assert!(cmd.contains("concat=n=1:v=1:a=1"));
    }

    #[test]
    fn guide_section_render_rejects_missing_or_point_markers() {
        let dir = tempfile::tempdir().unwrap();
        let otio_path = write_fixture_project_with_guide_section(dir.path());

        let missing =
            build_timeline_section_render_spec(dir.path(), "deliverables", "missing").unwrap_err();
        assert!(matches!(
            missing,
            RenderTimelineError::GuideSectionNotFound {
                guide_track_id,
                marker_id
            } if guide_track_id == "deliverables" && marker_id == "missing"
        ));

        let mut tl: Timeline = serde_json::from_slice(&fs::read(&otio_path).unwrap()).unwrap();
        tl.metadata.awidat.as_mut().unwrap().guide_tracks[0].markers[0].duration_s = None;
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

        let point_marker =
            build_timeline_section_render_spec(dir.path(), "deliverables", "cold-open")
                .unwrap_err();
        assert!(matches!(
            point_marker,
            RenderTimelineError::GuideSectionMissingDuration {
                guide_track_id,
                marker_id
            } if guide_track_id == "deliverables" && marker_id == "cold-open"
        ));
    }

    #[test]
    fn bezier_overlay_animation_attaches_to_render_plan() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_overlay_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-bezier-opacity".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".to_string(),
                    parameter: "overlay.opacity".to_string(),
                },
                keyframes: vec![
                    Keyframe {
                        time_s: 0.0,
                        value: 0.0,
                        interpolation: KeyframeInterpolation::Bezier,
                        easing: Easing::Linear,
                        bezier: Some(BezierHandles {
                            out_x: 0.25,
                            out_y: 0.1,
                            in_x: 0.25,
                            in_y: 1.0,
                        }),
                        tangent_mode: Default::default(),
                        spring: None,
                    },
                    Keyframe::linear(1.0, 1.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let (_, _, video_overlays, _, _, _, _, _, limitations) =
            collect_timeline_full_plan(dir.path()).unwrap();

        assert_eq!(video_overlays.len(), 1);
        assert_eq!(video_overlays[0].animations.len(), 1);
        assert!(limitations.is_empty());
    }

    #[test]
    fn timeline_render_spec_includes_bezier_animation_without_limitation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_overlay_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-bezier-opacity".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".to_string(),
                    parameter: "overlay.opacity".to_string(),
                },
                keyframes: vec![
                    Keyframe {
                        time_s: 0.0,
                        value: 0.0,
                        interpolation: KeyframeInterpolation::Bezier,
                        easing: Easing::Linear,
                        bezier: Some(BezierHandles {
                            out_x: 0.25,
                            out_y: 0.1,
                            in_x: 0.25,
                            in_y: 1.0,
                        }),
                        tangent_mode: Default::default(),
                        spring: None,
                    },
                    Keyframe::linear(1.0, 1.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let spec = build_timeline_render_spec(dir.path()).unwrap();

        assert!(spec.limitations.is_empty());
    }

    #[test]
    fn base_clip_unsupported_animation_surfaces_render_limitation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_base_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-base-brightness".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "c1".to_string(),
                    parameter: "awidat.color_correction.brightness".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.2), Keyframe::linear(1.0, 0.4)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: true,
                rationale: None,
            },
        );

        let spec = build_timeline_render_spec(dir.path()).unwrap();

        assert_eq!(spec.limitations.len(), 1);
        assert_eq!(spec.limitations[0].kind, "unsupported_parameter");
        assert_eq!(
            spec.limitations[0].animation_id.as_deref(),
            Some("anim-base-brightness")
        );
    }

    #[test]
    fn base_clip_blur_radius_animation_lowers_to_varblur_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_base_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-base-blur".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "c1".to_string(),
                    parameter: "awidat.blur.radius_px".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 12.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(spec.limitations.is_empty());
        assert!(
            filter.contains("varblur=min_r=0:max_r=100:planes=15"),
            "animated clip blur should lower through a radius-map varblur filter: {filter}"
        );
        assert!(
            filter.contains("geq=lum='min(max(("),
            "animated clip blur should drive the radius map from keyframes: {filter}"
        );
    }

    #[test]
    fn effect_parameter_alias_selects_blur_radius_animation() {
        let animations = vec![ParameterAnimation {
            id: "blur-alias".to_string(),
            target: awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-a".to_string(),
                parameter: "effects.awidat.blur.params.radius_px".to_string(),
            },
            keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 12.0)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        }];
        let mut consumed = BTreeSet::new();

        let selected = select_clip_effect_animation(
            &animations,
            "clip-a",
            "awidat.blur.radius_px",
            &mut consumed,
        )
        .expect("alias should select canonical blur radius animation");

        assert_eq!(selected.parameter, "awidat.blur.radius_px");
        assert!(consumed.contains("blur-alias"));
    }

    #[test]
    fn base_clip_shake_intensity_animation_lowers_to_dynamic_crop_expression() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_base_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-base-shake".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "c1".to_string(),
                    parameter: "awidat.shake.intensity_px".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 14.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(spec.limitations.is_empty());
        assert!(
            filter.contains("pad=iw+28:ih+28:14:14"),
            "animated shake should allocate padding from peak keyframe intensity: {filter}"
        );
        assert!(
            filter.contains("sin(2*PI*12*t)"),
            "animated shake should use the default shake frequency without a stamped effect: {filter}"
        );
        assert!(
            filter.contains("if(lt(t\\,0)\\,0"),
            "animated shake should drive displacement from keyframes: {filter}"
        );
    }

    #[test]
    fn base_clip_warp_k1_animation_lowers_to_sendcmd_lenscorrection() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_base_animation(
            dir.path(),
            ParameterAnimation {
                id: "anim-base-warp".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "c1".to_string(),
                    parameter: "awidat.warp.k1".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, -0.15), Keyframe::linear(1.0, 0.05)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let spec = build_timeline_render_spec(dir.path()).unwrap();
        let filter = filter_complex_from_argv(&spec.args);

        assert!(spec.limitations.is_empty());
        assert!(
            filter.contains("sendcmd=c='0 warp0 k1 -0.15"),
            "animated warp should command lenscorrection from first keyframe: {filter}"
        );
        assert!(
            filter.contains("1 warp0 k1 0.05"),
            "animated warp should command lenscorrection through final keyframe: {filter}"
        );
        assert!(
            filter.contains("0.5 warp0 k1 -0.05"),
            "animated warp should sample interpolated values between keyframes: {filter}"
        );
        assert!(
            filter.contains("lenscorrection@warp0=cx=0.5:cy=0.5:k1=-0.15:k2=0:i=bilinear"),
            "animated warp should initialize lenscorrection from first keyframe/defaults: {filter}"
        );
    }

    #[test]
    fn track_volume_db_animation_lowers_to_audio_automation_plan() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_audio_track_animation(
            dir.path(),
            ParameterAnimation {
                id: "music-duck".to_string(),
                target: awidat_proto::professional::AnimationTarget::TrackParameter {
                    track: "Music".to_string(),
                    parameter: "volume_db".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, -18.0), Keyframe::linear(2.0, -6.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let (_, _, _, _, _, _, audio_tracks, _, limitations) =
            collect_timeline_full_plan(dir.path()).unwrap();

        assert!(limitations.is_empty());
        assert_eq!(audio_tracks.len(), 1);
        let automation = audio_tracks[0]
            .volume_automation
            .as_ref()
            .expect("track volume animation should lower");
        assert_eq!(automation.parameter, "volume_db");
        assert!(automation.expression.contains("pow(10\\,"));
        assert_eq!(automation.keyframes.len(), 2);
    }

    #[test]
    fn base_clip_volume_db_animation_lowers_to_segment_audio_automation() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project_with_base_animation(
            dir.path(),
            ParameterAnimation {
                id: "clip-volume".to_string(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "c1".to_string(),
                    parameter: "volume_db".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, -18.0), Keyframe::linear(2.0, -3.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: None,
            },
        );

        let (segments, _, _, _, _, _, _, _, limitations) =
            collect_timeline_full_plan(dir.path()).unwrap();

        assert!(limitations.is_empty());
        assert_eq!(segments.len(), 1);
        let automation = segments[0]
            .volume_automation
            .as_ref()
            .expect("clip volume animation should lower");
        assert_eq!(automation.parameter, "volume_db");
        assert!(automation.expression.contains("pow(10\\,"));
        assert_eq!(automation.keyframes.len(), 2);
    }

    #[test]
    fn missing_asset_returns_missing_asset_error() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        // Delete the asset; spec build should fail with MissingAsset.
        fs::remove_file(dir.path().join("raw/x.mp4")).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::MissingAsset { .. }));
    }

    /// Build a basic TimelineSegment for tests — no volume / speed
    /// effects. Saves repeating `..Default::default()` 12 times.
    fn seg(path: &str, start: f64, dur: f64) -> TimelineSegment {
        TimelineSegment {
            asset_path: PathBuf::from(path),
            start_s: start,
            duration_s: dur,
            ..Default::default()
        }
    }

    fn trans(from: usize, to: usize, duration_s: f64) -> TransitionPlan {
        TransitionPlan {
            from_segment_index: from,
            to_segment_index: to,
            kind: "SMPTE_Dissolve".into(),
            in_offset_s: duration_s / 2.0,
            out_offset_s: duration_s / 2.0,
            duration_s,
            composition: None,
            gpu_shader: None,
        }
    }

    fn filter_complex_from_argv(argv: &[String]) -> String {
        argv.windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap()
    }

    #[test]
    fn explicit_audio_tracks_use_video_only_concat_and_amix() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "A1".into(),
            role: "dialogue".into(),
            volume: 0.8,
            volume_automation: None,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![
                AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path: PathBuf::from("/tmp/a.wav"),
                    start_s: 0.0,
                    duration_s: 2.0,
                    volume: Some(0.5),
                    volume_automation: None,
                    speed: None,
                    fade_in_s: Some(0.1),
                    fade_out_s: Some(0.2),
                    audio_fx: None,
                }),
                AudioTrackItemPlan::Gap { duration_s: 1.0 },
            ],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains("concat=n=1:v=1:a=0[vonly]"));
        assert!(filter.contains("atrim=0:2"));
        assert!(filter.contains("volume=0.5"));
        assert!(filter.contains("afade=t=in"));
        assert!(filter.contains("anullsrc"));
        assert!(filter.contains("amix=inputs=1"));
    }

    #[test]
    fn split_edit_metadata_synthesizes_overlapping_audio_tracks() {
        let mut a = seg("/tmp/a.mp4", 10.0, 5.0);
        a.clip_name = "a".into();
        a.source_available_start_s = Some(10.0);
        a.source_available_end_s = Some(20.0);
        a.audio_trail_s = Some(0.75);
        let mut b = seg("/tmp/b.mp4", 20.0, 5.0);
        b.clip_name = "b".into();
        b.source_available_start_s = Some(19.0);
        b.source_available_end_s = Some(30.0);
        b.audio_lead_s = Some(0.5);

        let tracks = synthesize_split_edit_audio_tracks(&[a, b]).unwrap();
        assert_eq!(tracks.len(), 2);
        let AudioTrackItemPlan::Clip(first) = &tracks[0].items[0] else {
            panic!("expected first clip")
        };
        assert_eq!(first.start_s, 10.0);
        assert_eq!(first.duration_s, 5.75);
        let AudioTrackItemPlan::Gap { duration_s } = tracks[1].items[0] else {
            panic!("expected J-cut lead gap")
        };
        assert!((duration_s - 4.5).abs() < 1e-9);
        let AudioTrackItemPlan::Clip(second) = &tracks[1].items[1] else {
            panic!("expected second clip")
        };
        assert_eq!(second.start_s, 19.5);
        assert_eq!(second.duration_s, 5.5);
    }

    #[test]
    fn split_edit_audio_tracks_use_video_only_render_path() {
        let mut a = seg("/tmp/a.mp4", 10.0, 5.0);
        a.source_available_start_s = Some(10.0);
        a.source_available_end_s = Some(20.0);
        a.audio_trail_s = Some(0.75);
        let mut b = seg("/tmp/b.mp4", 20.0, 5.0);
        b.source_available_start_s = Some(19.0);
        b.source_available_end_s = Some(30.0);
        b.audio_lead_s = Some(0.5);
        let segs = vec![a, b];
        let audio_tracks = synthesize_split_edit_audio_tracks(&segs).unwrap();
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains("concat=n=2:v=1:a=0[vonly]"));
        assert!(filter.contains("atrim=10:15.75"));
        assert!(filter.contains("anullsrc=r=48000:cl=stereo:d=4.5"));
        assert!(filter.contains("atrim=19.5:25"));
        assert!(filter.contains("amix=inputs=2"));
    }

    #[test]
    fn explicit_audio_tracks_preserve_visual_transitions_without_acrossfade() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 0.0, 2.0)];
        let transitions = vec![trans(0, 1, 0.5)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "A1".into(),
            role: "dialogue".into(),
            volume: 1.0,
            volume_automation: None,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                asset_path: PathBuf::from("/tmp/a.wav"),
                start_s: 0.0,
                duration_s: 4.0,
                volume: None,
                volume_automation: None,
                speed: None,
                fade_in_s: None,
                fade_out_s: None,
                audio_fx: None,
            })],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &transitions,
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(
            filter.contains("xfade=transition=fade:duration=0.5"),
            "filter graph: {filter}",
        );
        assert!(
            !filter.contains("acrossfade"),
            "explicit audio tracks should own audio transitions: {filter}",
        );
        assert!(filter.contains("amix=inputs=1"), "filter graph: {filter}");
    }

    #[test]
    fn timeline_loudness_target_appends_final_audio_loudnorm() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            Some(LoudnessTargetPlan {
                integrated_lufs: -16.0,
                true_peak_db: Some(-1.5),
            }),
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains(
            "[outa]aresample=async=1:first_pts=0,loudnorm=I=-16:TP=-1.5:LRA=11[mastera]"
        ));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "-map" && w[1] == "[mastera]")
        );
    }

    #[test]
    fn filter_planner_with_no_transitions_emits_smoothed_concat_graph() {
        // Pins the no-transition graph shape so hard cuts keep micro-fades.
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 1.0, 3.0)];
        let plan = FilterPlanner::new(&segs, &[]).plan();
        assert_eq!(
            plan.filter_complex,
            "[0:a:0]afade=t=out:st=1.97:d=0.03[bfade0];[1:a:0]afade=t=in:st=0:d=0.03[bfade1];[0:v:0][bfade0][1:v:0][bfade1]concat=n=2:v=1:a=1[outv][outa]",
        );
        assert_eq!(plan.video_out_label, "[outv]");
        assert_eq!(plan.audio_out_label, "[outa]");
    }

    #[test]
    fn filter_planner_with_one_transition_emits_xfade_pair() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 5.0), seg("/tmp/b.mp4", 0.0, 4.0)];
        let trans = vec![trans(0, 1, 1.0)];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        // xfade with kind=fade (mapped from SMPTE_Dissolve), offset =
        // from.duration - transition.duration = 4.0.
        assert!(
            plan.filter_complex
                .contains("xfade=transition=fade:duration=1:offset=4"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade for audio.
        assert!(
            plan.filter_complex.contains("acrossfade=d=1"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // The chunk pair feeds into a 1-input concat (the merged xfade
        // counts as one input pair).
        assert!(
            plan.filter_complex
                .contains("concat=n=1:v=1:a=1[outv][outa]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_uses_cut_audio_policy_for_motion_transitions() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 5.0), seg("/tmp/b.mp4", 0.0, 4.0)];
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "awidat.slide_left".into(),
            in_offset_s: 0.5,
            out_offset_s: 0.5,
            duration_s: 1.0,
            composition: None,
            gpu_shader: None,
        }];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        assert!(
            plan.filter_complex
                .contains("[apts0][apts1]acrossfade=d=1:c1=nofade:c2=nofade[xa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_with_transition_in_middle_of_three_segments() {
        // [A, B, C] with a transition between A and B → concat n=2:
        // input 1 = xfade(A, B), input 2 = C alone.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![trans(0, 1, 0.5)];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        assert!(plan.filter_complex.contains("xfade="));
        // Concat takes 2 inputs: chunk(A,B) + raw C.
        assert!(
            plan.filter_complex
                .contains("concat=n=2:v=1:a=1[outv][outa]")
        );
        // C's streams are timestamp-normalized, then its audio is
        // smoothed before concat.
        assert!(
            plan.filter_complex
                .contains("[apts2]afade=t=in:st=0:d=0.03[hfade1]")
        );
        assert!(plan.filter_complex.contains("[vpts2][hfade1]"));
    }

    #[test]
    fn filter_planner_renders_chained_transitions() {
        // [A, B, C] with transitions A-B AND B-C must render both
        // transitions. The second xfade consumes the first xfade's
        // output and uses the shortened chain duration for its offset.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![trans(0, 1, 0.5), trans(1, 2, 0.5)];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        let xfade_count = plan.filter_complex.matches("xfade=").count();
        assert_eq!(xfade_count, 2, "filter graph: {}", plan.filter_complex);
        assert!(
            plan.filter_complex
                .contains("[vpts0][vpts1]xfade=transition=fade:duration=0.5:offset=2.5[xv0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[xv0][vpts2]xfade=transition=fade:duration=0.5:offset=6[xv1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[apts0][apts1]acrossfade=d=0.5[xa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[xa0][apts2]acrossfade=d=0.5[xa1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("concat=n=1:v=1:a=1[outv][outa]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_preserves_hard_cuts_between_transition_runs() {
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
            seg("/tmp/d.mp4", 0.0, 5.0),
        ];
        let trans = vec![trans(0, 1, 0.5), trans(2, 3, 0.25)];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        let xfade_count = plan.filter_complex.matches("xfade=").count();
        assert_eq!(xfade_count, 2, "filter graph: {}", plan.filter_complex);
        assert!(
            plan.filter_complex
                .contains("[xv0][hfade0][xv1][hfade1]concat=n=2:v=1:a=1[outv][outa]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_volume_filter_when_segment_carries_value() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(0.5);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();
        assert!(
            plan.filter_complex.contains("[0:a:0]volume=0.5[av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Concat input pair for seg 0 uses faded post-volume audio.
        assert!(
            plan.filter_complex
                .contains("[av0]afade=t=out:st=1.97:d=0.03[bfade0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Seg 1 has no volume effect, but fades in after the hard cut.
        assert!(
            plan.filter_complex.contains("[1:v:0][bfade1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_skips_volume_filter_at_unity() {
        // volume = 1.0 is the no-op default; no filter should land.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(1.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("volume="),
            "filter graph should skip unity volume: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn explicit_audio_track_emits_volume_automation_filter() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "music".into(),
            role: "music".into(),
            volume: 1.0,
            volume_automation: Some(AudioAutomationPlan {
                parameter: "volume_db".into(),
                expression: "pow(10\\,(if(lt(t\\,2)\\,-12+(-6--12)*((t-0)/(2-0))\\,-6))/20)".into(),
                keyframes: vec![Keyframe::linear(0.0, -12.0), Keyframe::linear(2.0, -6.0)],
            }),
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                asset_path: PathBuf::from("/tmp/music.wav"),
                start_s: 0.0,
                duration_s: 2.0,
                volume: None,
                volume_automation: None,
                speed: None,
                fade_in_s: None,
                fade_out_s: None,
                audio_fx: None,
            })],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();

        assert!(
            filter.contains("volume='pow(10\\,"),
            "expected volume automation in filter graph: {filter}"
        );
        assert!(filter.contains(":eval=frame[atrackauto0]"));
    }

    #[test]
    fn filter_planner_emits_clip_volume_automation_for_timeline_segment() {
        let mut s = seg("/tmp/a.mp4", 0.0, 2.0);
        s.volume = Some(0.25);
        s.volume_automation = Some(AudioAutomationPlan {
            parameter: "volume_db".into(),
            expression: "pow(10\\,(if(lt(t\\,2)\\,-18+(-3--18)*((t-0)/(2-0))\\,-3))/20)".into(),
            keyframes: vec![Keyframe::linear(0.0, -18.0), Keyframe::linear(2.0, -3.0)],
        });

        let plan = FilterPlanner::new(&[s], &[]).plan();

        assert!(
            plan.filter_complex.contains("volume='pow(10\\,"),
            "expected clip-local volume automation in filter graph: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex.contains(":eval=frame[av0]"),
            "expected eval=frame clip-local volume automation label: {}",
            plan.filter_complex
        );
        assert!(
            !plan.filter_complex.contains("volume=0.25"),
            "scalar volume should not also apply when clip automation exists: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn explicit_audio_track_emits_clip_volume_automation_filter() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "music".into(),
            role: "music".into(),
            volume: 1.0,
            volume_automation: None,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                asset_path: PathBuf::from("/tmp/music.wav"),
                start_s: 0.0,
                duration_s: 2.0,
                volume: Some(0.25),
                volume_automation: Some(AudioAutomationPlan {
                    parameter: "volume".into(),
                    expression: "if(lt(t\\,2)\\,0.25\\,1)".into(),
                    keyframes: vec![Keyframe::linear(0.0, 0.25), Keyframe::linear(2.0, 1.0)],
                }),
                speed: None,
                fade_in_s: None,
                fade_out_s: None,
                audio_fx: None,
            })],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("volume='if(lt(t\\,2)\\,0.25\\,1)':eval=frame[avol0_0]"),
            "expected clip-local volume automation in explicit audio path: {filter}"
        );
        assert!(
            !filter.contains("volume=0.25"),
            "scalar volume should not also apply when clip automation exists: {filter}"
        );
    }

    #[test]
    fn explicit_audio_track_volume_automation_uses_post_concat_track_time() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 6.0)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "music".into(),
            role: "music".into(),
            volume: 1.0,
            volume_automation: Some(AudioAutomationPlan {
                parameter: "volume".into(),
                expression: "if(lt(t\\,5)\\,0.25\\,1)".into(),
                keyframes: vec![Keyframe::linear(5.0, 0.25), Keyframe::linear(6.0, 1.0)],
            }),
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![
                AudioTrackItemPlan::Gap { duration_s: 5.0 },
                AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path: PathBuf::from("/tmp/music.wav"),
                    start_s: 0.0,
                    duration_s: 1.0,
                    volume: None,
                    volume_automation: None,
                    speed: None,
                    fade_in_s: None,
                    fade_out_s: None,
                    audio_fx: None,
                }),
            ],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();

        assert!(filter.contains("anullsrc=r=48000:cl=stereo:d=5[agap0_0]"));
        assert!(filter.contains("volume='if(lt(t\\,5)\\,0.25\\,1)':eval=frame[atrackauto0]"));
        assert!(
            !filter.contains("t+5"),
            "automation runs after track concat, where t already includes leading gaps: {filter}"
        );
    }

    #[test]
    fn filter_planner_volume_threads_through_xfade_pair() {
        // Volume on the to-segment of an xfade pair must feed the
        // [av<i>] label into acrossfade, not the raw [i:a:0].
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut s1 = seg("/tmp/b.mp4", 0.0, 4.0);
        s1.volume = Some(0.3);
        let trans = vec![trans(0, 1, 1.0)];
        let plan = FilterPlanner::new(&[s0, s1], &trans).plan();
        assert!(
            plan.filter_complex.contains("[1:a:0]volume=0.3[av1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade reads timestamp-normalized audio after clip effects.
        assert!(
            plan.filter_complex.contains("[apts0][apts1]acrossfade"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_audio_fx_runs_before_speed_and_volume() {
        let mut s = seg("/tmp/a.mp4", 0.0, 4.0);
        s.audio_fx = Some(AudioFxPlan {
            high_pass_hz: Some(80.0),
            hum_notch_hz: Some(60.0),
            compressor_threshold_db: Some(-18.0),
            compressor_ratio: Some(2.5),
            limiter_limit_db: Some(-1.0),
            loudnorm_i: Some(-16.0),
            loudnorm_tp: Some(-1.5),
            ..Default::default()
        });
        s.speed = Some(SpeedPlan::new(1.1));
        s.volume = Some(0.8);
        let plan = FilterPlanner::new(&[s], &[]).plan();
        let filter = plan.filter_complex;
        let fx_pos = filter.find("highpass=f=80").unwrap();
        let speed_pos = filter.find("atempo=1.1").unwrap();
        let volume_pos = filter.find("volume=0.8").unwrap();
        assert!(fx_pos < speed_pos, "filter graph: {filter}");
        assert!(speed_pos < volume_pos, "filter graph: {filter}");
        assert!(filter.contains("bandstop=f=60"));
        assert!(filter.contains("acompressor=threshold=-18dB:ratio=2.5"));
        assert!(filter.contains("loudnorm=I=-16:TP=-1.5:LRA=11"));
    }

    #[test]
    fn filter_planner_audio_fx_emits_pan_cleanup_and_rnnoise_filters() {
        let mut s = seg("/tmp/a.mp4", 0.0, 4.0);
        s.audio_fx = Some(AudioFxPlan {
            pan: Some(-0.5),
            balance: Some(0.25),
            adeclick: Some(true),
            adeclip: Some(true),
            arnndn_model: Some("models/rnnoise:voice.rnnn".into()),
            ..Default::default()
        });

        let plan = FilterPlanner::new(&[s], &[]).plan();
        let filter = plan.filter_complex;

        assert!(
            filter.contains("adeclick,adeclip,arnndn=m=models/rnnoise\\:voice.rnnn"),
            "cleanup filters should be ordered before stereo shaping: {filter}"
        );
        assert!(
            filter.contains("pan=stereo|c0=1*FL|c1=0.5*FR"),
            "pan should lower to a stereo pan filter: {filter}"
        );
        assert!(
            filter.contains("pan=stereo|c0=0.75*FL|c1=1*FR"),
            "balance should lower to a stereo balance filter: {filter}"
        );
    }

    #[test]
    fn ducking_amount_db_changes_sidechain_compression_ratio() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let audio_tracks = vec![
            AudioTrackPlan {
                name: "dialogue".into(),
                role: "dialogue".into(),
                volume: 1.0,
                volume_automation: None,
                muted: false,
                solo: false,
                ducking: None,
                audio_fx: None,
                items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path: PathBuf::from("/tmp/dialogue.wav"),
                    start_s: 0.0,
                    duration_s: 2.0,
                    volume: None,
                    volume_automation: None,
                    speed: None,
                    fade_in_s: None,
                    fade_out_s: None,
                    audio_fx: None,
                })],
            },
            AudioTrackPlan {
                name: "music".into(),
                role: "music".into(),
                volume: 1.0,
                volume_automation: None,
                muted: false,
                solo: false,
                ducking: Some(DuckingPlan {
                    enabled: true,
                    amount_db: -6.0,
                    attack_ms: 25.0,
                    release_ms: 150.0,
                }),
                audio_fx: None,
                items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path: PathBuf::from("/tmp/music.wav"),
                    start_s: 0.0,
                    duration_s: 2.0,
                    volume: None,
                    volume_automation: None,
                    speed: None,
                    fade_in_s: None,
                    fade_out_s: None,
                    audio_fx: None,
                })],
            },
        ];

        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("sidechaincompress=threshold=0.05:ratio=4.5:attack=25:release=150"),
            "ducking amount should tune the sidechain compressor ratio: {filter}"
        );
    }

    #[test]
    fn filter_planner_emits_setpts_and_atempo_for_speed_segment() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(SpeedPlan::new(2.0));
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();
        // setpts on video with 1/factor.
        assert!(
            plan.filter_complex.contains("[0:v:0]setpts=0.5*PTS[sv0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // atempo single-stage (factor 2.0 sits inside [0.5, 2.0]).
        assert!(
            plan.filter_complex.contains("[0:a:0]atempo=2[sa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Concat for seg 0 reads faded post-speed audio.
        assert!(
            plan.filter_complex
                .contains("[sa0]afade=t=out:st=1.97:d=0.03[bfade0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_adds_micro_fades_at_hard_cut_boundaries() {
        let s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();

        assert!(
            plan.filter_complex
                .contains("[0:a:0]afade=t=out:st=3.97:d=0.03[bfade0]"),
            "outgoing segment should fade down before the hard cut: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[1:a:0]afade=t=in:st=0:d=0.03[bfade1]"),
            "incoming segment should fade up after the hard cut: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[0:v:0][bfade0][1:v:0][bfade1]"),
            "concat should consume smoothed audio labels: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_chains_atempo_for_extreme_speed() {
        // factor=4.0 → atempo=2.0 twice (2.0 × 2.0 = 4.0).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(SpeedPlan::new(4.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains("atempo=2,atempo=2"),
            "expected chained atempo for factor=4, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_chains_atempo_for_slow_speed() {
        // factor=0.25 → atempo=0.5 twice (0.5 × 0.5 = 0.25).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(SpeedPlan::new(0.25));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains("atempo=0.5,atempo=0.5"),
            "expected chained atempo for factor=0.25, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_speed_uses_effective_duration_for_xfade_offset() {
        // 4s @ 2× = 2s effective. xfade duration 0.5 → offset 1.5
        // (effective − transition.duration).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(SpeedPlan::new(2.0));
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let trans = vec![trans(0, 1, 0.5)];
        let plan = FilterPlanner::new(&[s0, s1], &trans).plan();
        assert!(
            plan.filter_complex.contains("offset=1.5"),
            "expected offset=1.5 (post-speed), got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_speed_and_volume_compose_in_order() {
        // Both effects on the same segment: setpts/atempo run first,
        // then volume runs on the time-stretched audio.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(SpeedPlan::new(2.0));
        s0.volume = Some(0.5);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        // atempo runs first (input [0:a:0] → [sa0]).
        assert!(
            plan.filter_complex.contains("[0:a:0]atempo=2[sa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // volume runs next on [sa0] → [av0].
        assert!(
            plan.filter_complex.contains("[sa0]volume=0.5[av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_color_correction_before_speed() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_correction = Some(ColorCorrectionPlan {
            exposure_ev: Some(0.5),
            contrast: Some(1.2),
            saturation: Some(0.9),
            temperature: Some(0.4),
            tint: None,
            shadows: None,
            highlights: Some(-0.3),
        });
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]eq=brightness=0.061:contrast=1.2:saturation=0.9,curves="),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains(",colorbalance="),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[cv0]setpts=0.5*PTS[sv0]"),
            "color correction should feed speed, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_tonemaps_hdr_transfer_before_color_correction() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.hdr_transfer = Some(HdrTransfer::Smpte2084);
        s0.color_correction = Some(ColorCorrectionPlan {
            exposure_ev: Some(0.25),
            contrast: Some(1.1),
            saturation: Some(1.05),
            temperature: None,
            tint: None,
            shadows: None,
            highlights: None,
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        let filter = plan.filter_complex;
        let tonemap_pos = filter.find("zscale=t=linear").unwrap();
        let color_pos = filter.find("eq=brightness=").unwrap();

        assert!(
            filter.contains("tonemap=tonemap=hable:desat=0"),
            "filter graph: {filter}",
        );
        assert!(
            filter.contains("zscale=transfer=bt709:matrix=bt709:primaries=bt709"),
            "filter graph: {filter}",
        );
        assert!(tonemap_pos < color_pos, "filter graph: {filter}");
    }

    #[test]
    fn filter_planner_emits_lut3d_before_speed_and_concat() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.lut_path = Some(PathBuf::from("/tmp/luts/show-look.cube"));
        s0.lut_interpolation = Some("tetrahedral".into());
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]lut3d=file='/tmp/luts/show-look.cube':interp=tetrahedral[lv0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[lv0]setpts=0.5*PTS[sv0]"),
            "LUT should feed speed, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[sv0][sa0]concat"),
            "post-LUT/post-speed labels should feed concat, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_split_and_blend_when_lut_strength_below_one() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.lut_path = Some(PathBuf::from("/tmp/luts/show-look.cube"));
        s0.lut_interpolation = Some("tetrahedral".into());
        s0.lut_strength = Some(0.6);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]split[lut_pre_0][lut_in_0];"),
            "expected split fork, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains(
                "[lut_in_0]lut3d=file='/tmp/luts/show-look.cube':interp=tetrahedral[lut_out_0];"
            ),
            "expected lut3d on forked branch, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex
                .contains("[lut_out_0][lut_pre_0]blend=all_opacity=0.6[lv0]"),
            "expected blend with LUT-as-top semantics (strength={{0.6}} → 60% LUT), got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn lut3d_filter_block_puts_lut_as_blend_top_input() {
        // FFmpeg's `blend` filter applies `all_opacity` to the
        // FIRST input. For `strength=X` to mean "X fraction of the
        // LUT visible," the LUT-applied stream must be first.
        // Regression guard against the inverted ordering that
        // shipped before the P1 fix.
        let block = lut3d_filter_block(
            "[base]",
            "[done]",
            "t",
            Path::new("/luts/x.cube"),
            Some("tetrahedral"),
            Some(0.4),
        );
        let blend_idx = block
            .find("blend=all_opacity=")
            .expect("blend filter must be present at strength<1");
        let blend_clause = &block[..blend_idx];
        let lut_label = "[lut_out_t]";
        let pre_label = "[lut_pre_t]";
        let lut_pos = blend_clause
            .rfind(lut_label)
            .expect("LUT-out label must precede blend");
        let pre_pos = blend_clause
            .rfind(pre_label)
            .expect("pre-LUT label must precede blend");
        assert!(
            lut_pos < pre_pos,
            "LUT-out must be the first (top) blend input — saw lut at {lut_pos}, pre at {pre_pos} in: {block}",
        );
    }

    #[test]
    fn filter_planner_skips_split_when_lut_strength_is_one() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.lut_path = Some(PathBuf::from("/tmp/luts/show-look.cube"));
        s0.lut_interpolation = Some("tetrahedral".into());
        s0.lut_strength = Some(1.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]lut3d=file='/tmp/luts/show-look.cube':interp=tetrahedral[lv0]"),
            "expected single-filter chain at full strength, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("split"),
            "should not emit split at full strength, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("blend=all_opacity"),
            "should not emit blend at full strength, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_lowers_color_pipeline_look_to_lut3d() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            working_space: None,
            lut_input_space: Some("rec709_g24".into()),
            output_space: Some("rec709_g24".into()),
            input_transform_lut: None,
            shaper_lut: None,
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            look_interpolation: Some("tetrahedral".into()),
            look_strength: None,
            output_transform_lut: None,
            mask_source: None,
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]lut3d=file='/tmp/luts/look.cube':interp=tetrahedral[cpv0]"),
            "expected single-LUT pipeline chain, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_lowers_color_pipeline_log_chain() {
        // Log clip + shaper + creative LUT + output transform — every
        // slot exercised.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "arri_logc4".into(),
            working_space: Some("rec709_g24".into()),
            lut_input_space: Some("rec709_g24".into()),
            output_space: Some("rec709_g24".into()),
            input_transform_lut: None,
            shaper_lut: Some(PathBuf::from("/tmp/luts/logc4_shaper.csp")),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            look_interpolation: Some("tetrahedral".into()),
            look_strength: Some(0.5),
            output_transform_lut: None,
            mask_source: None,
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        // 1D shaper first, on the clip's source label.
        assert!(
            plan.filter_complex
                .contains("[0:v:0]lut1d=file='/tmp/luts/logc4_shaper.csp'[cp_shaper_0]"),
            "expected shaper lut1d on clip input, got: {}",
            plan.filter_complex,
        );
        // Look LUT runs after the shaper with strength 0.5 (split/blend block).
        assert!(
            plan.filter_complex
                .contains("[cp_shaper_0]split[lut_pre_cp_look_0][lut_in_cp_look_0]"),
            "expected split after shaper, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("blend=all_opacity=0.5[cpv0]"),
            "expected blend strength to drive cpv0, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_color_pipeline_supersedes_legacy_lut() {
        // If both effects are on a clip, color_pipeline wins; the
        // legacy lut3d branch is skipped.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.lut_path = Some(PathBuf::from("/tmp/luts/legacy.cube"));
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            working_space: None,
            lut_input_space: Some("rec709_g24".into()),
            output_space: None,
            input_transform_lut: None,
            shaper_lut: None,
            look_lut: Some(PathBuf::from("/tmp/luts/new.cube")),
            look_interpolation: None,
            look_strength: None,
            output_transform_lut: None,
            mask_source: None,
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains("/tmp/luts/new.cube"),
            "expected new color_pipeline LUT in argv, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("/tmp/luts/legacy.cube"),
            "legacy lut3d should be skipped when color_pipeline is present, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_color_pipeline_with_only_metadata_is_noop_chain() {
        // No LUT slots set — the pipeline carries only color-space
        // metadata, so the filter chain has no LUT segment.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            working_space: None,
            lut_input_space: None,
            output_space: Some("rec709_g24".into()),
            input_transform_lut: None,
            shaper_lut: None,
            look_lut: None,
            look_interpolation: None,
            look_strength: None,
            output_transform_lut: None,
            mask_source: None,
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("lut3d"),
            "metadata-only pipeline must not emit any LUT, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("lut1d"),
            "metadata-only pipeline must not emit any LUT, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn build_clip_preview_filtergraph_emits_strength_blend_for_legacy_lut() {
        use awidat_proto::otio::{Clip, Effect};
        let mut clip = Clip::empty("partial".to_string());
        let mut lut = Effect::new("awidat.lut");
        lut.metadata
            .insert("lut_path".into(), serde_json::json!("luts/x.cube"));
        lut.metadata
            .insert("strength".into(), serde_json::json!(0.4));
        clip.effects.push(lut);
        let preview = build_clip_preview_filtergraph(&clip, Path::new("/p"));
        let graph = preview.graph.expect("graph emitted");
        // Strength<1 means split/blend exists in the graph.
        assert!(
            graph.contains("split[lut_pre_preview]"),
            "expected split fork in graph, got: {graph}",
        );
        // LUT-as-top semantics carry through.
        assert!(
            graph.contains("[lut_out_preview][lut_pre_preview]blend=all_opacity=0.4"),
            "expected blend with LUT-top in graph, got: {graph}",
        );
        // Final segment terminates at the canonical [grade_out].
        assert!(graph.ends_with("[grade_out]"), "graph: {graph}");
    }

    #[test]
    fn build_clip_preview_filtergraph_chains_color_correction_into_pipeline() {
        use awidat_proto::otio::{Clip, Effect};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        std::fs::write(dir.path().join("luts/look.cube"), b"# stub").unwrap();

        let mut clip = Clip::empty("graded".to_string());
        let mut cc = Effect::new("awidat.color_correction");
        cc.metadata
            .insert("exposure_ev".into(), serde_json::json!(0.5));
        clip.effects.push(cc);
        let mut cp = Effect::new("awidat.color_pipeline");
        cp.metadata
            .insert("clip_input_space".into(), serde_json::json!("rec709_g24"));
        cp.metadata
            .insert("lut_input_space".into(), serde_json::json!("rec709_g24"));
        cp.metadata
            .insert("look_lut".into(), serde_json::json!("luts/look.cube"));
        cp.metadata
            .insert("look_strength".into(), serde_json::json!(0.7));
        clip.effects.push(cp);
        let preview = build_clip_preview_filtergraph(&clip, dir.path());
        let graph = preview.graph.expect("graph emitted");
        // Color correction feeds the pipeline stage.
        assert!(graph.starts_with("[in]eq=brightness="), "graph: {graph}");
        assert!(graph.contains("[cc_preview]"), "graph: {graph}");
        assert!(graph.contains("blend=all_opacity=0.7"), "graph: {graph}");
        assert!(graph.ends_with("[grade_out]"), "graph: {graph}");
    }

    #[test]
    fn build_clip_preview_filtergraph_mirrors_hdr_tonemap() {
        use awidat_proto::otio::{Clip, Effect};
        let mut clip = Clip::empty("hdr".to_string());
        let mut sc = Effect::new("awidat.source_color");
        sc.metadata
            .insert("transfer".into(), serde_json::json!("smpte2084"));
        clip.effects.push(sc);
        let preview = build_clip_preview_filtergraph(&clip, Path::new("/p"));
        let graph = preview.graph.expect("HDR source must produce a chain");
        // Tonemap fires first, so the graph starts at [in] and
        // routes through the HDR stage before anything else.
        assert!(graph.starts_with("[in]zscale=t=linear"), "graph: {graph}");
        assert!(graph.contains("tonemap=tonemap=hable"), "graph: {graph}");
        // The HDR stage's output is the final stage because no
        // other effects are stamped; it should rewrite to
        // [grade_out].
        assert!(graph.ends_with("[grade_out]"), "graph: {graph}");
    }

    #[test]
    fn build_clip_preview_filtergraph_returns_none_for_bare_clip() {
        use awidat_proto::otio::Clip;
        let clip = Clip::empty("bare".to_string());
        let preview = build_clip_preview_filtergraph(&clip, Path::new("/p"));
        assert!(preview.graph.is_none());
        assert!(preview.skipped.is_empty());
    }

    #[test]
    fn build_clip_grade_chain_emits_color_and_lut_for_simple_clip() {
        use awidat_proto::otio::{Clip, Effect};
        let mut clip = Clip::empty("test-clip".to_string());
        let mut color = Effect::new("awidat.color_correction");
        color
            .metadata
            .insert("exposure_ev".into(), serde_json::json!(0.5));
        color
            .metadata
            .insert("saturation".into(), serde_json::json!(1.2));
        clip.effects.push(color);
        let mut lut = Effect::new("awidat.lut");
        lut.metadata
            .insert("lut_path".into(), serde_json::json!("luts/show-look.cube"));
        lut.metadata
            .insert("interpolation".into(), serde_json::json!("tetrahedral"));
        clip.effects.push(lut);

        // Use an arbitrary project root; we're not parsing the LUT
        // here, just building the filter string.
        let chain = build_clip_grade_chain(&clip, Path::new("/tmp/project"));
        let vf = chain.vf.expect("expected a non-empty grade chain");
        assert!(vf.contains("eq=brightness="));
        assert!(vf.contains("lut3d=file='/tmp/project/luts/show-look.cube':interp=tetrahedral"));
        // Order: color_correction then LUT.
        let eq_idx = vf.find("eq=").unwrap();
        let lut_idx = vf.find("lut3d=").unwrap();
        assert!(
            eq_idx < lut_idx,
            "color correction must precede LUT in: {vf}"
        );
        assert!(chain.skipped.is_empty());
    }

    #[test]
    fn build_clip_grade_chain_warns_about_partial_strength() {
        use awidat_proto::otio::{Clip, Effect};
        let mut clip = Clip::empty("partial".to_string());
        let mut lut = Effect::new("awidat.lut");
        lut.metadata
            .insert("lut_path".into(), serde_json::json!("luts/x.cube"));
        lut.metadata
            .insert("strength".into(), serde_json::json!(0.6));
        clip.effects.push(lut);
        let chain = build_clip_grade_chain(&clip, Path::new("/p"));
        assert!(chain.vf.is_some(), "LUT still applied as approximation");
        assert!(
            chain.skipped.iter().any(|m| m.contains("strength")),
            "expected strength<1 to be noted, got {:?}",
            chain.skipped
        );
    }

    #[test]
    fn build_clip_grade_chain_returns_none_for_bare_clip() {
        use awidat_proto::otio::Clip;
        let clip = Clip::empty("bare".to_string());
        let chain = build_clip_grade_chain(&clip, Path::new("/p"));
        assert!(chain.vf.is_none());
        assert!(chain.skipped.is_empty());
    }

    #[test]
    fn auto_pre_lut_zscale_inserts_for_display_referred_mismatch() {
        // sRGB-encoded clip, Rec.709-authored LUT, no shaper — the
        // chain must `zscale` into Rec.709 before applying the LUT.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "srgb".into(),
            lut_input_space: Some("rec709_g24".into()),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            ..Default::default()
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        // Both input-side and output-side params must be present.
        // Without `transferin/primariesin/matrixin`, FFmpeg fails
        // "no path between colorspaces" on untagged sources.
        assert!(
            plan.filter_complex.contains(
                "zscale=transferin=iec61966-2-1:primariesin=bt709:matrixin=bt709:transfer=bt709:primaries=bt709:matrix=bt709"
            ),
            "expected auto-zscale with sRGB→Rec.709 in+out params, got: {}",
            plan.filter_complex,
        );
        // zscale runs before the look LUT.
        let zscale_pos = plan.filter_complex.find("zscale=").unwrap();
        let lut_pos = plan.filter_complex.find("lut3d=").unwrap();
        assert!(zscale_pos < lut_pos);
    }

    #[test]
    fn auto_pre_lut_zscale_skipped_when_spaces_match() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            lut_input_space: Some("rec709_g24".into()),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            ..Default::default()
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("zscale="),
            "matching spaces should skip zscale, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn auto_pre_lut_zscale_skipped_when_shaper_present() {
        // Agent provided its own shaper — render trusts it and
        // doesn't second-guess with an auto-zscale.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "arri_logc4".into(),
            lut_input_space: Some("rec709_g24".into()),
            shaper_lut: Some(PathBuf::from("/tmp/luts/logc4_shaper.csp")),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            ..Default::default()
        });
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("zscale="),
            "agent-provided shaper should suppress auto-zscale, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn log_clip_without_shaper_surfaces_limitation() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.clip_name = "logc-shot".into();
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "arri_logc4".into(),
            lut_input_space: Some("rec709_g24".into()),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            ..Default::default()
        });
        let limitations = log_clip_without_shaper_limitations(&[s0]);
        assert_eq!(limitations.len(), 1);
        assert_eq!(limitations[0].kind, "log_clip_without_shaper");
        assert_eq!(limitations[0].clip_id.as_deref(), Some("logc-shot"));
    }

    #[test]
    fn log_clip_with_shaper_emits_no_limitation() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "arri_logc4".into(),
            lut_input_space: Some("rec709_g24".into()),
            shaper_lut: Some(PathBuf::from("/tmp/luts/logc4.csp")),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            ..Default::default()
        });
        assert!(log_clip_without_shaper_limitations(&[s0]).is_empty());
    }

    #[test]
    fn mask_source_surfaces_render_plan_limitation() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.clip_name = "clip-with-mask".into();
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            mask_source: Some(PathBuf::from("/tmp/masks/face.png")),
            ..Default::default()
        });
        let limitations = mask_source_limitations(&[s0]);
        assert_eq!(limitations.len(), 1);
        assert_eq!(limitations[0].kind, "mask_not_implemented");
        assert_eq!(limitations[0].clip_id.as_deref(), Some("clip-with-mask"));
        assert!(limitations[0].message.contains("mask_source"));
    }

    #[test]
    fn supported_mask_source_adds_mask_input_and_no_limitation() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.clip_name = "clip-with-renderable-mask".into();
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            mask_source: Some(PathBuf::from("/tmp/masks/face.png")),
            ..Default::default()
        });

        let argv = build_timeline_argv_full(
            &[s0.clone()],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );

        assert!(
            argv.windows(2)
                .any(|w| w[0] == "-i" && w[1] == "/tmp/masks/face.png"),
            "mask source should be added as a render input: {argv:?}"
        );
        assert!(mask_source_limitations(&[s0]).is_empty());
    }

    #[test]
    fn masked_lut_emits_alphamerge_and_overlay() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            mask_source: Some(PathBuf::from("/tmp/masks/face.png")),
            ..Default::default()
        });

        let argv = build_timeline_argv_full(
            &[s0],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("format=gray,loop=-1:1"),
            "static image mask should be looped as a grayscale stream: {filter}"
        );
        assert!(
            filter.contains("alphamerge") && filter.contains("overlay=format=auto"),
            "masked LUT should alpha-merge the mask and overlay over the original: {filter}"
        );
    }

    #[test]
    fn audio_track_inputs_shift_after_mask_inputs() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            look_lut: Some(PathBuf::from("/tmp/luts/look.cube")),
            mask_source: Some(PathBuf::from("/tmp/masks/face.png")),
            ..Default::default()
        });
        let audio_tracks = vec![AudioTrackPlan {
            name: "A1".into(),
            role: "dialogue".into(),
            volume: 1.0,
            volume_automation: None,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![AudioTrackItemPlan::Clip(AudioClipPlan {
                asset_path: PathBuf::from("/tmp/dialogue.wav"),
                start_s: 0.0,
                duration_s: 4.0,
                volume: None,
                volume_automation: None,
                speed: None,
                fade_in_s: None,
                fade_out_s: None,
                audio_fx: None,
            })],
        }];

        let argv = build_timeline_argv_with_audio_tracks(
            &[s0],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("[2:a:0]atrim=0:4"),
            "audio clip input should shift past segment and mask inputs: {filter}"
        );
    }

    #[test]
    fn no_mask_source_means_no_limitation() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_pipeline = Some(ColorPipelinePlan {
            clip_input_space: "rec709_g24".into(),
            look_lut: Some(PathBuf::from("/tmp/luts/x.cube")),
            ..Default::default()
        });
        let limitations = mask_source_limitations(&[s0]);
        assert!(limitations.is_empty());
    }

    #[test]
    fn output_color_tags_match_known_delivery_spaces() {
        let case = |space: &str| {
            let mut s = seg("/tmp/a.mp4", 0.0, 2.0);
            s.color_pipeline = Some(ColorPipelinePlan {
                clip_input_space: "rec709_g24".into(),
                output_space: Some(space.into()),
                ..Default::default()
            });
            output_color_tag_argv(&[s]).join(" ")
        };
        assert!(case("rec709_g24").contains("-color_primaries bt709"));
        assert!(case("rec709_g24").contains("-color_trc bt709"));
        assert!(case("rec709_g24").contains("-color_range tv"));
        assert!(case("pq_rec2020").contains("-color_trc smpte2084"));
        assert!(case("pq_rec2020").contains("-colorspace bt2020nc"));
        assert!(case("hlg_rec2020").contains("-color_trc arib-std-b67"));
        assert!(case("srgb").contains("-color_trc iec61966-2-1"));
        assert!(case("srgb").contains("-color_range pc"));
        // Log encodings and scene_linear deliberately omit tags.
        assert_eq!(case("arri_logc4"), "");
        assert_eq!(case("scene_linear"), "");
    }

    #[test]
    fn filter_planner_emits_reframe_before_speed_and_concat() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.reframe = Some(ReframePlan {
            zoom: 1.08,
            x: 0.25,
            y: -0.5,
        });
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains(
                "[0:v:0]scale=ceil(iw*1.08/2)*2:ceil(ih*1.08/2)*2,\
                 crop=iw/1.08:ih/1.08:x=(in_w-out_w)*0.625:y=(in_h-out_h)*0.25[rv0]"
            ),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[rv0]setpts=0.5*PTS[sv0]"),
            "reframe should feed speed, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[sv0][sa0]concat"),
            "post-reframe/post-speed labels should feed concat, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_shake_before_speed_and_concat() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.shake = Some(ShakePlan {
            intensity_px: 6.0,
            frequency_hz: 10.0,
        });
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains(
                "[0:v:0]pad=iw+12:ih+12:6:6,\
                 crop=w=iw-12:h=ih-12:x=6+6*sin(2*PI*10*t):y=6+6*cos(2*PI*7.3*t)[shv0]"
            ),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[shv0]setpts=0.5*PTS[sv0]"),
            "shake should feed speed, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[sv0][sa0]concat"),
            "post-shake/post-speed labels should feed concat, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_clip_blur_before_reframe_and_speed() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.blur = Some(BlurPlan { radius_px: 4.5 });
        s0.reframe = Some(ReframePlan {
            zoom: 1.2,
            x: 0.0,
            y: 0.0,
        });
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]gblur=sigma=4.5[blv0];"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[blv0]scale=ceil(iw*1.2/2)*2"),
            "blur should feed reframe, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[rv0]setpts=0.5*PTS[sv0]"),
            "reframe should feed speed, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_warp_after_reframe_before_shake_and_speed() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.reframe = Some(ReframePlan {
            zoom: 1.2,
            x: 0.0,
            y: 0.0,
        });
        s0.warp = Some(WarpPlan {
            k1: -0.12,
            k2: 0.03,
            center_x: 0.45,
            center_y: 0.55,
        });
        s0.shake = Some(ShakePlan {
            intensity_px: 5.0,
            frequency_hz: 8.0,
        });
        s0.speed = Some(SpeedPlan::new(2.0));
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[rv0]lenscorrection=cx=0.45:cy=0.55:k1=-0.12:k2=0.03:i=bilinear[wv0];"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[wv0]pad=iw+10:ih+10:5:5"),
            "warp should feed shake, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[shv0]setpts=0.5*PTS[sv0]"),
            "shake should feed speed, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_appends_drawtext_for_title_overlay() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let title = TitlePlan {
            text: "Hello".into(),
            start_s: 0.0,
            end_s: 3.0,
            position: TitlePosition::Top,
            font_size: 64,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Normal,
            animation: TitleAnimation::None,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        };
        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();
        assert!(
            plan.filter_complex.contains("drawtext=text='Hello'"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("fontsize=64"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("fontcolor=#FFFFFF"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("y=h*0.05"),
            "filter graph (top position): {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("enable='between(t\\,0\\,3)'"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // The video output label is now [titled_v]; audio still
        // [outa].
        assert_eq!(plan.video_out_label, "[titled_v]");
        assert_eq!(plan.audio_out_label, "[outa]");
    }

    #[test]
    fn dynamic_title_keywords_resolve_from_timeline_context() {
        let mut titles = vec![TitlePlan {
            text: "#timecode# #frame# #filename# #chapter_title# #speaker#".into(),
            start_s: 5.0,
            end_s: 8.0,
            position: TitlePosition::Top,
            font_size: 64,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Normal,
            animation: TitleAnimation::None,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        }];
        let segs = vec![seg("/tmp/interview.mov", 0.0, 10.0)];
        let mut overlay = awidat_proto::awidat_meta::BroadcastOverlayConfig::default();
        overlay.chapters = vec![awidat_proto::awidat_meta::BroadcastTimedEntry {
            time_seconds: 3.0,
            text: "Deep Dive".into(),
        }];
        let mut metadata = awidat_proto::awidat_meta::AwidatTimelineMetadata {
            broadcast_overlay: Some(overlay),
            ..Default::default()
        };
        metadata
            .extra
            .insert("speaker_label".into(), serde_json::json!("HOST"));

        apply_dynamic_title_keywords(
            &mut titles,
            &segs,
            Some(&metadata),
            Path::new("/tmp/project"),
            24.0,
        );

        assert_eq!(
            titles[0].text,
            "00:00:05:00 120 interview.mov Deep Dive HOST"
        );
    }

    #[test]
    fn filter_planner_chains_multiple_titles_with_commas() {
        let s0 = seg("/tmp/a.mp4", 0.0, 10.0);
        let titles = vec![
            TitlePlan {
                text: "One".into(),
                start_s: 0.0,
                end_s: 3.0,
                position: TitlePosition::Top,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: TitleWeight::Normal,
                animation: TitleAnimation::None,
                reveal: TextReveal::None,
                role: "title".into(),
                safe_area: None,
                rich_segments: Vec::new(),
                word_timings: Vec::new(),
                animations: Vec::new(),
            },
            TitlePlan {
                text: "Two".into(),
                start_s: 5.0,
                end_s: 8.0,
                position: TitlePosition::Bottom,
                font_size: 48,
                color: "#FFAA00".into(),
                font_weight: TitleWeight::Bold,
                animation: TitleAnimation::None,
                reveal: TextReveal::None,
                role: "caption".into(),
                safe_area: Some("mobile".into()),
                rich_segments: Vec::new(),
                word_timings: Vec::new(),
                animations: Vec::new(),
            },
        ];
        let plan = FilterPlanner::with_titles(&[s0], &[], &titles).plan();
        // Both titles land in the chain.
        assert!(plan.filter_complex.contains("text='One'"));
        assert!(plan.filter_complex.contains("text='Two'"));
        // Bold position uses borderw fallback (no bold-fontfile bundle).
        assert!(plan.filter_complex.contains("borderw=2"));
        // Mobile-safe captions sit higher than generic bottom titles.
        assert!(plan.filter_complex.contains("y=h*0.75"));
    }

    #[test]
    fn long_title_text_auto_wraps_before_drawtext() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let title = TitlePlan {
            text: "This is a deliberately long title that should wrap before it runs beyond the safe title width".into(),
            start_s: 0.0,
            end_s: 3.0,
            position: TitlePosition::Bottom,
            font_size: 72,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Bold,
            animation: TitleAnimation::FadeIn,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
rich_segments: Vec::new(),
word_timings: Vec::new(),
animations: Vec::new(),
        };

        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();

        assert!(
            plan.filter_complex.contains("\\n"),
            "long title should be wrapped in the drawtext payload: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn cjk_title_text_wraps_without_spaces() {
        let text = "天地玄黄宇宙洪荒".repeat(8);

        let wrapped = auto_wrap_title_text(&text, 72);

        assert!(
            wrapped.contains('\n'),
            "CJK titles without spaces should still wrap: {wrapped}"
        );
    }

    #[test]
    fn rich_title_width_estimate_accounts_for_proportional_glyphs() {
        let narrow = approximate_text_width_px("iiiiiiii", 72);
        let wide = approximate_text_width_px("mmmmmmmm", 72);

        assert!(
            narrow < wide,
            "narrow glyph runs should not use the same width as wide glyph runs"
        );
    }

    #[test]
    fn rich_title_segments_render_as_independent_drawtext_runs() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut title = title(TitleAnimation::None, TitlePosition::Bottom);
        title.text = "Launch now".into();
        title.rich_segments = vec![
            RichTextSegment {
                text: "Launch".into(),
                color: Some("#FFFFFF".into()),
                font_weight: Some(TitleWeight::Bold),
            },
            RichTextSegment {
                text: " now".into(),
                color: Some("#FFCC00".into()),
                font_weight: Some(TitleWeight::Normal),
            },
        ];

        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();

        assert!(
            plan.filter_complex.contains("drawtext=text='Launch'")
                && plan.filter_complex.contains("drawtext=text=' now'"),
            "rich title should render each run separately: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex.contains("fontcolor=#FFCC00"),
            "rich title should preserve per-run color: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex.contains("x=(w-"),
            "rich title should use centered run offsets: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn media_overlay_is_composited_before_caption_drawtext() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/overlay.mp4", 0.0, 3.0),
            track_start_s: 1.0,
            mode: VideoOverlayMode::FullFrame,
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        }];
        let captions = vec![TitlePlan {
            text: "Caption stays visible".into(),
            start_s: 1.0,
            end_s: 4.0,
            position: TitlePosition::Bottom,
            font_size: 48,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Bold,
            animation: TitleAnimation::None,
            reveal: TextReveal::None,
            role: "caption".into(),
            safe_area: Some("mobile".into()),
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        }];

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &overlays,
            &captions,
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);
        let overlay_pos = filter
            .find("overlay=x=0:y=0:enable='between(t\\,1\\,4)'[media_overlay_v0]")
            .unwrap();
        let caption_pos = filter
            .find("drawtext=text='Caption stays visible'")
            .unwrap();

        assert!(
            overlay_pos < caption_pos,
            "caption drawtext must be appended after media overlays: {filter}"
        );
        assert!(
            filter.contains("[media_overlay_v0]drawtext="),
            "caption drawtext should consume the composited overlay output: {filter}"
        );
    }

    #[test]
    fn svg_overlay_inputs_are_looped_like_still_graphics() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/brand/logo.svg", 0.0, 4.0),
            track_start_s: 1.0,
            mode: VideoOverlayMode::PiP {
                corner: "top_right".into(),
                scale: 0.2,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        }];

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &overlays,
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let args = argv.join(" ");

        assert!(
            args.contains("-loop 1 -t 4 -i /tmp/brand/logo.svg"),
            "SVG overlays should be held for their clip duration instead of decoded as a single frame: {args}"
        );
    }

    #[test]
    fn annotation_primitives_render_after_media_before_titles() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/logo.svg", 0.0, 4.0),
            track_start_s: 1.0,
            mode: VideoOverlayMode::FullFrame,
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        }];
        let annotations = vec![AnnotationPlan {
            kind: AnnotationKind::Rectangle,
            start_s: 1.0,
            end_s: 4.0,
            x: 0.1,
            y: 0.2,
            width: 0.3,
            height: 0.25,
            color: "#FFCC00".into(),
            stroke_width: 6,
            label: None,
        }];
        let titles = vec![TitlePlan {
            text: "Caption".into(),
            start_s: 1.0,
            end_s: 4.0,
            position: TitlePosition::Bottom,
            font_size: 48,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Bold,
            animation: TitleAnimation::None,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        }];

        let argv = build_timeline_argv_full_with_annotations(
            &segs,
            &[],
            &overlays,
            &annotations,
            &titles,
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);
        let Some(media_pos) = filter.find("[media_overlay_v0]") else {
            panic!("media overlay");
        };
        let Some(annotation_pos) = filter.find("drawbox=x=iw*0.1") else {
            panic!("annotation drawbox");
        };
        let Some(title_pos) = filter.find("drawtext=text='Caption'") else {
            panic!("title drawtext");
        };

        assert!(media_pos < annotation_pos);
        assert!(annotation_pos < title_pos);
        assert!(
            filter.contains("color=#FFCC00:t=6:enable='between(t\\,1\\,4)'"),
            "annotation should carry color, stroke, and timing: {filter}"
        );
    }

    #[test]
    fn annotation_color_filter_options_are_not_injected() {
        let annotations = vec![AnnotationPlan {
            kind: AnnotationKind::Rectangle,
            start_s: 1.0,
            end_s: 4.0,
            x: 0.1,
            y: 0.1,
            width: 0.2,
            height: 0.2,
            color: "red:t=fill".into(),
            stroke_width: 6,
            label: None,
        }];
        let plan = append_annotations(
            FilterPlan {
                filter_complex: "[0:v]null[base_v]".into(),
                video_out_label: "[base_v]".into(),
                audio_out_label: "[outa]".into(),
            },
            &annotations,
        );

        assert!(
            !plan.filter_complex.contains("color=red:t=fill"),
            "annotation color must not be able to inject filter options: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn blur_annotation_lowers_to_boxblur_region_overlay() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let annotations = vec![AnnotationPlan {
            kind: AnnotationKind::Blur,
            start_s: 1.0,
            end_s: 4.0,
            x: 0.1,
            y: 0.2,
            width: 0.3,
            height: 0.25,
            color: "#FFCC00".into(),
            stroke_width: 6,
            label: None,
        }];

        let argv = build_timeline_argv_full_with_annotations(
            &segs,
            &[],
            &[],
            &annotations,
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("crop=w=iw*0.3:h=ih*0.25:x=iw*0.1:y=ih*0.2"),
            "blur annotation should crop the normalized region: {filter}"
        );
        assert!(
            filter.contains("boxblur=12"),
            "blur annotation should blur the cropped region: {filter}"
        );
        assert!(
            filter.contains("overlay=x=main_w*0.1:y=main_h*0.2:enable='between(t\\,1\\,4)'"),
            "blur annotation should overlay the blurred region back in time: {filter}"
        );
    }

    #[test]
    fn circle_arrow_and_bracket_annotations_lower_to_draw_filters() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let annotations = vec![
            AnnotationPlan {
                kind: AnnotationKind::Circle,
                start_s: 1.0,
                end_s: 4.0,
                x: 0.1,
                y: 0.2,
                width: 0.3,
                height: 0.25,
                color: "#FFCC00".into(),
                stroke_width: 6,
                label: None,
            },
            AnnotationPlan {
                kind: AnnotationKind::Arrow,
                start_s: 1.0,
                end_s: 4.0,
                x: 0.2,
                y: 0.3,
                width: 0.3,
                height: 0.1,
                color: "#FFCC00".into(),
                stroke_width: 6,
                label: None,
            },
            AnnotationPlan {
                kind: AnnotationKind::Bracket,
                start_s: 1.0,
                end_s: 4.0,
                x: 0.3,
                y: 0.2,
                width: 0.2,
                height: 0.25,
                color: "#FFCC00".into(),
                stroke_width: 6,
                label: None,
            },
        ];

        let argv = build_timeline_argv_full_with_annotations(
            &segs,
            &[],
            &[],
            &annotations,
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(filter.contains("drawtext=text='○'"));
        assert!(filter.contains("drawtext=text='→'"));
        assert!(
            filter.contains("annotation_bracket_top")
                && filter.contains("annotation_bracket_left")
                && filter.contains("annotation_bracket_right"),
            "bracket annotation should lower to three drawbox sides: {filter}"
        );
    }

    #[test]
    fn typewriter_title_reveal_emits_prefix_drawtext_windows() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.text = "Hi".into();
        title.start_s = 1.0;
        title.end_s = 3.0;
        title.reveal = TextReveal::Typewriter;

        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();

        assert!(
            plan.filter_complex.contains("drawtext=text='H'"),
            "typewriter reveal should render first prefix: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex.contains("drawtext=text='Hi'"),
            "typewriter reveal should render final prefix: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex.contains("enable='between(t\\,1\\,2)'")
                && plan.filter_complex.contains("enable='between(t\\,2\\,3)'"),
            "typewriter reveal should split title window across prefixes: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn word_timed_caption_reveal_uses_transcript_timing_windows() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut title = title(TitleAnimation::None, TitlePosition::Bottom);
        title.role = "caption".into();
        title.text = "This changed everything".into();
        title.start_s = 1.0;
        title.end_s = 2.5;
        title.word_timings = vec![
            CaptionWordTiming {
                text: "This".into(),
                start_s: 1.0,
                end_s: 1.2,
            },
            CaptionWordTiming {
                text: "changed".into(),
                start_s: 1.24,
                end_s: 1.5,
            },
            CaptionWordTiming {
                text: "everything".into(),
                start_s: 1.55,
                end_s: 1.9,
            },
        ];

        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();

        assert!(
            plan.filter_complex.contains("drawtext=text='This'")
                && plan.filter_complex.contains("drawtext=text='This changed'")
                && plan
                    .filter_complex
                    .contains("drawtext=text='This changed everything'"),
            "word-timed captions should render cumulative word prefixes: {}",
            plan.filter_complex
        );
        assert!(
            plan.filter_complex
                .contains("enable='between(t\\,1\\,1.24)'")
                && plan
                    .filter_complex
                    .contains("enable='between(t\\,1.24\\,1.55)'")
                && plan
                    .filter_complex
                    .contains("enable='between(t\\,1.55\\,2.5)'"),
            "word-timed captions should use transcript word starts, not uniform steps: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn reveal_steps_keep_grapheme_clusters_intact() {
        assert_eq!(
            reveal_steps("👩\u{200d}💻", TextReveal::Typewriter),
            vec!["👩\u{200d}💻".to_string()]
        );
        assert_eq!(
            reveal_steps("👩\u{200d}💻 dev", TextReveal::Word),
            vec!["👩\u{200d}💻".to_string(), "👩\u{200d}💻 dev".to_string()]
        );
    }

    #[test]
    fn long_form_broadcast_overlay_suppresses_generic_titles() {
        let s0 = seg("/tmp/a.mp4", 0.0, 10.0);
        let title = TitlePlan {
            text: "Chapter".into(),
            start_s: 1.0,
            end_s: 4.0,
            position: TitlePosition::Bottom,
            font_size: 48,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Bold,
            animation: TitleAnimation::None,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        };
        let overlay = BroadcastOverlayPlan {
            config: BroadcastOverlayConfig {
                enabled: true,
                show_name: "SHOW".into(),
                sponsors: vec!["Sponsor".into()],
                ..BroadcastOverlayConfig::default()
            },
            project_root: PathBuf::from("/tmp"),
        };
        let plan =
            FilterPlanner::with_titles_and_broadcast_overlay(&[s0], &[], &[title], Some(&overlay))
                .plan();
        assert!(
            !plan.filter_complex.contains("text='Chapter'"),
            "filter graph: {}",
            plan.filter_complex
        );
    }

    #[test]
    fn filter_planner_appends_broadcast_overlay_filters() {
        let s0 = seg("/tmp/a.mp4", 0.0, 120.0);
        let mut config = BroadcastOverlayConfig {
            episode_title: "Ben Adams".into(),
            episode_subtitle: "Drone Hardware Entrepreneur".into(),
            show_name: "TECHNOLOGIA TALKS".into(),
            sponsors: vec!["LEARN-X".into(), "Throwly".into()],
            ..BroadcastOverlayConfig::default()
        };
        config.host_a.name = "Tadiwa Mbuwayesango".into();
        config.host_a.title = "Co-Host".into();
        config.host_a.photo_path = Some("branding/tadiwa.jpg".into());
        config.host_b.name = "Elvis Kimara".into();
        config.host_b.title = "Co-Host".into();
        config.host_b.photo_path = Some("branding/elvis.jpg".into());
        config
            .topics
            .push(awidat_proto::awidat_meta::BroadcastTimedEntry {
                time_seconds: 45.0,
                text: "Custom drones".into(),
            });
        config
            .chapters
            .push(awidat_proto::awidat_meta::BroadcastTimedEntry {
                time_seconds: 60.0,
                text: "Hardware barriers".into(),
            });
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("branding")).unwrap();
        fs::write(dir.path().join("branding/tadiwa.jpg"), b"stub").unwrap();
        fs::write(dir.path().join("branding/elvis.jpg"), b"stub").unwrap();
        let overlay = BroadcastOverlayPlan {
            config,
            project_root: dir.path().to_path_buf(),
        };
        let plan =
            FilterPlanner::with_titles_and_broadcast_overlay(&[s0], &[], &[], Some(&overlay))
                .plan();
        assert_eq!(plan.video_out_label, "[broadcast_v]");
        assert!(
            plan.filter_complex.contains("drawbox=x=0:y=ih-100"),
            "expected ticker bar, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("BEN ADAMS"),
            "expected title card text, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("NOW DISCUSSING"),
            "expected topic badge, got: {}",
            plan.filter_complex,
        );
        let sponsor_pos = plan.filter_complex.find("LEARN-X").unwrap();
        assert!(
            plan.filter_complex.contains("◆"),
            "expected diamond separator drawtext, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("Throwly"),
            "expected second sponsor drawtext, got: {}",
            plan.filter_complex,
        );
        let label_pos = plan.filter_complex.rfind("w=340:h=100:color=").unwrap();
        assert!(
            label_pos > sponsor_pos,
            "expected branded ticker label to draw after sponsor text so it masks the left lane, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("HARDWARE BARRIERS"),
            "expected chapter card, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("movie='"),
            "expected project-relative host photos to become movie filters, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("overlay=x=30:y=main_h-145"),
            "expected left host photo overlay, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn timeline_argv_composites_pip_overlay_after_base_concat() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/pip.mp4", 1.0, 2.0),
            track_start_s: 1.5,
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".into(),
                scale: 0.28,
                margin_pct: 0.035,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        }];
        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &overlays,
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains("concat=n=1:v=1:a=1[outv][outa]"));
        assert!(filter.contains("[1:v:0]setpts=PTS-STARTPTS+1.5/TB"));
        assert!(filter.contains("scale2ref=w=main_w*0.28:h=-2"));
        assert!(filter.contains("overlay=x=main_w-overlay_w-main_w*0.035:y=main_h-overlay_h-main_h*0.035:enable='between(t\\,1.5\\,3.5)'"));
    }

    #[test]
    fn overlay_parameter_animation_affects_position_and_scale() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![
                RenderParameterAnimation {
                    parameter: "overlay.x".to_string(),
                    keyframes: vec![
                        awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                        awidat_proto::professional::Keyframe::linear(1.0, -0.1),
                    ],
                    pre_extrapolation: ExtrapolationMode::Hold,
                    post_extrapolation: ExtrapolationMode::Hold,
                    motion_path: None,
                },
                RenderParameterAnimation {
                    parameter: "overlay.scale".to_string(),
                    keyframes: vec![
                        awidat_proto::professional::Keyframe::linear(0.0, 1.0),
                        awidat_proto::professional::Keyframe::linear(1.0, 1.2),
                    ],
                    pre_extrapolation: ExtrapolationMode::Hold,
                    post_extrapolation: ExtrapolationMode::Hold,
                    motion_path: None,
                },
            ],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("main_w*("),
            "overlay x should include normalized motion: {filter}"
        );
        assert!(
            filter.contains("w=main_w*0.3*("),
            "overlay scale should include multiplier expression: {filter}"
        );
        assert!(
            filter.contains("scale2ref=w=main_w*0.3*(") && filter.contains(":eval=frame"),
            "animated overlay scale should be evaluated per frame: {filter}"
        );
    }

    #[test]
    fn overlay_blur_animation_uses_keyframed_radius_map() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.5,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.blur".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 12.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("varblur=min_r=0:max_r=24"),
            "overlay blur should use a radius-map blur, not blend a fixed blurred stream: {filter}"
        );
        assert!(
            filter.contains("geq=lum='min(max(("),
            "overlay blur should drive the radius map from keyframes: {filter}"
        );
        assert!(
            !filter.contains("blend=all_expr="),
            "overlay blur should not approximate radius by blending sharp and fully blurred streams: {filter}"
        );
        assert!(
            filter.contains("(T-0.5)"),
            "overlay blur should evaluate keyframes in overlay-local time: {filter}"
        );
    }

    #[test]
    fn oversized_overlay_blur_animation_surfaces_clamp_limitation() {
        let animations = vec![awidat_proto::professional::ParameterAnimation {
            id: "big-blur".into(),
            target: awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "overlay-a".into(),
                parameter: "overlay.blur".into(),
            },
            keyframes: vec![
                awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                awidat_proto::professional::Keyframe::linear(1.0, 32.0),
            ],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        }];

        let selection =
            render_animations_for_clip_with_limitations(&animations, "overlay-a", "overlay");

        assert_eq!(selection.animations.len(), 1);
        assert!(
            selection.limitations.iter().any(|limitation| {
                limitation.kind == "overlay_blur_radius_clamped"
                    && limitation.parameter.as_deref() == Some("overlay.blur")
                    && limitation.message.contains("32")
            }),
            "oversized blur should surface a clamp limitation: {:?}",
            selection.limitations
        );
    }

    #[test]
    fn overlay_motion_blur_samples_animated_position() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: Some(OverlayMotionBlurPlan { shutter_s: 0.02 }),
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.x".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, -0.2),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("split=3[media_overlay_blur0_src0][media_overlay_blur0_src1][media_overlay_blur0_src2]"),
            "motion blur should render temporal transform samples from one overlay source: {filter}"
        );
        assert!(
            filter.contains("t-0.01") && filter.contains("t+0.01"),
            "motion blur should sample the animated transform around the current frame: {filter}"
        );
        assert!(
            filter.contains("alpha(X,Y)*0.333333"),
            "motion blur samples should be weighted instead of requiring authored opacity keyframes: {filter}"
        );
    }

    #[test]
    fn overlay_motion_blur_samples_animated_rotation() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: Some(OverlayMotionBlurPlan { shutter_s: 0.02 }),
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.rotation_deg".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 45.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("split=3[media_overlay_blur0_src0][media_overlay_blur0_src1][media_overlay_blur0_src2]"),
            "motion blur should render temporal transform samples from one overlay source: {filter}"
        );
        assert!(
            filter.matches("rotate='(PI/180)*(").count() >= 3,
            "motion blur should rotate each temporal transform sample independently: {filter}"
        );
        assert!(
            filter.contains("t-0.01") && filter.contains("t+0.01"),
            "motion blur should sample animated rotation around the current frame: {filter}"
        );
    }

    #[test]
    fn overlay_motion_blur_samples_animated_scale() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: Some(OverlayMotionBlurPlan { shutter_s: 0.02 }),
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.scale".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 1.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 1.5),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.matches("scale2ref=w=main_w*0.3*(").count() >= 3,
            "motion blur should scale each temporal transform sample independently: {filter}"
        );
        assert!(
            filter.contains("t-0.01") && filter.contains("t+0.01"),
            "motion blur should sample animated scale around the current frame: {filter}"
        );
    }

    #[test]
    fn overlay_rotation_animation_inserts_per_frame_rotate_stage() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.rotation_deg".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 15.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("rotate='(PI/180)*(") && filter.contains(":ow='hypot(iw\\,ih)'"),
            "overlay rotation should use a degree animation expression in a rotate filter: {filter}"
        );
        assert!(
            filter.contains("[media_overlay_rot0]"),
            "rotated overlay label should feed the overlay stage: {filter}"
        );
    }

    #[test]
    fn overlay_static_rotation_inserts_rotate_stage() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.0,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: -30.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("rotate='(PI/180)*(-30)'"),
            "static overlay rotation should be expressed in degrees: {filter}"
        );
    }

    #[test]
    fn overlay_opacity_animation_uses_time_aware_alpha_filter() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 2.0)];
        let overlay = VideoOverlayPlan {
            track_start_s: 0.5,
            segment: seg("/tmp/overlay.mp4", 0.0, 2.0),
            mode: VideoOverlayMode::PiP {
                corner: "bottom_right".to_string(),
                scale: 0.3,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![RenderParameterAnimation {
                parameter: "overlay.opacity".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 1.0),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            }],
        };

        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &[overlay],
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("geq="),
            "overlay opacity should use a filter that supports time-varying expressions: {filter}"
        );
        assert!(
            filter.contains("(T-0.5)"),
            "overlay opacity should evaluate against overlay-local time with FFmpeg's geq T variable: {filter}"
        );
        assert!(
            !filter.contains("colorchannelmixer=aa=if("),
            "overlay opacity should not put time expressions in colorchannelmixer aa: {filter}"
        );
    }

    #[test]
    fn ffmpeg_smoke_renders_animated_overlay_opacity_and_scale() {
        let Ok(ffmpeg) = crate::ffmpeg::ffmpeg_path() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("base.mp4");
        let overlay_path = dir.path().join("overlay.mp4");
        let output_path = dir.path().join("out.mp4");
        write_synthetic_video(&ffmpeg, &base_path, "blue");
        write_synthetic_video(&ffmpeg, &overlay_path, "red");

        let segs = vec![seg(&base_path.to_string_lossy(), 0.0, 1.0)];
        let overlay = VideoOverlayPlan {
            segment: seg(&overlay_path.to_string_lossy(), 0.0, 1.0),
            track_start_s: 0.0,
            mode: VideoOverlayMode::PiP {
                corner: "top_right".into(),
                scale: 0.35,
                margin_pct: 0.05,
            },
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: vec![
                RenderParameterAnimation {
                    parameter: "overlay.opacity".into(),
                    keyframes: vec![
                        awidat_proto::professional::Keyframe::linear(0.0, 0.2),
                        awidat_proto::professional::Keyframe::linear(1.0, 0.8),
                    ],
                    pre_extrapolation: ExtrapolationMode::Hold,
                    post_extrapolation: ExtrapolationMode::Hold,
                    motion_path: None,
                },
                RenderParameterAnimation {
                    parameter: "overlay.scale".into(),
                    keyframes: vec![
                        awidat_proto::professional::Keyframe::linear(0.0, 0.25),
                        awidat_proto::professional::Keyframe::linear(1.0, 0.45),
                    ],
                    pre_extrapolation: ExtrapolationMode::Hold,
                    post_extrapolation: ExtrapolationMode::Hold,
                    motion_path: None,
                },
            ],
        };
        let argv =
            build_timeline_argv_full(&segs, &[], &[overlay], &[], None, None, None, &output_path);

        let output = std::process::Command::new(ffmpeg)
            .args(&argv)
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "ffmpeg smoke failed\nargv: {}\nstderr:\n{}",
            argv.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output_path.exists());
    }

    #[test]
    fn ffmpeg_smoke_renders_annotation_and_rich_title() {
        let Ok(ffmpeg) = crate::ffmpeg::ffmpeg_path() else {
            return;
        };
        // Some CI ffmpeg builds (e.g. Homebrew ffmpeg 8.1 on macOS at
        // time of writing) ship without `drawtext` because libfreetype
        // wasn't linked. Skip rather than fail the suite on those hosts.
        let probe = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-filters"])
            .output();
        let has_drawtext = probe
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("drawtext"))
            .unwrap_or(false);
        if !has_drawtext {
            eprintln!(
                "skipping ffmpeg_smoke_renders_annotation_and_rich_title: drawtext filter unavailable"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("base.mp4");
        let output_path = dir.path().join("annotation-rich-title.mp4");
        write_synthetic_video(&ffmpeg, &base_path, "blue");

        let segs = vec![seg(&base_path.to_string_lossy(), 0.0, 1.0)];
        let annotations = vec![AnnotationPlan {
            kind: AnnotationKind::Rectangle,
            start_s: 0.0,
            end_s: 1.0,
            x: 0.1,
            y: 0.1,
            width: 0.4,
            height: 0.35,
            color: "#FFCC00".into(),
            stroke_width: 4,
            label: None,
        }];
        let mut title = title(TitleAnimation::None, TitlePosition::Bottom);
        title.start_s = 0.0;
        title.end_s = 1.0;
        title.text = "Launch now".into();
        title.rich_segments = vec![
            RichTextSegment {
                text: "Launch".into(),
                color: Some("#FFFFFF".into()),
                font_weight: Some(TitleWeight::Bold),
            },
            RichTextSegment {
                text: " now".into(),
                color: Some("#FFCC00".into()),
                font_weight: Some(TitleWeight::Normal),
            },
        ];

        let argv = build_timeline_argv_full_with_annotations(
            &segs,
            &[],
            &[],
            &annotations,
            &[title],
            None,
            None,
            None,
            &output_path,
        );
        let output = std::process::Command::new(ffmpeg)
            .args(&argv)
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "ffmpeg smoke failed\nargv: {}\nstderr:\n{}",
            argv.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output_path.exists());
    }

    fn write_synthetic_video(ffmpeg: &Path, path: &Path, color: &str) {
        let output = std::process::Command::new(ffmpeg)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("color=c={color}:s=160x120:d=1:r=24"),
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=48000:cl=stereo",
                "-shortest",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                &path.to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "failed to synthesize {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn render_animations_for_clip_selects_supported_title_animation() {
        let animations = vec![awidat_proto::professional::ParameterAnimation {
            id: "anim-title-opacity".to_string(),
            target: awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "title-1".to_string(),
                parameter: "title.opacity".to_string(),
            },
            keyframes: vec![awidat_proto::professional::Keyframe::linear(0.0, 0.0)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        }];

        let selected = render_animations_for_clip(&animations, "title-1", "title");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].parameter, "title.opacity");
    }

    #[test]
    fn render_animations_for_clip_rejects_overlay_animation_for_title() {
        let animations = vec![awidat_proto::professional::ParameterAnimation {
            id: "anim-overlay-x".to_string(),
            target: awidat_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "title-1".to_string(),
                parameter: "overlay.x".to_string(),
            },
            keyframes: vec![awidat_proto::professional::Keyframe::linear(0.0, 0.0)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        }];

        let selected = render_animations_for_clip(&animations, "title-1", "title");

        assert!(selected.is_empty());
    }

    #[test]
    fn timeline_argv_composites_full_frame_overlay_without_base_concat_append() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/cover.mp4", 0.0, 2.0),
            track_start_s: 1.0,
            mode: VideoOverlayMode::FullFrame,
            rotation_deg: 0.0,
            motion_blur: None,
            corner_pin_filter: None,
            matte_source: None,
            matte_input_index: None,
            mask: None,
            animations: Vec::new(),
        }];
        let argv = build_timeline_argv_full(
            &segs,
            &[],
            &overlays,
            &[],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains("concat=n=1:v=1:a=1[outv][outa]"));
        assert!(!filter.contains("concat=n=2"));
        assert!(filter.contains("scale2ref=w=main_w:h=main_h"));
        assert!(filter.contains("overlay=x=0:y=0:enable='between(t\\,1\\,3)'"));
    }

    #[test]
    fn filter_planner_short_form_broadcast_overlay_suppresses_long_form_layers() {
        let s0 = seg("/tmp/a.mp4", 0.0, 30.0);
        let config = BroadcastOverlayConfig {
            short_form_mode: true,
            episode_title: "Long Episode Title".into(),
            show_name: "TECHNOLOGIA TALKS".into(),
            sponsors: vec!["LEARN-X".into()],
            ..BroadcastOverlayConfig::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let overlay = BroadcastOverlayPlan {
            config,
            project_root: dir.path().to_path_buf(),
        };
        let plan =
            FilterPlanner::with_titles_and_broadcast_overlay(&[s0], &[], &[], Some(&overlay))
                .plan();
        assert_eq!(plan.video_out_label, "[broadcast_v]");
        assert!(
            plan.filter_complex.contains("TECHNOLOGIA TALKS"),
            "expected short-form show label, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("NOW DISCUSSING"),
            "short-form mode should suppress long-form ticker/topic layers: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("EPISODE"),
            "short-form mode should suppress title card layers: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn drawtext_escape_handles_special_chars() {
        let s = drawtext_escape("text: with 'quote', backslash\\");
        assert!(s.contains("\\:"));
        assert!(s.contains("\\'"));
        assert!(s.contains("\\\\"));
        assert!(s.contains("\\,"));
    }

    #[test]
    fn title_parameter_animation_overrides_legacy_opacity() {
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.animations = vec![RenderParameterAnimation {
            parameter: "title.opacity".to_string(),
            keyframes: vec![
                awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                awidat_proto::professional::Keyframe::linear(1.0, 1.0),
            ],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
        }];

        let argv = build_timeline_argv_full(
            &[],
            &[],
            &[],
            &[title],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("alpha="),
            "filter should include animated alpha: {filter}"
        );
        assert!(
            filter.contains("alpha='if(lt((t-1)"),
            "animated alpha should evaluate title keyframes in local time: {filter}"
        );
    }

    #[test]
    fn title_parameter_animation_offsets_position() {
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.animations = vec![
            RenderParameterAnimation {
                parameter: "title.x".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, -0.1),
                    awidat_proto::professional::Keyframe::linear(1.0, 0.1),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            },
            RenderParameterAnimation {
                parameter: "title.y".to_string(),
                keyframes: vec![
                    awidat_proto::professional::Keyframe::linear(0.0, 0.0),
                    awidat_proto::professional::Keyframe::linear(1.0, 0.2),
                ],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
            },
        ];

        let argv = build_timeline_argv_full(
            &[],
            &[],
            &[],
            &[title],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("x=((w-text_w)/2)+w*(if(lt((t-1)"),
            "title.x animation should offset the resting x expression: {filter}"
        );
        assert!(
            filter.contains("y=((h-text_h)/2)+h*(if(lt((t-1)"),
            "title.y animation should offset the resting y expression: {filter}"
        );
    }

    #[test]
    fn title_font_size_animation_modulates_drawtext_size() {
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.animations = vec![RenderParameterAnimation {
            parameter: "title.font_size".to_string(),
            keyframes: vec![
                awidat_proto::professional::Keyframe::linear(0.0, 48.0),
                awidat_proto::professional::Keyframe::linear(1.0, 72.0),
            ],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
        }];

        let argv = build_timeline_argv_full(
            &[],
            &[],
            &[],
            &[title],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("fontsize=if(lt((t-1)"),
            "title.font_size animation should drive drawtext fontsize: {filter}"
        );
        assert!(
            filter.contains("48+(72-48)"),
            "font size animation should preserve keyframe values: {filter}"
        );
    }

    #[test]
    fn title_motion_path_offsets_position() {
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.animations = vec![RenderParameterAnimation {
            parameter: "title.position".to_string(),
            keyframes: Vec::new(),
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: Some(awidat_proto::professional::MotionPath {
                points: vec![
                    awidat_proto::professional::MotionPathPoint {
                        time_s: 0.0,
                        x: -0.2,
                        y: 0.0,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                    awidat_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 0.2,
                        y: 0.4,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                ],
            }),
        }];

        let argv = build_timeline_argv_full(
            &[],
            &[],
            &[],
            &[title],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("x=((w-text_w)/2)+w*(if(lt((t-1)"),
            "title.position path should drive x offset: {filter}"
        );
        assert!(
            filter.contains("y=((h-text_h)/2)+h*(if(lt((t-1)"),
            "title.position path should drive y offset: {filter}"
        );
    }

    #[test]
    fn title_motion_path_uses_keyframe_progress_curve() {
        let mut title = title(TitleAnimation::None, TitlePosition::Center);
        title.animations = vec![RenderParameterAnimation {
            parameter: "title.position".to_string(),
            keyframes: vec![
                Keyframe {
                    time_s: 0.0,
                    value: 0.0,
                    interpolation: KeyframeInterpolation::Bezier,
                    easing: Easing::Linear,
                    bezier: Some(BezierHandles {
                        out_x: 0.25,
                        out_y: 0.9,
                        in_x: 0.75,
                        in_y: 0.1,
                    }),
                    tangent_mode: Default::default(),
                    spring: None,
                },
                Keyframe::linear(1.0, 1.0),
            ],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: Some(awidat_proto::professional::MotionPath {
                points: vec![
                    awidat_proto::professional::MotionPathPoint {
                        time_s: 0.0,
                        x: -0.2,
                        y: 0.0,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                    awidat_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 0.2,
                        y: 0.4,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                ],
            }),
        }];

        let argv = build_timeline_argv_full(
            &[],
            &[],
            &[],
            &[title],
            None,
            None,
            None,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv.join(" ");

        assert!(
            filter.contains("min(max("),
            "motion path should clamp keyframed progress before sampling path: {filter}"
        );
        assert!(
            filter.contains("0.03125") && filter.contains("0.96875"),
            "bezier progress should be sampled into the motion-path expression: {filter}"
        );
    }

    fn title(animation: TitleAnimation, position: TitlePosition) -> TitlePlan {
        TitlePlan {
            text: "Hi".into(),
            start_s: 1.0,
            end_s: 4.0,
            position,
            font_size: 48,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Normal,
            animation,
            reveal: TextReveal::None,
            role: "title".into(),
            safe_area: None,
            rich_segments: Vec::new(),
            word_timings: Vec::new(),
            animations: Vec::new(),
        }
    }

    #[test]
    fn fade_in_emits_alpha_ramp_at_start() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeIn, TitlePosition::Center)],
        )
        .plan();
        assert!(
            plan.filter_complex.contains("alpha='if(lt(t\\,1.5)"),
            "expected fade-in ramp ending at start+0.5=1.5, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn fade_out_emits_alpha_ramp_at_end() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeOut, TitlePosition::Center)],
        )
        .plan();
        // Fade-out plateau ends at end-0.5 = 3.5.
        assert!(
            plan.filter_complex.contains("alpha='if(lt(t\\,3.5)"),
            "expected fade-out plateau ending at 3.5, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn fade_in_out_emits_two_piece_alpha_expression() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeInOut, TitlePosition::Center)],
        )
        .plan();
        // Both ramps appear (1.5 = fade-in end, 3.5 = fade-out start).
        assert!(plan.filter_complex.contains("if(lt(t\\,1.5)"));
        assert!(plan.filter_complex.contains("if(lt(t\\,3.5)"));
    }

    #[test]
    fn slide_in_for_top_position_animates_y_from_above() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideIn, TitlePosition::Top)],
        )
        .plan();
        // Slide-in on top starts off-screen at y=-text_h.
        assert!(
            plan.filter_complex.contains("-text_h"),
            "expected slide-in y to start at -text_h, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn slide_in_for_center_position_animates_x_from_right() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideIn, TitlePosition::Center)],
        )
        .plan();
        // Slide-in on center starts off-screen at x=w (right edge).
        // Confirm an x= expression that mentions `w` (off-screen) and
        // the ramp end time (start_s + ramp = 1.5).
        assert!(
            plan.filter_complex.contains("x=if(lt(t\\,1.5)"),
            "expected slide-in x ramp on center title, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn slide_out_for_bottom_position_animates_y_to_below() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideOut, TitlePosition::Bottom)],
        )
        .plan();
        // Slide-out on bottom starts the ramp at end-0.5 = 3.5.
        assert!(
            plan.filter_complex.contains("if(lt(t\\,3.5)"),
            "expected slide-out ramp starting at 3.5, got: {}",
            plan.filter_complex,
        );
        // Off-screen target for bottom is y=h.
        assert!(
            plan.filter_complex.contains("h-(h*0.85)")
                || plan
                    .filter_complex
                    .contains("(h-({y_rest}))".replace("{y_rest}", "h*0.85").as_str())
                || plan.filter_complex.matches("h*0.85").count() >= 2
        );
    }

    #[test]
    fn build_timeline_argv_unchanged_after_extraction() {
        // The argv produced for a multi-segment fixture should retain
        // the paired concat path while smoothing hard-cut audio joins.
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 1.0, 3.0)];
        let argv = build_timeline_argv(&segs, Path::new("/tmp/out.mp4"));
        let cmd = argv.join(" ");
        // Two -ss / -t / -i triples preceded by `-y -loglevel info`.
        assert!(
            cmd.starts_with("-y -loglevel info -ss 0 -t 2 -i /tmp/a.mp4 -ss 1 -t 3 -i /tmp/b.mp4")
        );
        assert!(cmd.contains(
            "-filter_complex [0:a:0]afade=t=out:st=1.97:d=0.03[bfade0];\
             [1:a:0]afade=t=in:st=0:d=0.03[bfade1];\
             [0:v:0][bfade0][1:v:0][bfade1]concat=n=2:v=1:a=1[outv][outa] \
             -map [outv] -map [outa]",
        ));
        assert!(cmd.ends_with("/tmp/out.mp4"));
    }

    #[test]
    fn default_hard_cut_audio_fades_cover_every_segment_join() {
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 2.0),
            seg("/tmp/b.mp4", 0.0, 2.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];

        let argv = build_timeline_argv(&segs, Path::new("/tmp/out.mp4"));
        let filter = filter_complex_from_argv(&argv);

        assert!(
            filter.contains("[0:a:0]afade=t=out:st=1.97:d=0.03[bfade0]"),
            "first segment should fade out at the hard cut: {filter}"
        );
        assert!(
            filter.contains("[1:a:0]afade=t=in:st=0:d=0.03,afade=t=out:st=1.97:d=0.03[bfade1]"),
            "middle segment should fade both in and out: {filter}"
        );
        assert!(
            filter.contains("[2:a:0]afade=t=in:st=0:d=0.03[bfade2]"),
            "last segment should fade in at the hard cut: {filter}"
        );
        assert!(
            filter.contains("[0:v:0][bfade0][1:v:0][bfade1][2:v:0][bfade2]concat=n=3:v=1:a=1"),
            "concat should consume the anti-pop fade labels: {filter}"
        );
    }
}
