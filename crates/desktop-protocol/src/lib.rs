//! Wire protocol between `apps/desktop`'s Rust backend (Tauri) and its
//! React/TS frontend.
//!
//! # Why this crate exists
//!
//! `crates/core` exposes [`SessionEvent`](awidat_core::SessionEvent) — an
//! internal enum the TUI consumes directly. The desktop frontend cannot
//! consume that enum: it is not `serde`-friendly (carries non-serializable
//! variants like `Usage`), it is not stable across releases, and its
//! shape is dictated by the agent loop's needs, not the renderer's.
//!
//! This crate defines a stable, versioned, `serde` + TypeScript-friendly
//! protocol that:
//!
//! 1. Wraps every emission from the agent loop into a [`Item`] — a single
//!    addressable thing the UI can render (a tool call card, a text block,
//!    a proposed edit overlay).
//! 2. Groups Items into [`Turn`]s and Turns into [`Thread`]s for resume /
//!    fork / persistence semantics.
//! 3. Treats agent-proposed mutations as first-class via [`ProposedEdit`] —
//!    rendered as ghost overlays in the timeline, accept/reject/adjust by
//!    the user. This replaces the TUI's modal-approval pattern.
//!
//! # Versioning
//!
//! Every payload carries a [`PROTOCOL_VERSION`]. Breaking changes bump it.
//! The frontend refuses to connect to a backend whose major version it
//! does not understand.
//!
//! # TypeScript generation
//!
//! Types here `#[derive(TS)]` so `cargo test --features ts-export` writes
//! `.ts` files into `apps/desktop/src/protocol/generated/`. Hand-edits to
//! generated files are erased on the next run; edit the Rust source.
//!
//! # Status
//!
//! Day-one types only. `Item::Text` and `Item::ToolCall` cover the smoke
//! test (chat round-trip). `ProposedEdit`, `Thread`, multi-turn resume,
//! and the full set of `Item` variants land as later commits.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Protocol semver. Bump major on breaking changes; minor on additive.
///
/// The frontend refuses to connect to a backend whose major version it
/// does not understand. Minor mismatches log a warning but proceed.
pub const PROTOCOL_VERSION: &str = "0.1.0";

/// Stable identifier for any addressable protocol object.
///
/// IDs are opaque strings — typically ULID or short random — never
/// interpreted by the frontend beyond equality and ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./", type = "string")]
#[serde(transparent)]
pub struct Id(pub String);

