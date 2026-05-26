//! Tauri-managed app state. One instance per running app, threaded
//! into every command via `State<'_, AwidatState>`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use awidat_core::Session;
use awidat_core::edl::{AppliedOp, EdlEnvelope};
use awidat_core::tool::ApprovalDecision;
use awidat_desktop_protocol::Transcript;
use awidat_proto::otio::Timeline;
use awidat_render::JobManager;
use awidat_render_gpu::{GpuTransitionRenderer, TransitionShader};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

/// All app-level state that has to outlive a single command call.
#[derive(Default)]
pub struct AwidatState {
    /// Combined session slot — holds the lazily-built `Session` *and*
    /// the resume log path the next session should attach to, under a
    /// single mutex. Atomic swap is the load-bearing invariant: when
    /// the user loads an old chat the prior session's recorder is
    /// shutdown-and-flushed BEFORE the new resume path is set, so two
    /// recorders can never share a log file.
    ///
    /// Older code reached into `session` and `resume_log_path` as two
    /// independent mutexes; that produced the "ghost session" race
    /// where a stale recorder kept writing to the previous chat after
    /// the user picked a different one. See [`ActiveSession`].
    pub active: Mutex<ActiveSession>,
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
    ///
    /// **Step 8:** unused — codex's approval channel isn't bridged
    /// yet. Field kept so [`crate::bridges`] still compiles while the
    /// step-8b wiring is in flight.
    #[allow(dead_code)]
    pub pending_approvals: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    /// Pending `request_user_input` calls keyed by `call_id`.
    ///
    /// **Step 8:** unused — same rationale as
    /// [`Self::pending_approvals`].
    #[allow(dead_code)]
    pub pending_inputs: Mutex<HashMap<String, oneshot::Sender<String>>>,
    /// In-flight long jobs (yt-dlp / indexing) keyed by job-item id,
    /// so a `cancel_job` command can find them. Tracking by id rather
    /// than a single global slot lets concurrent jobs run (e.g. an
    /// import while indexing of a previously-imported asset
    /// continues, in a future commit). Read by the import/index
    /// commands in the next commit.
    #[allow(dead_code)]
    pub jobs: Mutex<HashMap<String, JobHandle>>,
    /// Proxy output paths currently owned by an active ffmpeg writer.
    /// Project-load backfill and UI-triggered backfill can overlap, so
    /// proxy generation needs a per-artifact gate rather than relying
    /// on job-card state that arrives asynchronously in the frontend.
    pub active_proxy_transcodes: Mutex<HashSet<PathBuf>>,
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
    /// Whisper-transcript cache keyed by proxy stem. Populated on
    /// first `read_transcript(stem)` call; invalidated when a
    /// whisper-indexer job completes (signal that the sidecar may
    /// have been refreshed). Keeps the transcript pane snappy on
    /// tab toggles — a 4 MB sidecar parses in single-digit ms but
    /// re-parsing on every tab click adds up.
    pub transcript_cache: Mutex<HashMap<String, Transcript>>,
    /// Tiny localhost range server for preview media. WKWebView's
    /// `asset:` protocol is silent for some mp4 audio tracks, while
    /// blob URLs load multi-GB proxies into RAM. This streams files
    /// over `http://127.0.0.1` with Range support instead.
    pub media_server: MediaServerState,
    /// One [`GpuTransitionRenderer`] per shader, lazily initialized
    /// on the first preview call. wgpu device/queue/pipeline creation
    /// runs ~100ms+ on cold start so we keep them warm across scrub
    /// frames. Wrapped in `Arc` so the command can drop the lock
    /// before doing the GPU work itself.
    pub gpu_preview_renderers: Mutex<HashMap<TransitionShader, Arc<GpuTransitionRenderer>>>,
}

/// Shared state for the localhost media streamer.
#[derive(Default)]
pub struct MediaServerState {
    /// Lazily initialized server and file-token map. Uses a std mutex
    /// because the serving thread is not async.
    pub inner: StdMutex<Option<MediaServerInner>>,
}

/// Running media server state.
pub struct MediaServerInner {
    /// Bound localhost port.
    pub port: u16,
    /// Random-ish token to canonical file path.
    pub files: Arc<StdMutex<HashMap<String, PathBuf>>>,
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

/// Slot that owns the current `Session` and the rollout log it should
/// resume from. Held under a single `Mutex` in [`AwidatState::active`]
/// so swapping the two — clearing the old session, setting a new
/// resume path, building a new session — is atomic and serializes
/// against `start_turn` reads.
///
/// Why this matters: when the user picks a different chat from the
/// history rail, we MUST be sure that no concurrent turn is mid-write
/// to the old log file. The previous design used two independent
/// mutexes (`session`, `resume_log_path`) and races were possible
/// where the old session kept writing while the new resume path was
/// already set. Symptom: a single rollout file with two
/// `SessionMeta` lines, then partial messages, then resume fails
/// silently and the next turn looks "stuck".
#[derive(Default)]
pub struct ActiveSession {
    /// Lazily-built session — `None` until `start_turn` constructs
    /// one (resuming from `resume_log_path` when present).
    pub session: Option<Arc<Session>>,
    /// Rollout log the next-built session should resume from. `None`
    /// for a fresh chat; `Some(path)` when the user loaded a prior
    /// session.
    pub resume_log_path: Option<PathBuf>,
}

impl ActiveSession {
    /// Atomically replace this slot's contents. Called by the
    /// chat-history commands when the user switches chats; the
    /// previous session (if any) is shut down and flushed before
    /// the new resume path lands so a follow-up `start_turn` can
    /// only see the new state.
    pub async fn replace(&mut self, resume_log_path: Option<PathBuf>) {
        if let Some(session) = self.session.take() {
            session.shutdown_recorder().await;
        }
        self.resume_log_path = resume_log_path;
    }

    /// Clear the session pointer without touching the resume path.
    /// Used by project-root changes — the next turn rebuilds against
    /// the new project root, but if a resume_log_path was set it
    /// stays the responsibility of the caller to clear/reset.
    pub async fn clear_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.shutdown_recorder().await;
        }
    }
}

/// Handle on a running turn. Owned by `AwidatState::turn`.
pub struct TurnHandle {
    /// Stable id for the active turn. Completion tasks use this to
    /// avoid clearing a newer turn if an older cleanup path runs late.
    /// Read-after-write for the codex subprocess driver in step 8 is
    /// in the reader task itself; the surface API doesn't expose it
    /// yet, so `#[allow(dead_code)]` keeps the warning quiet until
    /// step 8b adds the turn-correlation in turn-end events.
    #[allow(dead_code)]
    pub id: String,
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
