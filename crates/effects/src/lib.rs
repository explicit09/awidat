//! Semantic effect registry for Awidat graph-native OTIO effects.
//!
//! This crate is intentionally backend-light. It centralizes Awidat's stable
//! effect ids, parameter schemas, stack policies, phases, and current backend
//! support. Renderers and EDL application code consume this registry instead
//! of hard-coding semantic decisions in multiple places.

use serde_json::{Map, Number, Value};
use thiserror::Error;

/// Per-clip linear gain effect id.
pub const VOLUME: &str = "awidat.volume";
/// Per-clip playback speed effect id.
pub const SPEED: &str = "awidat.speed";
/// Clip-level curve-based time remap effect id.
pub const TIME_REMAP: &str = "awidat.time_remap";
/// Clip-level color correction effect id.
pub const COLOR_CORRECTION: &str = "awidat.color_correction";
/// Clip-level 3D LUT effect id.
pub const LUT: &str = "awidat.lut";
/// Clip-level atomic color-pipeline effect id. Carries the full
/// input-space / shaper / look / output-space chain in a single
/// effect so the agent can emit one thing instead of composing
/// multiple partial effects in the right order.
pub const COLOR_PIPELINE: &str = "awidat.color_pipeline";
/// Drawtext-backed title/caption effect id.
pub const TITLE: &str = "awidat.title";
/// Picture-in-picture / cutaway overlay effect id.
pub const VIDEO_OVERLAY: &str = "awidat.video_overlay";
/// Clip-level punch-in / reframing effect id.
pub const REFRAME: &str = "awidat.reframe";
/// Per-clip audio fade effect id.
pub const AUDIO_FADE: &str = "awidat.audio_fade";
/// FFmpeg-native clip/track audio processing effect id.
pub const AUDIO_FX: &str = "awidat.audio_fx";

const VOLUME_PARAMS: &[ParamDef] = &[ParamDef::number("value", true, Some(0.0), None, None)];

const SPEED_PARAMS: &[ParamDef] = &[ParamDef::number(
    "factor",
    true,
    Some(f64::MIN_POSITIVE),
    None,
    None,
)];

const TIME_REMAP_PARAMS: &[ParamDef] = &[ParamDef::json("curve", true, None)];

const COLOR_PARAMS: &[ParamDef] = &[
    ParamDef::number("exposure_ev", false, Some(-4.0), Some(4.0), None),
    ParamDef::number("contrast", false, Some(0.0), Some(3.0), None),
    ParamDef::number("saturation", false, Some(0.0), Some(3.0), None),
    ParamDef::number("temperature", false, Some(-1.0), Some(1.0), None),
    ParamDef::number("tint", false, Some(-1.0), Some(1.0), None),
    ParamDef::number("shadows", false, Some(-1.0), Some(1.0), None),
    ParamDef::number("highlights", false, Some(-1.0), Some(1.0), None),
];

const LUT_PARAMS: &[ParamDef] = &[
    ParamDef::string("lut_path", true, None),
    ParamDef::string("interpolation", false, None),
    // 1.0 = full LUT (current behavior); 0.0 = bypass. Render emits
    // a split/blend pattern when this drops below 1.0; see
    // `lut3d_filter_block` in crates/render/src/timeline.rs.
    ParamDef::number(
        "strength",
        false,
        Some(0.0),
        Some(1.0),
        Some(ParamDefault::Number(1.0)),
    ),
];

