//! Wire protocol between `apps/desktop`'s Rust backend (Tauri) and its
//! React/TS frontend.
//!
//! # Why this crate exists
//!
//! `crates/core` exposes `montage_core::SessionEvent` — an
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
//! 3. Treats agent-proposed mutations as first-class via `ProposedEdit` —
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

#![cfg_attr(test, allow(clippy::unwrap_used))]
//!
//! Types here `#[derive(TS)]` so
//! `MONTAGE_EXPORT_TS=1 cargo test -p montage-desktop-protocol` writes `.ts`
//! files into `apps/desktop/src/protocol/generated/`. Hand-edits to generated
//! files are erased on the next export; edit the Rust source.
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
#[allow(clippy::large_enum_variant)]
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
        /// Typed capability metadata for this approval. Kept as JSON so the
        /// desktop protocol does not depend on core internals.
        #[ts(type = "unknown")]
        capability_metadata: serde_json::Value,
        /// Optional short-form rationale — the agent's one-sentence
        /// justification for the underlying tool call, e.g.
        /// "trimmed 0.42s silence per podcast defaults". Captured from
        /// `apply_edl(reasoning = …)` and equivalent fields on other
        /// tool calls; absent when the producing tool does not (yet)
        /// emit a `reasoning` argument.
        ///
        /// Wave 3's Brief surface reads this on every row (approvals
        /// included). Distinct from `args_summary` (mechanical "what")
        /// and the matching ToolCall's full args (raw envelope).
        ///
        /// Backwards-compatible: `Option<String>` so older serialized
        /// approvals deserialize fine. Producers that don't yet emit
        /// it can keep emitting `None`.
        #[serde(default)]
        #[ts(optional)]
        rationale: Option<String>,
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
        /// toggle. Round-trippable through the montage-core parser.
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
        /// Optional agent-supplied long-form intent shown at the top
        /// of the Proposal Inspector ("Remove filler phrase and
        /// trailing pause while preserving speaker cadence…").
        ///
        /// All five inspector fields below are optional: the agent
        /// populates them progressively as Montage's reasoning matures.
        /// The frontend renders each block only when its field is
        /// present, so older producers (User-source edits, simple
        /// trims, legacy agent paths) keep working unchanged.
        #[serde(default)]
        #[ts(optional)]
        intent: Option<String>,
        /// Optional long-form explanation rendered under "EXPLANATION"
        /// in the Inspector. May overlap with `summary` (which stays
        /// one-line) — when both are present, `summary` is the chip,
        /// `explanation` is the body.
        #[serde(default)]
        #[ts(optional)]
        explanation: Option<String>,
        /// Optional 0..=1 confidence the agent assigns to this proposal.
        /// The Inspector renders both a ConfidenceRing and a bar.
        #[serde(default)]
        #[ts(optional)]
        confidence: Option<f32>,
        /// Optional risk tier. The Inspector renders this as a 4-dot
        /// indicator next to a colored label. Tier mapping:
        /// Low → safe to accept, Medium → review first,
        /// High → likely needs revision, VeryHigh → block.
        #[serde(default)]
        #[ts(optional)]
        risk: Option<RiskLevel>,
        /// Optional list of evidence rows. The Inspector renders each
        /// with a kind-specific icon and a High/Med/Low tier label
        /// derived from `confidence` if `confidence_level` is absent.
        #[serde(default)]
        evidence: Vec<ProposalEvidence>,
        /// Optional alternative proposals the user can compare or
        /// switch to. Empty list means no alternatives — the Inspector
        /// hides the section. Each entry is a *summary* of a sibling
        /// proposal, not a full ProposedEdit (the agent re-emits the
        /// full thing if the user picks one).
        #[serde(default)]
        alternatives: Vec<ProposalAlternative>,
        /// Optional short-form rationale — the agent's one-sentence
        /// justification for this proposal, e.g.
        /// "trimmed 0.42s silence per podcast defaults".
        ///
        /// This is the load-bearing trust signal Wave 3 surfaces on
        /// every proposal pill / tooltip / Brief row: a rationale is
        /// what lets the human reviewer take the agent's call on
        /// faith. Distinct from `explanation` (long-form body) and
        /// `intent` (what the agent is *trying* to do); `rationale`
        /// answers *why this specific decision*.
        ///
        /// Backwards-compatible: `Option<String>` so older serialized
        /// proposals deserialize fine. Producers that don't yet emit
        /// it can keep emitting `None`.
        #[serde(default)]
        #[ts(optional)]
        rationale: Option<String>,
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
    /// Review package for professional editing substrate artifacts:
    /// asset catalogs, selects, assembly, VFX, color, audio, delivery,
    /// preflight, workflow readiness, and autonomy prerequisites.
    ProfessionalReview {
        /// Stable id.
        id: Id,
        /// Lifecycle phase.
        phase: ItemLifecycle,
        /// Capability area this package belongs to.
        area: ProfessionalCapabilityArea,
        /// Lens where the package should appear.
        lens: WorkflowLensTag,
        /// Readiness state for this package.
        readiness: ReadinessStateTag,
        /// User-facing summary.
        summary: String,
        /// Evidence, blockers, or review rows.
        findings: Vec<ProfessionalReviewFinding>,
        /// Optional opaque payload for specialized inspectors.
        #[ts(type = "unknown")]
        payload: Option<serde_json::Value>,
    },
    /// An editorial finding the agent surfaced — "I noticed this,
    /// you decide." Distinct from [`Self::ProposedEdit`]: a Note is
    /// passive (no pending mutation), and lives in its own UI panel
    /// rather than the timeline canvas. Notes have a stable identity
    /// (the `id` field), persist across sessions via
    /// `<project>/.montage/notes.json`, and have a three-state
    /// lifecycle: `open` → `resolved` (user took action) or
    /// `dismissed` (user explicitly rejected this finding).
    EditorialNote {
        /// Stable id (matches the underlying Note record so dismiss /
        /// resolve commands can find it).
        id: Id,
        /// Lifecycle phase — `Started` on first emission, `Delta`
        /// when the lifecycle status changes, `Completed` when the
        /// note is fully resolved or dismissed (UI may then fade it).
        phase: ItemLifecycle,
        /// What kind of finding this is. The UI renders an icon
        /// per kind and the dismissal pattern matcher buckets by it.
        ///
        /// Named `note_kind` (not `kind`) to avoid colliding with the
        /// outer `Item` enum's `#[serde(tag = "kind")]` discriminator —
        /// same shape as `EdlOp`'s `op` tag-rename pattern.
        note_kind: EditorialNoteKind,
        /// Open / resolved / dismissed.
        status: EditorialNoteStatus,
        /// Master-timeline seconds where the finding centers. UI
        /// uses this for click-to-seek.
        anchor_at_s: f64,
        /// One-line human summary the panel renders ("dead air at
        /// 2:14 — 2.4s of silence").
        summary: String,
        /// Optional EDL text the user can apply directly via "Generate
        /// Proposal." `None` when the agent hasn't pre-built a fix
        /// (the user might still ask the agent to act on the note in
        /// chat). When `Some`, the panel's "Generate Proposal" button
        /// pipes this through `propose_user_edit`.
        suggested_proposal: Option<String>,
        /// For `continuity_warning` notes: the rule engine's verdict
        /// (`clean` / `risky` / `dirty` / `abstain`) so the panel
        /// can color-code the card. `None` for non-continuity kinds
        /// (silence, filler, etc). Dirty cuts render with a red
        /// border, risky get amber, abstain reads muted.
        #[serde(default)]
        continuity_verdict: Option<ContinuityVerdictTag>,
        /// For `continuity_warning` notes: the per-rule reasons the
        /// engine produced (only rules whose verdict ≠ `clean` are
        /// surfaced — clean rules are silent). The panel renders
        /// these as a bullet list under the summary so the user
        /// sees exactly *why* the cut was flagged. `None` for
        /// non-continuity kinds.
        #[serde(default)]
        continuity_reasons: Option<Vec<String>>,
        /// For `broll_suggestion` notes: the Pexels search query the
        /// agent generated when surfacing the note. Used by the UI's
        /// "Search Pexels" button (which dispatches a chat directive
        /// asking the agent to call `search_broll(query)` on the
        /// user's behalf). `None` for non-broll kinds.
        #[serde(default)]
        broll_query: Option<String>,
        /// For `broll_suggestion` notes: pre-fetched preview thumbnails
        /// (when the agent has already called `search_broll`). When
        /// present, the BrollNoteCard renders a thumbnail row with
        /// click-to-place. When absent, the card shows the query
        /// alongside a "Search Pexels" affordance.
        #[serde(default)]
        broll_previews: Option<Vec<BrollPreview>>,
        /// For `broll_suggestion` notes: exact anchor to pass to
        /// `use_broll`. The UI must not ask the agent to infer this
        /// from prose because placement needs to survive handoff.
        #[serde(default)]
        broll_anchor: Option<BrollAnchor>,
    },
}

