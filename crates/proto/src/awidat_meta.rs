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
///
/// Forward-compat: `version` is the only required field. All others have
/// `#[serde(default)]`, and unknown keys land in [`Self::extra`] (round-trip
/// preserved). Adding a new field in v1.5 is a non-breaking change as long as
/// the new field is optional or has a serde default — old engines reading
/// new files keep the value in `extra`, new engines reading old files use
/// the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatTimelineMetadata {
    /// Awidat project format version, e.g. `"0.1"`.
    #[serde(default)]
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
    /// Timeline-level broadcast overlay package. This is graph-native
    /// presentation state: render and desktop preview derive visible
    /// title cards, lower thirds, ticker, chapter cards, and host intro
    /// strips from this config instead of treating them as detached
    /// media files.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub broadcast_overlay: Option<BroadcastOverlayConfig>,
    /// Forward-compat passthrough. Future versions of this metadata can add
    /// fields here without breaking older readers. Engines that don't know
    /// about a field still round-trip it intact.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Reusable timeline-level broadcast overlay configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastOverlayConfig {
    /// Enable or disable rendering without deleting the saved preset.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional source template identifier for audit/debug UI.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub template_name: Option<String>,
    /// Episode title shown by the title card.
    #[serde(default)]
    pub episode_title: String,
    /// Optional subtitle shown under the title.
    #[serde(default)]
    pub episode_subtitle: String,
    /// Persistent show/brand name used by the ticker label.
    #[serde(default)]
    pub show_name: String,
    /// Left/primary host.
    #[serde(default)]
    pub host_a: BroadcastHost,
    /// Right/secondary host.
    #[serde(default)]
    pub host_b: BroadcastHost,
    /// Sponsor/brand names that scroll in the ticker.
    #[serde(default)]
    pub sponsors: Vec<String>,
    /// Timed topic labels used by the smart ticker.
    #[serde(default)]
    pub topics: Vec<BroadcastTimedEntry>,
    /// Timed chapter cards.
    #[serde(default)]
    pub chapters: Vec<BroadcastTimedEntry>,
    /// Optional project-relative logo path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brand_logo_path: Option<String>,
    /// Timing, colour, and layout style.
    #[serde(default)]
    pub style: BroadcastOverlayStyle,
}

impl Default for BroadcastOverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            template_name: None,
            episode_title: String::new(),
            episode_subtitle: String::new(),
            show_name: String::new(),
            host_a: BroadcastHost::default(),
            host_b: BroadcastHost::default(),
            sponsors: Vec::new(),
            topics: Vec::new(),
            chapters: Vec::new(),
            brand_logo_path: None,
            style: BroadcastOverlayStyle::default(),
        }
    }
}

/// Host data used by name bars and host intro strips.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BroadcastHost {
    /// Display name rendered in the lower-third.
    #[serde(default)]
    pub name: String,
    /// Subtitle / role rendered beside or below the name.
    #[serde(default)]
    pub title: String,
    /// Optional project-relative portrait path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub photo_path: Option<String>,
}

/// Timestamped chapter/topic label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastTimedEntry {
    /// Timeline time, in seconds.
    pub time_seconds: f64,
    /// Text displayed at that time.
    pub text: String,
}

/// Visual/timing defaults for the broadcast overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastOverlayStyle {
    /// Primary brand gold colour.
    #[serde(default = "default_gold_hex")]
    pub gold_hex: String,
    /// Lighter gold used by split host-intro bars.
    #[serde(default = "default_gold_light_hex")]
    pub gold_light_hex: String,
    /// Accent cyan used by topic badges.
    #[serde(default = "default_cyan_hex")]
    pub cyan_hex: String,
    /// Dark lower-third/ticker background colour.
    #[serde(default = "default_dark_navy_hex")]
    pub dark_navy_hex: String,
    /// End of title-card fade-in, in seconds.
    #[serde(default = "default_title_fade_in_end")]
    pub title_fade_in_end: f64,
    /// Start of title-card fade-out, in seconds.
    #[serde(default = "default_title_fade_out_start")]
    pub title_fade_out_start: f64,
    /// End of title-card visibility, in seconds.
    #[serde(default = "default_title_visible_end")]
    pub title_visible_end: f64,
    /// Start of host-intro lower-third, in seconds.
    #[serde(default = "default_host_intro_start")]
    pub host_intro_start: f64,
    /// End of host-intro lower-third, in seconds.
    #[serde(default = "default_host_intro_end")]
    pub host_intro_end: f64,
    /// Sponsor ticker display cadence, in seconds.
    #[serde(default = "default_ticker_sponsor_duration")]
    pub ticker_sponsor_duration: f64,
    /// Ticker fade duration, in seconds.
    #[serde(default = "default_ticker_fade_duration")]
    pub ticker_fade_duration: f64,
    /// Topic badge display duration, in seconds.
    #[serde(default = "default_ticker_topic_duration")]
    pub ticker_topic_duration: f64,
    /// Chapter-card display duration, in seconds.
    #[serde(default = "default_chapter_display_duration")]
    pub chapter_display_duration: f64,
    /// Persistent host-name bar height, in pixels at 3840x2160 reference.
    #[serde(default = "default_name_bar_height")]
    pub name_bar_height: f64,
    /// Bottom ticker height, in pixels at 3840x2160 reference.
    #[serde(default = "default_ticker_height")]
    pub ticker_height: f64,
    /// Host-intro strip height, in pixels at 3840x2160 reference.
    #[serde(default = "default_host_strip_height")]
    pub host_strip_height: f64,
}