// Color-pipeline slot schema. Atomic — the agent picks the slots it
// needs and leaves the rest absent rather than composing multiple
// effects in dependency order. Slot meanings:
//
// - `clip_input_space`: required. The color space the decoded pixels
//   actually live in (e.g., `arri_logc4`, `rec709_g24`, `scene_linear`).
// - `working_space`: optional. Intermediate space the chain operates
//   in; render picks a sensible default per `clip_input_space` when
//   absent.
// - `lut_input_space`: optional. The space the `look_lut` was authored
//   against — the render normalizes pixels into THIS space before
//   applying the look, not into linear unconditionally.
// - `output_space`: optional. Delivery target; falls back to the
//   project's default render space.
//
// LUT slots (all project-relative paths):
// - `input_transform_lut`: optional camera-to-working IDT.
// - `shaper_lut`: optional 1D shaper applied before the look LUT for
//   log-encoded sources.
// - `look_lut`: optional creative 3D LUT.
// - `look_interpolation`: optional interpolation for `look_lut`.
// - `look_strength`: optional 0..=1 blend; defaults to 1.0.
// - `output_transform_lut`: optional working-to-display ODT.
//
// Mask slot:
// - `mask_source`: optional project-relative path to a mask asset
//   (PNG with alpha, grayscale image, or video). Render supports the
//   v1 masked look-LUT path and reports explicit limitations for mask
//   combinations outside that scope.
const COLOR_PIPELINE_PARAMS: &[ParamDef] = &[
    ParamDef::string("clip_input_space", true, None),
    ParamDef::string("working_space", false, None),
    ParamDef::string("lut_input_space", false, None),
    ParamDef::string("output_space", false, None),
    ParamDef::string("input_transform_lut", false, None),
    ParamDef::string("shaper_lut", false, None),
    ParamDef::string("look_lut", false, None),
    ParamDef::string("look_interpolation", false, None),
    ParamDef::number(
        "look_strength",
        false,
        Some(0.0),
        Some(1.0),
        Some(ParamDefault::Number(1.0)),
    ),
    ParamDef::string("output_transform_lut", false, None),
    ParamDef::string("mask_source", false, None),
];

/// Extensions accepted for `awidat.color_pipeline.mask_source`. Image
/// formats carry the alpha matte; video formats let the mask
/// animate. The validator accepts these and treats anything else
/// as a malformed request.
pub const SUPPORTED_MASK_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "mp4", "mov"];

/// Stable identifiers for the color spaces awidat understands. These
/// are the *names* the agent uses when emitting an
/// `awidat.color_pipeline` effect; the actual transforms attached to
/// each are wired up in the render layer (Stage 3 work, see
/// `awidat-color-pipeline-plan` in the memory directory).
///
/// Display-referred spaces:
/// - `rec709_g24`: BT.709 primaries, gamma 2.4 (typical SDR delivery).
/// - `srgb`: BT.709 primaries, sRGB piecewise transfer.
/// - `dci_p3`: DCI-P3 primaries, gamma 2.6 (theatrical DCP).
///
/// Scene-referred:
/// - `scene_linear`: linear light in BT.709 primaries.
///
/// HDR delivery spaces:
/// - `pq_rec2020`: BT.2020 primaries, SMPTE ST 2084 (PQ) transfer.
/// - `hlg_rec2020`: BT.2020 primaries, ARIB STD-B67 (HLG) transfer.
///
/// Camera Log encodings (require linearization via a shaper LUT
/// before a Rec.709-authored creative LUT can be applied correctly):
/// - `arri_logc3`, `arri_logc4`
/// - `slog3_sgamut3`
/// - `vlog_vgamut`
/// - `redlog_filmgen5`
/// - `canon_log2`, `canon_log3`
/// - `bmd_film_gen5`
pub const KNOWN_COLOR_SPACES: &[&str] = &[
    "rec709_g24",
    "srgb",
    "dci_p3",
    "scene_linear",
    "pq_rec2020",
    "hlg_rec2020",
    "arri_logc3",
    "arri_logc4",
    "slog3_sgamut3",
    "vlog_vgamut",
    "redlog_filmgen5",
    "canon_log2",
    "canon_log3",
    "bmd_film_gen5",
];

/// `true` if `name` is one of [`KNOWN_COLOR_SPACES`].
pub fn is_known_color_space(name: &str) -> bool {
    KNOWN_COLOR_SPACES.contains(&name)
}

