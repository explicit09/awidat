//! OTIO node types: [`Timeline`], [`Stack`], [`Track`], [`Clip`], [`Gap`],
//! [`Effect`], [`Marker`], [`ExternalReference`], [`MissingReference`].
//!
//! Schema-discriminator handling
//! -----------------------------
//!
//! In OTIO JSON, every object carries an `OTIO_SCHEMA` field that names its
//! type and major version. There are three places where that appears in our
//! model:
//!
//! 1. **Standalone, never inside a tagged enum.** [`Timeline`], [`Effect`],
//!    [`Marker`] always serialize as themselves and own an explicit
//!    `otio_schema` field.
//! 2. **Inside a `#[serde(tag = "OTIO_SCHEMA")]` enum.** [`Track`], [`Clip`],
//!    [`Gap`], [`ExternalReference`], [`MissingReference`] only ever appear
//!    inside [`StackChild`], [`TrackChild`], or [`MediaReference`], so the
//!    enum's tag handles the schema string. They have no inline field.
//! 3. **Both.** [`Stack`] appears in [`StackChild`] / [`TrackChild`] AND
//!    standalone as [`Timeline::tracks`]. Its struct definition has no
//!    inline schema field; when used standalone we go through the
//!    [`stack_at_root`] (de)serialize helper.
//!
//! Schema-string forward-compat is handled at *load* time by
//! [`crate::project`] which rewrites known-name-unknown-major schemas to
//! the supported major before the typed deserialize runs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::awidat_meta::{AwidatClipMetadata, AwidatMarkerMetadata, AwidatTimelineMetadata};
use crate::error::JsonPath;
use crate::otio::time::{RationalTime, TimeRange};
use crate::ProtoError;

// --------- Timeline ---------------------------------------------------------

/// Top-level OTIO `Timeline.1`.
///
/// In our v1 superset, `tracks` is always a [`Stack`]; OTIO permits exactly
/// one stack as the timeline's root container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    /// Discriminator: `"Timeline.1"`. Always written; on read, see
    /// [`crate::project::read_otio_timeline`] for forward-compat handling.
    #[serde(rename = "OTIO_SCHEMA")]
    pub otio_schema: String,
    /// Display name.
    pub name: String,
    /// Optional global start time.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub global_start_time: Option<RationalTime>,
    /// Root stack of tracks. Stack-at-root needs custom (de)serialization
    /// because [`Stack`] itself omits its schema field (the field is only
    /// emitted at this position via [`stack_at_root`]).
    #[serde(with = "stack_at_root")]
    pub tracks: Stack,
    /// OTIO `metadata` blob; we model `awidat` strongly and keep the rest as
    /// a `serde_json::Value` map so we don't lose round-trip data from other
    /// tools.
    #[serde(default)]
    pub metadata: TimelineMetadata,
}

impl Timeline {
    /// Build a fresh empty timeline named `name` with no tracks.
    ///
    /// Seeds `metadata.awidat.version` with the current Awidat project format
    /// version (`PLAN.md` §3) so every fresh project carries a version pin
    /// from creation. Without this, schema migrations later have no anchor to
    /// distinguish "v0.1" from "unversioned legacy", and ambiguous projects
    /// will exist in the wild forever.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            otio_schema: "Timeline.1".to_string(),
            name: name.into(),
            global_start_time: None,
            tracks: Stack::empty("tracks"),
            metadata: TimelineMetadata {
                awidat: Some(crate::awidat_meta::AwidatTimelineMetadata {
                    version: crate::AWIDAT_PROJECT_VERSION.to_string(),
                    source_assets: Vec::new(),
                    anchors: std::collections::HashMap::new(),
                    edit_plan_id: None,
                }),
                other: HashMap::new(),
            },
        }
    }

    /// Validate this timeline. See [`crate::validate`] for the entry point.
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if let Some(t) = &self.global_start_time {
            t.validate(file, &path.field("global_start_time"))?;
        }
        self.tracks.validate(file, &path.field("tracks"))?;
        Ok(())
    }
}

/// Strongly-typed `metadata.awidat` block on a [`Timeline`], plus an opaque
/// passthrough for other namespaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineMetadata {
    /// Awidat-specific metadata. Strongly typed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub awidat: Option<AwidatTimelineMetadata>,
    /// Other tools' metadata. Preserved on round-trip.
    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

// --------- Stack ------------------------------------------------------------