/// Professional editing capability area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum ProfessionalCapabilityArea {
    /// Asset catalog.
    AssetCatalog,
    /// Source review/selects.
    SourceReviewSelects,
    /// Assembly/timeline operations.
    AssemblyAndTimelineOperations,
    /// Editorial intent/review.
    EditorialIntentAndReview,
    /// Parameter animation.
    ParameterAnimation,
    /// Motion graphics templates.
    MotionGraphicsTemplates,
    /// Composition graph.
    CompositionGraph,
    /// Tracking, masks, and mattes.
    TrackingMasksMattes,
    /// Color finishing.
    ColorFinishing,
    /// Audio finishing.
    AudioFinishing,
    /// Delivery profiles and preflight.
    DeliveryProfilesAndPreflight,
    /// Workflow lenses.
    WorkflowLenses,
    /// Pre-autonomy orchestration contracts.
    PreAutonomyOrchestrationContract,
}

/// Workflow lens tag for review routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLensTag {
    /// Media lens.
    Media,
    /// Selects lens.
    Selects,
    /// Assembly lens.
    Assembly,
    /// Edit review lens.
    EditReview,
    /// VFX lens.
    Vfx,
    /// Color lens.
    Color,
    /// Audio lens.
    Audio,
    /// Delivery lens.
    Delivery,
    /// Preflight lens.
    Preflight,
}

/// Readiness state tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStateTag {
    /// Ready.
    Ready,
    /// Pending.
    Pending,
    /// Blocked.
    Blocked,
    /// Unavailable.
    Unavailable,
}

/// One row in a professional review package.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct ProfessionalReviewFinding {
    /// Stable kind, for example `missing_proxy` or `loudness_out_of_range`.
    pub kind: String,
    /// Severity: info, warning, or error.
    pub severity: String,
    /// User-facing message.
    pub message: String,
    /// Optional fix proposal reference.
    pub fix_ref: Option<String>,
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
    /// `montage_render::transcode_proxy` over a single asset. Produces
    /// a 720p H.264 all-keyframe mp4 under `<project>/.montage/proxies/`
    /// that the live preview pane can scrub against without choking
    /// on the original's bitrate.
    Transcode,
    /// Filmstrip thumbnail extraction over a single asset's proxy.
    /// Produces a sequence of small JPEGs under
    /// `<project>/.montage/thumbnails/<stem>-<hash>/` that the timeline
    /// canvas tiles across each clip. Fires as a follow-up to
    /// `Transcode` once the proxy has landed.
    Thumbnails,
    /// Audio waveform peak extraction over a single asset. Produces a
    /// JSON sidecar of pre-bucketed peak amplitudes under
    /// `<project>/.montage/waveforms/<stem>-<hash>.json` that the
    /// timeline canvas reads to draw the per-clip waveform line on
    /// audio tracks. Fires alongside `Thumbnails` once the proxy has
    /// landed.
    Waveform,
    /// Silence range detection over a single asset. Produces a JSON
    /// sidecar of `(start_s, end_s, db_floor)` ranges under
    /// `<project>/.montage/silences/<stem>-<hash>.json` that the
    /// `find_dead_air` tool reads. Fires alongside `Waveform` once
    /// the proxy has landed.
    Silences,
    /// Per-second motion-magnitude sampling over a single asset's
    /// proxy. Produces a JSON sidecar of `Vec<f32>` scene-change
    /// scores under `<project>/.montage/motion/<stem>-<hash>.json`
    /// that the Phase 2 continuity engine reads to detect
    /// mid-motion cuts. Fires alongside `Silences`.
    Motion,
    /// `montage_index::run` over the project.
    Indexing,
    /// `montage_render::build_timeline_render_spec` + `JobManager::start`
    /// — desktop-initiated timeline export. Distinct from `Transcode`
    /// (proxy generation) and from agent-initiated `start_render` tool
    /// calls (those surface as `Item::ToolCall`).
    Render,
    /// External-provider video/image generation job (OpenRouter +
    /// SeeDance, etc.). Lifecycle is watched on the desktop side by
    /// tailing `<project>/.montage/generated-media/registry.json`;
    /// the agent itself sees these via `poll_generated_media_job`,
    /// but the user wants visibility into "how many in flight, when
    /// they land" without checking the registry by hand.
    GeneratedMedia,
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

/// Wire-side mirror of `montage_core::continuity::Verdict`. Lives
/// here in the protocol crate so the frontend can render it via
/// ts-rs without depending on `montage-core`. The panel maps each
/// variant to a color: clean → green, risky → amber, dirty → red,
/// abstain → muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum ContinuityVerdictTag {
    /// No rule flagged the cut. Renders dimmed; the user usually
    /// won't see a Note for clean cuts (the agent skips emitting
    /// them) but the variant exists for completeness.
    Clean,
    /// At least one rule flagged the cut as questionable. Amber.
    Risky,
    /// At least one rule is confident the cut would jar. Red.
    Dirty,
    /// Some rules had no input data (sidecars missing). Muted —
    /// the agent surfaces these to tell the user the project may
    /// need indexing.
    Abstain,
}

/// One Pexels preview attached to a [`Item::EditorialNote`] of kind
/// `broll_suggestion`. The agent populates this when it has already
/// called `search_broll` and wants to surface the top hits inline.
/// The UI renders a thumbnail row with click-to-place; clicking
/// triggers a chat directive that calls `use_broll(pexels_id, ...)`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct BrollPreview {
    /// Pexels video id — pass to `use_broll` to download.
    pub pexels_id: u64,
    /// JPEG thumbnail URL the UI shows in the preview row.
    pub thumbnail_url: String,
    /// Native length of the source clip in seconds. Informational —
    /// the actual cutaway in the timeline is `duration_s` from the
    /// note (or whatever the user picks).
    pub duration_s: u32,
    /// Attribution string ready to display ("by Alice").
    pub attribution: String,
    /// Pexels page URL — link target for the attribution string.
    pub pexels_page: String,
}

/// Exact anchor carried by a b-roll note for `use_broll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrollAnchor {
    /// Match by transcript snippet.
    TranscriptSnippet {
        /// Snippet text.
        text: String,
    },
    /// Match by stable clip UUID.
    ClipUuid {
        /// Clip UUID.
        uuid: String,
    },
}

/// What kind of editorial finding an [`Item::EditorialNote`] holds.
/// The dismissal-pattern matcher buckets by this so the user can
/// dismiss (e.g.) "all silence_trim notes" without having to dismiss
/// each one individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum EditorialNoteKind {
    /// Dead-air range the agent suggests trimming (silence > N seconds
    /// at the kind's threshold bucket).
    SilenceTrim,
    /// A filler word ("um", "uh", etc) that could be cut.
    FillerWord,
    /// A false-start — speaker began a thought, abandoned it, restarted.
    FalseStart,
    /// A continuity warning — a pending or proposed cut that risks
    /// jarring (mid-sentence, mid-motion, etc). Surfaced by Phase 2.
    ContinuityWarning,
    /// A b-roll opportunity — a moment that would land better with
    /// a visual. Surfaced by Phase 3.
    BrollSuggestion,
    /// Anything else the agent wants to surface. UI renders with a
    /// generic icon. Avoid leaning on this for things that ought to
    /// have a typed kind — adding a variant is cheap.
    Generic,
}

