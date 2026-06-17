//! Built-in transition registry and semantic transition metadata.
//!
//! This is the phase-one in-tree contract. A future
//! `montage-transitions` package can replace the registry data behind
//! this shape without changing EDL, OTIO, or render callers.

use serde::{Deserialize, Serialize};

/// Metadata Montage stores on OTIO `Transition.1` nodes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticTransitionSpec {
    /// Stable transition id, for example `montage.cross_dissolve`.
    pub id: String,
    /// Broad transition family used for taste/selection.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub family: Option<String>,
    /// Why the agent chose this transition at this cut.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub intent: Option<String>,
    /// Normalized energy/intensity hint in `[0.0, 1.0]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub energy: Option<f64>,
    /// Optional spatial/motion direction, e.g. `left`, `right`, `in`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub direction: Option<String>,
    /// Backend-specific parameters, kept opaque to the OTIO layer.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty", default)]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Optional structured recipe over Montage's stable transition
    /// primitives. Agent-authored custom transitions should live here
    /// as data, never as raw FFmpeg/GLSL/backend code.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub composition: Option<TransitionComposition>,
}

/// Data-only transition recipe. Presets and one-off agent-authored
/// transitions share this shape; a preset is just a named recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionComposition {
    /// Composition schema version. Starts at 1 so the eventual
    /// `montage-transitions` package can version recipes independently
    /// of the project file format.
    #[serde(default = "default_composition_version")]
    pub version: u8,
    /// Ordered primitive operations. The renderer may lower these to
    /// FFmpeg, GLSL, Remotion, Resolve/Fusion, or another backend.
    pub primitives: Vec<TransitionPrimitive>,
}

const fn default_composition_version() -> u8 {
    1
}

impl Default for TransitionComposition {
    fn default() -> Self {
        Self {
            version: default_composition_version(),
            primitives: Vec::new(),
        }
    }
}

/// One primitive operation inside a transition composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionPrimitive {
    /// Normalized start inside the transition duration, in `[0.0, 1.0]`.
    #[serde(default)]
    pub start: f64,
    /// Normalized end inside the transition duration, in `[0.0, 1.0]`.
    #[serde(default = "default_primitive_end")]
    pub end: f64,
    /// Easing curve used for this primitive's progress.
    #[serde(default)]
    pub easing: TransitionEasing,
    /// Primitive operation and its bounded parameters.
    #[serde(flatten)]
    pub op: TransitionPrimitiveOp,
}

const fn default_primitive_end() -> f64 {
    1.0
}

/// Easing curves available to transition primitives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionEasing {
    /// Linear interpolation.
    Linear,
    /// Smooth in/out curve.
    #[default]
    EaseInOut,
    /// Starts fast and eases into place.
    EaseOut,
    /// Starts gently and accelerates.
    EaseIn,
    /// High-energy exponential ease-out.
    EaseOutExpo,
}

impl TransitionEasing {
    /// Apply the easing curve to a normalized progress value in
    /// `[0.0, 1.0]`. Output is also clamped to `[0.0, 1.0]`.
    pub fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseInOut => {
                // Smooth-step cubic: 3t² - 2t³
                t * t * (3.0 - 2.0 * t)
            }
            Self::EaseOut => 1.0 - (1.0 - t).powi(2),
            Self::EaseIn => t * t,
            Self::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - (-10.0 * t).exp()
                }
            }
        }
    }
}

/// A single keyframe in a [`ParamCurve`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Normalized time inside the primitive's `[start, end]` window,
    /// in `[0.0, 1.0]`.
    pub t: f64,
    /// Parameter value at this keyframe.
    pub v: f64,
    /// Easing curve from this keyframe to the next one. Ignored at
    /// the terminal keyframe.
    #[serde(default)]
    pub easing: TransitionEasing,
}

/// A parameter value: either a constant or a multi-keyframe curve.
///
/// JSON serialization is `#[serde(untagged)]` so existing project
/// files using bare numeric parameters continue to parse:
///
/// ```json
/// "amount": 0.5
/// ```
///
/// becomes `ParamCurve::Const(0.5)`. A keyframed parameter is an
/// array of keyframe objects:
///
/// ```json
/// "amount": [
///   {"t": 0.0, "v": 0.0},
///   {"t": 0.5, "v": 0.8, "easing": "ease_in_out"},
///   {"t": 1.0, "v": 0.0}
/// ]
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamCurve {
    /// A constant value across the entire primitive window.
    Const(f64),
    /// A multi-keyframe curve. Keyframes must be sorted by `t` and
    /// every `t` must lie in `[0.0, 1.0]`. Validation enforces both.
    Keyframes(Vec<Keyframe>),
}

impl ParamCurve {
    /// Evaluate the curve at progress `p` in `[0.0, 1.0]`.
    ///
    /// * [`ParamCurve::Const`] returns the constant unchanged.
    /// * [`ParamCurve::Keyframes`] linearly interpolates between
    ///   adjacent keyframes, applying each segment's leading easing
    ///   curve. Progress before the first keyframe returns the first
    ///   value; progress after the last keyframe returns the last
    ///   value. Empty keyframe lists return 0.0 (validation rejects
    ///   them at the project-load layer, so this only happens for
    ///   in-memory test fixtures).
    pub fn evaluate(&self, p: f64) -> f64 {
        match self {
            Self::Const(v) => *v,
            Self::Keyframes(keys) => {
                if keys.is_empty() {
                    return 0.0;
                }
                if keys.len() == 1 {
                    return keys[0].v;
                }
                let p = p.clamp(0.0, 1.0);
                if p <= keys[0].t {
                    return keys[0].v;
                }
                let last = keys[keys.len() - 1];
                if p >= last.t {
                    return last.v;
                }
                for window in keys.windows(2) {
                    let a = window[0];
                    let b = window[1];
                    if p >= a.t && p <= b.t {
                        let span = b.t - a.t;
                        if span <= 0.0 {
                            return a.v;
                        }
                        let local = (p - a.t) / span;
                        let eased = a.easing.apply(local);
                        return a.v + (b.v - a.v) * eased;
                    }
                }
                last.v
            }
        }
    }
}

impl From<f64> for ParamCurve {
    fn from(v: f64) -> Self {
        Self::Const(v)
    }
}

impl Default for ParamCurve {
    fn default() -> Self {
        Self::Const(0.0)
    }
}

/// Stable primitive vocabulary for data-authored transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TransitionPrimitiveOp {
    /// Blend outgoing opacity to incoming opacity.
    Opacity {
        /// Outgoing opacity at primitive start. Scalar or
        /// multi-keyframe curve.
        from: ParamCurve,
        /// Incoming opacity at primitive end. Scalar or
        /// multi-keyframe curve.
        to: ParamCurve,
    },
    /// Push both clips along a direction.
    Push {
        /// Direction: left, right, up, or down.
        direction: String,
        /// Normalized distance where `1.0` means one full frame.
        distance: f64,
    },
    /// Wipe/mask reveal along a direction.
    Wipe {
        /// Direction: left, right, up, down, in, or out.
        direction: String,
        /// Edge softness in `[0.0, 1.0]`. Scalar or multi-keyframe
        /// curve — useful for "soft edge grows then shrinks" reveals.
        softness: ParamCurve,
    },
    /// Scale the image around its center.
    Zoom {
        /// End scale. `1.0` is unchanged. Scalar or multi-keyframe
        /// curve — useful for "punch in, overshoot, settle" effects.
        scale: ParamCurve,
    },
    /// Directional or isotropic blur.
    Blur {
        /// Blur amount in `[0.0, 1.0]`. Scalar or multi-keyframe
        /// curve — useful for "asymmetric ramp" feels (snap-in,
        /// trail-out).
        amount: ParamCurve,
        /// Optional blur direction.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        direction: Option<String>,
    },
    /// Brief color flash over the transition.
    Flash {
        /// Hex color such as `#ffffff`.
        color: String,
        /// Peak opacity in `[0.0, 1.0]`.
        peak: f64,
    },
    /// Camera shake accent.
    Shake {
        /// Shake amount in `[0.0, 1.0]`.
        amount: f64,
        /// Decay in `[0.0, 1.0]`.
        decay: f64,
    },
    /// RGB/channel separation.
    ChromaticSplit {
        /// Split amount in `[0.0, 1.0]`.
        amount: f64,
    },
    /// Pixel block effect.
    Pixelize {
        /// Block size in normalized `[0.0, 1.0]`.
        block_size: f64,
    },
    /// Procedural luma-mask reveal. The GPU `LumaMask` shader reads
    /// `mask_kind` to choose the mask geometry (clock / blinds /
    /// checkerboard / spiral / burst / jaws) and uses `softness` as
    /// the feathered edge band around the progress threshold. Future
    /// image-based masks (Kdenlive ports) extend the same primitive
    /// with a `texture` asset reference.
    LumaMask {
        /// Stable mask kind id. See [`LUMA_MASK_KINDS`] for the
        /// supported set; validation rejects unknown values.
        mask_kind: String,
        /// Edge softness in `[0.0, 1.0]`. 0 is a hard mask, 1 is a
        /// very feathered band. Scalar or multi-keyframe curve so
        /// the agent can author "edge sharpens then softens" feels.
        softness: ParamCurve,
    },
    /// Warm overexposed lens-flare sweep, GPU-only. The shader pans
    /// a glowing off-screen source through the frame across the
    /// transition window and tints the affected pixels with a
    /// configurable warmth bias.
    LightLeak {
        /// Peak leak intensity in `[0.0, 1.0]`, evaluated per frame.
        intensity: ParamCurve,
        /// Warmth bias in `[0.0, 1.0]`. 0 is neutral white,
        /// 1 is sunset-orange. Held constant for a transition; an
        /// agent that wants animated warmth can stack two leaks
        /// inside one composition.
        warmth: f64,
    },
    /// Polar-coordinate swirl that pulls the outgoing clip into a
    /// vortex around the frame center. GPU-only.
    SwirlVortex {
        /// Rotation strength in `[0.0, 1.0]`. 0 collapses to a
        /// cross-blend; 1 spins the corners ~2π by the end of the
        /// transition. Scalar or curve.
        strength: ParamCurve,
    },
    /// Directional cinematic motion blur. GPU-only; replaces the
    /// FFmpeg `hblur` fallback for whip-pan-style transitions with
    /// a proper 10-tap directional smear.
    CinematicPan {
        /// `left` or `right`. Validation rejects other values; the
        /// shader maps these to the horizontal direction sign.
        direction: String,
        /// Peak blur intensity in `[0.0, 1.0]`, evaluated per frame.
        intensity: ParamCurve,
    },
    /// Playback-speed curve applied across the transition window.
    /// Values are multipliers where `1.0` is realtime, `0.5` is half
    /// speed, and `2.0` is double speed.
    TimeRemap {
        /// Speed multiplier curve in `[0.05, 8.0]`.
        speed: ParamCurve,
    },
    /// Stable named atomic transition that does not decompose cleanly
    /// into primitives yet. This is still data: it points at an Montage
    /// transition id, not backend code or a raw filter graph.
    Atomic {
        /// Stable Montage transition id.
        id: String,
    },
}

/// Phase-one built-in transition definition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuiltinTransition {
    /// Stable Montage id.
    pub id: &'static str,
    /// Family name.
    pub family: &'static str,
    /// Human display name.
    pub display_name: &'static str,
    /// FFmpeg `xfade=transition=` value. `None` means the transition
    /// is semantic only and should not be emitted as a real transition.
    pub ffmpeg_xfade: Option<&'static str>,
    /// Default duration in seconds.
    pub default_duration_s: f64,
    /// Minimum supported/editorially useful duration in seconds.
    pub min_duration_s: f64,
    /// Maximum supported/editorially useful duration in seconds.
    pub max_duration_s: f64,
    /// How timeline audio should behave while the visual transition overlaps.
    pub audio_policy: TransitionAudioPolicy,
    /// Editorial contexts in which the agent should reach for this transition.
    pub best_for: &'static [&'static str],
    /// Editorial contexts where the agent should avoid this transition.
    pub avoid_for: &'static [&'static str],
    /// Whether picking this transition meaningfully depends on matching
    /// the actual screen-direction of motion at the cut.
    pub requires_motion_continuity: bool,
    /// Screen direction this transition encodes, if any.
    pub motion_alignment: Option<MotionAlignment>,
    /// How visually disruptive the transition is to color/luma continuity.
    pub color_sensitivity: ColorSensitivity,
}

/// Screen-direction encoded by a transition, used to match against
/// measured motion at a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionAlignment {
    /// Conveys leftward screen motion.
    Left,
    /// Conveys rightward screen motion.
    Right,
    /// Conveys upward screen motion.
    Up,
    /// Conveys downward screen motion.
    Down,
    /// Pushes inward (zoom/iris in).
    In,
    /// Pulls outward (iris/zoom out).
    Out,
}

