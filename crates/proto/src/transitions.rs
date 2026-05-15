//! Built-in transition registry and semantic transition metadata.
//!
//! This is the phase-one in-tree contract. A future
//! `awidat-transitions` package can replace the registry data behind
//! this shape without changing EDL, OTIO, or render callers.

use serde::{Deserialize, Serialize};

/// Metadata Awidat stores on OTIO `Transition.1` nodes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticTransitionSpec {
    /// Stable transition id, for example `awidat.cross_dissolve`.
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
    /// Optional structured recipe over Awidat's stable transition
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
    /// `awidat-transitions` package can version recipes independently
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

/// Stable primitive vocabulary for data-authored transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TransitionPrimitiveOp {
    /// Blend outgoing opacity to incoming opacity.
    Opacity {
        /// Outgoing opacity at primitive start.
        from: f64,
        /// Incoming opacity at primitive end.
        to: f64,
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
        /// Edge softness in `[0.0, 1.0]`.
        softness: f64,
    },
    /// Scale the image around its center.
    Zoom {
        /// End scale. `1.0` is unchanged.
        scale: f64,
    },
    /// Directional or isotropic blur.
    Blur {
        /// Blur amount in `[0.0, 1.0]`.
        amount: f64,
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
    /// Stable named atomic transition that does not decompose cleanly
    /// into primitives yet. This is still data: it points at an Awidat
    /// transition id, not backend code or a raw filter graph.
    Atomic {
        /// Stable Awidat transition id.
        id: String,
    },
}