/// Lifecycle of an [`Item::EditorialNote`]. Persists across sessions
/// via `<project>/.montage/notes.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum EditorialNoteStatus {
    /// Surfaced by the agent; user hasn't acted.
    Open,
    /// User accepted a generated proposal, or hand-edited the
    /// relevant section. Renders dimmed in the panel.
    Resolved,
    /// User explicitly rejected this finding. The dismissal pattern
    /// is also persisted in `dismissed_patterns.json` so the agent
    /// won't re-surface the same kind/threshold bucket later.
    Dismissed,
}

/// What "shape" of project this is. Drives per-format system-prompt
/// defaults and editorial heuristics. `Other` carries a free-text
/// description the agent appends to its system prompt verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectType {
    /// Long-form podcast cleanup — the v1 specialized mode.
    Podcast,
    /// Short-form vertical (60s, hook in first 3s, fast cuts).
    /// Gets specialized prompts in Phase 4.
    Shorts,
    /// Tutorial / demo / screencast — hold key frames longer, never
    /// cut over a code-typing moment. Specialized in Phase 4.
    Tutorial,
    /// Anything else. The free-text `description` is appended to
    /// the agent's system prompt so it has *something* to anchor on.
    Other {
        /// User-provided one-paragraph project description.
        description: String,
    },
}

/// User's permission level for the agent's autonomous editing. Maps
/// to Claude Code's permission modes (manual / accept-edits /
/// bypass). The dropdown lives in the action bar; selection persists
/// per project at `<project>/.montage/permission_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Default. Every proposal needs explicit Accept; agent doesn't
    /// surface editorial Notes proactively unless asked.
    Manual,
    /// Agent surfaces Notes for everything it finds; doesn't propose
    /// edits unless the user (or the Note's "Generate Proposal"
    /// button) asks.
    Copilot,
    /// Agent bundles all findings into one proposal at session end.
    /// User accepts or rejects the bundle.
    Autopilot,
}

/// One row in a Plan item — mirrors `montage_core::tool::PlanItem` but
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
    /// Optional timeline-level broadcast overlay config. When present,
    /// the desktop preview draws the same title card / lower-third /
    /// ticker layers that the timeline render path will burn in.
    pub broadcast_overlay: Option<BroadcastOverlayConfig>,
    /// Semantic editorial intent attached to hard-cut boundaries. This
    /// lets the UI inspect why a boundary is a cut on action, cutaway,
    /// match cut, J-cut, etc. without requiring a visible transition item.
    pub cut_boundaries: Vec<TimelineCutBoundary>,
    /// Known places where live desktop preview is less faithful than
    /// final render. The UI surfaces these as compact caveats instead
    /// of silently implying perfect preview/render parity.
    pub preview_limitations: Vec<TimelinePreviewLimitation>,
    /// Tracks in order: video first, then audio. Empty when project
    /// has no clips.
    pub tracks: Vec<TimelineTrack>,
}

/// One known preview/render parity limitation for the current snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelinePreviewLimitation {
    /// Stable machine-readable limitation kind.
    pub kind: String,
    /// User-facing explanation.
    pub message: String,
}

/// Renderable or previewable parameter animation attached to a timeline clip.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineParameterAnimation {
    /// Stable animation id from the project metadata.
    pub id: String,
    /// Clip-local animation target.
    pub target: TimelineAnimationTarget,
    /// Ordered keyframes in seconds relative to the clip start.
    pub keyframes: Vec<TimelineKeyframe>,
    /// Behavior before the first keyframe.
    pub pre_extrapolation: String,
    /// Behavior after the last keyframe.
    pub post_extrapolation: String,
    /// Optional 2D motion path for position parameters.
    pub motion_path: Option<TimelineMotionPath>,
    /// Optional agent/user rationale for review.
    pub rationale: Option<String>,
}

/// Clip-local animation target exposed to desktop preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineAnimationTarget {
    /// Target clip id.
    pub clip_id: String,
    /// Phase 3A parameter path such as `title.opacity`.
    pub parameter: String,
}

/// One keyframe exposed to desktop preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineKeyframe {
    /// Time in seconds relative to the clip start.
    pub time_s: f64,
    /// Numeric value.
    pub value: f64,
    /// Interpolation name from proto.
    pub interpolation: String,
    /// Easing name from proto.
    pub easing: String,
    /// Optional normalized cubic Bezier handles.
    pub bezier: Option<TimelineBezierHandles>,
    /// Tangent constraint mode from proto.
    pub tangent_mode: String,
    /// Optional spring parameters.
    pub spring: Option<TimelineSpringParameters>,
}

/// Normalized cubic Bezier handles exposed to desktop preview.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineBezierHandles {
    /// Outgoing control point x, normalized to the segment duration.
    pub out_x: f64,
    /// Outgoing control point y, normalized to the value delta.
    pub out_y: f64,
    /// Incoming control point x, normalized to the segment duration.
    pub in_x: f64,
    /// Incoming control point y, normalized to the value delta.
    pub in_y: f64,
}

/// Physical spring parameters exposed to desktop preview.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineSpringParameters {
    /// Moving mass.
    pub mass: f64,
    /// Spring stiffness.
    pub stiffness: f64,
    /// Damping coefficient.
    pub damping: f64,
}

/// 2D motion path exposed to desktop preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineMotionPath {
    /// Ordered path points.
    pub points: Vec<TimelineMotionPathPoint>,
}

/// One point on a 2D motion path.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineMotionPathPoint {
    /// Time in seconds relative to the clip start.
    pub time_s: f64,
    /// Horizontal viewport-width offset.
    pub x: f64,
    /// Vertical viewport-height offset.
    pub y: f64,
    /// Optional outgoing spatial control point for the segment after this point.
    #[serde(default)]
    #[ts(optional)]
    pub outgoing_control: Option<TimelineMotionPathControlPoint>,
    /// Optional incoming spatial control point for the segment before this point.
    #[serde(default)]
    #[ts(optional)]
    pub incoming_control: Option<TimelineMotionPathControlPoint>,
}

/// Absolute 2D control point for a cubic spatial motion-path segment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineMotionPathControlPoint {
    /// Horizontal viewport-width offset.
    pub x: f64,
    /// Vertical viewport-height offset.
    pub y: f64,
}

/// Timeline-level semantic metadata for one adjacent clip boundary.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineCutBoundary {
    /// Canonical metadata key, usually `from_clip_id::to_clip_id`.
    pub key: String,
    /// Outgoing clip id used by the metadata key.
    pub from_clip_id: String,
    /// Incoming clip id used by the metadata key.
    pub to_clip_id: String,
    /// Editorial grammar, e.g. `cut_on_action`, `cutaway`, `j_cut`.
    pub cut_type: String,
    /// Short machine-readable purpose.
    pub intent: String,
    /// Optional intensity in `[0, 1]`.
    pub energy: Option<f64>,
    /// Audio-picture relationship, e.g. `sync` or `audio_leads`.
    pub audio_relation: String,
    /// Optional planner confidence in `[0, 1]`.
    pub confidence: Option<f64>,
    /// Human-readable explanation.
    pub reason: Option<String>,
}

