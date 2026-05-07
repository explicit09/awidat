//! Typed `EdlOp` shape — what the parser emits and what
//! `apply_to_timeline` consumes.
//!
//! Each variant identifies its target by an [`Anchor`] (transcript
//! snippet, clip uuid, or scene-change index — content, not absolute
//! timestamps; per `PLAN.md` §6.2 property 1).

use std::fmt;

use serde::{Deserialize, Serialize};

/// One change in an EDL envelope.
///
/// Uses `op` as the discriminator tag (not `kind`) because
/// `InsertTransition` has a `kind: String` field for the SMPTE name
/// which would collide with serde's internal-tag injection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EdlOp {
    /// Trim a clip's source range. `start` / `end` are absolute seconds
    /// into the clip's media.
    TrimClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// New start (seconds into the source media), if changing.
        start: Option<f64>,
        /// New end (seconds into the source media), if changing.
        end: Option<f64>,
    },
    /// Remove a clip outright. Subsequent clips don't shift — a Gap of
    /// the deleted clip's duration takes its place. (Use `Move Clip` to
    /// re-order; the model shouldn't rely on Delete-then-collapse.)
    DeleteClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
    },
    /// Split one clip into two at a timestamp. The original clip's
    /// `source_range` partitions: `[start..at_s)` becomes the left
    /// piece (keeps the original name + metadata), `[at_s..end)`
    /// becomes a new right piece (new name `<original>-b`). Both
    /// pieces share the same media reference.
    ///
    /// `at_s` is in seconds **into the source media**, not absolute
    /// timeline seconds. The agent's typical flow is: find_moment to
    /// get a transcript timestamp → that timestamp is already into
    /// the source → pass it as `at_s`.
    SplitClip {
        /// Anchor identifying the clip to split.
        anchor: Anchor,
        /// Cut point, in seconds into the source media. Must lie
        /// strictly inside the clip's source_range or the op fails.
        at_s: f64,
    },
    /// Reset / extend a previously-trimmed clip's source range. This
    /// is the inverse-direction op of `Trim Clip` — Trim can only
    /// narrow; Untrim can widen back out (toward the original media
    /// bounds). Required because the agent's most common failure mode
    /// is overshoot-then-recover: trim too aggressively, realize the
    /// kept content extends further, need to widen the source range
    /// before splitting.
    ///
    /// Both fields are optional; omitted fields keep the current
    /// source-range value. If the media reference declares an
    /// `available_range`, the new range is capped to it; otherwise
    /// the agent is trusted (OTIO round-trip validation catches the
    /// pathological case).
    UntrimClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// New source-range start in seconds, if widening backward.
        /// `None` keeps the current start.
        start: Option<f64>,
        /// New source-range end in seconds, if widening forward.
        /// `None` keeps the current end.
        end: Option<f64>,
    },
    /// Insert a fresh clip on a track from an asset on disk. The
    /// load-bearing op for *building* a timeline from raw assets —
    /// previous ops only mutate existing clips. Without this the
    /// agent has to bash-edit the OTIO file to start a project.
    ///
    /// Track is identified by name; if the named track doesn't exist
    /// the op creates it (Video kind by default — F2 will add an
    /// `audio: true` field). The new clip's media_reference is an
    /// `ExternalReference` to the asset; the source_range defaults
    /// to `[0, available_range.duration)` if the asset declares an
    /// available range, otherwise the agent must specify `start`/`end`.
    ///
    /// `at_position` controls insertion order in the track's children
    /// vec. Default = append (end of track).
    ///
    /// New clip's `name` defaults to `clip-N` where N is the new
    /// child's index. The model can override via the optional `name`
    /// field.
    InsertClip {
        /// Project-relative asset path, e.g. `"raw/clip-1.MOV"`.
        asset: String,
        /// Track name; created with Video kind if missing.
        track: String,
        /// Where in the track to insert. `None` = append.
        at_position: Option<usize>,
        /// Source-media start in seconds. Defaults to 0.
        start: Option<f64>,
        /// Source-media end in seconds. Defaults to the asset's
        /// available_range end (if known) or to start+1 (the agent
        /// is expected to know the duration via inspect_clip /
        /// list_assets first).
        end: Option<f64>,
        /// Optional clip name override.
        name: Option<String>,
    },
    /// Insert b-roll over an anchor moment. Currently F2; carried in the
    /// type so the parser/handler signatures stay stable.
    InsertBRoll {
        /// Where to insert.
        anchor: Anchor,
        /// Asset path (project-relative).
        asset: String,
        /// Duration in seconds.
        duration_s: f64,
        /// Where it sits relative to the existing clips.
        position: BRollPosition,
    },
    /// Move a clip to a new track position. F2.
    MoveClip {
        /// Source anchor.
        anchor: Anchor,
        /// 0-based index in the destination track's children.
        to_position: usize,
    },
    /// Insert a transition between two adjacent clips. F2.
    InsertTransition {
        /// Anchor pair: the transition lands between these two anchors.
        between: TransitionBetween,
        /// Transition kind, e.g. `"SMPTE_Dissolve"`.
        kind: String,
        /// Duration in seconds.
        duration_s: f64,
    },
    /// Set per-clip audio volume. `value` is a linear gain multiplier
    /// where `0.0` mutes the clip, `1.0` is unity (no change), and
    /// values above 1.0 amplify (clipping risk). The apply layer
    /// stamps an `awidat.volume` Effect on the clip; render emits
    /// `volume=<value>` on that segment's audio stream before concat.
    /// Re-applying replaces the existing effect rather than stacking.
    SetVolume {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Linear gain multiplier. Must be finite and `>= 0.0`.
        value: f64,
    },
    /// Set per-clip playback speed. `factor` rescales both video and
    /// audio: `2.0` plays at double speed (half the timeline length),
    /// `0.5` plays at half speed (double the timeline length). `1.0`
    /// is unity. Render emits `setpts=<1/factor>*PTS` on video and
    /// chains `atempo=` filters on audio (atempo's per-instance
    /// range is `[0.5, 2.0]`; factors outside that chain). The
    /// segment's contribution to the master timeline duration is
    /// `source_duration / factor`.
    SetSpeed {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Speed multiplier. Must be finite and `> 0.0`.
        factor: f64,
    },
    /// Insert a title overlay on the project's "Titles" track. The
    /// titles track auto-creates on first insert (Video kind, flagged
    /// via metadata.awidat.extra["track_role"] = "titles") so the
    /// render pipeline can route these clips into `drawtext` filters
    /// instead of trying to decode them as media.
    ///
    /// `start_s` / `end_s` are absolute timeline-time seconds — when
    /// the overlay should appear and disappear over the underlying
    /// composition. The synthesized title clip carries an
    /// `awidat.title` Effect whose metadata holds all the styling
    /// fields below.
    InsertTitle {
        /// Title appears at this timeline-time, in seconds.
        start_s: f64,
        /// Title disappears at this timeline-time, in seconds.
        /// Must be `> start_s`.
        end_s: f64,
        /// Text to display.
        text: String,
        /// Vertical band on the frame.
        position: TitlePosition,
        /// Font size in pixels (rendered against a 1080p reference
        /// frame; ffmpeg scales proportionally).
        font_size: u32,
        /// Hex colour string like `"#FFFFFF"`. Validation deferred
        /// to ffmpeg's `drawtext` filter at render time.
        color: String,
        /// Bold vs normal weight.
        font_weight: TitleWeight,
        /// Entry / exit animation.
        animation: TitleAnimation,
    },
    /// Update an existing title overlay's styling. Anchored by the
    /// title clip's uuid (every InsertTitle stamps one). All fields
    /// are optional — only non-None fields update.
    SetTitle {
        /// Anchor identifying the title clip.
        anchor: Anchor,
        /// New start timestamp, if changing.
        start_s: Option<f64>,
        /// New end timestamp, if changing.
        end_s: Option<f64>,
        /// New text, if changing.
        text: Option<String>,
        /// New position, if changing.
        position: Option<TitlePosition>,
        /// New font size, if changing.
        font_size: Option<u32>,
        /// New colour, if changing.
        color: Option<String>,
        /// New weight, if changing.
        font_weight: Option<TitleWeight>,
        /// New animation, if changing.
        animation: Option<TitleAnimation>,
    },
}

