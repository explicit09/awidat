//! Tauri-managed app state. One instance per running app, threaded
//! into every command via `State<'_, AwidatState>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use awidat_core::Session;
use awidat_core::edl::{AppliedOp, EdlEnvelope};
use awidat_core::tool::ApprovalDecision;
use awidat_proto::otio::Timeline;
use awidat_render::JobManager;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

/// All app-level state that has to outlive a single command call.
#[derive(Default)]
pub struct AwidatState {
    /// Lazily-built `Session`. Reset to `None` when the project root
    /// changes so the next turn rebuilds against the new root.
    pub session: Mutex<Option<Arc<Session>>>,
    /// Active turn, if any. Set by `start_turn`, cleared on
    /// TurnEnd / Error / cancel.
    pub turn: Mutex<Option<TurnHandle>>,
    /// Project root the next-built `Session` will use. Set by
    /// `set_project_root` (or its callers like `init_project`).
    /// Defaulted from `AWIDAT_DESKTOP_PROJECT` env var on startup so
    /// dev runs work without configuring.
    pub project_root: Mutex<Option<PathBuf>>,
    /// Pending approval requests awaiting the user's decision, keyed
    /// by the `call_id` the frontend received in the matching
    /// `ApprovalRequest` Item.
    pub pending_approvals: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    /// Pending `request_user_input` calls keyed by `call_id`.
    pub pending_inputs: Mutex<HashMap<String, oneshot::Sender<String>>>,
    /// In-flight long jobs (yt-dlp / indexing) keyed by job-item id,
    /// so a `cancel_job` command can find them. Tracking by id rather
    /// than a single global slot lets concurrent jobs run (e.g. an
    /// import while indexing of a previously-imported asset
    /// continues, in a future commit). Read by the import/index
    /// commands in the next commit.
    #[allow(dead_code)]
    pub jobs: Mutex<HashMap<String, JobHandle>>,
    /// Latest media-pane state pushed by the frontend: which proxy
    /// is loaded and where the user has the playhead. Prefixed onto
    /// `start_turn` user input so the agent knows what's on screen.
    /// `None` when nothing is loaded or no playback events have
    /// arrived yet.
    pub view_state: Mutex<Option<ViewState>>,
    /// Background ffmpeg jobs the desktop owns directly — currently
    /// only timeline exports. The agent's `start_render` tool runs
    /// inside a Session and uses Session's own job_manager; this one
    /// is for desktop-initiated renders that don't go through the
    /// agent (Export button).
    pub render_jobs: JobManager,
    /// In-flight EDL proposals awaiting user accept / reject /
    /// adjust. Keyed by call_id (agent path) or a freshly-allocated
    /// id (user path). The Mutex is async because the proposal
    /// lifecycle hops between the bridge thread (when the agent
    /// proposes) and the command-thread pool (when the user
    /// responds).
    pub pending_proposals: Mutex<HashMap<String, PendingProposal>>,
}

/// One in-flight EDL proposal. Created by the bridge when an agent
/// `apply_edl` lands in the approval channel, or by
/// `propose_user_edit` for drag-to-trim / transcript-delete flows.
/// Mutated by `adjust_proposal`; consumed (and removed from the
/// state map) by `accept_proposal` or `reject_proposal`.
///
/// Several fields are unread until the commands::proposal module
/// (next commits) consumes them; allow(dead_code) keeps the warning
/// out while the struct shape lands ahead of its consumers.
#[allow(dead_code)]
pub struct PendingProposal {
    /// Stable identifier — matches the agent's tool `call_id` for
    /// agent-initiated proposals, freshly-allocated for user-initiated.
    pub call_id: String,
    /// Project root the proposal applies to. Captured at proposal
    /// time so the accept path doesn't have to re-resolve project
    /// state if the user changed projects mid-proposal (we'd reject
    /// in that case anyway, but defense in depth).
    pub project_root: PathBuf,
    /// Untouched current EDL envelope. Replaced on each
    /// `adjust_proposal` call. The accept path serializes the
    /// adjusted envelope back through `Project::write` after a
    /// final apply.
    pub envelope: EdlEnvelope,
    /// Cached current timeline as it was when the proposal opened.
    /// `apply()` runs against a clone of this; commit also writes
    /// the post-apply state (no second apply needed).
    pub original_timeline: Timeline,
    /// Result of applying `envelope` against `original_timeline`.
    /// Re-computed on each `adjust_proposal`.
    pub proposed_timeline: Timeline,
    /// `apply()` outcome, for diff-hints + final commit logging.
    pub applied: Vec<AppliedOp>,
    /// Monotonic adjustment counter. Bumped on each
    /// `adjust_proposal` so the frontend can drop stale Deltas
    /// from rapid drag races. Starts at 0 (the initial Started
    /// emit).
    pub revision: u32,
    /// Reply oneshot for agent-initiated proposals. `None` for
    /// user-initiated. On accept we drop or send `Allow` per
    /// the "Deny + apply user's version" semantics described in
    /// the plan.
    pub reply: Option<oneshot::Sender<ApprovalDecision>>,
}

/// Snapshot of what the user is looking at in the media pane.
/// Pushed by the frontend on scrub / play / pause, consumed by
/// `start_turn`'s context-injection step.
#[derive(Debug, Clone)]
pub struct ViewState {
    /// Stem of the asset currently loaded in the preview.
    pub stem: String,
    /// Playhead position in seconds.
    pub current_time_s: f64,
    /// Whether the player is actively playing. Mostly for context
    /// flavor — "user paused at 0:23" reads differently from "user
    /// is watching at 0:23."
    pub is_playing: bool,
}

/// Handle on a running turn. Owned by `AwidatState::turn`.
pub struct TurnHandle {
    /// Token the run-loop watches; flipped by `cancel_turn`.
    pub cancel: CancellationToken,
}

/// Handle on a running long-job (import, indexing). Owned by
/// `AwidatState::jobs[job_id]`. Used by the import/index commands
/// in the next commit; predeclared here so the state-layout commit
/// is self-contained.
#[allow(dead_code)]
pub struct JobHandle {
    /// Token the job watches; flipped by `cancel_job`.
    pub cancel: CancellationToken,
}