/// Timeline-level broadcast overlay config, stored in OTIO metadata
/// under `montage.broadcast_overlay` and surfaced to desktop preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct BroadcastOverlayConfig {
    /// Whether the overlay should render.
    pub enabled: bool,
    /// Optional source template name for audit/debug display.
    pub template_name: Option<String>,
    /// Episode title shown by the title card.
    pub episode_title: String,
    /// Optional subtitle shown under the title.
    pub episode_subtitle: String,
    /// Show/brand label used by the ticker.
    pub show_name: String,
    /// Left/primary host.
    pub host_a: BroadcastHost,
    /// Right/secondary host.
    pub host_b: BroadcastHost,
    /// Sponsor and brand names used by the ticker.
    pub sponsors: Vec<String>,
    /// Timed topic labels used by the ticker.
    pub topics: Vec<BroadcastTimedEntry>,
    /// Timed chapter cards.
    pub chapters: Vec<BroadcastTimedEntry>,
    /// Optional project-relative logo path.
    pub brand_logo_path: Option<String>,
    /// Whether to suppress full long-form overlays and render only the
    /// short-form brand bar.
    pub short_form_mode: bool,
    /// Timing, colour, and layout values.
    pub style: BroadcastOverlayStyle,
}

/// Host/person data used by broadcast lower thirds.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct BroadcastHost {
    /// Display name.
    pub name: String,
    /// Role/title text.
    pub title: String,
    /// Project-relative optional portrait path.
    pub photo_path: Option<String>,
}

/// Timed text used by topic badges and chapter cards.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct BroadcastTimedEntry {
    /// Timeline time, in seconds.
    pub time_seconds: f64,
    /// Display text.
    pub text: String,
}

/// Broadcast overlay style values. These remain data-driven so private
/// skills can define their own look without hard-coding branding in
/// Montage core.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct BroadcastOverlayStyle {
    /// Primary brand gold colour.
    pub gold_hex: String,
    /// Lighter gold for split host-intro bars.
    pub gold_light_hex: String,
    /// Accent cyan for topic badges.
    pub cyan_hex: String,
    /// Dark lower-third/ticker background colour.
    pub dark_navy_hex: String,
    /// End of title-card fade-in, in seconds.
    pub title_fade_in_end: f64,
    /// Start of title-card fade-out, in seconds.
    pub title_fade_out_start: f64,
    /// End of title-card visibility, in seconds.
    pub title_visible_end: f64,
    /// Start of host-intro strip, in seconds.
    pub host_intro_start: f64,
    /// End of host-intro strip, in seconds.
    pub host_intro_end: f64,
    /// Sponsor ticker display cadence, in seconds.
    pub ticker_sponsor_duration: f64,
    /// Ticker fade duration, in seconds.
    pub ticker_fade_duration: f64,
    /// Topic badge display duration, in seconds.
    pub ticker_topic_duration: f64,
    /// Chapter-card display duration, in seconds.
    pub chapter_display_duration: f64,
    /// Host-name bar height, in pixels at 1080p reference.
    pub name_bar_height: f64,
    /// Ticker height, in pixels at 1080p reference.
    pub ticker_height: f64,
    /// Host-intro strip height, in pixels at 1080p reference.
    pub host_strip_height: f64,
}

/// One row in [`TimelineSnapshot::tracks`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TimelineTrack {
    /// Track name from the OTIO file.
    pub name: String,
    /// Track kind: `"video"` or `"audio"`.
    pub kind: String,
    /// Optional montage-specific role tag from `track.metadata`.
    /// Today's only value is `"titles"` (set by InsertTitle's
    /// auto-create); the frontend renders title-role tracks as a
    /// special amber-on-black band rather than a regular video lane.
    /// `None` for ordinary V1 / V2 / audio tracks.
    pub role: Option<String>,
    /// Audio controls for audio tracks. `None` for video/title tracks.
    pub audio: Option<TrackAudioControls>,
    /// Items in this track in playback order.
    pub items: Vec<TimelineItem>,
}

/// Audio controls surfaced for one timeline audio track.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TrackAudioControls {
    /// Semantic track role: dialogue, music, or sfx.
    pub role: String,
    /// Linear track gain multiplier.
    pub volume: f64,
    /// Whether the track is muted.
    pub muted: bool,
    /// Whether the track is soloed.
    pub solo: bool,
    /// Optional ducking settings for this track.
    pub ducking: Option<DuckingControls>,
}

/// Ducking controls for reducing a non-dialogue track under dialogue.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct DuckingControls {
    /// Whether ducking is enabled.
    pub enabled: bool,
    /// Desired gain reduction in dB.
    pub amount_db: f64,
    /// Attack time in milliseconds.
    pub attack_ms: f64,
    /// Release time in milliseconds.
    pub release_ms: f64,
}

/// What kind of media URL the frontend should play for a clip.
///
/// `Proxy` — a 1080p H.264 all-keyframe mp4 is ready; cheap to scrub.
/// `Source` — no proxy yet, but the original asset is playable; heavier.
/// `Missing` — the source asset isn't on disk; the player must show
/// an offline overlay rather than try to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum PlayableKind {
    /// A 1080p H.264 all-keyframe proxy mp4 is ready; cheap to scrub.
    Proxy,
    /// No proxy yet, but the original asset is playable; heavier to scrub.
    Source,
    /// The source asset isn't on disk; show an offline overlay.
    Missing,
}

/// Readiness summary for every media asset known to the project.
///
/// This is the shared contract the UI and agent should use when deciding
/// whether an asset is playable, still processing, missing, or blocked by
/// a decode/cache failure.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MediaReadinessSnapshot {
    /// Optional project identifier, when the caller has one.
    pub project_id: Option<String>,
    /// Unix timestamp in milliseconds for when this snapshot was built.
    #[ts(type = "number")]
    pub generated_at_ms: u64,
    /// One entry per source asset.
    pub entries: Vec<MediaReadinessEntry>,
}

/// Media-service state for one source asset.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MediaReadinessEntry {
    /// Project-relative asset id, usually the raw media path.
    pub asset_id: String,
    /// Display name shown in media and timeline panes.
    pub display_name: String,
    /// Absolute source path if the source still resolves on disk.
    pub source_path: Option<String>,
    /// Overall state for this asset.
    pub state: MediaReadinessState,
    /// Best artifact to use for playback now, if one exists.
    pub playable: Option<PlayableArtifact>,
    /// Cache/index sidecars available for this asset.
    pub cache: MediaCacheReadiness,
    /// Machine-readable reasons explaining blocked, failed, or offline states.
    pub failures: Vec<MediaFailureReason>,
    /// Estimated transcript/index progress while the local worker is active.
    pub transcript_progress: Option<MediaProcessingProgress>,
    /// Source duration in seconds, when probed.
    pub duration_s: Option<f64>,
    /// Source file size in bytes, when known.
    #[ts(type = "number | null")]
    pub source_size_bytes: Option<u64>,
    /// Last status update time in Unix milliseconds.
    #[ts(type = "number | null")]
    pub updated_at_ms: Option<u64>,
}

/// Estimated progress for one media processing task.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MediaProcessingProgress {
    /// User-visible task label, e.g. `transcribing chunk 3 / 166`.
    pub label: String,
    /// Completed work units.
    pub completed_units: u32,
    /// Total estimated work units.
    pub total_units: u32,
    /// Current 1-based work unit, when known.
    pub current_unit: Option<u32>,
    /// Unit label, e.g. `chunks`.
    pub unit: String,
    /// Rounded 0..100 progress estimate.
    pub percent: Option<u8>,
}

/// Coarse media state for UI badges, agent gating, and processing queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum MediaReadinessState {
    /// A playable artifact exists and required editor metadata is ready.
    Ready,
    /// Some artifacts are ready, but one or more requested sidecars are missing.
    Partial,
    /// A local or remote worker is actively building cache artifacts.
    Processing,
    /// The asset cannot advance until the user or environment fixes something.
    Blocked,
    /// Processing completed with an error.
    Failed,
    /// The source file cannot be found.
    Offline,
}

