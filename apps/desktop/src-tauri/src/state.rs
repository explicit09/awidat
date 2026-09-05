//! Tauri-managed app state. One instance per running app, threaded
//! into every command via `State<'_, MontageState>`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use montage_core::edl::{AppliedOp, EdlEnvelope};
use montage_desktop_protocol::Transcript;
use montage_proto::otio::Timeline;
use montage_render::JobManager;
use montage_render_gpu::{GpuTransitionRenderer, TransitionShader};
use tokio::sync::Mutex;
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
    /// Guards the start_turn cold-launch window before Codex has
    /// returned a concrete turn id. Without this, two UI sends can
    /// both pass the "no active turn" check while the bridge is still
    /// launching.
    pub turn_start_gate: Mutex<()>,
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
    /// In-flight import, indexing, analysis, transcode, and reframe jobs keyed
    /// by job-item id so commands and app shutdown share one lifecycle.
    pub jobs: Mutex<HashMap<String, JobHandle>>,
    /// Parent cancellation token for every job in [`Self::jobs`]. Once app
    /// shutdown begins, child tokens created by late background work start in
    /// the cancelled state instead of launching another subprocess.
    job_shutdown: CancellationToken,
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
    /// Whisper-transcript cache keyed by project root + proxy stem. Populated on
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
    /// Set when permission mode changes during an active turn. Approval policy
    /// is passed to the external app-server at launch, so the next turn must
    /// rebuild the cached session after the current turn ends.
    pub permission_dirty: std::sync::atomic::AtomicBool,
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

/// One in-flight desktop EDL proposal, created by `propose_user_edit`
/// or the visual-support planner.
/// Mutated by `adjust_proposal`; consumed (and removed from the
/// state map) by `accept_proposal` or `reject_proposal`.
pub struct PendingProposal {
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
    pub id: String,
    /// Token the run-loop watches; flipped by `cancel_turn`.
    pub cancel: CancellationToken,
}

/// Handle on a running background job owned by [`MontageState::jobs`].
pub struct JobHandle {
    /// Token the job watches; flipped by `cancel_job`.
    pub cancel: CancellationToken,
}

impl MontageState {
    /// Register a long-running job and return the token it should observe.
    pub async fn register_job(&self, id: &str) -> CancellationToken {
        let token = self.job_shutdown.child_token();
        self.jobs.lock().await.insert(
            id.to_string(),
            JobHandle {
                cancel: token.clone(),
            },
        );
        token
    }

    /// Remove a completed job from the live registry.
    pub async fn unregister_job(&self, id: &str) {
        self.jobs.lock().await.remove(id);
    }

    /// Cancel every long-running job owned by the desktop process.
    pub async fn cancel_all_jobs(&self) {
        self.job_shutdown.cancel();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let remaining = self.jobs.lock().await.len();
            if remaining == 0 {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(remaining, "timed out waiting for background jobs to stop");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    pub async fn reserve_turn_start(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        let guard = self
            .turn_start_gate
            .try_lock()
            .map_err(|_| "a turn is already running - cancel it first".to_string())?;
        if self.turn.lock().await.is_some() {
            return Err("a turn is already running - cancel it first".to_string());
        }
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_all_jobs_waits_for_registered_jobs_to_finish() {
        let state = Arc::new(MontageState::default());
        let first = state.register_job("first").await;
        let observed = first.clone();
        let cleanup_state = Arc::clone(&state);
        tokio::spawn(async move {
            first.cancelled().await;
            cleanup_state.unregister_job("first").await;
        });

        state.cancel_all_jobs().await;

        assert!(observed.is_cancelled());
        assert!(state.jobs.lock().await.is_empty());
    }

    #[tokio::test]
    async fn jobs_registered_after_shutdown_start_cancel_immediately() {
        let state = MontageState::default();
        state.cancel_all_jobs().await;

        let late = state.register_job("late").await;

        assert!(late.is_cancelled());
    }
}