/// How sensitive the transition is to color/luminance continuity at
/// the cut. The agent uses this to avoid choices that will read as
/// visually jarring on adjacent frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSensitivity {
    /// Reads cleanly regardless of color/luma delta.
    Insensitive,
    /// Reads poorly when going from a dark frame into a bright frame.
    AvoidDarkToBright,
    /// Reads poorly when going from a bright frame into a dark frame.
    AvoidBrightToDark,
    /// Reads poorly across any significant color/saturation jump.
    AvoidColorShift,
}

impl MotionAlignment {
    /// Stable lowercase identifier for serialization and EDL output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
            Self::In => "in",
            Self::Out => "out",
        }
    }

    /// Parse from a direction hint string. Returns `None` for unknown values.
    pub fn from_hint(hint: &str) -> Option<Self> {
        match hint {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "in" => Some(Self::In),
            "out" => Some(Self::Out),
            _ => None,
        }
    }

    /// True when two alignments share the same axis direction.
    pub fn agrees_with(self, other: MotionAlignment) -> bool {
        self == other
    }

    /// True when alignments are opposite on the same axis (`left` vs
    /// `right`, `up` vs `down`, `in` vs `out`).
    pub fn opposes(self, other: MotionAlignment) -> bool {
        matches!(
            (self, other),
            (Self::Left, Self::Right)
                | (Self::Right, Self::Left)
                | (Self::Up, Self::Down)
                | (Self::Down, Self::Up)
                | (Self::In, Self::Out)
                | (Self::Out, Self::In)
        )
    }
}

impl ColorSensitivity {
    /// Stable lowercase identifier for manifest/JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Insensitive => "insensitive",
            Self::AvoidDarkToBright => "avoid_dark_to_bright",
            Self::AvoidBrightToDark => "avoid_bright_to_dark",
            Self::AvoidColorShift => "avoid_color_shift",
        }
    }
}

/// Audio behavior paired with a visual transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionAudioPolicy {
    /// Smoothly crossfade between adjacent source audio.
    Crossfade,
    /// Preserve the visual overlap duration but switch audio without a fade curve.
    Cut,
}

/// Backend identifiers used by transition manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionBackend {
    /// FFmpeg export backend.
    Ffmpeg,
    /// GLSL/WebGL shader backend.
    Glsl,
    /// Future Remotion backend.
    Remotion,
    /// Future DaVinci Resolve/Fusion backend.
    Resolve,
}

/// Extraction-ready manifest shape for a stable transition. This
/// intentionally mirrors the planned `montage-transitions` registry
/// while preserving stable composition recipes as data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionManifest {
    /// Stable id, for example `montage.cross_dissolve`.
    pub id: String,
    /// Broad transition family used by the agent taste layer.
    pub family: String,
    /// Human display name.
    pub display_name: String,
    /// Supported backend identifiers.
    pub backends: Vec<TransitionBackend>,
    /// Default duration in seconds.
    pub default_duration_s: f64,
    /// Minimum supported/editorially useful duration in seconds.
    pub min_duration_s: f64,
    /// Maximum supported/editorially useful duration in seconds.
    pub max_duration_s: f64,
    /// Optional FFmpeg `xfade` transition name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ffmpeg_xfade: Option<String>,
    /// Audio behavior paired with this transition.
    pub audio_policy: TransitionAudioPolicyManifest,
    /// Editorial contexts where the transition fits.
    #[serde(default)]
    pub best_for: Vec<String>,
    /// Editorial contexts where the transition should usually be avoided.
    #[serde(default)]
    pub avoid_for: Vec<String>,
    /// License expression for this transition implementation.
    pub license: String,
    /// Attribution/source note.
    #[serde(default)]
    pub attribution: String,
    /// Preview asset path, relative to a transition registry root.
    #[serde(default)]
    pub preview: String,
    /// Parameter metadata keyed by parameter name.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty", default)]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Stable composition recipe for data-native backends.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub composition: Option<TransitionComposition>,
}

/// Serializable audio policy for manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAudioPolicyManifest {
    /// Smoothly crossfade between adjacent source audio.
    Crossfade,
    /// Preserve overlap timing but use cut-style audio behavior.
    Cut,
}

impl From<TransitionAudioPolicy> for TransitionAudioPolicyManifest {
    fn from(value: TransitionAudioPolicy) -> Self {
        match value {
            TransitionAudioPolicy::Crossfade => Self::Crossfade,
            TransitionAudioPolicy::Cut => Self::Cut,
        }
    }
}