impl Id {
    /// Wraps an existing string as an Id. The caller is responsible for
    /// uniqueness within the relevant scope (Item ids unique per Turn,
    /// Turn ids unique per Thread, etc).
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// One addressable thing the agent emitted during a Turn. Every Item
/// has an [`Id`] and a [`ItemLifecycle`] phase so the frontend can
/// upsert progressively as deltas stream in.
///
/// # Lifecycle
///
/// An Item is born in [`ItemLifecycle::Started`], may be updated zero or
/// more times in [`ItemLifecycle::Delta`], and ends in
/// [`ItemLifecycle::Completed`]. The frontend MUST be able to render at
/// any phase — partial text, tool call with no args yet, etc.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    /// A message the user typed into the composer. Always emitted as
    /// a single Completed item — the desktop has no streaming user
    /// input. Lives in the same item stream as agent emissions so
    /// the frontend can render the conversation in arrival order
    /// and a future Thread persister can replay both sides.
    UserInput {
        /// Stable id (one per send).
        id: Id,
        /// What the user typed.
        text: String,
    },
    /// A run of model-generated text. Streams via Delta.
    Text {
        /// Stable id for upsert.
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Cumulative text so far. Always the full text, not just the
        /// delta — the frontend doesn't have to splice.
        text: String,
    },
    /// A tool call the agent issued. Args stream into `args_json` as
    /// the model emits them. `result` populates on Completed.
    ToolCall {
        /// Stable id for upsert.
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Tool name (e.g. `apply_edl`, `find_moment`).
        name: String,
        /// Cumulative args so far, as a JSON value. Empty object until
        /// the first delta lands. Typed as `unknown` on the TS side —
        /// individual tool argument schemas live in the frontend's
        /// per-tool card components.
        #[ts(type = "unknown")]
        args: serde_json::Value,
        /// Final result. `None` until Completed. `Ok` is the tool's
        /// stringified output; `Err` is the error message the model
        /// will see as `is_error: true`.
        result: Option<Result<String, String>>,
    },
    /// Snapshot of the agent's plan emitted by the `update_plan` tool.
    /// Each emission replaces the prior plan view in full. One Plan
    /// item per turn; the id stays stable so updates upsert in place.
    Plan {
        /// Stable id (one per turn).
        id: Id,
        /// Lifecycle phase. Plan emissions are typically Completed
        /// since the model writes the full snapshot each time, but the
        /// frontend should still tolerate Delta if used.
        phase: ItemLifecycle,
        /// Plan items in display order.
        items: Vec<PlanStep>,
        /// Optional one-line note from the model about progress / blockers.
        note: Option<String>,
    },
    /// The agent issued a `request_user_input` and is waiting for a
    /// reply. Frontend renders an inline question prompt and posts the
    /// reply via the `respond_user_input` Tauri command. The item ages
    /// out (becomes Completed) once a reply is delivered.
    AwaitingUserInput {
        /// Stable id matching the underlying tool call's `call_id`.
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Question text to display.
        question: String,
        /// Optional choice list. When present, frontend renders as
        /// radio / option list; otherwise as a free-form input.
        options: Option<Vec<String>>,
    },
    /// The agent loop wants the user to approve a mutating tool call.
    /// Frontend renders an inline approval card; user replies via
    /// `respond_approval`. Created in Started, never delta'd, marked
    /// Completed once the user responds (so it can fade out / collapse).
    ApprovalRequest {
        /// Stable id matching the tool's `call_id`.
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Tool name (e.g. `apply_edl`).
        tool_name: String,
        /// Short caller-built summary of the args. Full args stay on the
        /// matching ToolCall item; this is the user-facing one-liner.
        args_summary: String,
    },
    /// A turn-fatal error from the agent loop. Renders as a banner /
    /// red card. The turn ends after emitting this.
    Error {
        /// Stable id (one per error).
        id: Id,
        /// Error message.
        message: String,
    },
    /// A proposed edit to the project's timeline — the
    /// approval-as-diff replacement for `ApprovalRequest` on
    /// `apply_edl`. Carries the proposed post-state snapshot + diff
    /// hints so the timeline canvas can render a ghost overlay on
    /// top of the current timeline. The user accepts, rejects, or
    /// drags handles to adjust before accepting.
    ///
    /// The frontend never re-runs `apply()` itself — adjustments
    /// flow back through `adjust_proposal` (which mutates the
    /// backend's stored `PendingProposal`, re-runs apply, emits a
    /// Delta with bumped `revision`).
    ///
    /// `revision` exists for the rapid-drag race: two adjustments
    /// in quick succession produce two Deltas; the frontend drops
    /// any whose revision is older than the latest seen.
    ProposedEdit {
        /// Stable id matching either the agent's `call_id` (Agent
        /// source) or a freshly-allocated id (User source).
        id: Id,
        /// Lifecycle phase. Started when first emitted; Delta on
        /// each adjustment; Completed when accepted or rejected
        /// (the frontend uses Completed to fade / collapse the
        /// overlay).
        phase: ItemLifecycle,
        /// Where the proposal came from.
        source: ProposalSource,
        /// The full EDL text the user can preview in a "show EDL"
        /// toggle. Round-trippable through the awidat-core parser.
        edl_text: String,
        /// Post-apply snapshot of the timeline. Same shape
        /// `read_timeline` returns. The canvas paints this at
        /// alpha 1.0 over the current snapshot at alpha 0.45.
        snapshot: TimelineSnapshot,
        /// Per-op coloring metadata. Length matches the op count
        /// in the underlying `EdlEnvelope`; ordering is preserved.
        diff_hints: Vec<AppliedDiff>,
        /// One-line human summary ("trim 3 clips, insert 1") for
        /// the chat-side ProposedEdit reference card.
        summary: String,
        /// Monotonic counter bumped on each adjustment. Frontend
        /// drops Deltas with older revisions to absorb rapid-drag
        /// races.
        revision: u32,
    },
    /// Long-running background work (asset import, indexing,
    /// timeline render). Streams over the same item channel as
    /// agent emissions because the frontend renders the chat as a
    /// single timeline of project activity — "I downloaded foo.mp4"
    /// and "I indexed it" sit in the same place as "I cut clip 3."
    ///
    /// The same id is reused across a Started → many Delta →
    /// Completed lifecycle, so the frontend upserts in place.
    Job {
        /// Stable id (one per job invocation, e.g. one yt-dlp run or
        /// one indexer dispatch).
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Job kind. The frontend keys per-kind UI off this.
        /// Renamed from `kind` to avoid clashing with the enum's
        /// own serde-tag field.
        job_kind: JobKind,
        /// 0..=100 if the job has known progress, `None` for
        /// indeterminate (e.g. yt-dlp before it parses bitrate).
        percent: Option<u8>,
        /// One-line status (e.g. "downloading: 45.2 MB / 120 MB",
        /// "whisper · ep1.mp4: 12 / 84 pairs").
        status: String,
        /// On Completed: terminal state. Frontend uses it to color
        /// the card and show a retry button on Failed. None during
        /// Started / Delta.
        result: Option<JobResult>,
        /// Absolute path to the artifact this job produced, if any.
        /// Set for `Render` (the rendered mp4) and `Transcode` (the
        /// proxy mp4); `None` otherwise. Frontend uses it for the
        /// "Show in Finder" button on Render's Completed-Ok phase.
        output_path: Option<String>,
    },
}

