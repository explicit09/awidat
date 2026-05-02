//! Strongly-typed `metadata.awidat` namespace.
//!
//! Per `PLAN.md` §3, the only schema extension Awidat makes to OTIO is the
//! `metadata.awidat` block. We model it strongly (NOT as `serde_json::Value`)
//! so unknown fields surface as parse errors against the schema we own.
//!
//! Three locations of awidat metadata exist:
//!
//! - On a [`crate::otio::Timeline`]'s `metadata`: see
//!   [`AwidatTimelineMetadata`].
//! - On a [`crate::otio::Clip`]'s `metadata`: see [`AwidatClipMetadata`].
//! - On a [`crate::otio::Marker`]'s `metadata`: see
//!   [`AwidatMarkerMetadata`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level awidat metadata on a `Timeline.metadata.awidat`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwidatTimelineMetadata {
    /// Awidat project format version, e.g. `"0.1"`.
    pub version: String,
    /// Source asset paths relative to the project root.
    #[serde(default)]
    pub source_assets: Vec<String>,
    /// Per-clip content anchors, keyed by clip UUID. The anchor lets
    /// `apply_edl` find a clip after upstream edits shift its timeline
    /// position. See `PLAN.md` §3 / §6.2.
    #[serde(default)]
    pub anchors: HashMap<String, Anchor>,
    /// Most recent `edit-plan.json` item id this timeline applied.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edit_plan_id: Option<String>,
}

/// Content anchor for a clip. Used by `apply_edl` to relocate a clip after
/// upstream edits drift its absolute timeline position.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Anchor {
    /// A snippet of transcript that uniquely identifies the clip.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transcript_snippet: Option<String>,
    /// Index into the source asset's shot-boundary list.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scene_change_index: Option<u32>,
    /// SHA of an audio fingerprint window — most robust anchor.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_fingerprint_sha: Option<String>,
    /// Hash of the energy curve over the clip range — useful when the
    /// transcript is sparse.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub energy_curve_hash: Option<String>,
}

/// Per-clip awidat metadata on a `Clip.metadata.awidat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatClipMetadata {
    /// The agent's reasoning for keeping / inserting this clip. Free text.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<String>,
    /// Cross-reference to an `edit-plan.json` item id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edit_plan_ref: Option<String>,
    /// Per-clip anchor — same shape as in [`AwidatTimelineMetadata::anchors`]
    /// but inlined for clips that prefer it on the clip object. Either
    /// location is allowed; tools should check both.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub anchor: Option<Anchor>,
}

/// Per-marker awidat metadata on a `Marker.metadata.awidat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatMarkerMetadata {
    /// Marker category, e.g. `"laugh"`, `"key-quote"`, `"b-roll-cue"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<String>,
    /// Free-form note attached to the marker.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_partial_roundtrip() {
        let a = Anchor {
            transcript_snippet: Some("hello".into()),
            scene_change_index: None,
            audio_fingerprint_sha: None,
            energy_curve_hash: None,
        };
        let s = serde_json::to_string(&a).unwrap();
        // Optional `None` fields should be omitted.
        assert!(!s.contains("scene_change_index"));
        let back: Anchor = serde_json::from_str(&s).unwrap();
        assert_eq!(back.transcript_snippet.as_deref(), Some("hello"));
    }

    #[test]
    fn timeline_metadata_roundtrip() {
        let m = AwidatTimelineMetadata {
            version: "0.1".into(),
            source_assets: vec!["raw/foo.mp4".into()],
            anchors: HashMap::new(),
            edit_plan_id: None,
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: AwidatTimelineMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version, "0.1");
        assert_eq!(back.source_assets.len(), 1);
    }
}