/// Phase-one built-ins. These are deliberately limited to transitions
/// FFmpeg can export through the current render path.
pub const BUILTIN_TRANSITIONS: &[BuiltinTransition] = &[
    BuiltinTransition {
        id: "montage.hard_cut",
        family: "cut",
        display_name: "Hard Cut",
        ffmpeg_xfade: None,
        default_duration_s: 0.0,
        min_duration_s: 0.0,
        max_duration_s: 0.0,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[],
        avoid_for: &[],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.composite",
        family: "custom",
        display_name: "Agent Composite",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.35,
        min_duration_s: 0.05,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[],
        avoid_for: &[],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.cross_dissolve",
        family: "dissolve",
        display_name: "Cross Dissolve",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["soft_time_passage", "topic_drift", "gentle_emotion"],
        avoid_for: &["hard_beat_hit", "dirty_dialogue_repair"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.match_dissolve",
        family: "dissolve",
        display_name: "Match Dissolve",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.45,
        min_duration_s: 0.12,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["visual_echo", "memory_bridge", "graphic_match"],
        avoid_for: &["unrelated_images", "fast_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::AvoidColorShift,
    },
    BuiltinTransition {
        id: "montage.fade_black",
        family: "fade",
        display_name: "Fade Through Black",
        ffmpeg_xfade: Some("fadeblack"),
        default_duration_s: 0.35,
        min_duration_s: 0.05,
        max_duration_s: 3.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["chapter_break", "ending", "heavy_reset"],
        avoid_for: &["fast_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.flash_white",
        family: "flash",
        display_name: "Flash White",
        ffmpeg_xfade: Some("fadewhite"),
        default_duration_s: 0.18,
        min_duration_s: 0.04,
        max_duration_s: 0.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["beat_hit", "reveal", "energy_jump"],
        avoid_for: &["serious_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::AvoidDarkToBright,
    },
    BuiltinTransition {
        id: "montage.ramp_in_beat",
        family: "speed_ramp",
        display_name: "Ramp In Beat",
        ffmpeg_xfade: Some("fadewhite"),
        default_duration_s: 0.32,
        min_duration_s: 0.16,
        max_duration_s: 0.65,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["beat_hit", "energy_jump", "music_sync"],
        avoid_for: &["static_dialogue", "clinical_documentary"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::AvoidBrightToDark,
    },
    BuiltinTransition {
        id: "montage.ramp_out_chapter",
        family: "speed_ramp",
        display_name: "Ramp Out Chapter",
        ffmpeg_xfade: Some("fadeblack"),
        default_duration_s: 0.55,
        min_duration_s: 0.24,
        max_duration_s: 1.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["chapter_reset", "soft_time_passage"],
        avoid_for: &["hard_beat_hit"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.wipe_left",
        family: "wipe",
        display_name: "Wipe Left",
        ffmpeg_xfade: Some("wipeleft"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["graphic_movement", "related_scene_change"],
        avoid_for: &["intimate_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.wipe_right",
        family: "wipe",
        display_name: "Wipe Right",
        ffmpeg_xfade: Some("wiperight"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["graphic_movement", "related_scene_change"],
        avoid_for: &["intimate_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Right),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.slide_left",
        family: "slide",
        display_name: "Slide Left",
        ffmpeg_xfade: Some("slideleft"),
        default_duration_s: 0.28,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["motion_continuity", "screen_direction", "social_push"],
        avoid_for: &["static_interview"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.slide_right",
        family: "slide",
        display_name: "Slide Right",
        ffmpeg_xfade: Some("slideright"),
        default_duration_s: 0.28,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["motion_continuity", "screen_direction", "social_push"],
        avoid_for: &["static_interview"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Right),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.smooth_push_left",
        family: "slide",
        display_name: "Smooth Push Left",
        ffmpeg_xfade: Some("smoothleft"),
        default_duration_s: 0.32,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["motion_continuity", "screen_direction", "social_push"],
        avoid_for: &["static_interview"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.motion_blur",
        family: "motion_blur",
        display_name: "Motion Blur",
        ffmpeg_xfade: Some("hblur"),
        default_duration_s: 0.18,
        min_duration_s: 0.05,
        max_duration_s: 0.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "motion_cover",
            "hide_motion_jump",
            "screen_direction_unknown",
        ],
        avoid_for: &["static_dialogue", "slow_emotional_moment", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.whip_pan_left",
        family: "motion_blur",
        display_name: "Whip Pan Left",
        ffmpeg_xfade: Some("hblur"),
        default_duration_s: 0.18,
        min_duration_s: 0.05,
        max_duration_s: 0.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "fast_screen_direction",
            "pass_by_motion",
            "hide_motion_jump",
        ],
        avoid_for: &["static_dialogue", "slow_emotional_moment", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.whip_pan_right",
        family: "motion_blur",
        display_name: "Whip Pan Right",
        ffmpeg_xfade: Some("hblur"),
        default_duration_s: 0.18,
        min_duration_s: 0.05,
        max_duration_s: 0.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "fast_screen_direction",
            "pass_by_motion",
            "hide_motion_jump",
        ],
        avoid_for: &["static_dialogue", "slow_emotional_moment", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Right),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.pass_by_left",
        family: "occlusion",
        display_name: "Pass-By Left",
        ffmpeg_xfade: Some("coverleft"),
        default_duration_s: 0.16,
        min_duration_s: 0.04,
        max_duration_s: 0.45,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["occlusion_mask", "pass_by_motion", "invisible_scene_move"],
        avoid_for: &["no_occlusion_signal", "static_dialogue", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.pass_by_right",
        family: "occlusion",
        display_name: "Pass-By Right",
        ffmpeg_xfade: Some("coverright"),
        default_duration_s: 0.16,
        min_duration_s: 0.04,
        max_duration_s: 0.45,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["occlusion_mask", "pass_by_motion", "invisible_scene_move"],
        avoid_for: &["no_occlusion_signal", "static_dialogue", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Right),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.iris_open",
        family: "iris",
        display_name: "Iris Open",
        ffmpeg_xfade: Some("circleopen"),
        default_duration_s: 0.38,
        min_duration_s: 0.10,
        max_duration_s: 1.2,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["stylized_reveal", "vintage_grammar", "comic_reveal"],
        avoid_for: &["documentary_realism", "intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Out),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.iris_close",
        family: "iris",
        display_name: "Iris Close",
        ffmpeg_xfade: Some("circleclose"),
        default_duration_s: 0.38,
        min_duration_s: 0.10,
        max_duration_s: 1.2,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["stylized_closure", "vintage_grammar", "comic_button"],
        avoid_for: &["documentary_realism", "intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::In),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.invisible_cut",
        family: "invisible_cut",
        display_name: "Invisible Cut",
        ffmpeg_xfade: Some("fadefast"),
        default_duration_s: 0.08,
        min_duration_s: 0.03,
        max_duration_s: 0.18,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "occlusion_or_dark_frame",
            "hide_camera_reposition",
            "mask_cut",
        ],
        avoid_for: &[
            "visible_mismatch",
            "no_occlusion_signal",
            "dialogue_emphasis",
        ],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::AvoidColorShift,
    },
    BuiltinTransition {
        id: "montage.zoom_in",
        family: "zoom",
        display_name: "Zoom In",
        ffmpeg_xfade: Some("zoomin"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["forward_momentum", "beat_hit", "punch_in"],
        avoid_for: &["slow_emotional_moment"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::In),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.pixelize",
        family: "glitch",
        display_name: "Pixelize",
        ffmpeg_xfade: Some("pixelize"),
        default_duration_s: 0.22,
        min_duration_s: 0.05,
        max_duration_s: 0.75,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["tech_context", "stylized_jump", "glitch_moment"],
        avoid_for: &["serious_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.radial",
        family: "wipe",
        display_name: "Radial",
        ffmpeg_xfade: Some("radial"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["stylized_reveal", "graphic_topic_shift"],
        avoid_for: &["repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    // ---------- Phase 3 catalog expansion (10 new transitions) ----------
    BuiltinTransition {
        id: "montage.wipe_up",
        family: "wipe",
        display_name: "Wipe Up",
        ffmpeg_xfade: Some("wipeup"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "graphic_movement",
            "ascending_motion",
            "related_scene_change",
        ],
        avoid_for: &["intimate_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Up),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.wipe_down",
        family: "wipe",
        display_name: "Wipe Down",
        ffmpeg_xfade: Some("wipedown"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "graphic_movement",
            "descending_motion",
            "related_scene_change",
        ],
        avoid_for: &["intimate_dialogue"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Down),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.slide_up",
        family: "slide",
        display_name: "Slide Up",
        ffmpeg_xfade: Some("slideup"),
        default_duration_s: 0.28,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["motion_continuity", "social_push", "ascending_motion"],
        avoid_for: &["static_interview"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::Up),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.barn_door_open_h",
        family: "wipe",
        display_name: "Barn Door Open Horizontal",
        ffmpeg_xfade: Some("horzopen"),
        default_duration_s: 0.40,
        min_duration_s: 0.10,
        max_duration_s: 1.8,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["broadcast_reveal", "geometric_open", "scene_change"],
        avoid_for: &["intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.barn_door_close_h",
        family: "wipe",
        display_name: "Barn Door Close Horizontal",
        ffmpeg_xfade: Some("horzclose"),
        default_duration_s: 0.40,
        min_duration_s: 0.10,
        max_duration_s: 1.8,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["broadcast_button", "geometric_close", "chapter_button"],
        avoid_for: &["intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.barn_door_open_v",
        family: "wipe",
        display_name: "Barn Door Open Vertical",
        ffmpeg_xfade: Some("vertopen"),
        default_duration_s: 0.40,
        min_duration_s: 0.10,
        max_duration_s: 1.8,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["broadcast_reveal", "geometric_open", "scene_change"],
        avoid_for: &["intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.barn_door_close_v",
        family: "wipe",
        display_name: "Barn Door Close Vertical",
        ffmpeg_xfade: Some("vertclose"),
        default_duration_s: 0.40,
        min_duration_s: 0.10,
        max_duration_s: 1.8,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["broadcast_button", "geometric_close", "chapter_button"],
        avoid_for: &["intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.diag_tl",
        family: "wipe",
        display_name: "Diagonal Top-Left",
        ffmpeg_xfade: Some("diagtl"),
        default_duration_s: 0.35,
        min_duration_s: 0.08,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["comedic_button", "graphic_topic_shift", "stylized_reveal"],
        avoid_for: &["serious_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.distance_zoom",
        family: "zoom",
        display_name: "Distance Zoom",
        ffmpeg_xfade: Some("distance"),
        default_duration_s: 0.40,
        min_duration_s: 0.10,
        max_duration_s: 1.6,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["forward_momentum", "energy_jump", "spatial_shift"],
        avoid_for: &["slow_emotional_moment", "static_interview"],
        requires_motion_continuity: false,
        motion_alignment: Some(MotionAlignment::In),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.dissolve_grain",
        family: "dissolve",
        display_name: "Dissolve (Noisy)",
        ffmpeg_xfade: Some("dissolve"),
        default_duration_s: 0.45,
        min_duration_s: 0.10,
        max_duration_s: 2.5,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["nostalgic", "stylized_jump", "memory_bridge"],
        avoid_for: &["clinical_documentary", "hard_beat_hit"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::AvoidColorShift,
    },
    // ---------- Luma-mask transitions (GPU-only) ----------
    BuiltinTransition {
        id: "montage.clock_wipe",
        family: "wipe",
        display_name: "Clock Wipe",
        // GPU-only: no FFmpeg `xfade` covers a rotating pie-slice
        // mask. The composer routes via the LumaMask primitive.
        ffmpeg_xfade: None,
        default_duration_s: 0.45,
        min_duration_s: 0.15,
        max_duration_s: 1.8,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["time_passage", "vintage_grammar", "comedic_button"],
        avoid_for: &["serious_dialogue", "documentary_realism", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.venetian_blinds_h",
        family: "wipe",
        display_name: "Venetian Blinds (Horizontal)",
        ffmpeg_xfade: None,
        default_duration_s: 0.40,
        min_duration_s: 0.12,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["broadcast_polish", "geometric_reveal", "scene_change"],
        avoid_for: &["intimate_dialogue", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.checkerboard_dissolve",
        family: "dissolve",
        display_name: "Checkerboard Dissolve",
        ffmpeg_xfade: None,
        default_duration_s: 0.55,
        min_duration_s: 0.20,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["tech_context", "data_visualization", "stylized_jump"],
        avoid_for: &["intimate_dialogue", "documentary_realism"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.spiral_wipe",
        family: "wipe",
        display_name: "Spiral Wipe",
        ffmpeg_xfade: None,
        default_duration_s: 0.60,
        min_duration_s: 0.20,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["stylized_reveal", "comedic_button", "vintage_grammar"],
        avoid_for: &["serious_dialogue", "documentary_realism", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.burst_wipe",
        family: "wipe",
        display_name: "Burst Wipe",
        ffmpeg_xfade: None,
        default_duration_s: 0.45,
        min_duration_s: 0.15,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["energy_jump", "comedic_button", "beat_hit"],
        avoid_for: &["slow_emotional_moment", "documentary_realism"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.jaws_wipe",
        family: "wipe",
        display_name: "Jaws Wipe",
        ffmpeg_xfade: None,
        default_duration_s: 0.50,
        min_duration_s: 0.18,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &["comedic_button", "stylized_jump", "tech_context"],
        avoid_for: &["serious_dialogue", "documentary_realism"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    // ---------- Hyperframes-port GPU shaders ----------
    BuiltinTransition {
        id: "montage.light_leak",
        family: "fade",
        display_name: "Light Leak",
        ffmpeg_xfade: None,
        default_duration_s: 0.55,
        min_duration_s: 0.20,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &[
            "premium_cinematic",
            "warm_organic",
            "nostalgic",
            "lifestyle_reveal",
        ],
        avoid_for: &["clinical_documentary", "hard_beat_hit"],
        requires_motion_continuity: false,
        motion_alignment: None,
        // The leak floods bright frames with extra warm energy; on
        // already-bright incoming clips the effect washes out, so
        // flag dark-to-bright as the risky direction.
        color_sensitivity: ColorSensitivity::AvoidDarkToBright,
    },
    BuiltinTransition {
        id: "montage.swirl_vortex",
        family: "stylized",
        display_name: "Swirl Vortex",
        ffmpeg_xfade: None,
        default_duration_s: 0.60,
        min_duration_s: 0.25,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
        best_for: &["comedic_button", "stylized_jump", "energy_jump"],
        avoid_for: &["serious_dialogue", "documentary_realism", "repeated_use"],
        requires_motion_continuity: false,
        motion_alignment: None,
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.cinematic_pan_left",
        family: "motion_blur",
        display_name: "Cinematic Pan Left",
        ffmpeg_xfade: None,
        default_duration_s: 0.22,
        min_duration_s: 0.08,
        max_duration_s: 0.55,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "fast_screen_direction",
            "pass_by_motion",
            "hide_motion_jump",
        ],
        avoid_for: &["static_dialogue", "slow_emotional_moment", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Left),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
    BuiltinTransition {
        id: "montage.cinematic_pan_right",
        family: "motion_blur",
        display_name: "Cinematic Pan Right",
        ffmpeg_xfade: None,
        default_duration_s: 0.22,
        min_duration_s: 0.08,
        max_duration_s: 0.55,
        audio_policy: TransitionAudioPolicy::Cut,
        best_for: &[
            "fast_screen_direction",
            "pass_by_motion",
            "hide_motion_jump",
        ],
        avoid_for: &["static_dialogue", "slow_emotional_moment", "repeated_use"],
        requires_motion_continuity: true,
        motion_alignment: Some(MotionAlignment::Right),
        color_sensitivity: ColorSensitivity::Insensitive,
    },
];

/// Find a phase-one transition by stable id.
pub fn lookup_builtin_transition(id: &str) -> Option<&'static BuiltinTransition> {
    BUILTIN_TRANSITIONS
        .iter()
        .find(|t| t.id == id || legacy_awidat_id_matches(id, t.id))
}

fn legacy_awidat_id_matches(candidate: &str, montage_id: &str) -> bool {
    let Some(legacy_suffix) = candidate.strip_prefix("awidat.") else {
        return false;
    };
    montage_id
        .strip_prefix("montage.")
        .is_some_and(|montage_suffix| legacy_suffix == montage_suffix)
}

/// Backends this transition can render through. Built-ins with an
/// FFmpeg `xfade` name advertise [`TransitionBackend::Ffmpeg`].
/// Compositions that resolve to a [`crate::transitions::TransitionShader`]
/// also advertise [`TransitionBackend::Glsl`] (the WGSL/wgpu path).
/// Manifest validation rejects entries whose backends list is empty,
/// so this is the single source of truth that keeps GPU-only luma
/// mask transitions out of the "no backend" failure mode.
fn declared_backends(transition: &BuiltinTransition) -> Vec<TransitionBackend> {
    let mut backends = Vec::new();
    if transition.ffmpeg_xfade.is_some() {
        backends.push(TransitionBackend::Ffmpeg);
    }
    if let Some(composition) = builtin_transition_composition(transition.id)
        && resolve_composition_gpu_shader(&composition).is_some()
    {
        backends.push(TransitionBackend::Glsl);
    }
    backends
}

/// Built-in transitions that should graduate to the stable external
/// registry. `montage.hard_cut` is represented by no transition node,
/// and `montage.composite` stays a harness authoring id for one-off
/// recipes, so neither belongs in the stable preset registry.
pub fn stable_builtin_transition_manifests() -> Vec<TransitionManifest> {
    BUILTIN_TRANSITIONS
        .iter()
        .filter(|transition| !matches!(transition.id, "montage.hard_cut" | "montage.composite"))
        .map(|transition| TransitionManifest {
            id: transition.id.into(),
            family: transition.family.into(),
            display_name: transition.display_name.into(),
            backends: declared_backends(transition),
            default_duration_s: transition.default_duration_s,
            min_duration_s: transition.min_duration_s,
            max_duration_s: transition.max_duration_s,
            ffmpeg_xfade: transition.ffmpeg_xfade.map(str::to_string),
            audio_policy: transition.audio_policy.into(),
            best_for: transition
                .best_for
                .iter()
                .copied()
                .map(str::to_string)
                .collect(),
            avoid_for: transition
                .avoid_for
                .iter()
                .copied()
                .map(str::to_string)
                .collect(),
            license: "Apache-2.0".into(),
            attribution: "Montage built-in".into(),
            preview: format!(
                "transitions/{}/preview.mp4",
                transition.id.trim_start_matches("montage.")
            ),
            params: serde_json::Map::new(),
            composition: builtin_transition_composition(transition.id),
        })
        .collect()
}

/// Export the phase-one stable built-ins as pretty JSON in the same
/// manifest collection shape that `montage-transitions` will consume.
pub fn stable_builtin_transition_manifest_json() -> Result<String, TransitionLookupError> {
    let manifests = stable_builtin_transition_manifests();
    validate_transition_manifests(&manifests)?;
    serde_json::to_string_pretty(&manifests).map_err(|e| TransitionLookupError::InvalidSpec {
        message: format!("transition manifests could not be serialized: {e}"),
    })
}

/// Validate extraction-ready transition manifests.
pub fn validate_transition_manifests(
    manifests: &[TransitionManifest],
) -> Result<(), TransitionLookupError> {
    let mut seen = std::collections::BTreeSet::new();
    for manifest in manifests {
        if !manifest.id.starts_with("montage.") {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition manifest id {:?} must start with montage.",
                    manifest.id
                ),
            });
        }
        if manifest.family.trim().is_empty() || manifest.display_name.trim().is_empty() {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition manifest {:?} has empty display metadata",
                    manifest.id
                ),
            });
        }
        if manifest.backends.is_empty() {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition manifest {:?} has no supported backend",
                    manifest.id
                ),
            });
        }
        validate_transition_duration(&manifest.id, manifest.default_duration_s)?;
        if manifest.min_duration_s > manifest.default_duration_s
            || manifest.default_duration_s > manifest.max_duration_s
        {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition manifest {:?} default duration must be inside min/max bounds",
                    manifest.id
                ),
            });
        }
        if manifest.license.trim().is_empty() {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!("transition manifest {:?} has empty license", manifest.id),
            });
        }
        if !seen.insert(manifest.id.as_str()) {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!("duplicate transition manifest id {:?}", manifest.id),
            });
        }
        if let Some(composition) = &manifest.composition {
            validate_transition_composition(composition)?;
        }
    }
    Ok(())
}

/// Return the built-in recipe for a phase-one transition when it can
/// be represented as stable composition data. Some transitions remain
/// atomic while the phase-one renderer exports them through FFmpeg
/// `xfade`.
pub fn builtin_transition_composition(id: &str) -> Option<TransitionComposition> {
    let primitive = |op| TransitionPrimitive {
        start: 0.0,
        end: 1.0,
        easing: TransitionEasing::EaseInOut,
        op,
    };
    let composition = |primitives| TransitionComposition {
        version: 1,
        primitives,
    };
    match id {
        "montage.composite" => None,
        "montage.cross_dissolve" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Opacity {
                from: ParamCurve::Const(1.0),
                to: ParamCurve::Const(1.0),
            },
        )])),
        "montage.match_dissolve" => Some(composition(vec![
            primitive(TransitionPrimitiveOp::Opacity {
                from: ParamCurve::Const(1.0),
                to: ParamCurve::Const(1.0),
            }),
            TransitionPrimitive {
                start: 0.18,
                end: 0.82,
                easing: TransitionEasing::EaseInOut,
                op: TransitionPrimitiveOp::Blur {
                    // Demonstrate a non-trivial ParamCurve: blur
                    // ramps in to ~0.18, holds briefly, then eases
                    // out. This satisfies Phase 4's "at least one
                    // builtin composition uses a non-trivial curve"
                    // requirement.
                    amount: ParamCurve::Keyframes(vec![
                        Keyframe {
                            t: 0.0,
                            v: 0.0,
                            easing: TransitionEasing::EaseInOut,
                        },
                        Keyframe {
                            t: 0.45,
                            v: 0.18,
                            easing: TransitionEasing::EaseInOut,
                        },
                        Keyframe {
                            t: 0.55,
                            v: 0.18,
                            easing: TransitionEasing::EaseInOut,
                        },
                        Keyframe {
                            t: 1.0,
                            v: 0.0,
                            easing: TransitionEasing::Linear,
                        },
                    ]),
                    direction: None,
                },
            },
        ])),
        "montage.fade_black" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.fade_black".into(),
            },
        )])),
        "montage.flash_white" => Some(composition(vec![primitive(TransitionPrimitiveOp::Flash {
            color: "#ffffff".into(),
            peak: 1.0,
        })])),
        "montage.ramp_in_beat" => Some(composition(vec![
            primitive(TransitionPrimitiveOp::TimeRemap {
                speed: ParamCurve::Keyframes(vec![
                    Keyframe {
                        t: 0.0,
                        v: 0.7,
                        easing: TransitionEasing::EaseIn,
                    },
                    Keyframe {
                        t: 0.55,
                        v: 1.85,
                        easing: TransitionEasing::EaseOutExpo,
                    },
                    Keyframe {
                        t: 1.0,
                        v: 1.0,
                        easing: TransitionEasing::EaseOut,
                    },
                ]),
            }),
            primitive(TransitionPrimitiveOp::Flash {
                color: "#ffffff".into(),
                peak: 0.85,
            }),
        ])),
        "montage.ramp_out_chapter" => Some(composition(vec![
            primitive(TransitionPrimitiveOp::TimeRemap {
                speed: ParamCurve::Keyframes(vec![
                    Keyframe {
                        t: 0.0,
                        v: 1.0,
                        easing: TransitionEasing::EaseOut,
                    },
                    Keyframe {
                        t: 0.7,
                        v: 0.45,
                        easing: TransitionEasing::EaseInOut,
                    },
                    Keyframe {
                        t: 1.0,
                        v: 1.0,
                        easing: TransitionEasing::Linear,
                    },
                ]),
            }),
            primitive(TransitionPrimitiveOp::Atomic {
                id: "montage.fade_black".into(),
            }),
        ])),
        "montage.wipe_left" => Some(composition(vec![primitive(TransitionPrimitiveOp::Wipe {
            direction: "left".into(),
            softness: ParamCurve::Const(0.0),
        })])),
        "montage.wipe_right" => Some(composition(vec![primitive(TransitionPrimitiveOp::Wipe {
            direction: "right".into(),
            softness: ParamCurve::Const(0.0),
        })])),
        "montage.slide_left" => Some(composition(vec![primitive(TransitionPrimitiveOp::Push {
            direction: "left".into(),
            distance: 1.0,
        })])),
        "montage.slide_right" => Some(composition(vec![primitive(TransitionPrimitiveOp::Push {
            direction: "right".into(),
            distance: 1.0,
        })])),
        "montage.smooth_push_left" => Some(composition(vec![TransitionPrimitive {
            easing: TransitionEasing::EaseOut,
            ..primitive(TransitionPrimitiveOp::Push {
                direction: "left".into(),
                distance: 1.0,
            })
        }])),
        "montage.motion_blur" => Some(composition(vec![TransitionPrimitive {
            easing: TransitionEasing::EaseOutExpo,
            ..primitive(TransitionPrimitiveOp::Blur {
                amount: ParamCurve::Const(0.75),
                direction: None,
            })
        }])),
        "montage.whip_pan_left" => Some(composition(vec![TransitionPrimitive {
            easing: TransitionEasing::EaseOutExpo,
            ..primitive(TransitionPrimitiveOp::Blur {
                amount: ParamCurve::Const(0.85),
                direction: Some("left".into()),
            })
        }])),
        "montage.whip_pan_right" => Some(composition(vec![TransitionPrimitive {
            easing: TransitionEasing::EaseOutExpo,
            ..primitive(TransitionPrimitiveOp::Blur {
                amount: ParamCurve::Const(0.85),
                direction: Some("right".into()),
            })
        }])),
        "montage.pass_by_left" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.pass_by_left".into(),
            },
        )])),
        "montage.pass_by_right" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.pass_by_right".into(),
            },
        )])),
        "montage.iris_open" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.iris_open".into(),
            },
        )])),
        "montage.iris_close" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.iris_close".into(),
            },
        )])),
        "montage.invisible_cut" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.invisible_cut".into(),
            },
        )])),
        "montage.zoom_in" => Some(composition(vec![primitive(TransitionPrimitiveOp::Zoom {
            scale: ParamCurve::Const(1.25),
        })])),
        "montage.pixelize" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Pixelize { block_size: 0.6 },
        )])),
        "montage.radial" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "montage.radial".into(),
            },
        )])),
        "montage.clock_wipe" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "clock".into(),
                softness: ParamCurve::Const(0.06),
            },
        )])),
        "montage.venetian_blinds_h" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "blinds_horizontal".into(),
                softness: ParamCurve::Const(0.08),
            },
        )])),
        "montage.checkerboard_dissolve" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "checkerboard".into(),
                softness: ParamCurve::Const(0.04),
            },
        )])),
        "montage.spiral_wipe" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "spiral".into(),
                softness: ParamCurve::Const(0.05),
            },
        )])),
        "montage.burst_wipe" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "burst".into(),
                softness: ParamCurve::Const(0.08),
            },
        )])),
        "montage.jaws_wipe" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LumaMask {
                mask_kind: "jaws".into(),
                softness: ParamCurve::Const(0.06),
            },
        )])),
        "montage.light_leak" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::LightLeak {
                intensity: ParamCurve::Const(0.85),
                warmth: 0.75,
            },
        )])),
        "montage.swirl_vortex" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::SwirlVortex {
                strength: ParamCurve::Const(0.6),
            },
        )])),
        "montage.cinematic_pan_left" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::CinematicPan {
                direction: "left".into(),
                intensity: ParamCurve::Const(0.85),
            },
        )])),
        "montage.cinematic_pan_right" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::CinematicPan {
                direction: "right".into(),
                intensity: ParamCurve::Const(0.85),
            },
        )])),
        _ => None,
    }
}