const TITLE_PARAMS: &[ParamDef] = &[
    ParamDef::number("start_s", true, Some(0.0), None, None),
    ParamDef::number("end_s", true, Some(0.0), None, None),
    ParamDef::string("text", true, None),
    ParamDef::string("position", false, Some(ParamDefault::String("center"))),
    ParamDef::integer(
        "font_size",
        false,
        Some(1),
        None,
        Some(ParamDefault::Integer(64)),
    ),
    ParamDef::string("color", false, Some(ParamDefault::String("#FFFFFF"))),
    ParamDef::string("font_weight", false, Some(ParamDefault::String("normal"))),
    ParamDef::string("animation", false, Some(ParamDefault::String("none"))),
    ParamDef::string("role", false, None),
    ParamDef::string("safe_area", false, None),
];

const VIDEO_OVERLAY_PARAMS: &[ParamDef] = &[
    ParamDef::string("mode", false, Some(ParamDefault::String("pip"))),
    ParamDef::string("corner", false, None),
    ParamDef::number("scale", false, Some(0.01), Some(1.0), None),
    ParamDef::number("margin_pct", false, Some(0.0), Some(0.5), None),
    ParamDef::number(
        "rotation_deg",
        false,
        Some(-360.0),
        Some(360.0),
        Some(ParamDefault::Number(0.0)),
    ),
    ParamDef::bool("motion_blur", false, Some(ParamDefault::Bool(false))),
    ParamDef::number(
        "motion_blur_shutter_s",
        false,
        Some(0.0),
        Some(0.2),
        Some(ParamDefault::Number(1.0 / 60.0)),
    ),
];

const REFRAME_PARAMS: &[ParamDef] = &[
    ParamDef::number("zoom", true, Some(1.0), Some(3.0), None),
    ParamDef::number(
        "x",
        false,
        Some(-1.0),
        Some(1.0),
        Some(ParamDefault::Number(0.0)),
    ),
    ParamDef::number(
        "y",
        false,
        Some(-1.0),
        Some(1.0),
        Some(ParamDefault::Number(0.0)),
    ),
];

const AUDIO_FADE_PARAMS: &[ParamDef] = &[
    ParamDef::number("fade_in_s", false, Some(0.0), None, None),
    ParamDef::number("fade_out_s", false, Some(0.0), None, None),
];

const AUDIO_FX_PARAMS: &[ParamDef] = &[
    ParamDef::number("high_pass_hz", false, Some(0.0), None, None),
    ParamDef::number("low_pass_hz", false, Some(0.0), None, None),
    ParamDef::json("eq_bands", false, None),
    ParamDef::number("compressor_threshold_db", false, None, None, None),
    ParamDef::number("compressor_ratio", false, Some(0.0), None, None),
    ParamDef::number("limiter_limit_db", false, None, None, None),
    ParamDef::number("noise_gate_threshold_db", false, None, None, None),
    ParamDef::number("hum_notch_hz", false, Some(0.0), None, None),
    ParamDef::number("de_ess_hz", false, Some(0.0), None, None),
    ParamDef::number("de_ess_reduction_db", false, Some(0.0), None, None),
    ParamDef::number("loudnorm_i", false, None, None, None),
    ParamDef::number("loudnorm_tp", false, None, None, None),
];

