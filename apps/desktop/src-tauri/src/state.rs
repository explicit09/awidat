//! Tauri-managed app state. One instance per running app, threaded
//! into every command via `State<'_, AwidatState>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use awidat_core::Session;
use awidat_core::tool::ApprovalDecision;
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