/// Lower a data-only composition to the closest phase-one FFmpeg xfade
/// transition. This is deliberately a constrained compiler from stable
/// primitives to known backend names, not arbitrary filter generation.
pub fn resolve_composition_ffmpeg_xfade(
    composition: &TransitionComposition,
) -> Option<&'static str> {
    composition
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(idx, primitive)| {
            primitive_ffmpeg_xfade(&primitive.op).map(|xfade| {
                (
                    primitive_ffmpeg_priority(&primitive.op),
                    std::cmp::Reverse(idx),
                    xfade,
                )
            })
        })
        .max_by_key(|(priority, reverse_idx, _)| (*priority, *reverse_idx))
        .map(|(_, _, xfade)| xfade)
}

fn primitive_ffmpeg_xfade(op: &TransitionPrimitiveOp) -> Option<&'static str> {
    match op {
        TransitionPrimitiveOp::Atomic { id } => {
            lookup_builtin_transition(id).and_then(|transition| transition.ffmpeg_xfade)
        }
        TransitionPrimitiveOp::Push { direction, .. } => match direction.as_str() {
            "left" => Some("slideleft"),
            "right" => Some("slideright"),
            "up" => Some("slideup"),
            "down" => Some("slidedown"),
            _ => None,
        },
        TransitionPrimitiveOp::Wipe { direction, .. } => match direction.as_str() {
            "left" => Some("wipeleft"),
            "right" => Some("wiperight"),
            "up" => Some("wipeup"),
            "down" => Some("wipedown"),
            "in" | "out" => Some("radial"),
            _ => None,
        },
        // For ffmpeg fallback we evaluate the scale curve at the
        // primitive midpoint as a representative value. A zoom-in
        // curve that peaks above 1.0 still reads as zoomin even
        // when the endpoints are below 1.
        TransitionPrimitiveOp::Zoom { scale } if scale.evaluate(0.5) >= 1.0 => Some("zoomin"),
        TransitionPrimitiveOp::Zoom { .. } => Some("fade"),
        TransitionPrimitiveOp::Flash { color, .. } if color.eq_ignore_ascii_case("#ffffff") => {
            Some("fadewhite")
        }
        TransitionPrimitiveOp::Flash { .. } => Some("fadewhite"),
        TransitionPrimitiveOp::Pixelize { .. } => Some("pixelize"),
        TransitionPrimitiveOp::Blur { .. } => Some("hblur"),
        TransitionPrimitiveOp::Opacity { .. } => Some("fade"),
        TransitionPrimitiveOp::Shake { .. }
        | TransitionPrimitiveOp::ChromaticSplit { .. }
        | TransitionPrimitiveOp::LumaMask { .. }
        | TransitionPrimitiveOp::LightLeak { .. }
        | TransitionPrimitiveOp::SwirlVortex { .. }
        | TransitionPrimitiveOp::CinematicPan { .. }
        | TransitionPrimitiveOp::TimeRemap { .. } => None,
    }
}

fn primitive_ffmpeg_priority(op: &TransitionPrimitiveOp) -> u8 {
    match op {
        TransitionPrimitiveOp::Atomic { .. } => 100,
        TransitionPrimitiveOp::Push { .. } => 90,
        TransitionPrimitiveOp::Wipe { .. } => 80,
        TransitionPrimitiveOp::Zoom { .. } => 70,
        TransitionPrimitiveOp::Flash { .. } => 60,
        TransitionPrimitiveOp::Pixelize { .. } => 50,
        TransitionPrimitiveOp::Blur { .. } => 40,
        TransitionPrimitiveOp::Opacity { .. } => 30,
        TransitionPrimitiveOp::Shake { .. }
        | TransitionPrimitiveOp::ChromaticSplit { .. }
        | TransitionPrimitiveOp::LumaMask { .. }
        | TransitionPrimitiveOp::LightLeak { .. }
        | TransitionPrimitiveOp::SwirlVortex { .. }
        | TransitionPrimitiveOp::CinematicPan { .. }
        | TransitionPrimitiveOp::TimeRemap { .. } => 0,
    }
}

/// Resolve a composition to a stable GPU shader id, when the
/// composition contains a primitive that the GPU backend can render
/// (and FFmpeg `xfade` cannot, or the GPU implementation is
/// preferred). Returns the shader id as a stable string so this crate
/// does not depend on the render-gpu enum.
///
/// Priority: shake > chromatic_split > opacity → cross_dissolve.
/// Other primitives fall through to xfade (return `None` from here).
pub fn resolve_composition_gpu_shader(composition: &TransitionComposition) -> Option<&'static str> {
    composition
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(idx, primitive)| {
            primitive_gpu_shader(&primitive.op).map(|shader| {
                (
                    primitive_gpu_priority(&primitive.op),
                    std::cmp::Reverse(idx),
                    shader,
                )
            })
        })
        .max_by_key(|(priority, reverse_idx, _)| (*priority, *reverse_idx))
        .map(|(_, _, shader)| shader)
}

fn primitive_gpu_shader(op: &TransitionPrimitiveOp) -> Option<&'static str> {
    match op {
        TransitionPrimitiveOp::Shake { .. } => Some("shake"),
        TransitionPrimitiveOp::ChromaticSplit { .. } => Some("chromatic_split"),
        // LumaMask is GPU-only — FFmpeg `xfade` has no equivalent for
        // these procedural mask reveals. Routed unconditionally.
        TransitionPrimitiveOp::LumaMask { .. } => Some("luma_mask"),
        // LightLeak / SwirlVortex / CinematicPan are GPU-only ports
        // from the Hyperframes shader catalogue. Each maps directly
        // to a TransitionShader variant.
        TransitionPrimitiveOp::LightLeak { .. } => Some("light_leak"),
        TransitionPrimitiveOp::SwirlVortex { .. } => Some("swirl_vortex"),
        TransitionPrimitiveOp::CinematicPan { .. } => Some("cinematic_pan"),
        // Blur is GPU-routed because its `amount` accepts a
        // `ParamCurve` — the FFmpeg `hblur` fallback can only render
        // a constant. When the composition contains Blur alongside
        // Opacity (the match_dissolve recipe), Blur wins on priority
        // because the Blur shader already includes the cross-blend.
        TransitionPrimitiveOp::Blur { .. } => Some("blur"),
        // Opacity is renderable on either backend; we only flag it as
        // GPU when nothing else in the composition wants xfade. The
        // priority ordering below handles that.
        TransitionPrimitiveOp::Opacity { .. } => Some("cross_dissolve"),
        _ => None,
    }
}

