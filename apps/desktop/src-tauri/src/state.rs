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
