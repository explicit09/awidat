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

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use awidat_proto::awidat_meta::{BroadcastHost, BroadcastOverlayConfig, BroadcastOverlayStyle};
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::{files, read_otio_timeline};
use awidat_proto::transitions::{self, TransitionComposition};
use chrono::Utc;
use serde::Deserialize;
use thiserror::Error;

use crate::animation::{
    MotionPathCoordinate, is_phase_3a_parameter, keyframes_to_ffmpeg_expr_with_extrapolation,
    motion_path_to_ffmpeg_expr,
};
use crate::job::{RenderJobSpec, RenderPlanLimitation};
use crate::output_safety::{OutputPathPolicy, validate_render_output_path};

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
    /// Linear gain multiplier for this segment's audio. `None` means
    /// no `awidat.volume` effect is on the underlying clip — the
    /// FilterPlanner skips emitting a `volume=` filter and the audio
    /// passes through unchanged. `Some(1.0)` is unity (functionally
    /// identical to `None` but the planner still emits the filter
    /// for explicitness).
    pub volume: Option<f64>,
    /// Playback rate multiplier. `None` means no `awidat.speed`
    /// effect — the segment plays at 1×. The segment's contribution
    /// to the master timeline duration is `duration_s / factor` when
    /// `factor` is `Some`.
    pub speed: Option<f64>,
    /// Optional clip-level color correction controls, read from the
    /// `awidat.color_correction` effect.
    pub color_correction: Option<ColorCorrectionPlan>,
    /// Optional HDR transfer metadata, read from `awidat.source_color`.
    pub hdr_transfer: Option<HdrTransfer>,
    /// Optional clip-level reframe / punch-in controls, read from the
    /// `awidat.reframe` effect.
    pub reframe: Option<ReframePlan>,
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
    /// Incoming audio lead for J-cut export, in seconds.
    pub audio_lead_s: Option<f64>,
    /// Outgoing audio trail for L-cut export, in seconds.
    pub audio_trail_s: Option<f64>,
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
    /// grayscale image, or video). Reserved schema slot — render
    /// does not apply masking yet; presence surfaces a
    /// `mask_not_implemented` [`RenderPlanLimitation`] so the agent
    /// sees the field is being ignored.
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
    /// Supported Phase 3A parameter animations attached to this overlay.
    pub animations: Vec<RenderParameterAnimation>,
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
    /// Optional playback speed multiplier.
    pub speed: Option<f64>,
    /// Optional fade-in duration in seconds.
    pub fade_in_s: Option<f64>,
    /// Optional fade-out duration in seconds.
    pub fade_out_s: Option<f64>,
    /// Optional FFmpeg-native audio FX chain.
    pub audio_fx: Option<AudioFxPlan>,
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
    // We don't require the mask to exist on disk yet — render
    // doesn't read it, and failing here would block a forward-
    // compatible workflow where the agent stages the mask later.
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
    Option<BroadcastOverlayPlan>,
    Vec<AudioTrackPlan>,
    Option<LoudnessTargetPlan>,
    Vec<RenderPlanLimitation>,
);