impl Default for BroadcastOverlayStyle {
    fn default() -> Self {
        Self {
            gold_hex: default_gold_hex(),
            gold_light_hex: default_gold_light_hex(),
            cyan_hex: default_cyan_hex(),
            dark_navy_hex: default_dark_navy_hex(),
            title_fade_in_end: default_title_fade_in_end(),
            title_fade_out_start: default_title_fade_out_start(),
            title_visible_end: default_title_visible_end(),
            host_intro_start: default_host_intro_start(),
            host_intro_end: default_host_intro_end(),
            ticker_sponsor_duration: default_ticker_sponsor_duration(),
            ticker_fade_duration: default_ticker_fade_duration(),
            ticker_topic_duration: default_ticker_topic_duration(),
            chapter_display_duration: default_chapter_display_duration(),
            name_bar_height: default_name_bar_height(),
            ticker_height: default_ticker_height(),
            host_strip_height: default_host_strip_height(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_gold_hex() -> String {
    "#C9A028".into()
}
fn default_gold_light_hex() -> String {
    "#E8C040".into()
}
fn default_cyan_hex() -> String {
    "#22D3EE".into()
}
fn default_dark_navy_hex() -> String {
    "#070D17".into()
}
fn default_title_fade_in_end() -> f64 {
    1.5
}
fn default_title_fade_out_start() -> f64 {
    29.0
}
fn default_title_visible_end() -> f64 {
    30.0
}
fn default_host_intro_start() -> f64 {
    38.0
}
fn default_host_intro_end() -> f64 {
    92.0
}
fn default_ticker_sponsor_duration() -> f64 {
    25.0
}
fn default_ticker_fade_duration() -> f64 {
    1.0
}
fn default_ticker_topic_duration() -> f64 {
    14.0
}
fn default_chapter_display_duration() -> f64 {
    6.0
}
fn default_name_bar_height() -> f64 {
    150.0
}
fn default_ticker_height() -> f64 {
    200.0
}
fn default_host_strip_height() -> f64 {
    320.0
}

/// Content anchor for a clip. Used by `apply_edl` to relocate a clip after
/// upstream edits drift its absolute timeline position.
///
/// New anchor channels (e.g. v1.5's `face_embedding_sha`) slot in via
/// [`Self::extra`] without breaking older engines.
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
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
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
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
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
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_partial_roundtrip() {
        let a = Anchor {
            transcript_snippet: Some("hello".into()),
            ..Anchor::default()
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
            ..AwidatTimelineMetadata::default()
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: AwidatTimelineMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version, "0.1");
        assert_eq!(back.source_assets.len(), 1);
    }

    #[test]
    fn unknown_fields_round_trip_through_extra() {
        // A future engine adds a `taste_profile_id` field to the awidat
        // namespace. An older reader (this code) deserializes it into
        // `extra`, then serializes it back out unchanged.
        let json = serde_json::json!({
            "version": "0.1",
            "source_assets": [],
            "taste_profile_id": "tp-001",
            "fancy_new_field": { "nested": true }
        });
        let m: AwidatTimelineMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(
            m.extra.get("taste_profile_id").and_then(|v| v.as_str()),
            Some("tp-001")
        );
        // Round-trips intact.
        let out = serde_json::to_value(&m).unwrap();
        assert_eq!(out["taste_profile_id"], "tp-001");
        assert_eq!(out["fancy_new_field"]["nested"], true);
    }
}
