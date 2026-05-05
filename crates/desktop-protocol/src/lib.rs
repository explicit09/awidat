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
    /// Long-running background work (asset import, indexing). Streams
    /// over the same item channel as agent emissions because the
    /// frontend renders the chat as a single timeline of project
    /// activity — "I downloaded foo.mp4" and "I indexed it" sit in
    /// the same place as "I cut clip 3."
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
    /// `awidat_index::run` over the project.
    Indexing,
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
