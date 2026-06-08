//! Tauri-managed app state. One instance per running app, threaded
//! into every command via `State<'_, MontageState>`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use montage_core::edl::{AppliedOp, EdlEnvelope};
use montage_core::tool::ApprovalDecision;
use montage_desktop_protocol::Transcript;
use montage_proto::otio::Timeline;
use montage_render::JobManager;
use montage_render_gpu::{GpuTransitionRenderer, TransitionShader};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

/// All app-level state that has to outlive a single command call.
///
/// **Step 8d:** the legacy `montage_core::Session` is no longer driven
/// from the desktop. Each turn now spawns a `codex-exec` subprocess
/// (see [`crate::codex_runner`]), so the prior `ActiveSession` slot,
/// `pending_approvals`, and `pending_inputs` were dead containers and
/// have been removed.
#[derive(Default)]
pub struct MontageState {
    /// Live codex bridge for the currently-open project. `None` when
    /// no project is open or after teardown (project switch, app
    /// shutdown). Lazily constructed on the first `start_turn` for a
    /// given `project_root`; rebuilt when the project changes
    /// (different cwd / MCP env override).
    pub codex: Mutex<Option<crate::codex_session::CodexSession>>,
    /// Watcher tailing `<project>/.montage/generated-media/registry.json`
    /// and emitting `Item::Job { kind: GeneratedMedia, … }` lifecycle
    /// events so the chat UI shows external video-gen progress. One
    /// per opened project; torn down on close / app exit.
    pub generated_media_watcher:
        Mutex<Option<crate::generated_media_watcher::GeneratedMediaWatcher>>,
    /// Active turn, if any. Set by `start_turn`, cleared on
    /// TurnEnd / Error / cancel.
    pub turn: Mutex<Option<TurnHandle>>,
    /// Project root the codex subprocess will be invoked against.
    /// Set by `set_project_root` (or its callers like `init_project`).
    /// Defaulted from `MONTAGE_DESKTOP_PROJECT` env var on startup so
    /// dev runs work without configuring.
    pub project_root: Mutex<Option<PathBuf>>,
    /// Thin authenticated HTTPS client of the `montage-social-server` (Phase 5).
    /// `None` until initialized in the Tauri `.setup()` hook from
    /// `MONTAGE_SOCIAL_SERVER_URL` plus either
    /// `MONTAGE_SOCIAL_SUPABASE_ACCESS_TOKEN` (multi-user) or
    /// `MONTAGE_SOCIAL_AUTH_TOKEN` (local dev); stays `None` when the server URL
    /// is unconfigured, and the `social_*` commands then surface a clear
    /// "social client not initialized" error. Provider token material still
    /// lives server-side.
    pub social_client: Mutex<Option<crate::social_client::SocialClient>>,
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
    /// only timeline exports. The codex subprocess owns its own
    /// process-isolated job state for agent-driven renders; this one
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
    /// Per-render upload fan-out (W5.A2). Mirrors the frontend's
    /// `RenderQueueEntry.uploadTargets` so the publishing pipeline can
    /// chain `render done → uploading → published / failed` per target
    /// without the frontend having to drive each transition itself.
    pub upload_queue: crate::publishing::UploadQueue,
    /// Cancel handle + task join handle for an in-flight "Sign in with
    /// ChatGPT" OAuth login. A later auth action (set API key / sign out / a
    /// second sign-in) shuts the pending callback server down *and awaits the
    /// task* so codex can't finish persisting ChatGPT credentials after the
    /// newer choice was written. See [`crate::commands::auth`].
    pub pending_oauth: Mutex<
        Option<(
            montage_auth::ShutdownHandle,
            tauri::async_runtime::JoinHandle<()>,
        )>,
    >,
    /// Monotonic id source for OAuth logins.
    pub oauth_generation: std::sync::atomic::AtomicU64,
    /// Id of the login that is currently "the choice". A pending OAuth task only
    /// applies its result if this still equals its own id (else it was
    /// superseded or cancelled). `0` means none.
    pub current_oauth_id: std::sync::atomic::AtomicU64,
    /// Set when an auth change happened during an active turn (we can't swap
    /// credentials mid-turn). The next `start_turn` honors it by tearing the
    /// session down and relaunching with the new `auth.json`.
    pub auth_dirty: std::sync::atomic::AtomicBool,
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

/// Handle on a running turn. Owned by `MontageState::turn`.
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
/// `MontageState::jobs[job_id]`. Used by the import/index commands
/// in the next commit; predeclared here so the state-layout commit
/// is self-contained.
#[allow(dead_code)]
pub struct JobHandle {
    /// Token the job watches; flipped by `cancel_job`.
    pub cancel: CancellationToken,
}