/// Discriminator for [`Item::Job`]. The frontend doesn't render
/// kinds it doesn't know about — keep variants small and concrete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// `yt-dlp` URL download into the project's `raw/` dir.
    UrlImport,
    /// Local-file copy / symlink into `raw/`.
    LocalImport,
    /// `awidat_render::transcode_proxy` over a single asset. Produces
    /// a 720p H.264 all-keyframe mp4 under `<project>/.awidat/proxies/`
    /// that the live preview pane can scrub against without choking
    /// on the original's bitrate.
    Transcode,
    /// `awidat_index::run` over the project.
    Indexing,
    /// `awidat_render::build_timeline_render_spec` + `JobManager::start`
    /// — desktop-initiated timeline export. Distinct from `Transcode`
    /// (proxy generation) and from agent-initiated `start_render` tool
    /// calls (those surface as `Item::ToolCall`).
    Render,
}

/// Terminal state of an [`Item::Job`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum JobResult {
    /// Job finished cleanly. Optional one-liner ("Imported foo.mp4 (84MB)").
    Ok {
        /// Optional success summary.
        summary: Option<String>,
    },
    /// Job failed. Error string for display.
    Err {
        /// Error message.
        message: String,
    },
    /// User cancelled the job.
    Cancelled,
}

/// One row in a Plan item — mirrors `awidat_core::tool::PlanItem` but
/// stripped to fields the frontend actually renders.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct PlanStep {
    /// Free-text step description.
    pub step: String,
    /// One of `pending`, `in_progress`, `completed`. Kept as a string
    /// (not an enum) because the agent may emit other states the
    /// frontend should render as "unknown" rather than crash.
    pub status: String,
}

/// Snapshot of the project's timeline. Returned from
/// `read_timeline`, also embedded in [`Item::ProposedEdit`]'s
/// `snapshot` field. Empty `tracks` is a normal "fresh project, no
/// clips yet" state.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineSnapshot {
    /// Total project duration in seconds (max track end across all
    /// tracks). Zero when timeline is empty.
    pub duration_s: f64,
    /// Tracks in order: video first, then audio. Empty when project
    /// has no clips.
    pub tracks: Vec<TimelineTrack>,
}

/// One row in [`TimelineSnapshot::tracks`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineTrack {
    /// Track name from the OTIO file.
    pub name: String,
    /// Track kind: `"video"` or `"audio"`.
    pub kind: String,
    /// Items in this track in playback order.
    pub items: Vec<TimelineItem>,
}