/// Where on the frame a title sits. Render maps these to proportional
/// `y=` expressions (`h*0.05` for top, etc) so they survive resolution
/// changes without hard-coded pixel positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlePosition {
    /// Near the top edge.
    Top,
    /// Vertically centered.
    Center,
    /// Near the bottom edge.
    Bottom,
}

/// Font weight for title rendering. v1 ships only normal + bold;
/// custom weights would need a richer font bundling story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleWeight {
    /// Regular weight.
    Normal,
    /// Bold weight.
    Bold,
}

/// Entry / exit animation for a title. v1 lands fade variants;
/// slide variants are wired in 16.4 if time permits and gated behind
/// careful x/y expression construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleAnimation {
    /// No animation — title pops on at start_s and off at end_s.
    None,
    /// Fade in over the leading 500ms; full alpha thereafter.
    FadeIn,
    /// Full alpha until the trailing 500ms; fade to zero.
    FadeOut,
    /// Fade in at start_s, fade out at end_s.
    FadeInOut,
    /// Slide in from off-screen on the side matching the position.
    SlideIn,
    /// Slide out off-screen on the side matching the position.
    SlideOut,
}

/// Where to anchor a `Trim`/`Delete`/`Move`/etc.
///
/// Uses `anchor_kind` as the discriminator tag instead of `kind` to avoid
/// collision with [`EdlOp`]'s own `kind` tag when an op contains an
/// anchor as a field (`#[serde(tag = "kind")]` on the outer enum injects
/// `kind` into the JSON object, conflicting with the inner enum's tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "anchor_kind", rename_all = "snake_case")]
pub enum Anchor {
    /// Match a clip whose `awidat.anchor.transcript_snippet` (or marker
    /// metadata) contains this substring.
    TranscriptSnippet {
        /// The text to look for.
        text: String,
    },
    /// Match a clip by its UUID. Most robust; survives any text edit.
    ClipUuid {
        /// UUID string.
        uuid: String,
    },
    /// Match by scene-change index (zero-based, into the scenedetect
    /// sidecar's `shots` array).
    SceneChangeIndex {
        /// Asset id whose scenes we index into.
        asset_id: String,
        /// 0-based index.
        index: u32,
    },
}