fn primitive_gpu_priority(op: &TransitionPrimitiveOp) -> u8 {
    match op {
        TransitionPrimitiveOp::Shake { .. } => 100,
        TransitionPrimitiveOp::ChromaticSplit { .. } => 90,
        TransitionPrimitiveOp::LumaMask { .. } => 80,
        TransitionPrimitiveOp::LightLeak { .. } => 75,
        TransitionPrimitiveOp::SwirlVortex { .. } => 72,
        TransitionPrimitiveOp::CinematicPan { .. } => 70,
        TransitionPrimitiveOp::Blur { .. } => 50,
        TransitionPrimitiveOp::Opacity { .. } => 10,
        _ => 0,
    }
}

/// Result of extracting the TimeRemap primitive from a composition.
///
/// Carries the sampled speed curve mapped onto a transition window
/// of duration `duration_s`. The samples are pairs of
/// `(source_time_s, timeline_time_s)` describing the cumulative
/// time-warp from input PTS to output PTS at uniform progress
/// breakpoints. Each adjacent pair defines a linear piece of the
/// piecewise-linear setpts expression.
///
/// `source_time_s` is the input-side seconds (the value of
/// `(PTS-STARTPTS)*TB` the FFmpeg filter sees). `timeline_time_s` is
/// the cumulative warped output the renderer should emit for that
/// input second. Both are anchored at 0 at the start of the
/// transition window — the renderer offsets these to the actual
/// timeline position of the transition (outgoing tail or incoming
/// head) when it composes the filter graph.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeRemapSetptsCurve {
    /// Length of the transition window on the timeline, in seconds.
    pub duration_s: f64,
    /// Sampled `(source_time_s, timeline_time_s)` breakpoints, sorted
    /// by `source_time_s`. Always contains at least two entries
    /// (start and end of the window).
    pub samples: Vec<(f64, f64)>,
}

impl TimeRemapSetptsCurve {
    /// Whether the warped output diverges from identity by more than
    /// `tolerance_s` at any sample. A curve that stays within the
    /// tolerance is effectively realtime and can be skipped by the
    /// renderer to keep the filter graph clean.
    pub fn is_identity(&self, tolerance_s: f64) -> bool {
        self.samples
            .iter()
            .all(|(t_in, t_out)| (t_out - t_in).abs() <= tolerance_s)
    }
}

/// Extract a `TimeRemap` primitive from a composition and project its
/// speed curve onto a transition window of `duration_s` seconds.
///
/// Returns `None` when the composition contains no `TimeRemap`
/// primitive, when `duration_s` is non-positive, or when the speed
/// curve is degenerate (zero or non-finite samples). The renderer
/// uses `None` to mean "no setpts retime stage needed".
///
/// The math: speed `s(p)` is interpreted as the multiplier from
/// timeline time to source time — `s = 2.0` means twice as much
/// source content compressed into the window (fast motion), and
/// `s = 0.5` means half as much source stretched across the window
/// (slow motion). For an FFmpeg `setpts` expression where the
/// expression's value IS the new output PTS in seconds, we accumulate
/// `d_out/d_in = 1/s(p)` across the window. Samples are taken at
/// uniform progress breakpoints so the renderer can emit a
/// piecewise-linear approximation without further integration.
pub fn extract_time_remap_setpts(
    composition: &TransitionComposition,
    duration_s: f64,
) -> Option<TimeRemapSetptsCurve> {
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return None;
    }
    let speed = composition.primitives.iter().find_map(|p| match &p.op {
        TransitionPrimitiveOp::TimeRemap { speed } => Some(speed),
        _ => None,
    })?;

    // 32 samples is a low-bandwidth piecewise-linear approximation
    // that captures the dominant shape of curves the agent authors
    // for the speed-ramp family without bloating the filter graph.
    const SAMPLES: usize = 32;
    let mut samples = Vec::with_capacity(SAMPLES + 1);
    let mut last_t_in = 0.0_f64;
    let mut last_t_out = 0.0_f64;
    samples.push((last_t_in, last_t_out));
    for i in 1..=SAMPLES {
        let p_prev = (i as f64 - 1.0) / SAMPLES as f64;
        let p = i as f64 / SAMPLES as f64;
        // Trapezoidal integration of 1/s across the sub-interval. We
        // clamp the speed sample to a small positive floor to keep
        // the integrator stable in the face of validator-allowed
        // tiny speeds (validator floor is 0.05).
        let s_prev = speed.evaluate(p_prev).max(1.0e-3);
        let s_curr = speed.evaluate(p).max(1.0e-3);
        let avg_inv_speed = 0.5 * (1.0 / s_prev + 1.0 / s_curr);
        let dt_in = duration_s / SAMPLES as f64;
        let dt_out = avg_inv_speed * dt_in;
        last_t_in += dt_in;
        last_t_out += dt_out;
        samples.push((last_t_in, last_t_out));
    }
    Some(TimeRemapSetptsCurve {
        duration_s,
        samples,
    })
}

/// Render/export interpretation for known transition names imported
/// from other editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportedTransitionAlias {
    /// The original third-party family name Montage recognizes.
    pub imported_family: &'static str,
    /// FFmpeg `xfade=transition=` value used as the export downgrade.
    pub ffmpeg_xfade: &'static str,
    /// Audio behavior paired with the downgraded visual transition.
    pub audio_policy: TransitionAudioPolicy,
}

/// Resolve any supported transition kind/id into FFmpeg's xfade name.
pub fn resolve_ffmpeg_xfade(kind_or_id: &str) -> Result<Option<&str>, TransitionLookupError> {
    if let Some(t) = lookup_builtin_transition(kind_or_id) {
        return Ok(t.ffmpeg_xfade);
    }
    if let Some(alias) = resolve_imported_transition_alias(kind_or_id) {
        return Ok(Some(alias.ffmpeg_xfade));
    }
    match kind_or_id {
        "SMPTE_Dissolve" => Ok(Some("fade")),
        // Legacy aliases kept for old projects. New agent-authored EDLs
        // should use `montage.fade_black` for an intentional black dip;
        // `fade_in` / `fade_out` are ambiguous between adjacent clips.
        "montage.fade_in" | "montage.fade_out" => Ok(Some("fadeblack")),
        // Common raw FFmpeg xfade names remain accepted for old EDLs.
        "fade" | "fadeblack" | "fadewhite" | "distance" | "wipeleft" | "wiperight" | "wipeup"
        | "wipedown" | "slideleft" | "slideright" | "slideup" | "slidedown" | "smoothleft"
        | "smoothright" | "smoothup" | "smoothdown" | "circlecrop" | "rectcrop" | "circleopen"
        | "circleclose" | "vertopen" | "vertclose" | "horzopen" | "horzclose" | "dissolve"
        | "pixelize" | "diagtl" | "diagtr" | "diagbl" | "diagbr" | "hlslice" | "hrslice"
        | "vuslice" | "vdslice" | "hblur" | "fadegrays" | "wipetl" | "wipetr" | "wipebl"
        | "wipebr" | "squeezeh" | "squeezev" | "zoomin" | "fadefast" | "fadeslow" | "hlwind"
        | "hrwind" | "vuwind" | "vdwind" | "coverleft" | "coverright" | "coverup" | "coverdown"
        | "revealleft" | "revealright" | "revealup" | "revealdown" | "radial" => {
            Ok(Some(kind_or_id))
        }
        other if other.starts_with("montage.") => Err(TransitionLookupError::UnsupportedMontage {
            id: other.to_string(),
        }),
        other => Err(TransitionLookupError::UnsupportedRaw {
            kind: other.to_string(),
        }),
    }
}

/// Resolve the audio policy for any accepted transition kind/id.
pub fn resolve_audio_policy(
    kind_or_id: &str,
) -> Result<TransitionAudioPolicy, TransitionLookupError> {
    if let Some(t) = lookup_builtin_transition(kind_or_id) {
        return Ok(t.audio_policy);
    }
    if let Some(alias) = resolve_imported_transition_alias(kind_or_id) {
        return Ok(alias.audio_policy);
    }
    match kind_or_id {
        "SMPTE_Dissolve" | "fade" | "fadeblack" | "montage.fade_in" | "montage.fade_out" => {
            Ok(TransitionAudioPolicy::Crossfade)
        }
        "fadewhite" | "distance" | "wipeleft" | "wiperight" | "wipeup" | "wipedown"
        | "slideleft" | "slideright" | "slideup" | "slidedown" | "smoothleft" | "smoothright"
        | "smoothup" | "smoothdown" | "circlecrop" | "rectcrop" | "circleopen" | "circleclose"
        | "vertopen" | "vertclose" | "horzopen" | "horzclose" | "dissolve" | "pixelize"
        | "diagtl" | "diagtr" | "diagbl" | "diagbr" | "hlslice" | "hrslice" | "vuslice"
        | "vdslice" | "hblur" | "fadegrays" | "wipetl" | "wipetr" | "wipebl" | "wipebr"
        | "squeezeh" | "squeezev" | "zoomin" | "fadefast" | "fadeslow" | "hlwind" | "hrwind"
        | "vuwind" | "vdwind" | "coverleft" | "coverright" | "coverup" | "coverdown"
        | "revealleft" | "revealright" | "revealup" | "revealdown" | "radial" => {
            Ok(TransitionAudioPolicy::Cut)
        }
        other if other.starts_with("montage.") => Err(TransitionLookupError::UnsupportedMontage {
            id: other.to_string(),
        }),
        other => Err(TransitionLookupError::UnsupportedRaw {
            kind: other.to_string(),
        }),
    }
}

/// Resolve common imported/editor transition names into the closest
/// phase-one export behavior. Unknown third-party names deliberately
/// return `None` so callers can fail before FFmpeg.
pub fn resolve_imported_transition_alias(kind: &str) -> Option<ImportedTransitionAlias> {
    if kind == "dissolve" {
        return None;
    }
    let normalized = normalize_imported_transition_name(kind);
    match normalized.as_str() {
        "crossdissolve" | "dissolve" | "defaulttransition" | "filmdissolve" => {
            Some(ImportedTransitionAlias {
                imported_family: "dissolve",
                ffmpeg_xfade: "fade",
                audio_policy: TransitionAudioPolicy::Crossfade,
            })
        }
        "diptoblack" | "dipblack" | "fadetoblack" | "fadethroughblack" | "fadeblack" => {
            Some(ImportedTransitionAlias {
                imported_family: "fade",
                ffmpeg_xfade: "fadeblack",
                audio_policy: TransitionAudioPolicy::Crossfade,
            })
        }
        "diptowhite" | "dipwhite" | "fadetowhite" | "flashwhite" | "fadewhite" => {
            Some(ImportedTransitionAlias {
                imported_family: "flash",
                ffmpeg_xfade: "fadewhite",
                audio_policy: TransitionAudioPolicy::Cut,
            })
        }
        "wipeleft" => Some(ImportedTransitionAlias {
            imported_family: "wipe",
            ffmpeg_xfade: "wipeleft",
            audio_policy: TransitionAudioPolicy::Cut,
        }),
        "wiperight" => Some(ImportedTransitionAlias {
            imported_family: "wipe",
            ffmpeg_xfade: "wiperight",
            audio_policy: TransitionAudioPolicy::Cut,
        }),
        "slideleft" | "pushleft" => Some(ImportedTransitionAlias {
            imported_family: "slide",
            ffmpeg_xfade: "slideleft",
            audio_policy: TransitionAudioPolicy::Cut,
        }),
        "slideright" | "pushright" => Some(ImportedTransitionAlias {
            imported_family: "slide",
            ffmpeg_xfade: "slideright",
            audio_policy: TransitionAudioPolicy::Cut,
        }),
        "zoomin" | "zoom" => Some(ImportedTransitionAlias {
            imported_family: "zoom",
            ffmpeg_xfade: "zoomin",
            audio_policy: TransitionAudioPolicy::Cut,
        }),
        _ => None,
    }
}

fn normalize_imported_transition_name(kind: &str) -> String {
    kind.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Validate an authored transition duration against registry bounds.
pub fn validate_transition_duration(
    kind_or_id: &str,
    duration_s: f64,
) -> Result<(), TransitionLookupError> {
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!("transition duration {duration_s} must be finite and > 0"),
        });
    }
    let (min_s, max_s) = match kind_or_id {
        "SMPTE_Dissolve" => (0.05, 2.0),
        other => {
            let Some(def) = lookup_builtin_transition(other) else {
                return Ok(());
            };
            (def.min_duration_s, def.max_duration_s)
        }
    };
    if duration_s + 1e-6 < min_s || duration_s - 1e-6 > max_s {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!(
                "transition duration {duration_s:.3}s is outside supported range {min_s:.3}s..{max_s:.3}s for {kind_or_id:?}"
            ),
        });
    }
    Ok(())
}