/// One concrete media artifact that can be handed to a playback backend.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct PlayableArtifact {
    /// Artifact class.
    pub kind: PlayableArtifactKind,
    /// Absolute path or URL. `None` means the state explains why unavailable.
    pub path: Option<String>,
    /// Decode path expected to consume this artifact.
    pub backend: MediaDecodeBackend,
    /// Container name such as `mov`, `mp4`, or `mkv`, when probed.
    pub container: Option<String>,
    /// Video codec such as `h264`, `hevc`, or `prores`, when probed.
    pub video_codec: Option<String>,
    /// Audio codec such as `aac` or `pcm_s16le`, when probed.
    pub audio_codec: Option<String>,
    /// Pixel width for video artifacts.
    pub width: Option<u32>,
    /// Pixel height for video artifacts.
    pub height: Option<u32>,
    /// Artifact duration in seconds.
    pub duration_s: Option<f64>,
}

/// Artifact classes the media service can expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum PlayableArtifactKind {
    /// Original source media.
    Source,
    /// Full proxy optimized for timeline scrubbing.
    Proxy,
    /// Lightweight compatibility artifact for preview decode.
    CompatibilityMedia,
    /// Streamed artifact produced on demand by a media service.
    Stream,
}

/// Decode backend expected to play or produce a media artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum MediaDecodeBackend {
    /// Browser/WebKit media element.
    Webkit,
    /// FFmpeg remux without visual re-encode.
    FfmpegRemux,
    /// FFmpeg transcoded media.
    FfmpegTranscode,
    /// libmpv/MPV-backed playback.
    Libmpv,
    /// OS-native player backend.
    Native,
    /// Backend is not known yet.
    Unknown,
}

/// Cache/index sidecar readiness for one asset.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MediaCacheReadiness {
    /// Timeline playback proxy.
    pub proxy: MediaCacheArtifactStatus,
    /// Browser-safe preview/cache media.
    pub compatibility_media: MediaCacheArtifactStatus,
    /// Filmstrip thumbnails.
    pub thumbnails: MediaCacheArtifactStatus,
    /// Waveform peaks sidecar.
    pub waveform: MediaCacheArtifactStatus,
    /// Word/segment transcript sidecar.
    pub transcript: MediaCacheArtifactStatus,
    /// Caption export/index sidecar.
    pub captions: MediaCacheArtifactStatus,
    /// Scene detection sidecar.
    pub scenes: MediaCacheArtifactStatus,
    /// Face detection sidecar.
    pub face_detection: MediaCacheArtifactStatus,
    /// Color analysis sidecar.
    pub color_analysis: MediaCacheArtifactStatus,
    /// Motion analysis sidecar.
    pub motion_analysis: MediaCacheArtifactStatus,
    /// Audio analysis sidecar.
    pub audio_analysis: MediaCacheArtifactStatus,
    /// Silence detection sidecar.
    pub silence_detection: MediaCacheArtifactStatus,
}

/// Status for a single generated artifact or index sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum MediaCacheArtifactStatus {
    /// No artifact exists yet.
    Missing,
    /// Work is queued or currently running.
    Pending,
    /// Artifact exists and is current.
    Ready,
    /// Artifact exists but should be refreshed.
    Stale,
    /// Last build failed.
    Failed,
    /// Artifact is intentionally not needed for this source.
    Skipped,
    /// This source cannot produce that artifact.
    Unsupported,
}

/// Machine-readable cause for media being unavailable or degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum MediaFailureReason {
    /// Source path no longer exists.
    SourceMissing,
    /// Source is outside the currently allowed project roots.
    SourceOutsideProject,
    /// Container cannot be decoded by the selected backend.
    UnsupportedContainer,
    /// Codec cannot be decoded by the selected backend.
    UnsupportedCodec,
    /// Browser/WebKit failed to decode the artifact.
    BrowserDecodeFailed,
    /// Proxy was expected but is absent.
    ProxyMissing,
    /// Proxy generation failed.
    ProxyFailed,
    /// Compatibility media was expected but is absent.
    CompatibilityMediaMissing,
    /// Compatibility media generation failed.
    CompatibilityMediaFailed,
    /// Required cache/index sidecar is missing.
    CacheMissing,
    /// Required cache/index sidecar failed.
    CacheFailed,
    /// Not enough disk space to build the requested artifact.
    DiskSpaceLow,
    /// FFmpeg could not be found or started.
    FfmpegUnavailable,
    /// Media probing failed.
    ProbeFailed,
    /// Unknown fallback reason.
    Unknown,
}

/// One drawable item on a track. Variant-tagged so the frontend can
/// render each kind differently.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum TimelineItem {
    /// A clip — references an asset, has a source range.
    Clip {
        /// Index of this item within its track. Stable across reads.
        index: usize,
        /// Display name (clip's OTIO `name` field).
        name: String,
        /// Anchor uuid for `Anchor::ClipUuid` in EDL ops. Pulled from
        /// `clip.metadata.montage.extra["clip_uuid"]` if present;
        /// otherwise falls back to the clip's display name (which the
        /// `montage_core::edl::anchor` resolver also matches against).
        /// Drag-to-trim wires this directly into
        /// `TrimClip { anchor: ClipUuid { uuid } }`.
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
        /// Absolute path the frontend should feed to `<video src>` to
        /// play this clip *right now*, regardless of whether a proxy
        /// exists. Falls back to the source asset when no proxy is
        /// ready. `None` only when the asset is genuinely missing on
        /// disk — in that case `playable_kind` is `Missing` and the
        /// player draws an offline overlay.
        #[ts(optional)]
        playable_path: Option<String>,
        /// Discriminator for what `playable_path` points at. Lets the
        /// frontend tint the timeline ruler (proxy = green, source =
        /// amber, missing = red) without re-deriving the kind from
        /// the path string.
        playable_kind: PlayableKind,
        /// Absolute path to the directory holding this asset's
        /// extracted filmstrip JPEGs (e.g.
        /// `<project>/.montage/thumbnails/<stem>-<hash>/`). The
        /// timeline canvas reads `frame-NNNN.jpg` files from this dir
        /// and tiles them across the clip's pixel width. `None` when
        /// thumbnails haven't been generated yet (the
        /// [`JobKind::Thumbnails`] job hasn't completed) or the asset
        /// doesn't resolve to a known thumbnails dir.
        thumbnail_dir: Option<String>,
        /// Absolute path to this asset's waveform-peaks JSON sidecar
        /// (e.g. `<project>/.montage/waveforms/<stem>-<hash>.json`).
        /// Frontend fetches the sidecar via the `read_waveform` Tauri
        /// command and draws a centered amplitude line across the
        /// clip's pixel width. `None` when waveform extraction
        /// hasn't completed (the [`JobKind::Waveform`] job hasn't
        /// landed) or the asset has no audio stream.
        waveform_path: Option<String>,
        /// Per-clip linear gain multiplier (`montage.volume` Effect).
        /// `None` when the clip has no volume effect; `1.0` is unity
        /// (no gain change). Frontend reads this to populate the
        /// PropertiesPane volume slider and to paint a `🔉 0.5×` badge
        /// on clips with non-default values.
        volume: Option<f64>,
        /// Per-clip playback rate multiplier (`montage.speed` Effect).
        /// `None` when the clip has no speed effect; `1.0` is unity.
        /// `2.0` plays at double speed (half timeline length).
        /// Frontend reads this to populate the PropertiesPane speed
        /// input and to paint a `⚡ 2×` badge on clips with non-default
        /// values.
        speed: Option<f64>,
        /// Per-clip audio fade in seconds from clip start.
        fade_in_s: Option<f64>,
        /// Per-clip audio fade out seconds into clip end.
        fade_out_s: Option<f64>,
        /// Incoming audio lead for a J-cut, in seconds.
        audio_lead_s: Option<f64>,
        /// Outgoing audio trail for an L-cut, in seconds.
        audio_trail_s: Option<f64>,
        /// Human-readable split-edit reason.
        split_edit_reason: Option<String>,
        /// Optional split-edit planner confidence.
        split_edit_confidence: Option<f64>,
        /// Link group shared by related video/audio clips imported
        /// from the same source.
        link_group_id: Option<String>,
        /// Whether the referenced asset has a video stream.
        has_video: Option<bool>,
        /// Whether the referenced asset has an audio stream.
        has_audio: Option<bool>,
        /// Clip-level color controls (`montage.color_correction` Effect).
        /// `None` when no correction is stamped on this clip.
        color_correction: Option<ColorCorrectionStyling>,
        /// Project-relative LUT path (`montage.lut` Effect), if present.
        lut_path: Option<String>,
        /// Title-overlay styling, populated when the clip carries an
        /// `montage.title` Effect (i.e. it's on the Titles track).
        /// `None` for ordinary media clips. The frontend renders the
        /// title editor in PropertiesPane when this is `Some` and
        /// paints the title text inline on the timeline band.
        title: Option<TitleStyling>,
        /// Video overlay styling for upper-track media clips. `None`
        /// means a regular full-frame overlay/cutaway when the clip
        /// is on an upper video track.
        video_overlay: Option<VideoOverlayStyling>,
        /// Procedural MotionScene shape styling. `None` for ordinary
        /// media clips and MotionScene text clips.
        motion_shape: Option<MotionShapeStyling>,
        /// Procedural MotionScene image styling. `None` for ordinary
        /// media clips and non-image MotionScene layers.
        motion_image: Option<MotionImageStyling>,
        /// Supported parameter animations attached to this clip.
        animations: Vec<TimelineParameterAnimation>,
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
        /// Seconds before the cut occupied by the transition.
        in_offset_s: f64,
        /// Seconds after the cut occupied by the transition.
        out_offset_s: f64,
        /// Effect name from the OTIO transition (e.g.
        /// `"SMPTE_Dissolve"`).
        effect_name: String,
        /// Stable semantic Montage transition id, when the transition
        /// carries `metadata.montage_transition`.
        transition_id: Option<String>,
        /// Semantic transition family, for example `dissolve` or
        /// `motion_blur`.
        transition_family: Option<String>,
        /// Why this visible transition belongs at the cut.
        transition_intent: Option<String>,
        /// Optional transition intensity in `[0, 1]`.
        transition_energy: Option<f64>,
        /// Optional spatial direction such as `left`, `right`, or `in`.
        transition_direction: Option<String>,
        /// Resolved transition audio behavior: `crossfade` or `cut`.
        audio_policy: Option<String>,
    },
}