/// OTIO `Stack.1`. A stack contains parallel children (e.g. multiple tracks).
/// Stacks may be nested (b-roll layered over interview, etc.).
///
/// Note: `Stack` has no inline `otio_schema` field. When it appears inside
/// a tagged enum, the enum's tag emits the schema; when it appears standalone
/// at [`Timeline::tracks`], the [`stack_at_root`] helper emits it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stack {
    /// Stack name.
    pub name: String,
    /// Children. v1 accepts tracks, nested stacks, clips, and gaps.
    #[serde(default)]
    pub children: Vec<StackChild>,
    /// Optional source range.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_range: Option<TimeRange>,
    /// Free metadata. (No strongly-typed awidat fields here in v1.)
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Stack {
    /// Empty stack with the given name.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            source_range: None,
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if let Some(r) = &self.source_range {
            r.validate(file, &path.field("source_range"))?;
        }
        for (i, child) in self.children.iter().enumerate() {
            child.validate(file, &path.field("children").index(i))?;
        }
        Ok(())
    }
}

/// Helper to (de)serialize [`Stack`] at root position with an explicit
/// `OTIO_SCHEMA: "Stack.1"` field.
mod stack_at_root {
    use super::Stack;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    pub fn serialize<S>(s: &Stack, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Re-encode into a JSON map with OTIO_SCHEMA prepended, then serialize
        // that. Relies on serde_json being capable of any Serializer; this
        // always runs against serde_json in our pipeline.
        let mut v = serde_json::to_value(s).map_err(serde::ser::Error::custom)?;
        if let Value::Object(map) = &mut v {
            map.insert(
                "OTIO_SCHEMA".to_string(),
                Value::String("Stack.1".to_string()),
            );
        }
        v.serialize(ser)
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Stack, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut v = Value::deserialize(de)?;
        if let Value::Object(map) = &mut v {
            map.remove("OTIO_SCHEMA");
        }
        serde_json::from_value(v).map_err(serde::de::Error::custom)
    }
}

/// What can sit inside a [`Stack`]: a track, a nested stack, a clip, or a
/// gap. The serde tag is `OTIO_SCHEMA` so it deserializes natively from
/// OTIO JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "OTIO_SCHEMA")]
pub enum StackChild {
    /// Track (audio or video).
    #[serde(rename = "Track.1")]
    Track(Track),
    /// Nested stack (e.g. b-roll layered over interview).
    #[serde(rename = "Stack.1")]
    Stack(Stack),
    /// Bare clip directly in the stack.
    #[serde(rename = "Clip.1")]
    Clip(Clip),
    /// Bare gap directly in the stack.
    #[serde(rename = "Gap.1")]
    Gap(Gap),
}

impl StackChild {
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        match self {
            Self::Track(t) => t.validate(file, path),
            Self::Stack(s) => s.validate(file, path),
            Self::Clip(c) => c.validate(file, path),
            Self::Gap(g) => g.validate(file, path),
        }
    }
}

// --------- Track ------------------------------------------------------------

/// OTIO `Track.1`. A track holds a sequential list of clips and gaps.
/// Always lives inside a [`StackChild::Track`] envelope; no inline schema
/// field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    /// Track name.
    pub name: String,
    /// Audio or video.
    pub kind: TrackKind,
    /// Children — clips and gaps in playback order.
    #[serde(default)]
    pub children: Vec<TrackChild>,
    /// Optional source range.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_range: Option<TimeRange>,
    /// Track-level metadata. v1 has no strongly-typed awidat fields here;
    /// kept as a free map for forward-compat.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Track {
    /// New empty track.
    pub fn empty(name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            name: name.into(),
            kind,
            children: Vec::new(),
            source_range: None,
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if let Some(r) = &self.source_range {
            r.validate(file, &path.field("source_range"))?;
        }
        for (i, child) in self.children.iter().enumerate() {
            child.validate(file, &path.field("children").index(i))?;
        }
        Ok(())
    }
}

/// Track kind. OTIO's underlying string is `"Video"` or `"Audio"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    /// Video track.
    Video,
    /// Audio track.
    Audio,
}

/// What can sit inside a [`Track`]: a clip, a gap, or a nested stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "OTIO_SCHEMA")]
pub enum TrackChild {
    /// A clip on the track.
    #[serde(rename = "Clip.1")]
    Clip(Clip),
    /// A gap on the track.
    #[serde(rename = "Gap.1")]
    Gap(Gap),
    /// A nested stack (multi-cam, picture-in-picture, etc.).
    #[serde(rename = "Stack.1")]
    Stack(Stack),
}

impl TrackChild {
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        match self {
            Self::Clip(c) => c.validate(file, path),
            Self::Gap(g) => g.validate(file, path),
            Self::Stack(s) => s.validate(file, path),
        }
    }
}

// --------- Clip -------------------------------------------------------------