/// In-tree registry entries for graph-native Awidat effects.
pub const EFFECTS: &[EffectDef] = &[
    EffectDef {
        id: VOLUME,
        display_name: "Volume",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Audio,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: VOLUME_PARAMS,
    },
    EffectDef {
        id: SPEED,
        display_name: "Speed",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Both,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: SPEED_PARAMS,
    },
    EffectDef {
        id: TIME_REMAP,
        display_name: "Time Remap",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Both,
        phase: EffectPhase::Clip,
        support: SupportStatus::Experimental,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: TIME_REMAP_PARAMS,
    },
    EffectDef {
        id: COLOR_CORRECTION,
        display_name: "Color Correction",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 1,
        params: COLOR_PARAMS,
    },
    EffectDef {
        id: LUT,
        display_name: "3D LUT",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: LUT_PARAMS,
    },
    EffectDef {
        id: COLOR_PIPELINE,
        display_name: "Color Pipeline",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::Clip,
        // Render supports the color-management/LUT chain plus the v1
        // masked look-LUT subset. Unsupported mask combinations still
        // surface explicit render limitations.
        support: SupportStatus::Experimental,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: COLOR_PIPELINE_PARAMS,
    },
    EffectDef {
        id: TITLE,
        display_name: "Title",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::TimelineOverlay,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: TITLE_PARAMS,
    },
    EffectDef {
        id: VIDEO_OVERLAY,
        display_name: "Video Overlay",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::Transform,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: VIDEO_OVERLAY_PARAMS,
    },
    EffectDef {
        id: REFRAME,
        display_name: "Reframe",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Video,
        phase: EffectPhase::Transform,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 0,
        params: REFRAME_PARAMS,
    },
    EffectDef {
        id: AUDIO_FADE,
        display_name: "Audio Fade",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Audio,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 1,
        params: AUDIO_FADE_PARAMS,
    },
    EffectDef {
        id: AUDIO_FX,
        display_name: "Audio FX",
        scope: EffectScope::Clip,
        media_kind: MediaKind::Audio,
        phase: EffectPhase::Clip,
        support: SupportStatus::Stable,
        stack_policy: StackPolicy::ReplaceSameId,
        backend: BackendKind::FfmpegNative,
        min_present_params: 1,
        params: AUDIO_FX_PARAMS,
    },
];

/// Static definition for one Awidat semantic effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectDef {
    /// Stable graph-native effect id.
    pub id: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Scope where the effect can be attached.
    pub scope: EffectScope,
    /// Media streams the effect influences.
    pub media_kind: MediaKind,
    /// Render/evaluation phase.
    pub phase: EffectPhase,
    /// Current support status.
    pub support: SupportStatus,
    /// Whether repeated same-id effects replace or stack.
    pub stack_policy: StackPolicy,
    /// Current backend family.
    pub backend: BackendKind,
    /// Minimum number of user-supplied parameters required when no specific
    /// required param exists. Useful for sparse controls such as color.
    pub min_present_params: usize,
    /// Parameter schema.
    pub params: &'static [ParamDef],
}

impl EffectDef {
    /// Validate `params` and return a normalized metadata map with defaults.
    pub fn normalize_params(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Map<String, Value>, ValidationError> {
        for key in params.keys() {
            if self.param(key).is_none() {
                return Err(ValidationError::UnknownParam {
                    effect_id: self.id,
                    param: key.clone(),
                });
            }
        }

        let supplied = params.len();
        if supplied < self.min_present_params {
            return Err(ValidationError::NotEnoughParams {
                effect_id: self.id,
                min: self.min_present_params,
            });
        }

        let mut out = Map::new();
        for def in self.params {
            match params.get(def.key) {
                Some(value) => {
                    def.validate_value(self.id, value)?;
                    out.insert(def.key.to_string(), value.clone());
                }
                None if def.required => {
                    return Err(ValidationError::MissingParam {
                        effect_id: self.id,
                        param: def.key,
                    });
                }
                None => {
                    if let Some(default) = def.default {
                        out.insert(def.key.to_string(), default.to_value()?);
                    }
                }
            }
        }
        Ok(out)
    }

    fn param(&self, key: &str) -> Option<&'static ParamDef> {
        self.params.iter().find(|p| p.key == key)
    }
}

/// Where an effect can be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectScope {
    /// Attached to a clip.
    Clip,
    /// Attached to a track.
    Track,
    /// Attached to the whole timeline.
    Timeline,
}

/// Which media streams an effect influences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Video stream only.
    Video,
    /// Audio stream only.
    Audio,
    /// Audio and video.
    Both,
}

