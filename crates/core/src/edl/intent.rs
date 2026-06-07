//! Typed editorial intent layer above the EDL apply pipeline.
//!
//! This mirrors the reference editor's `EditorIntent -> Command -> Execute`
//! shape while preserving Montage's single mutation path: commands lower to an
//! [`EdlEnvelope`] and execute through [`super::apply`].

use montage_proto::otio::Timeline;
use montage_proto::professional::SourceRange;
use serde::{Deserialize, Serialize};

use super::anchor::AnchorContext;
use super::apply::{ApplyError, ApplyOutcome};
use super::op::{
    Anchor, EdlEnvelope, EdlOp, ProfessionalTimelineEdit, SnapOptions, TransitionBetween,
};

/// Shared typed vocabulary for authoring editorial changes before EDL lowering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum EditorialIntent {
    /// Split one clip at a source-media timestamp.
    SplitClip(SplitClipIntent),
    /// Trim one clip's source range.
    TrimClip(TrimClipIntent),
    /// Move one clip in its current track.
    MoveClip(MoveClipIntent),
    /// Add a timeline marker.
    SetMarker(MarkerIntent),
    /// Professional timeline operation.
    Professional(ProfessionalIntent),
    /// Multiple intents lowered as one ordered EDL envelope.
    Batch(Vec<EditorialIntent>),
}

/// Split one clip at a source-media timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitClipIntent {
    /// Anchor identifying the clip.
    pub anchor: Anchor,
    /// Cut point in seconds into the source media.
    pub at_s: f64,
}

/// Trim one clip's source range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrimClipIntent {
    /// Anchor identifying the clip.
    pub anchor: Anchor,
    /// New source-media start in seconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_s: Option<f64>,
    /// New source-media end in seconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub end_s: Option<f64>,
}

/// Move one clip in its current track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveClipIntent {
    /// Anchor identifying the clip.
    pub anchor: Anchor,
    /// Zero-based destination child index.
    pub to_position: usize,
    /// Optional absolute timeline start time.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub at_s: Option<f64>,
}

/// Add a timeline marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerIntent {
    /// Marker time in timeline seconds.
    pub at_s: f64,
    /// Marker label.
    pub label: String,
    /// Optional marker category.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<String>,
}

/// Professional edit intents supported by the typed intent layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "professional_intent", rename_all = "snake_case")]
pub enum ProfessionalIntent {
    /// Roll an edit point between adjacent clips.
    RollEdit {
        /// Adjacent clips.
        between: TransitionBetween,
        /// Boundary delta in seconds.
        delta_s: f64,
    },
    /// Append source media to a track.
    Append {
        /// Destination track.
        track: String,
        /// Source asset id/path.
        asset: String,
        /// Source range.
        range: SourceRange,
    },
}

/// Resolves editorial intents into EDL-backed commands.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditorialIntentResolver;

impl EditorialIntentResolver {
    /// Create a resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve one intent into an executable EDL-backed command.
    pub fn resolve(self, intent: EditorialIntent) -> EditorialCommand {
        EditorialCommand {
            envelope: lower_intent(intent),
        }
    }
}

/// Command produced by resolving an [`EditorialIntent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorialCommand {
    envelope: EdlEnvelope,
}

impl EditorialCommand {
    /// Borrow the lowered EDL envelope.
    pub fn edl(&self) -> &EdlEnvelope {
        &self.envelope
    }

    /// Consume this command and return its lowered EDL envelope.
    pub fn into_edl(self) -> EdlEnvelope {
        self.envelope
    }

    /// Execute the command through Montage's existing EDL apply pipeline.
    pub fn execute(
        &self,
        timeline: &Timeline,
        ctx: &AnchorContext,
    ) -> Result<(Timeline, ApplyOutcome), ApplyError> {
        super::apply(timeline, &self.envelope, ctx)
    }
}

fn lower_intent(intent: EditorialIntent) -> EdlEnvelope {
    let mut ops = Vec::new();
    push_lowered(intent, &mut ops);
    EdlEnvelope { ops }
}

fn push_lowered(intent: EditorialIntent, ops: &mut Vec<EdlOp>) {
    match intent {
        EditorialIntent::SplitClip(intent) => ops.push(EdlOp::SplitClip {
            anchor: intent.anchor,
            at_s: intent.at_s,
            snap: None::<SnapOptions>,
        }),
        EditorialIntent::TrimClip(intent) => ops.push(EdlOp::TrimClip {
            anchor: intent.anchor,
            start: intent.start_s,
            end: intent.end_s,
        }),
        EditorialIntent::MoveClip(intent) => ops.push(EdlOp::MoveClip {
            anchor: intent.anchor,
            to_position: intent.to_position,
            at_s: intent.at_s,
            snap: None::<SnapOptions>,
        }),
        EditorialIntent::SetMarker(intent) => {
            ops.push(EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::AddMarker {
                    at_s: intent.at_s,
                    label: intent.label,
                    category: intent.category,
                },
            });
        }
        EditorialIntent::Professional(intent) => {
            ops.push(EdlOp::ProfessionalTimelineEdit {
                edit: match intent {
                    ProfessionalIntent::RollEdit { between, delta_s } => {
                        ProfessionalTimelineEdit::RollEdit { between, delta_s }
                    }
                    ProfessionalIntent::Append {
                        track,
                        asset,
                        range,
                    } => ProfessionalTimelineEdit::Append {
                        track,
                        asset,
                        range,
                    },
                },
            });
        }
        EditorialIntent::Batch(intents) => {
            for intent in intents {
                push_lowered(intent, ops);
            }
        }
    }
}