/// OTIO `Clip.1`. A single piece of media on a track. Always lives inside a
/// [`TrackChild::Clip`] / [`StackChild::Clip`] envelope; no inline schema
/// field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    /// Clip name.
    pub name: String,
    /// Optional source range (subrange of the underlying media).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_range: Option<TimeRange>,
    /// Reference to the media file. Always present; may be a
    /// [`MissingReference`] when the agent is planning ahead.
    pub media_reference: MediaReference,
    /// Effects in stack order (innermost first).
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Markers attached to this clip.
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Clip-level metadata.
    #[serde(default)]
    pub metadata: ClipMetadata,
}

impl Clip {
    /// New empty clip with no media. The given name is used for the clip;
    /// `media_reference` defaults to a [`MissingReference`] so a freshly
    /// constructed clip is valid for the v1 "agent planned a b-roll insert
    /// but the asset isn't loaded yet" pattern.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_range: None,
            media_reference: MediaReference::Missing(MissingReference::default()),
            effects: Vec::new(),
            markers: Vec::new(),
            metadata: ClipMetadata::default(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if let Some(r) = &self.source_range {
            r.validate(file, &path.field("source_range"))?;
        }
        self.media_reference
            .validate(file, &path.field("media_reference"))?;
        for (i, e) in self.effects.iter().enumerate() {
            e.validate(file, &path.field("effects").index(i))?;
        }
        for (i, m) in self.markers.iter().enumerate() {
            m.validate(file, &path.field("markers").index(i))?;
        }
        Ok(())
    }
}

/// Strongly-typed clip metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipMetadata {
    /// Awidat-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub awidat: Option<AwidatClipMetadata>,
    /// Other tools' metadata, preserved on round-trip.
    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

// --------- Gap --------------------------------------------------------------

/// OTIO `Gap.1`. Empty space on a track. No inline schema field; lives
/// inside a tagged enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gap {
    /// Gap name (often empty in OTIO).
    #[serde(default)]
    pub name: String,
    /// Source range — `duration` is the gap length.
    pub source_range: TimeRange,
    /// Free metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Gap {
    /// New gap of the given duration at the given rate.
    pub fn of_duration(duration_seconds: f64, rate: f64) -> Self {
        Self {
            name: String::new(),
            source_range: TimeRange::new(
                RationalTime::zero(rate),
                RationalTime::new(duration_seconds * rate, rate),
            ),
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        self.source_range
            .validate(file, &path.field("source_range"))
    }
}

// --------- Media references -------------------------------------------------

/// OTIO media reference. v1 supports `ExternalReference` and
/// `MissingReference`; `GeneratorReference` and `ImageSequenceReference` are
/// out of scope per `OTIO_NOTES.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "OTIO_SCHEMA")]
pub enum MediaReference {
    /// External media file (the common case).
    #[serde(rename = "ExternalReference.1")]
    External(ExternalReference),
    /// Media that hasn't resolved yet (agent planned a b-roll insert but
    /// the asset isn't loaded). Per `PLAN.md` Concern B.
    #[serde(rename = "MissingReference.1")]
    Missing(MissingReference),
}

impl MediaReference {
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        match self {
            Self::External(e) => e.validate(file, path),
            Self::Missing(m) => m.validate(file, path),
        }
    }
}

/// OTIO `ExternalReference.1`. Points at a media file. No inline schema
/// field; lives inside [`MediaReference`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalReference {
    /// URL or relative path to the media. v1 usage: relative path from the
    /// project root, e.g. `"raw/ep-014-cam-a.mp4"`.
    pub target_url: String,
    /// Available range of the media (i.e. the whole asset's range).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub available_range: Option<TimeRange>,
    /// Free metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ExternalReference {
    /// Construct a reference to a target URL.
    pub fn new(target_url: impl Into<String>) -> Self {
        Self {
            target_url: target_url.into(),
            available_range: None,
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if self.target_url.is_empty() {
            return Err(ProtoError::validation(
                file,
                path.field("target_url"),
                "target_url must not be empty",
            ));
        }
        if let Some(r) = &self.available_range {
            r.validate(file, &path.field("available_range"))?;
        }
        Ok(())
    }
}

/// OTIO `MissingReference.1`. Used when the agent has planned a clip but
/// hasn't resolved the underlying media yet. No inline schema field; lives
/// inside [`MediaReference`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissingReference {
    /// Optional human-readable name describing the missing media.
    #[serde(default)]
    pub name: String,
    /// Optional intended available range.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub available_range: Option<TimeRange>,
    /// Free metadata. The agent often writes a hint here describing what
    /// asset it wants.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl MissingReference {
    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if let Some(r) = &self.available_range {
            r.validate(file, &path.field("available_range"))?;
        }
        Ok(())
    }
}