/// Clip-level color controls, lifted off `montage.color_correction`.
/// Fields are optional because an EDL op can set only the controls
/// it needs; omitted fields use render defaults.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct ColorCorrectionStyling {
    /// Exposure offset in stops.
    pub exposure_ev: Option<f64>,
    /// Contrast multiplier.
    pub contrast: Option<f64>,
    /// Saturation multiplier.
    pub saturation: Option<f64>,
    /// Normalized warm/cool control.
    pub temperature: Option<f64>,
    /// Normalized green/magenta control.
    pub tint: Option<f64>,
    /// Normalized shadow control.
    pub shadows: Option<f64>,
    /// Normalized highlight control.
    pub highlights: Option<f64>,
}

/// Styling fields for a title overlay, lifted off the
/// `montage.title` Effect's metadata. Mirror of the EDL grammar
/// values — strings rather than enums on the wire so the frontend
/// can pass them straight back through `*** Set Title` without
/// having to know the typed enum names.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TitleStyling {
    /// Text the overlay renders.
    pub text: String,
    /// Vertical band: `"top"`, `"center"`, or `"bottom"`.
    pub position: String,
    /// Font size in pixels.
    pub font_size: u32,
    /// Hex colour like `"#FFFFFF"`.
    pub color: String,
    /// Font weight: `"normal"` or `"bold"`.
    pub font_weight: String,
    /// Animation: `"none"`, `"fade_in"`, `"fade_out"`, `"fade_in_out"`,
    /// `"slide_in"`, or `"slide_out"`.
    pub animation: String,
    /// Text reveal: `"none"`, `"typewriter"`, `"word"`, or `"line"`.
    pub reveal: String,
}

/// Clip-level media overlay styling, lifted off
/// `montage.video_overlay` effect metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct VideoOverlayStyling {
    /// Overlay mode: `"pip"` or `"full_frame"`.
    pub mode: String,
    /// PiP corner. `None` for full-frame overlays.
    pub corner: Option<String>,
    /// PiP width as a fraction of output width.
    pub scale: Option<f64>,
    /// PiP margin as a fraction of output width/height.
    pub margin_pct: Option<f64>,
}

/// Styling fields for a native MotionScene shape preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MotionShapeStyling {
    /// Shape primitive. The first native subset supports `"rect"`.
    pub shape: String,
    /// Normalized x coordinate in output space.
    pub x: f64,
    /// Normalized y coordinate in output space.
    pub y: f64,
    /// Normalized width in output space.
    pub width: f64,
    /// Normalized height in output space.
    pub height: f64,
    /// Fill color, usually a hex color like `"#224466"`.
    pub color: String,
    /// Fill opacity in `[0, 1]`.
    pub opacity: f64,
    /// Static scale multiplier.
    pub scale: f64,
    /// Transform origin x in `[0, 1]`.
    pub anchor_x: f64,
    /// Transform origin y in `[0, 1]`.
    pub anchor_y: f64,
    /// Static clockwise rotation in degrees.
    pub rotation_deg: f64,
}

/// Styling fields for a native MotionScene image preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct MotionImageStyling {
    /// Project-relative asset id/path.
    pub asset_id: String,
    /// Normalized x coordinate in output space.
    pub x: f64,
    /// Normalized y coordinate in output space.
    pub y: f64,
    /// Normalized width in output space.
    pub width: f64,
    /// Normalized height in output space.
    pub height: f64,
    /// Layer opacity in `[0, 1]`.
    pub opacity: f64,
    /// Fit behavior: `"cover"`, `"contain"`, or `"stretch"`.
    pub fit: String,
    /// Static scale multiplier.
    pub scale: f64,
    /// Transform origin x in `[0, 1]`.
    pub anchor_x: f64,
    /// Transform origin y in `[0, 1]`.
    pub anchor_y: f64,
    /// Static clockwise rotation in degrees.
    pub rotation_deg: f64,
}

/// One paragraph-sized segment from a whisper transcript sidecar.
/// Segment-level granularity is what the transcript pane renders as
/// a virtualized row; word-level granularity drives selection,
/// click-to-seek, and active-word highlight.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct TranscriptSegment {
    /// Stable segment id when provided by the transcript source.
    #[serde(default)]
    pub id: Option<String>,
    /// Stable phrase id when this segment maps to an alignment phrase.
    #[serde(default)]
    pub phrase_id: Option<String>,
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
    /// Stable word id for transcript-driven editing.
    #[serde(default)]
    pub id: Option<String>,
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

/// Risk tier the agent assigns to a `ProposedEdit`.
///
/// Maps to the Inspector's RiskIndicator dots:
/// `Low` → 1 dot (green), `Medium` → 2 (amber), `High` → 3 (orange),
/// `VeryHigh` → 4 (red). Color alone is never load-bearing — the label is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe to accept. The agent has high confidence and no flagged risks.
    Low,
    /// Review before accepting. Some heuristic flagged a concern.
    Medium,
    /// Likely needs revision. The agent suggests but recommends a closer look.
    High,
    /// Block by default. The user must explicitly opt in.
    VeryHigh,
}

/// One row in a `ProposedEdit`'s evidence list. Each kind maps to an
/// icon in the Inspector (see `EVIDENCE_ICON` on the frontend).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalEvidenceKind {
    /// Transcript boundary or token-level signal.
    Transcript,
    /// Audio energy drop, peak, or boundary.
    AudioEnergy,
    /// Speaker handoff detected via diarization.
    SpeakerHandoff,
    /// Silence detected (above the project's silence threshold).
    Silence,
    /// Pacing deviation vs the surrounding context.
    Pacing,
    /// Filler phrase / disfluency.
    Filler,
    /// Visual continuity / cut-boundary signal.
    Visual,
    /// Generic cut-boundary heuristic.
    CutBoundary,
}