/// Evaluation phase for ordered render planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPhase {
    /// Decode/source-facing phase.
    Source,
    /// Clip-local phase before timeline combination.
    Clip,
    /// Transform/composite phase.
    Transform,
    /// Transition phase.
    Transition,
    /// Timeline-level overlay phase.
    TimelineOverlay,
    /// Final output phase.
    Output,
}

/// Current implementation support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportStatus {
    /// Supported and expected to work.
    Stable,
    /// Available for testing, not yet stable.
    Experimental,
    /// Defined but currently unavailable.
    Unavailable,
}

/// Same-id stacking behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackPolicy {
    /// Replace existing effects with the same id.
    ReplaceSameId,
    /// Allow multiple effects with the same id.
    Stackable,
}

/// Backend family used by the current renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Lowered to FFmpeg-native filters.
    FfmpegNative,
    /// Stored semantically but not rendered yet.
    SemanticOnly,
}

/// Static parameter definition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDef {
    /// Parameter key stored in OTIO metadata.
    pub key: &'static str,
    /// Expected JSON value type.
    pub kind: ParamKind,
    /// Whether this parameter is required.
    pub required: bool,
    /// Inclusive minimum for number/integer parameters.
    pub min: Option<f64>,
    /// Inclusive maximum for number/integer parameters.
    pub max: Option<f64>,
    /// Default inserted when omitted.
    pub default: Option<ParamDefault>,
}

impl ParamDef {
    /// Define a numeric parameter.
    pub const fn number(
        key: &'static str,
        required: bool,
        min: Option<f64>,
        max: Option<f64>,
        default: Option<ParamDefault>,
    ) -> Self {
        Self {
            key,
            kind: ParamKind::Number,
            required,
            min,
            max,
            default,
        }
    }

    /// Define an integer parameter.
    pub const fn integer(
        key: &'static str,
        required: bool,
        min: Option<i64>,
        max: Option<i64>,
        default: Option<ParamDefault>,
    ) -> Self {
        Self {
            key,
            kind: ParamKind::Integer,
            required,
            min: match min {
                Some(v) => Some(v as f64),
                None => None,
            },
            max: match max {
                Some(v) => Some(v as f64),
                None => None,
            },
            default,
        }
    }

    /// Define a string parameter.
    pub const fn string(key: &'static str, required: bool, default: Option<ParamDefault>) -> Self {
        Self {
            key,
            kind: ParamKind::String,
            required,
            min: None,
            max: None,
            default,
        }
    }

    /// Define a boolean parameter.
    pub const fn bool(key: &'static str, required: bool, default: Option<ParamDefault>) -> Self {
        Self {
            key,
            kind: ParamKind::Bool,
            required,
            min: None,
            max: None,
            default,
        }
    }

    /// Define a JSON parameter.
    pub const fn json(key: &'static str, required: bool, default: Option<ParamDefault>) -> Self {
        Self {
            key,
            kind: ParamKind::Json,
            required,
            min: None,
            max: None,
            default,
        }
    }

    fn validate_value(
        &self,
        effect_id: &'static str,
        value: &Value,
    ) -> Result<(), ValidationError> {
        match self.kind {
            ParamKind::Number => validate_number(effect_id, self, value),
            ParamKind::Integer => validate_integer(effect_id, self, value),
            ParamKind::String => {
                if value.as_str().is_some() {
                    Ok(())
                } else {
                    Err(ValidationError::WrongType {
                        effect_id,
                        param: self.key,
                        expected: "string",
                    })
                }
            }
            ParamKind::Bool => {
                if value.as_bool().is_some() {
                    Ok(())
                } else {
                    Err(ValidationError::WrongType {
                        effect_id,
                        param: self.key,
                        expected: "bool",
                    })
                }
            }
            ParamKind::Json => Ok(()),
        }
    }
}

/// Expected JSON parameter kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// JSON number.
    Number,
    /// JSON integer number.
    Integer,
    /// JSON string.
    String,
    /// JSON bool.
    Bool,
    /// Any JSON value.
    Json,
}