// --------- Effect -----------------------------------------------------------

/// OTIO `Effect.1` base type. v1 holds `effect_name` plus a free
/// `serde_json::Value` for parameters; specializations (color grade, time
/// warp, etc.) land in v1.5+ per `PLAN.md` Concern B.
///
/// Always serialized standalone (inside `Vec<Effect>` on a clip), so it
/// owns its `otio_schema` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Effect {
    /// Discriminator: `"Effect.1"`.
    #[serde(rename = "OTIO_SCHEMA")]
    pub otio_schema: String,
    /// Effect display name.
    #[serde(default)]
    pub name: String,
    /// Effect type identifier. Convention: `"awidat.<kind>"` for our own.
    pub effect_name: String,
    /// Effect parameters. Untyped to the engine; specialized inside whatever
    /// applies the effect.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Effect {
    /// Construct an Effect with the given `effect_name`.
    pub fn new(effect_name: impl Into<String>) -> Self {
        Self {
            otio_schema: "Effect.1".to_string(),
            name: String::new(),
            effect_name: effect_name.into(),
            metadata: serde_json::Value::Null,
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if self.effect_name.is_empty() {
            return Err(ProtoError::validation(
                file,
                path.field("effect_name"),
                "effect_name must not be empty",
            ));
        }
        Ok(())
    }
}

// --------- Marker -----------------------------------------------------------

/// OTIO `Marker.1`. Heavily used by Week 4 and the agent ("the laugh at
/// 4:12", "speaker mentioned Stripe here"). Always serialized standalone
/// (inside `Vec<Marker>` on a clip), so it owns its `otio_schema` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    /// Discriminator: `"Marker.1"`.
    #[serde(rename = "OTIO_SCHEMA")]
    pub otio_schema: String,
    /// Display name.
    pub name: String,
    /// Range of the timeline this marker covers. May be a point (zero
    /// duration).
    pub marked_range: TimeRange,
    /// Optional color string, free-form (e.g. `"RED"`, `"#ff8800"`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,
    /// Strongly-typed awidat metadata + flatten passthrough.
    #[serde(default)]
    pub metadata: MarkerMetadata,
}

impl Marker {
    /// New marker.
    pub fn new(name: impl Into<String>, marked_range: TimeRange) -> Self {
        Self {
            otio_schema: "Marker.1".to_string(),
            name: name.into(),
            marked_range,
            color: None,
            metadata: MarkerMetadata::default(),
        }
    }

    pub(crate) fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        self.marked_range
            .validate(file, &path.field("marked_range"))
    }
}