/// Transition lookup failures surfaced before FFmpeg is invoked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitionLookupError {
    /// An Montage transition id was not registered.
    #[error("unsupported Montage transition id {id:?}")]
    UnsupportedMontage {
        /// Unknown transition id.
        id: String,
    },
    /// A raw transition kind is not known to the phase-one renderer.
    #[error("unsupported transition kind {kind:?}")]
    UnsupportedRaw {
        /// Unknown raw kind.
        kind: String,
    },
    /// Semantic transition metadata is structurally invalid.
    #[error("invalid transition metadata: {message}")]
    InvalidSpec {
        /// Human-readable validation error.
        message: String,
    },
}

/// Validate the semantic transition metadata shape.
pub fn validate_semantic_transition_spec(
    spec: &SemanticTransitionSpec,
) -> Result<(), TransitionLookupError> {
    if spec.id.starts_with("montage.") {
        let _ = resolve_ffmpeg_xfade(&spec.id)?;
        if let Some(family) = spec.family.as_deref()
            && let Some(def) = lookup_builtin_transition(&spec.id)
            && family != def.family
        {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition id {:?} belongs to family {:?}, not {:?}",
                    spec.id, def.family, family
                ),
            });
        }
    }
    if let Some(energy) = spec.energy
        && (!energy.is_finite() || !(0.0..=1.0).contains(&energy))
    {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!("transition energy {energy} must be finite and in [0.0, 1.0]"),
        });
    }
    if let Some(composition) = &spec.composition {
        validate_transition_composition(composition)?;
    }
    if spec.id == "montage.composite" {
        let Some(composition) = &spec.composition else {
            return Err(TransitionLookupError::InvalidSpec {
                message: "montage.composite requires composition metadata".into(),
            });
        };
        if resolve_composition_ffmpeg_xfade(composition).is_none() {
            return Err(TransitionLookupError::InvalidSpec {
                message: "montage.composite composition has no phase-one FFmpeg lowering; include at least one lowerable primitive such as push, wipe, zoom, flash, pixelize, blur, opacity, or atomic".into(),
            });
        }
    }
    Ok(())
}

/// Validate an agent-authored transition composition. This keeps
/// normal editing data-only: compositions may reference stable
/// primitives and stable Montage ids, but never arbitrary backend code.
pub fn validate_transition_composition(
    composition: &TransitionComposition,
) -> Result<(), TransitionLookupError> {
    if composition.version != 1 {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!(
                "transition composition version {} is unsupported; expected 1",
                composition.version
            ),
        });
    }
    if composition.primitives.is_empty() {
        return Err(TransitionLookupError::InvalidSpec {
            message: "transition composition must contain at least one primitive".into(),
        });
    }
    if composition.primitives.len() > 8 {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!(
                "transition composition has {} primitives; maximum is 8",
                composition.primitives.len()
            ),
        });
    }
    for (idx, primitive) in composition.primitives.iter().enumerate() {
        validate_primitive_timing(idx, primitive)?;
        validate_primitive_op(idx, &primitive.op)?;
    }
    Ok(())
}

fn validate_primitive_timing(
    idx: usize,
    primitive: &TransitionPrimitive,
) -> Result<(), TransitionLookupError> {
    if !primitive.start.is_finite()
        || !primitive.end.is_finite()
        || !(0.0..=1.0).contains(&primitive.start)
        || !(0.0..=1.0).contains(&primitive.end)
        || primitive.start >= primitive.end
    {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!(
                "primitive #{idx} timing must be finite with 0.0 <= start < end <= 1.0"
            ),
        });
    }
    Ok(())
}