/// One drawable item on a track. Variant-tagged so the frontend can
/// render each kind differently.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineItem {
    /// A clip — references an asset, has a source range.
    Clip {
        /// Index of this item within its track. Stable across reads.
        index: usize,
        /// Display name (clip's OTIO `name` field).
        name: String,
        /// Anchor uuid for `Anchor::ClipUuid` in EDL ops. Pulled from
        /// `clip.metadata.awidat.extra["clip_uuid"]` if present;
        /// otherwise falls back to the clip's display name (which the
        /// `awidat_core::edl::anchor` resolver also matches against).
        /// Step 8's drag-to-trim builds `TrimClip { anchor:
        /// ClipUuid { uuid } }` from this field.
        clip_uuid: String,
        /// Start of this clip on the track timeline, in seconds.
        track_start_s: f64,
        /// Duration on the track, in seconds.
        duration_s: f64,
        /// Asset id (project-relative path), if the clip references
        /// one. `None` for clips with missing or non-external refs.
        asset_id: Option<String>,
        /// Source-asset start offset, in seconds. Useful when the
        /// frontend wants to map "this clip plays source[12.5s..]".
        source_start_s: Option<f64>,
        /// Absolute path to the asset's 720p proxy mp4 on disk, if
        /// the proxy has finished generating. The frontend feeds this
        /// into `convertFileSrc()` and `<video src>` to play the
        /// segment without ever touching the original media. `None`
        /// when the asset is missing, the proxy hasn't finished
        /// transcoding, or the proxies dir doesn't exist yet.
        proxy_path: Option<String>,
    },
    /// Empty time on the track (silence / black frames).
    Gap {
        /// Index of this item within its track.
        index: usize,
        /// Start position on the track, in seconds.
        track_start_s: f64,
        /// Gap duration, in seconds.
        duration_s: f64,
    },
    /// Transition (cross-dissolve, fade) between two clips.
    Transition {
        /// Index of this item within its track.
        index: usize,
        /// Anchor position on the track, in seconds.
        track_start_s: f64,
        /// Cumulative effect duration (in_offset + out_offset).
        duration_s: f64,
        /// Effect name from the OTIO transition (e.g.
        /// `"SMPTE_Dissolve"`).
        effect_name: String,
    },
}

/// One paragraph-sized segment from a whisper transcript sidecar.
/// Segment-level granularity is what the transcript pane renders as
/// a virtualized row; word-level granularity drives selection,
/// click-to-seek, and active-word highlight.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TranscriptSegment {
    /// Concatenated text of the segment (whisper's own merging).
    pub text: String,
    /// Source-time start, in seconds.
    pub start_s: f64,
    /// Source-time end, in seconds.
    pub end_s: f64,
    /// Diarized speaker id (e.g. `"SPEAKER_00"`), if available.
    pub speaker_id: Option<String>,
}

/// One word-level alignment from a whisper transcript sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TranscriptWord {
    /// Word text (whisper-trimmed, may include punctuation).
    pub text: String,
    /// Source-time start, in seconds.
    pub start_s: f64,
    /// Source-time end, in seconds.
    pub end_s: f64,
    /// Diarized speaker id, if available.
    pub speaker_id: Option<String>,
}

/// Speaker summary from a diarized whisper sidecar. Empty when
/// diarization wasn't run.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TranscriptSpeaker {
    /// Diarized id (e.g. `"SPEAKER_00"`).
    pub id: String,
    /// Total seconds of speech attributed to this speaker.
    pub total_speech_s: f64,
}

/// Full transcript for a single asset, deserialized from the
/// project's whisper sidecar. The frontend renders this in the
/// transcript pane (Step 6 — Descript-style click-word-to-seek +
/// drag-select-to-delete).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct Transcript {
    /// Asset stem this transcript is for. Matches `ProxyEntry.stem`
    /// so the frontend can key transcripts by the same id it uses
    /// for the media pane.
    pub asset_stem: String,
    /// BCP-47 language tag (e.g. `"en"`).
    pub language: String,
    /// Whether diarization ran (`speakers` will be empty when false).
    pub diarized: bool,
    /// Segment-level paragraphs.
    pub segments: Vec<TranscriptSegment>,
    /// Word-level alignment. May be empty if the indexer didn't
    /// produce word timestamps for this asset.
    pub words: Vec<TranscriptWord>,
    /// Speaker summary (empty when not diarized).
    pub speakers: Vec<TranscriptSpeaker>,
}