/// Strongly-typed marker metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarkerMetadata {
    /// Awidat-specific marker metadata.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub awidat: Option<AwidatMarkerMetadata>,
    /// Pass-through for other namespaces.
    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awidat_meta::Anchor;

    fn rt(value: f64, rate: f64) -> RationalTime {
        RationalTime::new(value, rate)
    }

    fn tr(start: f64, dur: f64, rate: f64) -> TimeRange {
        TimeRange::new(rt(start, rate), rt(dur, rate))
    }

    #[test]
    fn empty_timeline_roundtrips() {
        let t = Timeline::empty("ep-001");
        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "ep-001");
        assert_eq!(back.otio_schema, "Timeline.1");
    }

    #[test]
    fn empty_timeline_seeds_awidat_namespace() {
        // Per `PLAN.md` §3, `metadata.awidat.version` must be present from
        // creation so future schema migrations have an anchor.
        let t = Timeline::empty("ep-001");
        let awidat = t
            .metadata
            .awidat
            .as_ref()
            .expect("awidat namespace must be seeded by Timeline::empty");
        assert_eq!(awidat.version, crate::AWIDAT_PROJECT_VERSION);
        assert!(awidat.source_assets.is_empty());
        assert!(awidat.anchors.is_empty());

        // Also verify it's in the serialized JSON.
        let json = serde_json::to_string(&t).unwrap();
        assert!(
            json.contains("\"awidat\""),
            "fresh timeline JSON must contain awidat namespace, got: {json}"
        );
        assert!(
            json.contains(&format!(
                "\"version\":\"{}\"",
                crate::AWIDAT_PROJECT_VERSION
            )),
            "fresh timeline JSON must contain version pin, got: {json}"
        );
    }

    #[test]
    fn timeline_emits_stack_schema_at_root() {
        let t = Timeline::empty("ep");
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"OTIO_SCHEMA\":\"Timeline.1\""));
        // Stack at root must serialize with its own OTIO_SCHEMA so the JSON is
        // a valid OTIO file.
        assert!(json.contains("\"OTIO_SCHEMA\":\"Stack.1\""));
    }

    #[test]
    fn timeline_validates_clean() {
        let t = Timeline::empty("ep");
        t.validate("p.json", &JsonPath::root()).unwrap();
    }

    #[test]
    fn track_with_clip_roundtrips() {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/foo.mp4"));
        clip.source_range = Some(tr(0.0, 24.0, 24.0));

        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));

        let mut t = Timeline::empty("ep");
        t.tracks.children.push(StackChild::Track(track));

        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks.children.len(), 1);
    }

    #[test]
    fn nested_stack_roundtrips() {
        let inner = Stack::empty("interview");
        let mut outer = Stack::empty("root");
        outer.children.push(StackChild::Stack(inner));

        // Nest the outer stack inside a Timeline so we exercise the full
        // round-trip (outer goes through stack_at_root, inner goes through
        // the StackChild tag).
        let mut t = Timeline::empty("ep");
        t.tracks = outer;
        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks.children.len(), 1);
        match &back.tracks.children[0] {
            StackChild::Stack(s) => assert_eq!(s.name, "interview"),
            _ => panic!("expected nested Stack"),
        }
    }

    #[test]
    fn marker_with_awidat_meta_roundtrips() {
        let mut m = Marker::new("laugh-at-4-12", tr(99.0, 0.0, 24.0));
        m.color = Some("YELLOW".into());
        m.metadata.awidat = Some(AwidatMarkerMetadata {
            category: Some("laugh".into()),
            note: Some("biggest laugh in episode".into()),
        });
        let json = serde_json::to_string(&m).unwrap();
        let back: Marker = serde_json::from_str(&json).unwrap();
        assert_eq!(back.color.as_deref(), Some("YELLOW"));
        let am = back.metadata.awidat.unwrap();
        assert_eq!(am.category.as_deref(), Some("laugh"));
    }

    #[test]
    fn effect_serializes_with_schema() {
        let e = Effect::new("awidat.color_correction");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("Effect.1"));
        let back: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(back.effect_name, "awidat.color_correction");
    }

    #[test]
    fn missing_reference_round_trips_inside_media_reference() {
        let mr = MediaReference::Missing(MissingReference::default());
        let json = serde_json::to_string(&mr).unwrap();
        // Inside the enum, the tag injects OTIO_SCHEMA.
        assert!(json.contains("MissingReference.1"));
        let back: MediaReference = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, MediaReference::Missing(_)));
    }

    #[test]
    fn external_reference_rejects_empty_url() {
        let e = ExternalReference::new("");
        let err = e
            .validate("p.json", &JsonPath::root())
            .expect_err("empty target_url should fail");
        assert!(err.to_string().contains("target_url"));
    }

    #[test]
    fn awidat_anchors_roundtrip_in_timeline_metadata() {
        let mut t = Timeline::empty("ep");
        let mut anchors = HashMap::new();
        anchors.insert(
            "c-1".to_string(),
            Anchor {
                transcript_snippet: Some("hello world".into()),
                scene_change_index: Some(3),
                audio_fingerprint_sha: None,
                energy_curve_hash: None,
            },
        );
        t.metadata.awidat = Some(AwidatTimelineMetadata {
            version: crate::AWIDAT_PROJECT_VERSION.to_string(),
            source_assets: vec!["raw/ep.mp4".into()],
            anchors,
            edit_plan_id: Some("plan-001".into()),
        });
        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        let am = back.metadata.awidat.unwrap();
        assert_eq!(am.anchors.len(), 1);
        assert!(am.anchors.contains_key("c-1"));
    }

    #[test]
    fn effect_validate_rejects_empty_name() {
        let mut e = Effect::new("foo");
        e.effect_name.clear();
        e.validate("p.json", &JsonPath::root()).unwrap_err();
    }

    #[test]
    fn clip_with_marker_and_effect_roundtrips() {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/foo.mp4"));
        clip.markers.push(Marker::new("m", tr(0.0, 0.0, 24.0)));
        clip.effects.push(Effect::new("awidat.fade_in"));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut t = Timeline::empty("ep");
        t.tracks.children.push(StackChild::Track(track));
        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        // Drill in to confirm.
        let StackChild::Track(rt) = &back.tracks.children[0] else {
            panic!("expected Track")
        };
        let TrackChild::Clip(rc) = &rt.children[0] else {
            panic!("expected Clip")
        };
        assert_eq!(rc.markers.len(), 1);
        assert_eq!(rc.effects.len(), 1);
    }
}