/// One row in the Inspector's "EVIDENCE" section. Producers can set
/// either `confidence` (0..1) or `confidence_level` (categorical) —
/// when both are present, the categorical wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct ProposalEvidence {
    /// What kind of evidence this row represents.
    pub kind: ProposalEvidenceKind,
    /// Human-readable label ("Transcript boundary", "Audio energy drop").
    pub label: String,
    /// Optional confidence 0..=1 for this specific signal.
    #[serde(default)]
    #[ts(optional)]
    pub confidence: Option<f32>,
    /// Optional categorical tier. When present, overrides `confidence`
    /// for tier color rendering on the frontend.
    #[serde(default)]
    #[ts(optional)]
    pub confidence_level: Option<ConfidenceTier>,
}

/// Coarse confidence tier surfaced when a numeric score isn't meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceTier {
    /// 80–100% — safe.
    High,
    /// 55–79% — review.
    Medium,
    /// 30–54% — revise.
    Low,
    /// 0–29% — block.
    VeryLow,
}

/// Sibling proposal the user can switch to from the Inspector's
/// "ALTERNATIVES" section. This is a *summary*; switching emits a
/// fresh `ProposedEdit` with the full payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "./")]
pub struct ProposalAlternative {
    /// Stable id for this alternative. Used by the frontend when the
    /// user picks one — the backend re-emits the full ProposedEdit
    /// keyed on this id.
    pub id: Id,
    /// Short label ("Keep more context", "Tighter cut", "Hard cut").
    pub label: String,
    /// Optional one-line detail ("+0.9s", "-1.2s", "—"). Rendered in
    /// the right column of the alternative chip in mono.
    #[serde(default)]
    #[ts(optional)]
    pub detail: Option<String>,
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
    /// A b-roll clip was inserted. Indexes refer to the **proposed**
    /// snapshot.
    InsertBRoll {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the proposed snapshot.
        track_index: usize,
        /// Index of the inserted b-roll item within that track.
        item_index: usize,
    },
    /// A picture-in-picture clip was inserted. Indexes refer to the
    /// **proposed** snapshot.
    InsertPiP {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the proposed snapshot.
        track_index: usize,
        /// Index of the inserted PiP item within that track.
        item_index: usize,
    },
    /// A clip moved. `from_*` indexes refer to the original snapshot;
    /// `to_*` indexes refer to the proposed snapshot.
    Move {
        /// Index of the originating op in the EDL envelope.
        op_index: usize,
        /// Track index in the original snapshot.
        from_track_index: usize,
        /// Item index in the original snapshot.
        from_item_index: usize,
        /// Track index in the proposed snapshot.
        to_track_index: usize,
        /// Item index in the proposed snapshot.
        to_item_index: usize,
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
    fn item_approval_request_roundtrips_capability_metadata() {
        let item = Item::ApprovalRequest {
            id: Id::new("approval-1"),
            phase: ItemLifecycle::Started,
            tool_name: "start_render".into(),
            args_summary: "{\"output\":\"out.mp4\"}".into(),
            capability_metadata: serde_json::json!({
                "graph_mutates": false,
                "preview_supported": "not_supported",
                "export_supported": "supported",
                "required_indexes": [],
                "approval_required": true,
                "side_effects": ["starts an ffmpeg render job", "writes render output files"],
                "known_limitations": []
            }),
            rationale: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::ApprovalRequest {
                tool_name,
                capability_metadata,
                rationale,
                ..
            } => {
                assert_eq!(tool_name, "start_render");
                assert_eq!(capability_metadata["export_supported"], "supported");
                assert_eq!(
                    capability_metadata["side_effects"],
                    serde_json::json!([
                        "starts an ffmpeg render job",
                        "writes render output files"
                    ])
                );
                assert!(rationale.is_none());
            }
            _ => panic!("expected Item::ApprovalRequest"),
        }
    }

    /// Lock the rationale plumbing contract on the approval path:
    /// when the bridge captures `reasoning` from an `apply_edl` (or
    /// equivalent) tool call, the field must round-trip through JSON
    /// so the frontend's Brief surface and approval card can render
    /// the agent's "why" alongside the mechanical `args_summary`.
    ///
    /// Backwards-compatible: a producer that emits no `rationale` key
    /// on the wire still deserializes — this test covers the missing-
    /// field path too.
    #[test]
    fn item_approval_request_rationale_roundtrips() {
        let item = Item::ApprovalRequest {
            id: Id::new("approval-2"),
            phase: ItemLifecycle::Started,
            tool_name: "apply_edl".into(),
            args_summary: "trim 1 clip".into(),
            capability_metadata: serde_json::json!({}),
            rationale: Some("trimmed 0.42s silence per podcast defaults".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(
            json.contains("\"rationale\":\"trimmed 0.42s silence per podcast defaults\""),
            "rationale must serialize on the wire, got: {json}"
        );
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::ApprovalRequest { rationale, .. } => {
                assert_eq!(
                    rationale.as_deref(),
                    Some("trimmed 0.42s silence per podcast defaults"),
                );
            }
            _ => panic!("expected Item::ApprovalRequest"),
        }

        // Legacy producer path: a serialized payload without the
        // `rationale` key must deserialize cleanly to `None`.
        let legacy = serde_json::json!({
            "kind": "approval_request",
            "id": "approval-legacy",
            "phase": "started",
            "tool_name": "bash",
            "args_summary": "ls -l",
            "capability_metadata": {},
        })
        .to_string();
        let parsed: Item = serde_json::from_str(&legacy).unwrap();
        match parsed {
            Item::ApprovalRequest { rationale, .. } => assert!(rationale.is_none()),
            _ => panic!("expected Item::ApprovalRequest"),
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
                broadcast_overlay: None,
                cut_boundaries: vec![],
                preview_limitations: vec![],
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
            intent: None,
            explanation: None,
            confidence: None,
            risk: None,
            evidence: vec![],
            alternatives: vec![],
            rationale: None,
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
                rationale,
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
                assert!(rationale.is_none());
            }
            _ => panic!("expected Item::ProposedEdit"),
        }
    }

    /// Lock the rationale plumbing contract: when a producer fills in
    /// `rationale: Some(_)`, the field must round-trip through JSON
    /// intact so the frontend can render it in the Proposal Inspector
    /// and on the proposal-pill tooltips Wave 3 introduces. The
    /// `Option<String>` shape keeps older proposals (where the field
    /// is absent on the wire) deserializing cleanly — this test
    /// asserts both directions.
    #[test]
    fn item_proposed_edit_rationale_roundtrips() {
        let item = Item::ProposedEdit {
            id: Id::new("proposal-2"),
            phase: ItemLifecycle::Started,
            source: ProposalSource::Agent {
                tool_name: "apply_edl".into(),
            },
            edl_text: "*** Begin EDL\n*** End EDL\n".into(),
            snapshot: TimelineSnapshot {
                duration_s: 0.0,
                broadcast_overlay: None,
                cut_boundaries: vec![],
                preview_limitations: vec![],
                tracks: vec![],
            },
            diff_hints: vec![],
            summary: "trim filler".into(),
            revision: 0,
            intent: None,
            explanation: None,
            confidence: None,
            risk: None,
            evidence: vec![],
            alternatives: vec![],
            rationale: Some("trimmed 0.42s silence per podcast defaults".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(
            json.contains("\"rationale\":\"trimmed 0.42s silence per podcast defaults\""),
            "rationale must serialize on the wire, got: {json}"
        );
        let back: Item = serde_json::from_str(&json).unwrap();
        match back {
            Item::ProposedEdit { rationale, .. } => {
                assert_eq!(
                    rationale.as_deref(),
                    Some("trimmed 0.42s silence per podcast defaults")
                );
            }
            _ => panic!("expected Item::ProposedEdit"),
        }

        // Older proposals omit the field entirely; deserialization must
        // accept that and default to None.
        let legacy_json = json.replace(
            ",\"rationale\":\"trimmed 0.42s silence per podcast defaults\"",
            "",
        );
        let legacy: Item = serde_json::from_str(&legacy_json).unwrap();
        match legacy {
            Item::ProposedEdit { rationale, .. } => assert!(rationale.is_none()),
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
    fn timeline_clip_can_carry_parameter_animations() {
        let item = TimelineItem::Clip {
            index: 0,
            name: "Title".to_string(),
            clip_uuid: "title-1".to_string(),
            track_start_s: 0.0,
            duration_s: 2.0,
            asset_id: None,
            source_start_s: Some(0.0),
            proxy_path: None,
            playable_path: None,
            playable_kind: PlayableKind::Missing,
            thumbnail_dir: None,
            waveform_path: None,
            volume: None,
            speed: None,
            fade_in_s: None,
            fade_out_s: None,
            audio_lead_s: None,
            audio_trail_s: None,
            split_edit_reason: None,
            split_edit_confidence: None,
            link_group_id: None,
            has_video: Some(true),
            has_audio: Some(false),
            color_correction: None,
            lut_path: None,
            title: None,
            video_overlay: None,
            motion_shape: None,
            motion_image: None,
            animations: vec![TimelineParameterAnimation {
                id: "anim-title-opacity".to_string(),
                target: TimelineAnimationTarget {
                    clip_id: "title-1".to_string(),
                    parameter: "title.opacity".to_string(),
                },
                keyframes: vec![
                    TimelineKeyframe {
                        time_s: 0.0,
                        value: 0.0,
                        interpolation: "linear".to_string(),
                        easing: "linear".to_string(),
                        bezier: None,
                        tangent_mode: "auto".to_string(),
                        spring: None,
                    },
                    TimelineKeyframe {
                        time_s: 0.5,
                        value: 1.0,
                        interpolation: "linear".to_string(),
                        easing: "ease_out".to_string(),
                        bezier: None,
                        tangent_mode: "auto".to_string(),
                        spring: None,
                    },
                ],
                pre_extrapolation: "hold".to_string(),
                post_extrapolation: "hold".to_string(),
                motion_path: None,
                rationale: Some("Fade title in".to_string()),
            }],
        };

        let json = serde_json::to_value(item).unwrap();
        assert_eq!(
            json["animations"][0]["target"]["parameter"],
            "title.opacity"
        );
    }

    #[test]
    fn broll_anchor_roundtrips_json() {
        let anchor = BrollAnchor::ClipUuid {
            uuid: "clip-3".into(),
        };
        let json = serde_json::to_string(&anchor).unwrap();
        let back: BrollAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, anchor);
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

    #[test]
    fn media_readiness_enums_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&MediaReadinessState::Processing).unwrap(),
            "\"processing\""
        );
        assert_eq!(
            serde_json::to_string(&PlayableArtifactKind::CompatibilityMedia).unwrap(),
            "\"compatibility_media\""
        );
        assert_eq!(
            serde_json::to_string(&MediaDecodeBackend::FfmpegRemux).unwrap(),
            "\"ffmpeg_remux\""
        );
        assert_eq!(
            serde_json::to_string(&MediaCacheArtifactStatus::Unsupported).unwrap(),
            "\"unsupported\""
        );
        assert_eq!(
            serde_json::to_string(&MediaFailureReason::BrowserDecodeFailed).unwrap(),
            "\"browser_decode_failed\""
        );
    }

    #[test]
    fn media_readiness_snapshot_roundtrips_json() {
        let cache = MediaCacheReadiness {
            proxy: MediaCacheArtifactStatus::Pending,
            compatibility_media: MediaCacheArtifactStatus::Ready,
            thumbnails: MediaCacheArtifactStatus::Ready,
            waveform: MediaCacheArtifactStatus::Ready,
            transcript: MediaCacheArtifactStatus::Missing,
            captions: MediaCacheArtifactStatus::Missing,
            scenes: MediaCacheArtifactStatus::Ready,
            face_detection: MediaCacheArtifactStatus::Ready,
            color_analysis: MediaCacheArtifactStatus::Ready,
            motion_analysis: MediaCacheArtifactStatus::Ready,
            audio_analysis: MediaCacheArtifactStatus::Ready,
            silence_detection: MediaCacheArtifactStatus::Ready,
        };
        let snapshot = MediaReadinessSnapshot {
            project_id: Some("project-1".into()),
            generated_at_ms: 1_775_010_000_000,
            entries: vec![MediaReadinessEntry {
                asset_id: "raw/interview.mov".into(),
                display_name: "interview.mov".into(),
                source_path: Some("/abs/raw/interview.mov".into()),
                state: MediaReadinessState::Partial,
                playable: Some(PlayableArtifact {
                    kind: PlayableArtifactKind::CompatibilityMedia,
                    path: Some("/abs/.montage/compat/interview.mp4".into()),
                    backend: MediaDecodeBackend::Webkit,
                    container: Some("mp4".into()),
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    width: Some(1920),
                    height: Some(1080),
                    duration_s: Some(4626.8),
                }),
                cache,
                failures: vec![MediaFailureReason::ProxyMissing],
                transcript_progress: Some(MediaProcessingProgress {
                    label: "transcribing chunk 3 / 166".into(),
                    completed_units: 2,
                    total_units: 166,
                    current_unit: Some(3),
                    unit: "chunks".into(),
                    percent: Some(1),
                }),
                duration_s: Some(4626.8),
                source_size_bytes: Some(3_567_000_000),
                updated_at_ms: Some(1_775_010_000_100),
            }],
        };

        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["entries"][0]["state"], "partial");
        assert_eq!(
            json["entries"][0]["playable"]["kind"],
            "compatibility_media"
        );
        assert_eq!(json["entries"][0]["playable"]["backend"], "webkit");
        assert_eq!(json["entries"][0]["failures"][0], "proxy_missing");
        assert_eq!(
            json["entries"][0]["transcript_progress"]["total_units"],
            166
        );

        let back: MediaReadinessSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].asset_id, "raw/interview.mov");
    }

    #[test]
    fn playable_kind_serializes_as_snake_case() {
        let proxy = serde_json::to_string(&PlayableKind::Proxy).unwrap();
        let source = serde_json::to_string(&PlayableKind::Source).unwrap();
        let missing = serde_json::to_string(&PlayableKind::Missing).unwrap();
        assert_eq!(proxy, "\"proxy\"");
        assert_eq!(source, "\"source\"");
        assert_eq!(missing, "\"missing\"");
    }

    #[test]
    fn timeline_item_clip_carries_playable_fields() {
        let item = TimelineItem::Clip {
            index: 0,
            name: "x".into(),
            clip_uuid: "u".into(),
            track_start_s: 0.0,
            duration_s: 1.0,
            asset_id: Some("raw/x.mov".into()),
            source_start_s: Some(0.0),
            proxy_path: None,
            playable_path: Some("/abs/raw/x.mov".into()),
            playable_kind: PlayableKind::Source,
            thumbnail_dir: None,
            waveform_path: None,
            volume: None,
            speed: None,
            fade_in_s: None,
            fade_out_s: None,
            audio_lead_s: None,
            audio_trail_s: None,
            split_edit_reason: None,
            split_edit_confidence: None,
            link_group_id: None,
            has_video: Some(true),
            has_audio: Some(true),
            color_correction: None,
            lut_path: None,
            title: None,
            video_overlay: None,
            motion_shape: None,
            motion_image: None,
            animations: vec![],
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["playable_path"], "/abs/raw/x.mov");
        assert_eq!(json["playable_kind"], "source");
    }
}
