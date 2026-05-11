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
    },
    BuiltinTransition {
        id: "awidat.cross_dissolve",
        family: "dissolve",
        display_name: "Cross Dissolve",
        ffmpeg_xfade: Some("fade"),
        default_duration_s: 0.30,
    },
    BuiltinTransition {
        id: "awidat.fade_black",
        family: "fade",
        display_name: "Fade Through Black",
        ffmpeg_xfade: Some("fadeblack"),
        default_duration_s: 0.35,
    },
    BuiltinTransition {
        id: "awidat.flash_white",
        family: "flash",
        display_name: "Flash White",
        ffmpeg_xfade: Some("fadewhite"),
        default_duration_s: 0.18,
    },
    BuiltinTransition {
        id: "awidat.wipe_left",
        family: "wipe",
        display_name: "Wipe Left",
        ffmpeg_xfade: Some("wipeleft"),
        default_duration_s: 0.30,
    },
    BuiltinTransition {
        id: "awidat.wipe_right",
        family: "wipe",
        display_name: "Wipe Right",
        ffmpeg_xfade: Some("wiperight"),
        default_duration_s: 0.30,
    },
    BuiltinTransition {
        id: "awidat.slide_left",
        family: "slide",
        display_name: "Slide Left",
        ffmpeg_xfade: Some("slideleft"),
        default_duration_s: 0.28,
    },
    BuiltinTransition {
        id: "awidat.slide_right",
        family: "slide",
        display_name: "Slide Right",
        ffmpeg_xfade: Some("slideright"),
        default_duration_s: 0.28,
    },
    BuiltinTransition {
        id: "awidat.smooth_push_left",
        family: "slide",
        display_name: "Smooth Push Left",
        ffmpeg_xfade: Some("smoothleft"),
        default_duration_s: 0.32,
    },
    BuiltinTransition {
        id: "awidat.zoom_in",
        family: "zoom",
        display_name: "Zoom In",
        ffmpeg_xfade: Some("zoomin"),
        default_duration_s: 0.30,
    },
    BuiltinTransition {
        id: "awidat.pixelize",
        family: "glitch",
        display_name: "Pixelize",
        ffmpeg_xfade: Some("pixelize"),
        default_duration_s: 0.22,
    },
    BuiltinTransition {
        id: "awidat.radial",
        family: "wipe",
        display_name: "Radial",
        ffmpeg_xfade: Some("radial"),
        default_duration_s: 0.30,
    },
];

/// Find a phase-one transition by stable id.
pub fn lookup_builtin_transition(id: &str) -> Option<&'static BuiltinTransition> {
    BUILTIN_TRANSITIONS.iter().find(|t| t.id == id)
}

/// Resolve any supported transition kind/id into FFmpeg's xfade name.
pub fn resolve_ffmpeg_xfade(kind_or_id: &str) -> Result<Option<&str>, TransitionLookupError> {
    if let Some(t) = lookup_builtin_transition(kind_or_id) {
        return Ok(t.ffmpeg_xfade);
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
}

/// Validate the semantic transition metadata shape.
pub fn validate_semantic_transition_spec(
    spec: &SemanticTransitionSpec,
) -> Result<(), TransitionLookupError> {
    if spec.id.starts_with("awidat.") {
        let _ = resolve_ffmpeg_xfade(&spec.id)?;
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
    fn rejects_unknown_awidat_ids() {
        let err = resolve_ffmpeg_xfade("awidat.not_registered").unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }
}