/// Walk `<project_root>/project.otio.json` and collect every
/// video-track clip's `(asset, source_range)` in playback order.
/// Skips Gap, Transition, and nested Stack children for v1.
///
/// Wraps [`collect_timeline_plan`] and drops the transitions +
/// titles — preserved for callers that don't need either.
pub fn collect_timeline_segments(
    project_root: &Path,
) -> Result<Vec<TimelineSegment>, RenderTimelineError> {
    let (segs, _, _, _, _, _, _, _) = collect_timeline_full_plan(project_root)?;
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
    let (segs, transitions, _, _, _, _, _, _) = collect_timeline_full_plan(project_root)?;
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
    let parameter_animations: &[awidat_proto::professional::ParameterAnimation] = timeline
        .metadata
        .awidat
        .as_ref()
        .map(|metadata| metadata.parameter_animations.as_slice())
        .unwrap_or(&[]);

    let mut segs = Vec::new();
    let mut transitions = Vec::new();
    let mut video_overlays = Vec::new();
    let mut titles = Vec::new();
    let mut audio_tracks = Vec::new();
    let mut render_limitations = Vec::new();
    let mut consumed_animation_ids = BTreeSet::new();
    let mut saw_base_video_track = false;
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if matches!(track.kind, TrackKind::Audio) {
            let audio_selection =
                collect_audio_track_plan(project_root, track, parameter_animations)?;
            consumed_animation_ids.extend(audio_selection.consumed_animation_ids);
            audio_tracks.push(audio_selection.track);
            continue;
        }
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        if is_titles_track(track) {
            // Walk titles separately; don't try to read media off it.
            for tc in &track.children {
                let TrackChild::Clip(clip) = tc else { continue };
                let Some((plan, animation_selection)) =
                    parse_title_plan(clip, parameter_animations)
                else {
                    continue;
                };
                render_limitations.extend(animation_selection.limitations);
                consumed_animation_ids.extend(animation_selection.consumed_animation_ids);
                titles.push(plan);
            }
            continue;
        }
        if saw_base_video_track {
            let mut track_cursor_s = 0.0_f64;
            for tc in &track.children {
                match tc {
                    TrackChild::Clip(clip) => {
                        let Some(segment) = collect_timeline_segment(project_root, clip)? else {
                            continue;
                        };
                        let mode = read_video_overlay_mode(clip);
                        let clip_id = render_clip_id(clip);
                        let animation_selection = render_animations_for_clip_with_limitations(
                            parameter_animations,
                            &clip_id,
                            "overlay",
                        );
                        render_limitations.extend(animation_selection.limitations);
                        consumed_animation_ids.extend(animation_selection.consumed_animation_ids);
                        video_overlays.push(VideoOverlayPlan {
                            segment,
                            track_start_s: track_cursor_s,
                            mode,
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
                    let Some(segment) = collect_timeline_segment(project_root, clip)? else {
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
                TrackChild::Gap(_) | TrackChild::Stack(_) => {
                    pending_transition = None;
                }
            }
        }
        if let Some((kind, _, _, _)) = pending_transition {
            return Err(RenderTimelineError::InvalidTransitionPlacement {
                message: format!("transition {kind:?} has no incoming clip"),
            });
        }
    }
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
        parameter_animations,
        &consumed_animation_ids,
    ));
    render_limitations.extend(mask_source_limitations(&segs));
    render_limitations.extend(log_clip_without_shaper_limitations(&segs));
    Ok((
        segs,
        transitions,
        video_overlays,
        titles,
        broadcast_overlay,
        audio_tracks,
        loudness_target,
        render_limitations,
    ))
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

fn collect_timeline_segment(
    project_root: &Path,
    clip: &awidat_proto::otio::Clip,
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
        volume: read_effect_number(clip, "awidat.volume", "value"),
        speed: read_effect_number(clip, "awidat.speed", "factor"),
        color_correction: read_color_correction(clip),
        hdr_transfer: read_hdr_transfer(clip),
        reframe: read_reframe(clip),
        lut_path,
        lut_interpolation,
        lut_strength,
        color_pipeline,
        audio_fx: read_clip_audio_fx(clip),
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
                items.push(AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path,
                    start_s: range.start_time.to_seconds(),
                    duration_s: range.duration.to_seconds(),
                    volume: read_effect_number(clip, "awidat.volume", "value"),
                    speed: read_effect_number(clip, "awidat.speed", "factor"),
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
        consumed_animation_ids: automation_selection.consumed_animation_ids,
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
        animations,
    };
    Some((plan, animation_selection))
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

/// Surface a `mask_not_implemented` limitation for every segment
/// whose `color_pipeline.mask_source` is set. The mask itself is
/// stamped on the OTIO and validated at apply time, but render's
/// `color_pipeline_filter_block` does not honor it yet — the agent
/// needs to know its regional grading request was a no-op.
fn mask_source_limitations(segs: &[TimelineSegment]) -> Vec<RenderPlanLimitation> {
    segs.iter()
        .filter_map(|seg| {
            let plan = seg.color_pipeline.as_ref()?;
            let mask = plan.mask_source.as_ref()?;
            Some(RenderPlanLimitation {
                kind: "mask_not_implemented".to_string(),
                animation_id: None,
                clip_id: Some(seg.clip_name.clone()),
                parameter: Some("color_pipeline.mask_source".to_string()),
                message: format!(
                    "render ignored awidat.color_pipeline.mask_source {} on clip {:?}: regional grading is reserved but not yet implemented \
                     (see docs/color-pipeline/render-mask-lowering.md for the implementation plan)",
                    mask.display(),
                    seg.clip_name,
                ),
            })
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
    /// Supported Phase 3A parameter animations attached to this title.
    pub animations: Vec<RenderParameterAnimation>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        }
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
        // Comma-separate the drawtext filters so they all run on the
        // same input → single output. drawtext's `enable=` keeps each
        // bounded to its window without cross-contamination.
        let parts: Vec<String> = self
            .titles
            .iter()
            .map(|title| format_drawtext_filter(title, self.broadcast_overlay))
            .collect();
        filter.push_str(&parts.join(","));
        filter.push_str(&out_label);

        FilterPlan {
            filter_complex: filter,
            video_out_label: out_label,
            audio_out_label: base.audio_out_label,
        }
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
                filter.push_str(&format!(
                    "{from_v}{to_v}xfade=transition={kind}:duration={dur}:offset={off}{out};",
                    from_v = current_v,
                    to_v = inputs[next].0,
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
        let scale_multiplier = overlay_animation_value_expr(overlay, "overlay.scale", "1", "t");
        let (scale_expr, x_expr, y_expr) = match &overlay.mode {
            VideoOverlayMode::FullFrame => {
                let scale_expr = if has_overlay_animation(overlay, "overlay.scale") {
                    format!(
                        "w=main_w*({scale_multiplier}):h=main_h*({scale_multiplier}):eval=frame"
                    )
                } else {
                    "w=main_w:h=main_h".to_string()
                };
                (scale_expr, "0".to_string(), "0".to_string())
            }
            VideoOverlayMode::PiP {
                corner,
                scale,
                margin_pct,
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
                let scale_expr = if has_overlay_animation(overlay, "overlay.scale") {
                    format!("w=main_w*{scale}*({scale_multiplier}):h=-2:eval=frame")
                } else {
                    format!("w=main_w*{scale}:h=-2")
                };
                (scale_expr, x, y)
            }
        };
        let x_expr = if has_overlay_animation(overlay, "overlay.position") {
            format!(
                "({})+main_w*({})",
                x_expr,
                overlay_motion_path_expr(overlay, MotionPathCoordinate::X, "t")
                    .unwrap_or_else(|| "0".to_string())
            )
        } else if has_overlay_animation(overlay, "overlay.x") {
            format!(
                "({})+main_w*({})",
                x_expr,
                overlay_animation_value_expr(overlay, "overlay.x", "0", "t")
            )
        } else {
            x_expr
        };
        let y_expr = if has_overlay_animation(overlay, "overlay.position") {
            format!(
                "({})+main_h*({})",
                y_expr,
                overlay_motion_path_expr(overlay, MotionPathCoordinate::Y, "t")
                    .unwrap_or_else(|| "0".to_string())
            )
        } else if has_overlay_animation(overlay, "overlay.y") {
            format!(
                "({})+main_h*({})",
                y_expr,
                overlay_animation_value_expr(overlay, "overlay.y", "0", "t")
            )
        } else {
            y_expr
        };
        let opacity_filter = if has_overlay_animation(overlay, "overlay.opacity") {
            let opacity = overlay_animation_value_expr(overlay, "overlay.opacity", "1", "T");
            let alpha_label = format!("[media_overlay_alpha{idx}]");
            let filter = format!(
                "{scaled_label}format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*({opacity})'{alpha_label};"
            );
            (filter, alpha_label)
        } else {
            (String::new(), scaled_label.clone())
        };
        filter.push(';');
        filter.push_str(&format!(
            "{overlay_input}setpts=PTS-STARTPTS+{start}/TB{pts_label};\
             {pts_label}{current}scale2ref={scale_expr}{scaled_label}{ref_label};\
             {opacity_filter}\
             {ref_label}{overlay_video_label}overlay=x={x_expr}:y={y_expr}:enable='between(t\\,{start}\\,{end})'{next}",
            opacity_filter = opacity_filter.0,
            overlay_video_label = opacity_filter.1,
        ));
        current = next;
    }
    FilterPlan {
        filter_complex: filter,
        video_out_label: current,
        audio_out_label: base.audio_out_label,
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
        .and_then(|animation| animation.motion_path.as_ref())
        .map(|path| {
            let local_time_var = format!("({time_var}-{})", overlay.track_start_s);
            motion_path_to_ffmpeg_expr(path, coordinate, &local_time_var)
        })
}

fn has_overlay_animation(overlay: &VideoOverlayPlan, parameter: &str) -> bool {
    overlay
        .animations
        .iter()
        .any(|animation| animation.parameter == parameter)
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
    if let Some(reframe) = seg.reframe.as_ref()
        && let Some(chain) = reframe_filter_chain(reframe)
    {
        let rv = format!("[media_overlay_rv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!("{video_label}{chain}{rv}"));
        video_label = rv;
    }
    if let Some(factor) = seg.speed
        && (factor - 1.0).abs() > 1e-9
        && factor > 0.0
    {
        let sv = format!("[media_overlay_sv{input_idx}]");
        filter.push(';');
        filter.push_str(&format!(
            "{video_label}setpts={inv}*PTS{sv}",
            inv = 1.0 / factor,
        ));
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

    if let Some(plan) = seg.color_pipeline.as_ref() {
        if plan.has_any_lut() {
            let cv = format!("[cpv{i}]");
            filter.push_str(&color_pipeline_filter_block(
                &video_label,
                &cv,
                &i.to_string(),
                plan,
            ));
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

    if let Some(reframe) = seg.reframe.as_ref()
        && let Some(chain) = reframe_filter_chain(reframe)
    {
        let rv = format!("[rv{i}]");
        filter.push_str(&format!("{video_label}{chain}{rv};"));
        video_label = rv;
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

    // Speed after color/audio cleanup: setpts on video, atempo
    // (possibly chained) on audio.
    if let Some(factor) = seg.speed
        && (factor - 1.0).abs() > 1e-9
        && factor > 0.0
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!(
            "{video_label}setpts={inv}*PTS{sv};",
            inv = 1.0 / factor,
        ));
        video_label = sv;

        let sa = format!("[sa{i}]");
        let chain = atempo_chain(factor);
        filter.push_str(&format!("{audio_label}{chain}{sa};"));
        audio_label = sa;
    }

    // Volume next: applies to whatever the audio_label currently
    // points at (raw input or speed-stretched stream).
    if let Some(v) = seg.volume
        && (v - 1.0).abs() > 1e-9
    {
        let av = format!("[av{i}]");
        filter.push_str(&format!("{audio_label}volume={v}{av};"));
        audio_label = av;
    }
    (video_label, audio_label)
}

fn audio_fx_filter_chain(plan: &AudioFxPlan) -> Option<String> {
    let mut filters = Vec::new();

    // cleanup
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

fn positive(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0)
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
    let escaped_text = drawtext_escape(text);
    let resting_y = match t.position {
        TitlePosition::Top => "h*0.05".to_string(),
        TitlePosition::Center => "(h-text_h)/2".to_string(),
        TitlePosition::Bottom => title_bottom_y(t, broadcast_overlay),
    };
    let resting_x = "(w-text_w)/2".to_string();
    let weight_attr = match t.font_weight {
        TitleWeight::Normal => "",
        // ffmpeg drawtext doesn't have a `font_weight=` flag — bold
        // is communicated via the fontfile itself. Without a custom
        // bold font bundle, we approximate bold by stroking the
        // text with the same color (`borderw` adds a thicker outline,
        // which visually thickens the strokes).
        TitleWeight::Bold => ":borderw=2",
    };
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

fn title_motion_path_expr(title: &TitlePlan, coordinate: MotionPathCoordinate) -> Option<String> {
    title
        .animations
        .iter()
        .find(|animation| animation.parameter == "title.position")
        .and_then(|animation| animation.motion_path.as_ref())
        .map(|path| {
            let local_time_var = format!("(t-{})", title.start_s);
            motion_path_to_ffmpeg_expr(path, coordinate, &local_time_var)
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

fn reveal_steps(text: &str, reveal: TextReveal) -> Vec<String> {
    match reveal {
        TextReveal::None => vec![text.to_string()],
        TextReveal::Typewriter => text
            .char_indices()
            .map(|(index, character)| {
                let end = index + character.len_utf8();
                text[..end].to_string()
            })
            .collect(),
        TextReveal::Word => {
            let mut steps = Vec::new();
            let mut end = 0;
            for word in text.split_whitespace() {
                if let Some(relative) = text[end..].find(word) {
                    end += relative + word.len();
                    steps.push(text[..end].to_string());
                }
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
/// awidat.speed effect: `duration_s / factor` when factor is set,
/// raw `duration_s` otherwise. A 4s clip at 2× plays in 2s; at 0.5×
/// it plays in 8s.
fn effective_duration(seg: &TimelineSegment) -> f64 {
    seg.duration_s / segment_speed(seg)
}

fn visible_effective_duration(seg: &TimelineSegment) -> f64 {
    (effective_duration(seg) - seg.pre_handle_s - seg.post_handle_s).max(0.0)
}

fn segment_speed(seg: &TimelineSegment) -> f64 {
    match seg.speed {
        Some(f) if f > 0.0 => f,
        _ => 1.0,
    }
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
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    for overlay in video_overlays {
        argv.extend([
            "-ss".into(),
            format!("{}", overlay.segment.start_s),
            "-t".into(),
            format!("{}", overlay.segment.duration_s),
            "-i".into(),
            overlay.segment.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(path) = browser_broadcast_overlay {
        argv.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    }
    let base = FilterPlanner::new(segs, transitions).plan();
    let media = append_video_overlays(base, video_overlays, segs.len());
    let titles = if broadcast_overlay_owns_program_titles(broadcast_overlay) {
        &[]
    } else {
        titles
    };
    let ffmpeg_broadcast_overlay = browser_broadcast_overlay
        .is_none()
        .then_some(broadcast_overlay)
        .flatten();
    let plan = FilterPlanner::with_titles_and_broadcast_overlay(
        &[],
        &[],
        titles,
        ffmpeg_broadcast_overlay,
    )
    .decorate_video_filter(media.filter_complex, media.video_out_label);
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
        media.audio_out_label,
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
    argv.extend(output_color_tag_argv(segs));
    argv.extend([
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
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
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    for overlay in video_overlays {
        argv.extend([
            "-ss".into(),
            format!("{}", overlay.segment.start_s),
            "-t".into(),
            format!("{}", overlay.segment.duration_s),
            "-i".into(),
            overlay.segment.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(path) = browser_broadcast_overlay {
        argv.extend(["-i".into(), path.to_string_lossy().into_owned()]);
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
    let mut filter = plan_video_only_filter(segs, transitions, fallback_video_duration);
    let base_video_label = "[vonly]";
    let media = append_video_overlays(
        FilterPlan {
            filter_complex: filter,
            video_out_label: base_video_label.to_string(),
            audio_out_label: String::new(),
        },
        video_overlays,
        segs.len(),
    );
    filter = media.filter_complex;
    let mut video_label = media.video_out_label;
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
        let decorated = FilterPlanner::with_titles_and_broadcast_overlay(
            &[],
            &[],
            titles,
            ffmpeg_broadcast_overlay,
        )
        .decorate_video_filter(filter, video_label.clone());
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

    let mut next_input =
        segs.len() + video_overlays.len() + usize::from(browser_broadcast_overlay.is_some());
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
    argv.extend(output_color_tag_argv(segs));
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
                if let Some(factor) = c.speed
                    && factor > 0.0
                {
                    return c.duration_s / factor;
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
    if let Some(plan) = seg.color_pipeline.as_ref() {
        if plan.has_any_lut() {
            let cv = format!("[cpv{i}]");
            filter.push_str(&color_pipeline_filter_block(
                &video_label,
                &cv,
                &i.to_string(),
                plan,
            ));
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
    if let Some(reframe) = seg.reframe.as_ref()
        && let Some(chain) = reframe_filter_chain(reframe)
    {
        let rv = format!("[rv{i}]");
        filter.push_str(&format!("{video_label}{chain}{rv};"));
        video_label = rv;
    }
    if let Some(factor) = seg.speed
        && (factor - 1.0).abs() > 1e-9
        && factor > 0.0
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!(
            "{video_label}setpts={inv}*PTS{sv};",
            inv = 1.0 / factor,
        ));
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
            filter.push_str(&format!(
                "{label}{sidechain}sidechaincompress=threshold=0.05:ratio=8:attack={}:release={}{};",
                duck.attack_ms, duck.release_ms, out
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
                if let Some(factor) = clip.speed
                    && (factor - 1.0).abs() > 1e-9
                    && factor > 0.0
                {
                    let sped = format!("[aspeed{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}{}{};", atempo_chain(factor), sped));
                    label = sped;
                }
                if let Some(v) = clip.volume
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
    let mut argv = if audio_tracks.is_empty() {
        build_timeline_argv_full(
            &segs,
            &transitions,
            &video_overlays,
            &titles,
            broadcast_overlay.as_ref(),
            browser_broadcast_overlay.as_deref(),
            loudness_target,
            &output_path,
        )
    } else {
        build_timeline_argv_with_audio_tracks(
            &segs,
            &transitions,
            &video_overlays,
            &titles,
            broadcast_overlay.as_ref(),
            browser_broadcast_overlay.as_deref(),
            loudness_target,
            &audio_tracks,
            &output_path,
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
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
    };
    use awidat_proto::professional::{
        BezierHandles, Easing, ExtrapolationMode, Keyframe, KeyframeInterpolation,
        ParameterAnimation,
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

        let (_, _, video_overlays, _, _, _, _, limitations) =
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

        let (_, _, _, _, _, audio_tracks, _, limitations) =
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
        s.speed = Some(1.1);
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
    fn filter_planner_emits_setpts_and_atempo_for_speed_segment() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(2.0);
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
        s0.speed = Some(4.0);
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
        s0.speed = Some(0.25);
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
        s0.speed = Some(2.0);
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
        s0.speed = Some(2.0);
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
        s0.speed = Some(2.0);
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
        s0.speed = Some(2.0);
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
        s0.speed = Some(2.0);
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
    fn media_overlay_is_composited_before_caption_drawtext() {
        let segs = vec![seg("/tmp/base.mp4", 0.0, 5.0)];
        let overlays = vec![VideoOverlayPlan {
            segment: seg("/tmp/overlay.mp4", 0.0, 3.0),
            track_start_s: 1.0,
            mode: VideoOverlayMode::FullFrame,
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
                    },
                    awidat_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 0.2,
                        y: 0.4,
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