impl fmt::Display for Anchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Anchor::TranscriptSnippet { text } => write!(f, "transcript_snippet={text:?}"),
            Anchor::ClipUuid { uuid } => write!(f, "clip_uuid={uuid}"),
            Anchor::SceneChangeIndex { asset_id, index } => {
                write!(f, "scene_change_index={asset_id}:{index}")
            }
        }
    }
}

/// Layering for b-roll inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BRollPosition {
    /// New clip overlays the existing video on a higher video track.
    Overlay,
    /// New clip replaces the existing media (split + insert).
    Replace,
}

/// Anchor pair for `InsertTransition`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionBetween {
    /// Outgoing clip anchor.
    pub from: Anchor,
    /// Incoming clip anchor.
    pub to: Anchor,
}

/// One parsed EDL envelope (Begin EDL ... End EDL block).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EdlEnvelope {
    /// Operations in source order. `apply` walks them left-to-right.
    pub ops: Vec<EdlOp>,
}

impl EdlEnvelope {
    /// Empty envelope.
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Number of ops.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// True iff no ops.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_display_is_informative() {
        let a = Anchor::TranscriptSnippet {
            text: "hello world".into(),
        };
        assert_eq!(a.to_string(), "transcript_snippet=\"hello world\"");
        let a = Anchor::ClipUuid {
            uuid: "c-001".into(),
        };
        assert_eq!(a.to_string(), "clip_uuid=c-001");
    }

    #[test]
    fn envelope_default_is_empty() {
        let e = EdlEnvelope::default();
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
    }

    #[test]
    fn op_serde_roundtrip() {
        let op = EdlOp::TrimClip {
            anchor: Anchor::TranscriptSnippet {
                text: "foo bar".into(),
            },
            start: Some(1.5),
            end: Some(3.0),
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: EdlOp = serde_json::from_str(&json).unwrap();
        assert_eq!(op, back);
    }
}