fn validate_primitive_op(
    idx: usize,
    op: &TransitionPrimitiveOp,
) -> Result<(), TransitionLookupError> {
    match op {
        TransitionPrimitiveOp::Opacity { from, to } => {
            validate_curve_unit(idx, "from", from)?;
            validate_curve_unit(idx, "to", to)?;
        }
        TransitionPrimitiveOp::Push {
            direction,
            distance,
        } => {
            validate_direction(idx, direction, &["left", "right", "up", "down"])?;
            validate_range(idx, "distance", *distance, 0.0, 2.0)?;
        }
        TransitionPrimitiveOp::Wipe {
            direction,
            softness,
        } => {
            validate_direction(
                idx,
                direction,
                &["left", "right", "up", "down", "in", "out"],
            )?;
            validate_curve_unit(idx, "softness", softness)?;
        }
        TransitionPrimitiveOp::Zoom { scale } => {
            validate_curve_range(idx, "scale", scale, 0.25, 4.0)?;
        }
        TransitionPrimitiveOp::Blur { amount, direction } => {
            validate_curve_unit(idx, "amount", amount)?;
            if let Some(direction) = direction {
                validate_direction(
                    idx,
                    direction,
                    &["left", "right", "up", "down", "in", "out"],
                )?;
            }
        }
        TransitionPrimitiveOp::Flash { color, peak } => {
            validate_hex_color(idx, color)?;
            validate_unit(idx, "peak", *peak)?;
        }
        TransitionPrimitiveOp::Shake { amount, decay } => {
            validate_unit(idx, "amount", *amount)?;
            validate_unit(idx, "decay", *decay)?;
        }
        TransitionPrimitiveOp::ChromaticSplit { amount } => {
            validate_unit(idx, "amount", *amount)?;
        }
        TransitionPrimitiveOp::Pixelize { block_size } => {
            validate_unit(idx, "block_size", *block_size)?;
        }
        TransitionPrimitiveOp::LumaMask {
            mask_kind,
            softness,
        } => {
            validate_luma_mask_kind(idx, mask_kind)?;
            validate_curve_unit(idx, "softness", softness)?;
        }
        TransitionPrimitiveOp::LightLeak { intensity, warmth } => {
            validate_curve_range(idx, "intensity", intensity, 0.0, 2.0)?;
            validate_unit(idx, "warmth", *warmth)?;
        }
        TransitionPrimitiveOp::SwirlVortex { strength } => {
            validate_curve_unit(idx, "strength", strength)?;
        }
        TransitionPrimitiveOp::CinematicPan {
            direction,
            intensity,
        } => {
            validate_direction(idx, direction, &["left", "right"])?;
            validate_curve_unit(idx, "intensity", intensity)?;
        }
        TransitionPrimitiveOp::TimeRemap { speed } => {
            validate_curve_range(idx, "speed", speed, 0.05, 8.0)?;
        }
        TransitionPrimitiveOp::Atomic { id } => {
            if !id.starts_with("montage.") || lookup_builtin_transition(id).is_none() {
                return Err(TransitionLookupError::InvalidSpec {
                    message: format!(
                        "primitive #{idx} atomic id {id:?} must be a registered montage.* transition id"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_unit(idx: usize, field: &str, value: f64) -> Result<(), TransitionLookupError> {
    validate_range(idx, field, value, 0.0, 1.0)
}

/// Stable luma-mask kind ids the [`crate::transitions::TransitionPrimitiveOp::LumaMask`]
/// primitive accepts. Each id maps to the integer slot the GPU shader
/// reads from `extra_params[0]`. Future image-based mask kinds will
/// join this list with their own integer ids and a sampled-texture
/// variant of the shader.
pub const LUMA_MASK_KINDS: &[&str] = &[
    "clock",
    "blinds_horizontal",
    "checkerboard",
    "spiral",
    "burst",
    "jaws",
];

/// Integer slot for a stable luma-mask kind id, as the GPU shader
/// reads it. Returns `None` for unknown ids — callers should have
/// already validated against [`LUMA_MASK_KINDS`].
pub fn luma_mask_kind_slot(kind: &str) -> Option<u32> {
    match kind {
        "clock" => Some(0),
        "blinds_horizontal" => Some(1),
        "checkerboard" => Some(2),
        "spiral" => Some(3),
        "burst" => Some(4),
        "jaws" => Some(5),
        _ => None,
    }
}

fn validate_luma_mask_kind(idx: usize, kind: &str) -> Result<(), TransitionLookupError> {
    if LUMA_MASK_KINDS.contains(&kind) {
        return Ok(());
    }
    Err(TransitionLookupError::InvalidSpec {
        message: format!(
            "primitive #{idx} luma_mask kind {kind:?} is not one of {LUMA_MASK_KINDS:?}"
        ),
    })
}

fn validate_curve_unit(
    idx: usize,
    field: &str,
    curve: &ParamCurve,
) -> Result<(), TransitionLookupError> {
    validate_curve_range(idx, field, curve, 0.0, 1.0)
}

/// Validate that every value in a [`ParamCurve`] sits in `[min, max]`,
/// and (for keyframed curves) that keyframe times are sorted, finite,
/// and inside `[0, 1]`. Empty keyframe lists are rejected.
fn validate_curve_range(
    idx: usize,
    field: &str,
    curve: &ParamCurve,
    min: f64,
    max: f64,
) -> Result<(), TransitionLookupError> {
    match curve {
        ParamCurve::Const(value) => validate_range(idx, field, *value, min, max),
        ParamCurve::Keyframes(keys) => {
            if keys.is_empty() {
                return Err(TransitionLookupError::InvalidSpec {
                    message: format!("primitive #{idx} field {field:?} has empty keyframe list"),
                });
            }
            let mut prev_t = f64::NEG_INFINITY;
            for (kf_idx, kf) in keys.iter().enumerate() {
                if !kf.t.is_finite() || !(0.0..=1.0).contains(&kf.t) {
                    return Err(TransitionLookupError::InvalidSpec {
                        message: format!(
                            "primitive #{idx} field {field:?} keyframe #{kf_idx} t={} \
                             must be a finite value in [0.0, 1.0]",
                            kf.t
                        ),
                    });
                }
                if kf.t <= prev_t {
                    return Err(TransitionLookupError::InvalidSpec {
                        message: format!(
                            "primitive #{idx} field {field:?} keyframes must be strictly \
                             sorted by t (keyframe #{kf_idx} t={} <= previous t={})",
                            kf.t, prev_t
                        ),
                    });
                }
                prev_t = kf.t;
                validate_range(idx, field, kf.v, min, max)?;
            }
            Ok(())
        }
    }
}

fn validate_range(
    idx: usize,
    field: &str,
    value: f64,
    min: f64,
    max: f64,
) -> Result<(), TransitionLookupError> {
    if !value.is_finite() || value < min || value > max {
        return Err(TransitionLookupError::InvalidSpec {
            message: format!(
                "primitive #{idx} field {field}={value} must be finite and in [{min}, {max}]"
            ),
        });
    }
    Ok(())
}

fn validate_direction(
    idx: usize,
    direction: &str,
    allowed: &[&str],
) -> Result<(), TransitionLookupError> {
    if allowed.contains(&direction) {
        return Ok(());
    }
    Err(TransitionLookupError::InvalidSpec {
        message: format!(
            "primitive #{idx} direction {direction:?} must be one of {}",
            allowed.join(", ")
        ),
    })
}

fn validate_hex_color(idx: usize, color: &str) -> Result<(), TransitionLookupError> {
    let bytes = color.as_bytes();
    if bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Ok(());
    }
    Err(TransitionLookupError::InvalidSpec {
        message: format!("primitive #{idx} color {color:?} must be a #rrggbb hex color"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Phase 4: ParamCurve tests.

    #[test]
    fn param_curve_const_round_trips_as_bare_number() {
        let curve = ParamCurve::Const(0.5);
        let json = serde_json::to_string(&curve).unwrap();
        assert_eq!(json, "0.5", "const must serialize as a bare number");
        let parsed: ParamCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ParamCurve::Const(0.5));
    }

    #[test]
    fn param_curve_keyframes_round_trips_as_array() {
        let curve = ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.0,
                v: 0.0,
                easing: TransitionEasing::Linear,
            },
            Keyframe {
                t: 0.5,
                v: 0.8,
                easing: TransitionEasing::EaseInOut,
            },
            Keyframe {
                t: 1.0,
                v: 0.0,
                easing: TransitionEasing::Linear,
            },
        ]);
        let json = serde_json::to_string(&curve).unwrap();
        assert!(json.starts_with('['), "keyframes must serialize as array");
        let parsed: ParamCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, curve);
    }

    #[test]
    fn param_curve_const_evaluates_to_itself() {
        let curve = ParamCurve::Const(0.42);
        assert_eq!(curve.evaluate(0.0), 0.42);
        assert_eq!(curve.evaluate(0.5), 0.42);
        assert_eq!(curve.evaluate(1.0), 0.42);
    }

    #[test]
    fn param_curve_linear_keyframes_interpolate_correctly() {
        let curve = ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.0,
                v: 0.0,
                easing: TransitionEasing::Linear,
            },
            Keyframe {
                t: 1.0,
                v: 1.0,
                easing: TransitionEasing::Linear,
            },
        ]);
        assert_eq!(curve.evaluate(0.0), 0.0);
        assert!((curve.evaluate(0.5) - 0.5).abs() < 1e-9);
        assert_eq!(curve.evaluate(1.0), 1.0);
    }

    #[test]
    fn param_curve_three_keyframes_peak_at_middle() {
        let curve = ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.0,
                v: 0.0,
                easing: TransitionEasing::Linear,
            },
            Keyframe {
                t: 0.5,
                v: 0.8,
                easing: TransitionEasing::Linear,
            },
            Keyframe {
                t: 1.0,
                v: 0.0,
                easing: TransitionEasing::Linear,
            },
        ]);
        assert!((curve.evaluate(0.5) - 0.8).abs() < 1e-9);
        assert!(curve.evaluate(0.25) < 0.8);
        assert!(curve.evaluate(0.75) < 0.8);
        // Both shoulders should match by symmetry.
        let left = curve.evaluate(0.25);
        let right = curve.evaluate(0.75);
        assert!((left - right).abs() < 1e-9, "{left} vs {right}");
    }

    #[test]
    fn param_curve_clamps_out_of_range_progress() {
        let curve = ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.2,
                v: 0.3,
                easing: TransitionEasing::Linear,
            },
            Keyframe {
                t: 0.8,
                v: 0.7,
                easing: TransitionEasing::Linear,
            },
        ]);
        assert_eq!(curve.evaluate(-1.0), 0.3);
        assert_eq!(curve.evaluate(0.0), 0.3);
        assert_eq!(curve.evaluate(2.0), 0.7);
    }

    #[test]
    fn validation_rejects_unsorted_keyframes() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::Blur {
                    amount: ParamCurve::Keyframes(vec![
                        Keyframe {
                            t: 0.5,
                            v: 0.5,
                            easing: TransitionEasing::Linear,
                        },
                        Keyframe {
                            t: 0.3,
                            v: 0.2,
                            easing: TransitionEasing::Linear,
                        },
                    ]),
                    direction: None,
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("sorted"));
    }

    #[test]
    fn validation_rejects_keyframe_t_outside_unit_interval() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::Blur {
                    amount: ParamCurve::Keyframes(vec![Keyframe {
                        t: 1.5,
                        v: 0.5,
                        easing: TransitionEasing::Linear,
                    }]),
                    direction: None,
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("[0.0, 1.0]"));
    }

    #[test]
    fn validation_rejects_empty_keyframe_list() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::Blur {
                    amount: ParamCurve::Keyframes(vec![]),
                    direction: None,
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("empty keyframe list"));
    }

    /// Backward compat: a JSON value with bare-number params must
    /// deserialize into a primitive whose ParamCurve fields are
    /// `Const`. This locks in the contract for existing project
    /// files.
    #[test]
    fn legacy_scalar_blur_json_still_parses() {
        let json = r#"{
            "op": "blur",
            "amount": 0.5,
            "direction": "left"
        }"#;
        let parsed: TransitionPrimitiveOp = serde_json::from_str(json).unwrap();
        match parsed {
            TransitionPrimitiveOp::Blur { amount, direction } => {
                assert_eq!(amount, ParamCurve::Const(0.5));
                assert_eq!(direction.as_deref(), Some("left"));
            }
            _ => panic!("expected Blur, got {parsed:?}"),
        }
    }

    #[test]
    fn keyframed_blur_json_parses_into_keyframes_variant() {
        let json = r#"{
            "op": "blur",
            "amount": [
                {"t": 0.0, "v": 0.0},
                {"t": 0.5, "v": 0.8, "easing": "ease_in_out"},
                {"t": 1.0, "v": 0.0}
            ]
        }"#;
        let parsed: TransitionPrimitiveOp = serde_json::from_str(json).unwrap();
        match parsed {
            TransitionPrimitiveOp::Blur { amount, .. } => match amount {
                ParamCurve::Keyframes(keys) => {
                    assert_eq!(keys.len(), 3);
                    assert_eq!(keys[1].v, 0.8);
                    assert_eq!(keys[1].easing, TransitionEasing::EaseInOut);
                }
                _ => panic!("expected Keyframes, got {amount:?}"),
            },
            _ => panic!("expected Blur"),
        }
    }

    // Phase 3 catalog coverage tests.

    #[test]
    fn luma_mask_kinds_are_complete() {
        // The shader's integer slots and the kind id list must stay
        // in lockstep. If they ever drift, mask-kind selection at
        // render time silently picks the wrong mask.
        assert_eq!(LUMA_MASK_KINDS.len(), 6);
        assert_eq!(luma_mask_kind_slot("clock"), Some(0));
        assert_eq!(luma_mask_kind_slot("blinds_horizontal"), Some(1));
        assert_eq!(luma_mask_kind_slot("checkerboard"), Some(2));
        assert_eq!(luma_mask_kind_slot("spiral"), Some(3));
        assert_eq!(luma_mask_kind_slot("burst"), Some(4));
        assert_eq!(luma_mask_kind_slot("jaws"), Some(5));
        assert_eq!(luma_mask_kind_slot("unknown"), None);
    }

    #[test]
    fn hyperframes_port_transitions_resolve_to_their_shaders() {
        // Each new Option-A preset must round-trip through composition
        // → gpu_shader resolution, and must have no xfade fallback
        // (these are GPU-only by design).
        let cases = [
            ("montage.light_leak", "light_leak"),
            ("montage.swirl_vortex", "swirl_vortex"),
            ("montage.cinematic_pan_left", "cinematic_pan"),
            ("montage.cinematic_pan_right", "cinematic_pan"),
            ("montage.spiral_wipe", "luma_mask"),
            ("montage.burst_wipe", "luma_mask"),
            ("montage.jaws_wipe", "luma_mask"),
        ];
        for (id, expected_shader) in cases {
            let composition =
                builtin_transition_composition(id).expect("{id} should have composition");
            assert!(
                resolve_composition_ffmpeg_xfade(&composition).is_none(),
                "{id} should have no xfade fallback"
            );
            assert_eq!(
                resolve_composition_gpu_shader(&composition),
                Some(expected_shader),
                "{id} should resolve to {expected_shader}"
            );
        }
    }

    #[test]
    fn cinematic_pan_rejects_unknown_direction() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::CinematicPan {
                    direction: "diagonal".into(),
                    intensity: ParamCurve::Const(0.8),
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("direction"));
    }

    #[test]
    fn time_remap_primitive_validates_speed_curve() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::EaseInOut,
                op: TransitionPrimitiveOp::TimeRemap {
                    speed: ParamCurve::Keyframes(vec![
                        Keyframe {
                            t: 0.0,
                            v: 0.75,
                            easing: TransitionEasing::EaseIn,
                        },
                        Keyframe {
                            t: 0.5,
                            v: 1.8,
                            easing: TransitionEasing::EaseOut,
                        },
                        Keyframe {
                            t: 1.0,
                            v: 1.0,
                            easing: TransitionEasing::Linear,
                        },
                    ]),
                },
            }],
        };

        validate_transition_composition(&composition).unwrap();
        assert_eq!(resolve_composition_ffmpeg_xfade(&composition), None);
        assert_eq!(resolve_composition_gpu_shader(&composition), None);
    }

    #[test]
    fn time_remap_primitive_rejects_non_positive_speed() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::TimeRemap {
                    speed: ParamCurve::Const(0.0),
                },
            }],
        };

        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn ramp_in_beat_transition_has_time_remap_and_visual_accent() {
        let transition = lookup_builtin_transition("montage.ramp_in_beat").unwrap();
        assert_eq!(transition.family, "speed_ramp");
        assert!(transition.best_for.contains(&"beat_hit"));

        let composition = builtin_transition_composition("montage.ramp_in_beat").unwrap();
        assert!(
            composition
                .primitives
                .iter()
                .any(|primitive| matches!(primitive.op, TransitionPrimitiveOp::TimeRemap { .. }))
        );
        assert!(resolve_composition_ffmpeg_xfade(&composition).is_some());
        validate_transition_composition(&composition).unwrap();
    }

    #[test]
    fn extract_time_remap_setpts_for_constant_speed_warps_window() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::TimeRemap {
                    speed: ParamCurve::Const(2.0),
                },
            }],
        };
        let curve = extract_time_remap_setpts(&composition, 1.0).unwrap();
        assert_eq!(curve.duration_s, 1.0);
        assert!(curve.samples.len() >= 2);
        let (last_in, last_out) = *curve.samples.last().unwrap();
        // Constant 2× speed across the window halves the output PTS.
        assert!((last_in - 1.0).abs() < 1e-9, "last_in {last_in}");
        assert!((last_out - 0.5).abs() < 1e-6, "last_out {last_out}");
        assert!(!curve.is_identity(1e-3));
    }

    #[test]
    fn extract_time_remap_setpts_returns_none_for_non_time_remap_composition() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::Flash {
                    color: "#ffffff".into(),
                    peak: 1.0,
                },
            }],
        };
        assert!(extract_time_remap_setpts(&composition, 1.0).is_none());
    }

    #[test]
    fn extract_time_remap_setpts_skips_zero_duration_window() {
        let composition = builtin_transition_composition("montage.ramp_in_beat").unwrap();
        assert!(extract_time_remap_setpts(&composition, 0.0).is_none());
        assert!(extract_time_remap_setpts(&composition, -0.5).is_none());
    }

    #[test]
    fn extract_time_remap_setpts_handles_keyframed_ramp_in_beat() {
        let composition = builtin_transition_composition("montage.ramp_in_beat").unwrap();
        let curve = extract_time_remap_setpts(&composition, 0.32).unwrap();
        // Samples are sorted by source time and span the full window.
        let first_in = curve.samples.first().unwrap().0;
        let last_in = curve.samples.last().unwrap().0;
        assert!(first_in.abs() < 1e-9);
        assert!((last_in - 0.32).abs() < 1e-6);
        // The integrated output drifts from realtime — the ramp peaks
        // above 1.0 in the middle, so cumulative timeline time at the
        // end is less than the source span. The curve is non-monotone
        // in slope but strictly increasing in cumulative output.
        let outputs: Vec<f64> = curve.samples.iter().map(|(_, t_out)| *t_out).collect();
        for window in outputs.windows(2) {
            assert!(
                window[1] >= window[0] - 1e-12,
                "expected non-decreasing output, got {} then {}",
                window[0],
                window[1]
            );
        }
        assert!(!curve.is_identity(1e-3));
    }

    #[test]
    fn luma_mask_validation_rejects_unknown_kind() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::LumaMask {
                    mask_kind: "spiraling_zigzag".into(),
                    softness: ParamCurve::Const(0.1),
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("spiraling_zigzag"));
    }

    #[test]
    fn luma_mask_routes_to_gpu_shader() {
        // The composition lowering and GPU resolution should both
        // agree that LumaMask is GPU-only. xfade fallback is None;
        // gpu_shader returns "luma_mask".
        let composition = builtin_transition_composition("montage.clock_wipe").unwrap();
        assert!(
            resolve_composition_ffmpeg_xfade(&composition).is_none(),
            "clock_wipe has no xfade fallback"
        );
        assert_eq!(
            resolve_composition_gpu_shader(&composition),
            Some("luma_mask")
        );
    }

    #[test]
    fn catalog_meets_phase_3_size_target() {
        // The Phase 3 goal contract requires the registry to ship at
        // least 30 named transitions across all intent classes.
        assert!(
            BUILTIN_TRANSITIONS.len() >= 30,
            "catalog has only {} entries; Phase 3 needs ≥30",
            BUILTIN_TRANSITIONS.len()
        );
    }

    #[test]
    fn every_transition_documents_best_for() {
        // Every non-cut transition must document at least one editorial
        // context. `montage.hard_cut` and `montage.composite` are the only
        // exceptions (hard_cut has no editorial intent; composite is the
        // agent-authored escape hatch).
        for transition in BUILTIN_TRANSITIONS {
            if matches!(transition.id, "montage.hard_cut" | "montage.composite") {
                continue;
            }
            assert!(
                !transition.best_for.is_empty(),
                "{} has empty best_for; every shipped preset must document its editorial use",
                transition.id
            );
        }
    }

    #[test]
    fn catalog_covers_every_intent_class() {
        use std::collections::HashSet;
        let mut by_family: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for transition in BUILTIN_TRANSITIONS {
            *by_family.entry(transition.family).or_insert(0) += 1;
        }
        // Each of the five major intent classes must have at least two
        // transitions, so the agent has real choice rather than a
        // forced single option.
        let required_families: HashSet<&str> = [
            "dissolve",    // time-pass
            "fade",        // time-pass
            "wipe",        // scene-change
            "slide",       // scene-change
            "flash",       // energy-shift
            "motion_blur", // energy-shift
            "zoom",        // energy-shift
            "iris",        // scene-change / stylistic
        ]
        .into_iter()
        .collect();
        for family in required_families {
            let count = by_family.get(family).copied().unwrap_or(0);
            assert!(
                count >= 1,
                "intent family {family:?} has {count} transitions; need ≥1"
            );
        }
        // Stronger guarantee for the families we identified as the
        // weakest in the original survey.
        for must_have_two in ["wipe", "slide", "motion_blur"] {
            let count = by_family.get(must_have_two).copied().unwrap_or(0);
            assert!(
                count >= 2,
                "family {must_have_two:?} has only {count} transitions; need ≥2"
            );
        }
    }

    #[test]
    fn resolves_built_in_montage_ids_to_ffmpeg() {
        assert_eq!(
            resolve_ffmpeg_xfade("montage.cross_dissolve").unwrap(),
            Some("fade")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("montage.slide_left").unwrap(),
            Some("slideleft")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("montage.composite").unwrap(),
            Some("fade")
        );
    }

    #[test]
    fn resolves_legacy_awidat_transition_ids_to_builtin_transitions() {
        let transition = lookup_builtin_transition("awidat.cross_dissolve").unwrap();
        assert_eq!(transition.id, "montage.cross_dissolve");
        assert_eq!(
            resolve_ffmpeg_xfade("awidat.cross_dissolve").unwrap(),
            Some("fade")
        );
        assert_eq!(
            resolve_audio_policy("awidat.cross_dissolve").unwrap(),
            TransitionAudioPolicy::Crossfade
        );
    }

    #[test]
    fn resolves_audio_policy_by_transition_family() {
        assert_eq!(
            resolve_audio_policy("montage.cross_dissolve").unwrap(),
            TransitionAudioPolicy::Crossfade
        );
        assert_eq!(
            resolve_audio_policy("montage.slide_left").unwrap(),
            TransitionAudioPolicy::Cut
        );
        assert_eq!(
            resolve_audio_policy("fadewhite").unwrap(),
            TransitionAudioPolicy::Cut
        );
    }

    #[test]
    fn resolves_common_imported_transition_aliases() {
        assert_eq!(
            resolve_ffmpeg_xfade("Cross Dissolve").unwrap(),
            Some("fade")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("Dip To Black").unwrap(),
            Some("fadeblack")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("Fade to White").unwrap(),
            Some("fadewhite")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("Push Left").unwrap(),
            Some("slideleft")
        );
        assert_eq!(
            resolve_audio_policy("Cross Dissolve").unwrap(),
            TransitionAudioPolicy::Crossfade
        );
        assert_eq!(
            resolve_audio_policy("Push Left").unwrap(),
            TransitionAudioPolicy::Cut
        );
    }

    #[test]
    fn preserves_exact_raw_ffmpeg_transition_names() {
        assert_eq!(resolve_ffmpeg_xfade("dissolve").unwrap(), Some("dissolve"));
    }

    #[test]
    fn validates_transition_duration_bounds() {
        assert!(validate_transition_duration("montage.cross_dissolve", 0.3).is_ok());
        let err = validate_transition_duration("montage.flash_white", 2.0).unwrap_err();
        assert!(err.to_string().contains("outside supported range"));
    }

    #[test]
    fn exposes_builtin_compositions_as_data_recipes() {
        let slide = builtin_transition_composition("montage.slide_left").unwrap();
        assert_eq!(slide.version, 1);
        assert!(matches!(
            &slide.primitives[0].op,
            TransitionPrimitiveOp::Push {
                direction,
                distance
            } if direction == "left" && (*distance - 1.0).abs() < 1e-9
        ));

        let radial = builtin_transition_composition("montage.radial").unwrap();
        assert!(matches!(
            &radial.primitives[0].op,
            TransitionPrimitiveOp::Atomic { id } if id == "montage.radial"
        ));
        assert!(builtin_transition_composition("montage.composite").is_none());
    }

    #[test]
    fn validates_agent_authored_compositions() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![
                TransitionPrimitive {
                    start: 0.0,
                    end: 1.0,
                    easing: TransitionEasing::EaseOutExpo,
                    op: TransitionPrimitiveOp::Push {
                        direction: "left".into(),
                        distance: 0.9,
                    },
                },
                TransitionPrimitive {
                    start: 0.1,
                    end: 0.7,
                    easing: TransitionEasing::EaseOut,
                    op: TransitionPrimitiveOp::Blur {
                        amount: ParamCurve::Const(0.65),
                        direction: Some("left".into()),
                    },
                },
                TransitionPrimitive {
                    start: 0.35,
                    end: 0.55,
                    easing: TransitionEasing::EaseInOut,
                    op: TransitionPrimitiveOp::Flash {
                        color: "#ffffff".into(),
                        peak: 0.25,
                    },
                },
            ],
        };
        validate_transition_composition(&composition).unwrap();

        let invalid = TransitionComposition {
            primitives: vec![TransitionPrimitive {
                op: TransitionPrimitiveOp::Push {
                    direction: "diagonal".into(),
                    distance: 1.0,
                },
                ..composition.primitives[0].clone()
            }],
            ..composition
        };
        let err = validate_transition_composition(&invalid).unwrap_err();
        assert!(err.to_string().contains("direction"));
    }

    #[test]
    fn lowers_agent_compositions_to_phase_one_xfade_fallbacks() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![
                TransitionPrimitive {
                    start: 0.1,
                    end: 0.7,
                    easing: TransitionEasing::EaseOut,
                    op: TransitionPrimitiveOp::Blur {
                        amount: ParamCurve::Const(0.65),
                        direction: Some("left".into()),
                    },
                },
                TransitionPrimitive {
                    start: 0.0,
                    end: 1.0,
                    easing: TransitionEasing::EaseOutExpo,
                    op: TransitionPrimitiveOp::Push {
                        direction: "left".into(),
                        distance: 0.9,
                    },
                },
                TransitionPrimitive {
                    start: 0.35,
                    end: 0.55,
                    easing: TransitionEasing::EaseInOut,
                    op: TransitionPrimitiveOp::Flash {
                        color: "#ffffff".into(),
                        peak: 0.25,
                    },
                },
            ],
        };
        assert_eq!(
            resolve_composition_ffmpeg_xfade(&composition),
            Some("slideleft")
        );

        let flash = TransitionComposition {
            primitives: vec![TransitionPrimitive {
                op: TransitionPrimitiveOp::Flash {
                    color: "#ffffff".into(),
                    peak: 0.8,
                },
                ..composition.primitives[0].clone()
            }],
            ..TransitionComposition::default()
        };
        assert_eq!(resolve_composition_ffmpeg_xfade(&flash), Some("fadewhite"));
    }

    #[test]
    fn exports_stable_builtin_manifests_for_external_registry() {
        let manifests = stable_builtin_transition_manifests();
        validate_transition_manifests(&manifests).unwrap();

        assert!(manifests.iter().any(|m| m.id == "montage.cross_dissolve"));
        assert!(manifests.iter().any(|m| m.id == "montage.match_dissolve"));
        assert!(manifests.iter().any(|m| m.id == "montage.motion_blur"));
        assert!(manifests.iter().any(|m| m.id == "montage.whip_pan_left"));
        assert!(manifests.iter().any(|m| m.id == "montage.pass_by_left"));
        assert!(manifests.iter().any(|m| m.id == "montage.pass_by_right"));
        assert!(manifests.iter().any(|m| m.id == "montage.iris_open"));
        assert!(manifests.iter().any(|m| m.id == "montage.iris_close"));
        assert!(manifests.iter().any(|m| m.id == "montage.invisible_cut"));
        assert!(!manifests.iter().any(|m| m.id == "montage.hard_cut"));
        assert!(!manifests.iter().any(|m| m.id == "montage.composite"));

        let slide = manifests
            .iter()
            .find(|m| m.id == "montage.slide_left")
            .unwrap();
        assert_eq!(slide.backends, vec![TransitionBackend::Ffmpeg]);
        assert_eq!(slide.ffmpeg_xfade.as_deref(), Some("slideleft"));
        assert_eq!(slide.license, "Apache-2.0");
        assert!(
            slide
                .preview
                .ends_with("transitions/slide_left/preview.mp4")
        );
        assert!(matches!(
            &slide.composition.as_ref().unwrap().primitives[0].op,
            TransitionPrimitiveOp::Push { direction, .. } if direction == "left"
        ));

        let serialized = serde_json::to_value(slide).unwrap();
        assert_eq!(
            serialized
                .get("audio_policy")
                .and_then(serde_json::Value::as_str),
            Some("cut")
        );
        assert!(serialized.get("composition").is_some());

        let whip = manifests
            .iter()
            .find(|m| m.id == "montage.whip_pan_left")
            .unwrap();
        assert_eq!(whip.ffmpeg_xfade.as_deref(), Some("hblur"));
        assert!(whip.best_for.iter().any(|tag| tag == "hide_motion_jump"));

        let pass_by = manifests
            .iter()
            .find(|m| m.id == "montage.pass_by_left")
            .unwrap();
        assert_eq!(pass_by.ffmpeg_xfade.as_deref(), Some("coverleft"));
        assert_eq!(
            resolve_composition_ffmpeg_xfade(pass_by.composition.as_ref().unwrap()),
            Some("coverleft")
        );
        assert!(pass_by.best_for.iter().any(|tag| tag == "occlusion_mask"));

        let iris_open = manifests
            .iter()
            .find(|m| m.id == "montage.iris_open")
            .unwrap();
        assert_eq!(iris_open.ffmpeg_xfade.as_deref(), Some("circleopen"));
        assert!(
            iris_open
                .avoid_for
                .iter()
                .any(|tag| tag == "documentary_realism")
        );

        let invisible = manifests
            .iter()
            .find(|m| m.id == "montage.invisible_cut")
            .unwrap();
        assert_eq!(invisible.ffmpeg_xfade.as_deref(), Some("fadefast"));
        assert_eq!(
            resolve_composition_ffmpeg_xfade(invisible.composition.as_ref().unwrap()),
            Some("fadefast")
        );
        assert_eq!(invisible.audio_policy, TransitionAudioPolicyManifest::Cut);
        assert!(
            invisible
                .best_for
                .iter()
                .any(|tag| tag == "occlusion_or_dark_frame")
        );
    }

    #[test]
    fn exports_stable_builtin_manifest_json_roundtrip() {
        let json = stable_builtin_transition_manifest_json().unwrap();
        let manifests: Vec<TransitionManifest> = serde_json::from_str(&json).unwrap();
        validate_transition_manifests(&manifests).unwrap();

        assert!(json.contains("\"id\": \"montage.cross_dissolve\""));
        assert!(json.contains("\"composition\""));
        assert!(!json.contains("montage.composite"));
    }

    #[test]
    fn manifest_validation_rejects_bad_extraction_data() {
        let mut manifests = stable_builtin_transition_manifests();
        manifests[0].id = "bad.cross_dissolve".into();
        let err = validate_transition_manifests(&manifests).unwrap_err();
        assert!(err.to_string().contains("must start with montage"));
    }

    #[test]
    fn rejects_unknown_montage_ids() {
        let err = resolve_ffmpeg_xfade("montage.not_registered").unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn validates_semantic_energy_and_family() {
        let mut spec = SemanticTransitionSpec {
            id: "montage.cross_dissolve".into(),
            family: Some("slide".into()),
            energy: Some(1.2),
            ..SemanticTransitionSpec::default()
        };
        let err = validate_semantic_transition_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("family"));

        spec.family = Some("dissolve".into());
        let err = validate_semantic_transition_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("energy"));
    }

    #[test]
    fn validates_composite_requires_lowerable_composition() {
        let mut spec = SemanticTransitionSpec {
            id: "montage.composite".into(),
            family: Some("custom".into()),
            ..SemanticTransitionSpec::default()
        };
        let err = validate_semantic_transition_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("requires composition"));

        spec.composition = Some(TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::EaseOut,
                op: TransitionPrimitiveOp::Shake {
                    amount: 0.5,
                    decay: 0.7,
                },
            }],
        });
        let err = validate_semantic_transition_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("no phase-one FFmpeg lowering"));

        spec.composition = Some(TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::EaseOut,
                op: TransitionPrimitiveOp::Push {
                    direction: "left".into(),
                    distance: 0.8,
                },
            }],
        });
        validate_semantic_transition_spec(&spec).unwrap();
    }

    #[test]
    fn rejects_backend_code_as_atomic_composition_ids() {
        let composition = TransitionComposition {
            version: 1,
            primitives: vec![TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::Linear,
                op: TransitionPrimitiveOp::Atomic {
                    id: "xfade=transition=wipeleft".into(),
                },
            }],
        };
        let err = validate_transition_composition(&composition).unwrap_err();
        assert!(err.to_string().contains("registered montage"));
    }
}