/// Default parameter value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamDefault {
    /// Numeric default.
    Number(f64),
    /// Integer default.
    Integer(i64),
    /// Static string default.
    String(&'static str),
    /// Boolean default.
    Bool(bool),
}

impl ParamDefault {
    fn to_value(self) -> Result<Value, ValidationError> {
        match self {
            Self::Number(n) => {
                let Some(number) = Number::from_f64(n) else {
                    return Err(ValidationError::InvalidDefault);
                };
                Ok(Value::Number(number))
            }
            Self::Integer(n) => Ok(Value::Number(Number::from(n))),
            Self::String(s) => Ok(Value::String(s.to_string())),
            Self::Bool(v) => Ok(Value::Bool(v)),
        }
    }
}

/// Find a registered effect by id.
pub fn lookup(id: &str) -> Option<&'static EffectDef> {
    EFFECTS.iter().find(|e| e.id == id)
}

/// Validate and normalize parameters for `effect_id`.
pub fn normalize_params(
    effect_id: &str,
    params: &Map<String, Value>,
) -> Result<(&'static EffectDef, Map<String, Value>), ValidationError> {
    let Some(effect) = lookup(effect_id) else {
        return Err(ValidationError::UnknownEffect {
            effect_id: effect_id.to_string(),
        });
    };
    let normalized = effect.normalize_params(params)?;
    Ok((effect, normalized))
}

fn validate_number(
    effect_id: &'static str,
    def: &ParamDef,
    value: &Value,
) -> Result<(), ValidationError> {
    let Some(n) = value.as_f64() else {
        return Err(ValidationError::WrongType {
            effect_id,
            param: def.key,
            expected: "number",
        });
    };
    if !n.is_finite() {
        return Err(ValidationError::NonFinite {
            effect_id,
            param: def.key,
        });
    }
    validate_range(effect_id, def, n)
}

fn validate_integer(
    effect_id: &'static str,
    def: &ParamDef,
    value: &Value,
) -> Result<(), ValidationError> {
    let Some(n) = value.as_i64() else {
        return Err(ValidationError::WrongType {
            effect_id,
            param: def.key,
            expected: "integer",
        });
    };
    validate_range(effect_id, def, n as f64)
}

fn validate_range(
    effect_id: &'static str,
    def: &ParamDef,
    value: f64,
) -> Result<(), ValidationError> {
    if let Some(min) = def.min
        && value < min
    {
        return Err(ValidationError::OutOfRange {
            effect_id,
            param: def.key,
            value,
            min: def.min,
            max: def.max,
        });
    }
    if let Some(max) = def.max
        && value > max
    {
        return Err(ValidationError::OutOfRange {
            effect_id,
            param: def.key,
            value,
            min: def.min,
            max: def.max,
        });
    }
    Ok(())
}

