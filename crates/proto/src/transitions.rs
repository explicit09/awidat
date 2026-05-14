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
        .filter(|c| c.is_ascii_alphanumeric())
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
    Ok(())
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
}