/// Phase-one built-in transition definition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuiltinTransition {
    /// Stable Awidat id.
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
/// intentionally mirrors the planned `awidat-transitions` registry
/// while preserving stable composition recipes as data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionManifest {
    /// Stable id, for example `awidat.cross_dissolve`.
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
        id: "awidat.hard_cut",
        family: "cut",
        display_name: "Hard Cut",
        ffmpeg_xfade: None,
        default_duration_s: 0.0,
        min_duration_s: 0.0,
        max_duration_s: 0.0,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.composite",
        family: "custom",
        display_name: "Agent Composite",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.35,
        min_duration_s: 0.05,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.cross_dissolve",
        family: "dissolve",
        display_name: "Cross Dissolve",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 2.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
    },
    BuiltinTransition {
        id: "awidat.fade_black",
        family: "fade",
        display_name: "Fade Through Black",
        ffmpeg_xfade: Some("fadeblack"),
        default_duration_s: 0.35,
        min_duration_s: 0.05,
        max_duration_s: 3.0,
        audio_policy: TransitionAudioPolicy::Crossfade,
    },
    BuiltinTransition {
        id: "awidat.flash_white",
        family: "flash",
        display_name: "Flash White",
        ffmpeg_xfade: Some("fadewhite"),
        default_duration_s: 0.18,
        min_duration_s: 0.04,
        max_duration_s: 0.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.wipe_left",
        family: "wipe",
        display_name: "Wipe Left",
        ffmpeg_xfade: Some("wipeleft"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.wipe_right",
        family: "wipe",
        display_name: "Wipe Right",
        ffmpeg_xfade: Some("wiperight"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.slide_left",
        family: "slide",
        display_name: "Slide Left",
        ffmpeg_xfade: Some("slideleft"),
        default_duration_s: 0.28,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.slide_right",
        family: "slide",
        display_name: "Slide Right",
        ffmpeg_xfade: Some("slideright"),
        default_duration_s: 0.28,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.smooth_push_left",
        family: "slide",
        display_name: "Smooth Push Left",
        ffmpeg_xfade: Some("smoothleft"),
        default_duration_s: 0.32,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.zoom_in",
        family: "zoom",
        display_name: "Zoom In",
        ffmpeg_xfade: Some("zoomin"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.pixelize",
        family: "glitch",
        display_name: "Pixelize",
        ffmpeg_xfade: Some("pixelize"),
        default_duration_s: 0.22,
        min_duration_s: 0.05,
        max_duration_s: 0.75,
        audio_policy: TransitionAudioPolicy::Cut,
    },
    BuiltinTransition {
        id: "awidat.radial",
        family: "wipe",
        display_name: "Radial",
        ffmpeg_xfade: Some("radial"),
        default_duration_s: 0.30,
        min_duration_s: 0.05,
        max_duration_s: 1.5,
        audio_policy: TransitionAudioPolicy::Cut,
    },
];

/// Find a phase-one transition by stable id.
pub fn lookup_builtin_transition(id: &str) -> Option<&'static BuiltinTransition> {
    BUILTIN_TRANSITIONS.iter().find(|t| t.id == id)
}

/// Built-in transitions that should graduate to the stable external
/// registry. `awidat.hard_cut` is represented by no transition node,
/// and `awidat.composite` stays a harness authoring id for one-off
/// recipes, so neither belongs in the stable preset registry.
pub fn stable_builtin_transition_manifests() -> Vec<TransitionManifest> {
    BUILTIN_TRANSITIONS
        .iter()
        .filter(|transition| !matches!(transition.id, "awidat.hard_cut" | "awidat.composite"))
        .map(|transition| {
            let (best_for, avoid_for) = editorial_metadata(transition.id);
            TransitionManifest {
                id: transition.id.into(),
                family: transition.family.into(),
                display_name: transition.display_name.into(),
                backends: transition
                    .ffmpeg_xfade
                    .map(|_| vec![TransitionBackend::Ffmpeg])
                    .unwrap_or_default(),
                default_duration_s: transition.default_duration_s,
                min_duration_s: transition.min_duration_s,
                max_duration_s: transition.max_duration_s,
                ffmpeg_xfade: transition.ffmpeg_xfade.map(str::to_string),
                audio_policy: transition.audio_policy.into(),
                best_for: best_for.into_iter().map(str::to_string).collect(),
                avoid_for: avoid_for.into_iter().map(str::to_string).collect(),
                license: "Apache-2.0".into(),
                attribution: "Awidat built-in".into(),
                preview: format!(
                    "transitions/{}/preview.mp4",
                    transition.id.trim_start_matches("awidat.")
                ),
                params: serde_json::Map::new(),
                composition: builtin_transition_composition(transition.id),
            }
        })
        .collect()
}

/// Export the phase-one stable built-ins as pretty JSON in the same
/// manifest collection shape that `awidat-transitions` will consume.
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
        if !manifest.id.starts_with("awidat.") {
            return Err(TransitionLookupError::InvalidSpec {
                message: format!(
                    "transition manifest id {:?} must start with awidat.",
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

fn editorial_metadata(id: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    match id {
        "awidat.cross_dissolve" => (
            vec!["soft_time_passage", "topic_drift", "gentle_emotion"],
            vec!["hard_beat_hit"],
        ),
        "awidat.fade_black" => (
            vec!["chapter_break", "ending", "heavy_reset"],
            vec!["fast_dialogue"],
        ),
        "awidat.flash_white" => (
            vec!["beat_hit", "reveal", "energy_jump"],
            vec!["serious_dialogue"],
        ),
        "awidat.wipe_left" | "awidat.wipe_right" => (
            vec!["graphic_movement", "related_scene_change"],
            vec!["intimate_dialogue"],
        ),
        "awidat.slide_left" | "awidat.slide_right" | "awidat.smooth_push_left" => (
            vec!["motion_continuity", "screen_direction", "social_push"],
            vec!["static_interview"],
        ),
        "awidat.zoom_in" => (
            vec!["forward_momentum", "beat_hit", "punch_in"],
            vec!["slow_emotional_moment"],
        ),
        "awidat.pixelize" => (
            vec!["tech_context", "stylized_jump", "glitch_moment"],
            vec!["serious_dialogue"],
        ),
        "awidat.radial" => (
            vec!["stylized_reveal", "graphic_topic_shift"],
            vec!["repeated_use"],
        ),
        _ => (Vec::new(), Vec::new()),
    }
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
        "awidat.composite" => None,
        "awidat.cross_dissolve" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Opacity { from: 1.0, to: 1.0 },
        )])),
        "awidat.fade_black" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "awidat.fade_black".into(),
            },
        )])),
        "awidat.flash_white" => Some(composition(vec![primitive(TransitionPrimitiveOp::Flash {
            color: "#ffffff".into(),
            peak: 1.0,
        })])),
        "awidat.wipe_left" => Some(composition(vec![primitive(TransitionPrimitiveOp::Wipe {
            direction: "left".into(),
            softness: 0.0,
        })])),
        "awidat.wipe_right" => Some(composition(vec![primitive(TransitionPrimitiveOp::Wipe {
            direction: "right".into(),
            softness: 0.0,
        })])),
        "awidat.slide_left" => Some(composition(vec![primitive(TransitionPrimitiveOp::Push {
            direction: "left".into(),
            distance: 1.0,
        })])),
        "awidat.slide_right" => Some(composition(vec![primitive(TransitionPrimitiveOp::Push {
            direction: "right".into(),
            distance: 1.0,
        })])),
        "awidat.smooth_push_left" => Some(composition(vec![TransitionPrimitive {
            easing: TransitionEasing::EaseOut,
            ..primitive(TransitionPrimitiveOp::Push {
                direction: "left".into(),
                distance: 1.0,
            })
        }])),
        "awidat.zoom_in" => Some(composition(vec![primitive(TransitionPrimitiveOp::Zoom {
            scale: 1.25,
        })])),
        "awidat.pixelize" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Pixelize { block_size: 0.6 },
        )])),
        "awidat.radial" => Some(composition(vec![primitive(
            TransitionPrimitiveOp::Atomic {
                id: "awidat.radial".into(),
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
        TransitionPrimitiveOp::Zoom { scale } if *scale >= 1.0 => Some("zoomin"),
        TransitionPrimitiveOp::Zoom { .. } => Some("fade"),
        TransitionPrimitiveOp::Flash { color, .. } if color.eq_ignore_ascii_case("#ffffff") => {
            Some("fadewhite")
        }
        TransitionPrimitiveOp::Flash { .. } => Some("fadewhite"),
        TransitionPrimitiveOp::Pixelize { .. } => Some("pixelize"),
        TransitionPrimitiveOp::Blur { .. } => Some("hblur"),
        TransitionPrimitiveOp::Opacity { .. } => Some("fade"),
        TransitionPrimitiveOp::Shake { .. } | TransitionPrimitiveOp::ChromaticSplit { .. } => None,
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
        TransitionPrimitiveOp::Shake { .. } | TransitionPrimitiveOp::ChromaticSplit { .. } => 0,
    }
}

/// Render/export interpretation for known transition names imported
/// from other editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportedTransitionAlias {
    /// The original third-party family name Awidat recognizes.
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
        // should use `awidat.fade_black` for an intentional black dip;
        // `fade_in` / `fade_out` are ambiguous between adjacent clips.
        "awidat.fade_in" | "awidat.fade_out" => Ok(Some("fadeblack")),
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
        other if other.starts_with("awidat.") => Err(TransitionLookupError::UnsupportedAwidat {
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
        "SMPTE_Dissolve" | "fade" | "fadeblack" | "awidat.fade_in" | "awidat.fade_out" => {
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
        other if other.starts_with("awidat.") => Err(TransitionLookupError::UnsupportedAwidat {
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
    /// An Awidat transition id was not registered.
    #[error("unsupported Awidat transition id {id:?}")]
    UnsupportedAwidat {
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
    if spec.id.starts_with("awidat.") {
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
    if spec.id == "awidat.composite" {
        let Some(composition) = &spec.composition else {
            return Err(TransitionLookupError::InvalidSpec {
                message: "awidat.composite requires composition metadata".into(),
            });
        };
        if resolve_composition_ffmpeg_xfade(composition).is_none() {
            return Err(TransitionLookupError::InvalidSpec {
                message: "awidat.composite composition has no phase-one FFmpeg lowering; include at least one lowerable primitive such as push, wipe, zoom, flash, pixelize, blur, opacity, or atomic".into(),
            });
        }
    }
    Ok(())
}

/// Validate an agent-authored transition composition. This keeps
/// normal editing data-only: compositions may reference stable
/// primitives and stable Awidat ids, but never arbitrary backend code.
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
            validate_unit(idx, "from", *from)?;
            validate_unit(idx, "to", *to)?;
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
            validate_unit(idx, "softness", *softness)?;
        }
        TransitionPrimitiveOp::Zoom { scale } => {
            validate_range(idx, "scale", *scale, 0.25, 4.0)?;
        }
        TransitionPrimitiveOp::Blur { amount, direction } => {
            validate_unit(idx, "amount", *amount)?;
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
        TransitionPrimitiveOp::Atomic { id } => {
            if !id.starts_with("awidat.") || lookup_builtin_transition(id).is_none() {
                return Err(TransitionLookupError::InvalidSpec {
                    message: format!(
                        "primitive #{idx} atomic id {id:?} must be a registered awidat.* transition id"
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

    #[test]
    fn resolves_built_in_awidat_ids_to_ffmpeg() {
        assert_eq!(
            resolve_ffmpeg_xfade("awidat.cross_dissolve").unwrap(),
            Some("fade")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("awidat.slide_left").unwrap(),
            Some("slideleft")
        );
        assert_eq!(
            resolve_ffmpeg_xfade("awidat.composite").unwrap(),
            Some("fade")
        );
    }

    #[test]
    fn resolves_audio_policy_by_transition_family() {
        assert_eq!(
            resolve_audio_policy("awidat.cross_dissolve").unwrap(),
            TransitionAudioPolicy::Crossfade
        );
        assert_eq!(
            resolve_audio_policy("awidat.slide_left").unwrap(),
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
        assert!(validate_transition_duration("awidat.cross_dissolve", 0.3).is_ok());
        let err = validate_transition_duration("awidat.flash_white", 2.0).unwrap_err();
        assert!(err.to_string().contains("outside supported range"));
    }

    #[test]
    fn exposes_builtin_compositions_as_data_recipes() {
        let slide = builtin_transition_composition("awidat.slide_left").unwrap();
        assert_eq!(slide.version, 1);
        assert!(matches!(
            &slide.primitives[0].op,
            TransitionPrimitiveOp::Push {
                direction,
                distance
            } if direction == "left" && (*distance - 1.0).abs() < 1e-9
        ));

        let radial = builtin_transition_composition("awidat.radial").unwrap();
        assert!(matches!(
            &radial.primitives[0].op,
            TransitionPrimitiveOp::Atomic { id } if id == "awidat.radial"
        ));
        assert!(builtin_transition_composition("awidat.composite").is_none());
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
                        amount: 0.65,
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
                        amount: 0.65,
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

        assert!(manifests.iter().any(|m| m.id == "awidat.cross_dissolve"));
        assert!(!manifests.iter().any(|m| m.id == "awidat.hard_cut"));
        assert!(!manifests.iter().any(|m| m.id == "awidat.composite"));

        let slide = manifests
            .iter()
            .find(|m| m.id == "awidat.slide_left")
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
    }

    #[test]
    fn exports_stable_builtin_manifest_json_roundtrip() {
        let json = stable_builtin_transition_manifest_json().unwrap();
        let manifests: Vec<TransitionManifest> = serde_json::from_str(&json).unwrap();
        validate_transition_manifests(&manifests).unwrap();

        assert!(json.contains("\"id\": \"awidat.cross_dissolve\""));
        assert!(json.contains("\"composition\""));
        assert!(!json.contains("awidat.composite"));
    }

    #[test]
    fn manifest_validation_rejects_bad_extraction_data() {
        let mut manifests = stable_builtin_transition_manifests();
        manifests[0].id = "bad.cross_dissolve".into();
        let err = validate_transition_manifests(&manifests).unwrap_err();
        assert!(err.to_string().contains("must start with awidat"));
    }

    #[test]
    fn rejects_unknown_awidat_ids() {
        let err = resolve_ffmpeg_xfade("awidat.not_registered").unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn validates_semantic_energy_and_family() {
        let mut spec = SemanticTransitionSpec {
            id: "awidat.cross_dissolve".into(),
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
            id: "awidat.composite".into(),
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
        assert!(err.to_string().contains("registered awidat"));
    }
}