/// Where a proposed edit came from. The frontend renders agent and
/// user proposals identically (same ghost overlay, same handles,
/// same Accept/Reject), but the source helps with chat-side
/// summaries ("agent proposed…" vs "you proposed…").
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ProposalSource {
    /// The agent emitted an `apply_edl` tool call. The proposal
    /// rides on the same `call_id` as the underlying ApprovalRequest;
    /// accepting wires through to the agent's reply oneshot.
    Agent {
        /// Tool name (always `"apply_edl"` today; future EDL-emitting
        /// tools fold into this same path).
        tool_name: String,
    },
    /// The user initiated the edit (drag-to-trim, transcript
    /// delete-range, etc.). No agent oneshot — the desktop applies
    /// directly on accept.
    User,
}

/// Which side of a clip's edge a `TrimEdge` diff hint refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Left (start) edge.
    Left,
    /// Right (end) edge.
    Right,
}

/// Per-op diff metadata. Tells the canvas how to color the proposed
/// snapshot relative to the original — the canvas doesn't compute
/// the diff itself; the backend produced it from the `apply()`
/// outcome and ships it alongside the post-state snapshot.
///
/// Every variant carries `op_index`, the position of the
/// originating op in the proposal's `EdlEnvelope`. The frontend's
/// drag handles use it to fire `adjust_proposal { op_index, ... }`
/// without re-discovering which op produced which hint. A single
/// op can produce multiple hints (e.g. `TrimClip` with both bounds
/// set emits two `TrimEdge` entries with the same `op_index`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppliedDiff {
    /// A clip's edge moved. Track + item indexes refer to the
    /// **proposed** snapshot; `delta_s` is signed (positive =
    /// trimmed inward, negative = extended outward via Untrim).
    TrimEdge {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the proposed snapshot.
        track_index: usize,
        /// Item index within that track.
        item_index: usize,
        /// Which edge moved.
        side: Side,
        /// Signed shift in seconds.
        delta_s: f64,
    },
    /// A clip was removed. Indexes refer to the **original**
    /// snapshot — the proposed snapshot doesn't contain it.
    Delete {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the original snapshot.
        track_index: usize,
        /// Item index within that track.
        item_index: usize,
    },
    /// A clip was split into two. Indexes refer to the **proposed**
    /// snapshot — the left half keeps the original index, the
    /// right half is at `item_index + 1`.
    Split {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the proposed snapshot.
        track_index: usize,
        /// Index of the left half within that track.
        item_index: usize,
        /// Cut point in source-media seconds.
        at_s: f64,
    },
    /// A new clip was inserted. Indexes refer to the **proposed**
    /// snapshot.
    Insert {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the proposed snapshot.
        track_index: usize,
        /// Index of the inserted item within that track.
        item_index: usize,
    },
}

/// One adjustment the user applied to a proposed edit before
/// accepting. The `op_index` is into the proposal's `EdlEnvelope`
/// (which the backend keeps in `PendingProposal`), so the frontend
/// only needs to send what changed — the backend re-runs `apply()`
/// to get the new snapshot + diff hints.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct EditAdjustment {
    /// Position in the proposal's EDL envelope.
    pub op_index: usize,
    /// Which field of that op the user changed.
    pub field: AdjustField,
    /// New value in seconds. Interpretation depends on the field.
    pub value_s: f64,
}

/// Which field of an `EdlOp` an [`EditAdjustment`] is targeting.
/// Only the fields the user can drag handles for show up here —
/// boolean / asset-path fields aren't draggable in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum AdjustField {
    /// `TrimClip.start` / `UntrimClip.start`.
    TrimStart,
    /// `TrimClip.end` / `UntrimClip.end`.
    TrimEnd,
    /// `SplitClip.at_s`.
    SplitAt,
    /// `InsertClip.start`.
    InsertStart,
    /// `InsertClip.end`.
    InsertEnd,
}

/// What phase of its lifecycle an [`Item`] is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum ItemLifecycle {
    /// First emission. Frontend should create a new render slot.
    Started,
    /// Incremental update. Frontend should re-render the existing slot.
    Delta,
    /// Final emission. Frontend should mark the slot as terminal (no
    /// more updates). Animations etc may stop here.
    Completed,
}