/// Effect registry validation error.
#[derive(Debug, Error, PartialEq)]
pub enum ValidationError {
    /// The effect id is not registered.
    #[error("unknown effect id {effect_id:?}")]
    UnknownEffect {
        /// Unknown id.
        effect_id: String,
    },
    /// A required parameter is missing.
    #[error("effect {effect_id:?} missing required parameter {param:?}")]
    MissingParam {
        /// Effect id.
        effect_id: &'static str,
        /// Parameter key.
        param: &'static str,
    },
    /// Too few sparse parameters were supplied.
    #[error("effect {effect_id:?} requires at least {min} parameter(s)")]
    NotEnoughParams {
        /// Effect id.
        effect_id: &'static str,
        /// Minimum supplied parameter count.
        min: usize,
    },
    /// Parameter key is not known for the effect.
    #[error("effect {effect_id:?} does not accept parameter {param:?}")]
    UnknownParam {
        /// Effect id.
        effect_id: &'static str,
        /// Unknown parameter key.
        param: String,
    },
    /// Parameter value type is wrong.
    #[error("effect {effect_id:?} parameter {param:?} must be {expected}")]
    WrongType {
        /// Effect id.
        effect_id: &'static str,
        /// Parameter key.
        param: &'static str,
        /// Expected type label.
        expected: &'static str,
    },
    /// Number was non-finite.
    #[error("effect {effect_id:?} parameter {param:?} must be finite")]
    NonFinite {
        /// Effect id.
        effect_id: &'static str,
        /// Parameter key.
        param: &'static str,
    },
    /// Numeric value is outside its safe range.
    #[error("effect {effect_id:?} parameter {param:?} value {value} is outside range")]
    OutOfRange {
        /// Effect id.
        effect_id: &'static str,
        /// Parameter key.
        param: &'static str,
        /// Supplied value.
        value: f64,
        /// Inclusive minimum.
        min: Option<f64>,
        /// Inclusive maximum.
        max: Option<f64>,
    },
    /// A static default could not be converted to JSON.
    #[error("invalid effect registry default")]
    InvalidDefault,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_lookup(id: &str) -> &'static EffectDef {
        match lookup(id) {
            Some(effect) => effect,
            None => panic!("{id} should be registered"),
        }
    }

    fn must_normalize(
        effect_id: &str,
        params: &Map<String, Value>,
    ) -> (&'static EffectDef, Map<String, Value>) {
        match normalize_params(effect_id, params) {
            Ok(normalized) => normalized,
            Err(err) => panic!("{effect_id} should validate, got {err}"),
        }
    }

    fn must_reject(effect_id: &str, params: &Map<String, Value>) -> ValidationError {
        match normalize_params(effect_id, params) {
            Ok((effect, metadata)) => {
                panic!("{effect_id} should reject, got {effect:?} with {metadata:?}")
            }
            Err(err) => err,
        }
    }

    #[test]
    fn lookup_finds_existing_effects() {
        let volume = must_lookup(VOLUME);
        assert_eq!(volume.display_name, "Volume");
        assert_eq!(volume.stack_policy, StackPolicy::ReplaceSameId);
    }

    #[test]
    fn unknown_effect_fails() {
        let err = must_reject("awidat.nope", &Map::new());
        assert!(matches!(err, ValidationError::UnknownEffect { .. }));
    }

    #[test]
    fn required_param_is_enforced() {
        let err = must_reject(VOLUME, &Map::new());
        assert!(matches!(
            err,
            ValidationError::MissingParam { param: "value", .. }
        ));
    }

    #[test]
    fn numeric_range_is_enforced() {
        let mut params = Map::new();
        params.insert("factor".into(), Value::from(0.0));
        let err = must_reject(SPEED, &params);
        assert!(matches!(
            err,
            ValidationError::OutOfRange {
                effect_id: SPEED,
                param: "factor",
                ..
            }
        ));
    }

    #[test]
    fn sparse_effect_requires_at_least_one_param() {
        let err = must_reject(COLOR_CORRECTION, &Map::new());
        assert!(matches!(
            err,
            ValidationError::NotEnoughParams {
                effect_id: COLOR_CORRECTION,
                min: 1
            }
        ));
    }

    #[test]
    fn defaults_are_inserted() {
        let mut params = Map::new();
        params.insert("start_s".into(), Value::from(0.0));
        params.insert("end_s".into(), Value::from(2.0));
        params.insert("text".into(), Value::from("Hello"));
        let (_, normalized) = must_normalize(TITLE, &params);
        assert_eq!(
            normalized.get("position").and_then(Value::as_str),
            Some("center")
        );
        assert_eq!(
            normalized.get("font_size").and_then(Value::as_i64),
            Some(64)
        );
    }

    #[test]
    fn video_overlay_accepts_rotation_degrees() {
        let mut params = Map::new();
        params.insert("mode".into(), Value::from("pip"));
        params.insert("rotation_deg".into(), Value::from(12.5));
        let (_, normalized) = must_normalize(VIDEO_OVERLAY, &params);
        assert_eq!(
            normalized.get("rotation_deg").and_then(Value::as_f64),
            Some(12.5)
        );
    }

    #[test]
    fn video_overlay_accepts_transform_motion_blur() {
        let mut params = Map::new();
        params.insert("mode".into(), Value::from("pip"));
        params.insert("motion_blur".into(), Value::from(true));
        params.insert("motion_blur_shutter_s".into(), Value::from(0.02));
        let (_, normalized) = must_normalize(VIDEO_OVERLAY, &params);
        assert_eq!(
            normalized.get("motion_blur").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            normalized
                .get("motion_blur_shutter_s")
                .and_then(Value::as_f64),
            Some(0.02)
        );
    }

    #[test]
    fn time_remap_accepts_curve_points() {
        let mut params = Map::new();
        params.insert(
            "curve".into(),
            serde_json::json!([
                {"source_time_s": 0.0, "timeline_time_s": 0.0},
                {"source_time_s": 1.0, "timeline_time_s": 2.5},
                {"source_time_s": 2.0, "timeline_time_s": 3.5}
            ]),
        );
        let (_, normalized) = must_normalize(TIME_REMAP, &params);
        assert!(normalized.get("curve").and_then(Value::as_array).is_some());
    }

    #[test]
    fn reframe_defaults_position_to_center() {
        let mut params = Map::new();
        params.insert("zoom".into(), Value::from(1.08));
        let (_, normalized) = must_normalize(REFRAME, &params);
        assert_eq!(normalized.get("zoom").and_then(Value::as_f64), Some(1.08));
        assert_eq!(normalized.get("x").and_then(Value::as_f64), Some(0.0));
        assert_eq!(normalized.get("y").and_then(Value::as_f64), Some(0.0));
    }

    #[test]
    fn color_pipeline_requires_clip_input_space() {
        let err = must_reject(COLOR_PIPELINE, &Map::new());
        assert!(matches!(
            err,
            ValidationError::MissingParam {
                effect_id: COLOR_PIPELINE,
                param: "clip_input_space",
            }
        ));
    }

    #[test]
    fn color_pipeline_accepts_minimal_metadata() {
        let mut params = Map::new();
        params.insert("clip_input_space".into(), Value::from("rec709_g24"));
        let (_, normalized) = must_normalize(COLOR_PIPELINE, &params);
        assert_eq!(
            normalized.get("look_strength").and_then(Value::as_f64),
            Some(1.0),
            "look_strength should default to 1.0",
        );
    }

    #[test]
    fn color_pipeline_registry_advertises_ffmpeg_backend() {
        let definition = match lookup(COLOR_PIPELINE) {
            Some(definition) => definition,
            None => panic!("color pipeline effect is registered"),
        };

        assert_eq!(definition.backend, BackendKind::FfmpegNative);
    }

    #[test]
    fn color_pipeline_range_checks_look_strength() {
        let mut params = Map::new();
        params.insert("clip_input_space".into(), Value::from("rec709_g24"));
        params.insert("look_strength".into(), Value::from(1.5));
        let err = must_reject(COLOR_PIPELINE, &params);
        assert!(matches!(
            err,
            ValidationError::OutOfRange {
                effect_id: COLOR_PIPELINE,
                param: "look_strength",
                ..
            }
        ));
    }

    #[test]
    fn known_color_spaces_includes_log_encodings() {
        assert!(is_known_color_space("arri_logc4"));
        assert!(is_known_color_space("rec709_g24"));
        assert!(is_known_color_space("scene_linear"));
        assert!(!is_known_color_space("Made_Up_Space"));
    }

    #[test]
    fn unknown_param_is_rejected() {
        let mut params = Map::new();
        params.insert("value".into(), Value::from(0.5));
        params.insert("surprise".into(), Value::from(true));
        let err = must_reject(VOLUME, &params);
        assert!(matches!(
            err,
            ValidationError::UnknownParam { param, .. } if param == "surprise"
        ));
    }
}