/// One conversational round: user input → zero or more agent responses
/// (Items) → control returned to user.
///
/// A Turn lives inside a Thread. Turns are append-only within a Thread;
/// re-running an old turn forks the Thread.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct Turn {
    /// Stable id within the parent Thread.
    pub id: Id,
    /// The user input that opened this Turn.
    pub user_input: String,
    /// Items emitted during this Turn, in arrival order.
    pub items: Vec<Item>,
    /// Whether this Turn is still running. `true` between TurnStart and
    /// TurnEnd events; `false` once the agent loop hands control back.
    pub running: bool,
}

/// A persistent conversation: an ordered list of [`Turn`]s. Threads are
/// the unit of save / resume / fork — the frontend identifies a session
/// by its `Thread::id`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct Thread {
    /// Stable thread id.
    pub id: Id,
    /// Human-readable title (auto-generated or user-edited).
    pub title: String,
    /// Turns in arrival order.
    pub turns: Vec<Turn>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Smoke test: an Item round-trips through JSON without losing
    /// shape. If this breaks, every frontend render path breaks.
    #[test]
    fn item_text_roundtrips_json() {
        let item = Item::Text {
            id: Id::new("t-1"),
            phase: ItemLifecycle::Completed,
            text: "hello world".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::Text { id, phase, text } => {
                assert_eq!(id.0, "t-1");
                assert_eq!(phase, ItemLifecycle::Completed);
                assert_eq!(text, "hello world");
            }
            _ => panic!("expected Item::Text"),
        }
    }

    #[test]
    fn item_tool_call_with_partial_args_roundtrips() {
        let item = Item::ToolCall {
            id: Id::new("tc-1"),
            phase: ItemLifecycle::Delta,
            name: "find_moment".into(),
            args: serde_json::json!({ "query": "the funny part" }),
            result: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::ToolCall {
                id,
                phase,
                name,
                args,
                result,
            } => {
                assert_eq!(id.0, "tc-1");
                assert_eq!(phase, ItemLifecycle::Delta);
                assert_eq!(name, "find_moment");
                assert_eq!(args["query"], "the funny part");
                assert!(result.is_none());
            }
            _ => panic!("expected Item::ToolCall"),
        }
    }

    #[test]
    fn item_proposed_edit_roundtrips_json() {
        let item = Item::ProposedEdit {
            id: Id::new("proposal-1"),
            phase: ItemLifecycle::Started,
            source: ProposalSource::Agent {
                tool_name: "apply_edl".into(),
            },
            edl_text: "*** Begin EDL\n*** End EDL\n".into(),
            snapshot: TimelineSnapshot {
                duration_s: 12.5,
                tracks: vec![],
            },
            diff_hints: vec![AppliedDiff::TrimEdge {
                op_index: 0,
                track_index: 0,
                item_index: 1,
                side: Side::Right,
                delta_s: 1.5,
            }],
            summary: "trim 1 clip".into(),
            revision: 0,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::ProposedEdit {
                id,
                source,
                edl_text,
                summary,
                revision,
                diff_hints,
                ..
            } => {
                assert_eq!(id.0, "proposal-1");
                assert!(matches!(
                    source,
                    ProposalSource::Agent { ref tool_name } if tool_name == "apply_edl"
                ));
                assert_eq!(edl_text, "*** Begin EDL\n*** End EDL\n");
                assert_eq!(summary, "trim 1 clip");
                assert_eq!(revision, 0);
                assert_eq!(diff_hints.len(), 1);
            }
            _ => panic!("expected Item::ProposedEdit"),
        }
    }

    #[test]
    fn edit_adjustment_roundtrips_json() {
        let adj = EditAdjustment {
            op_index: 2,
            field: AdjustField::TrimEnd,
            value_s: 4.21,
        };
        let json = serde_json::to_string(&adj).unwrap();
        let back: EditAdjustment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op_index, 2);
        assert_eq!(back.field, AdjustField::TrimEnd);
        assert!((back.value_s - 4.21).abs() < 1e-9);
    }

    #[test]
    fn protocol_version_is_semver_shaped() {
        let parts: Vec<&str> = PROTOCOL_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "PROTOCOL_VERSION must be x.y.z");
        for part in parts {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "PROTOCOL_VERSION components must be numeric"
            );
        }
    }
}
