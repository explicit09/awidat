//! Desktop-initiated timeline render — the Export button's backend.
//!
//! Three Tauri commands, frontend-driven polling. We don't push
//! progress through the protocol Item channel because:
//!
//! 1. The render path doesn't go through `Session`, so there's no
//!    natural broadcast subscriber to plug into.
//! 2. `JobManager` exposes a watch-channel snapshot (`status()`) but
//!    not a broadcast — we'd have to spawn our own task to bridge.
//! 3. The polling cadence the frontend wants (~500ms) is fine for a
//!    progress bar; pushing on every ffmpeg progress line would be
//!    noisier than useful.
//!
//! So: frontend kicks off the render, polls every 500ms, synthesizes
//! `Item::Job` events into the agent store so JobCard renders the
//! same UI as imports / indexing without a code path fork.

use awidat_render::{JobStatus, RenderPlanLimitation, build_timeline_render_spec};
use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

/// Reply from `start_timeline_render`: enough info for the frontend
/// to start polling and to wire up "Show in Finder" later.
#[derive(Debug, Clone, Serialize)]
pub struct RenderJobInfo {
    /// JobId stringified — opaque to the frontend, passed back in
    /// `poll_timeline_render` and `cancel_timeline_render`.
    pub job_id: String,
    /// Where the output mp4 will land. Frontend stashes this so the
    /// "Show in Finder" button has it without re-polling.
    pub output_path: String,
    /// Total source duration in seconds, summed across timeline
    /// clips. Frontend uses it for "0:23 of 0:56" status text when
    /// JobStatus's own time_done_s arrives.
    pub total_duration_s: Option<f64>,
    /// Non-fatal planning limitations for metadata the renderer ignored.
    pub render_limitations: Vec<RenderPlanLimitation>,
}

/// Plan + start a timeline render. Returns immediately with the
/// JobId; the actual ffmpeg invocation runs in the background.
#[tauri::command]
pub async fn start_timeline_render(state: State<'_, AwidatState>) -> Result<RenderJobInfo, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    // build_timeline_render_spec is sync (reads OTIO from disk + walks).
    // Wrap in spawn_blocking to keep the runtime free.
    let project_root_for_spec = project_root.clone();
    let spec = tokio::task::spawn_blocking(move || {
        awidat_core::lessons::apply_learned_project_format_defaults(&project_root_for_spec)
            .map_err(|e| format!("learned defaults: {e}"))?;
        build_timeline_render_spec(&project_root_for_spec).map_err(|e| format!("plan: {e}"))
    })
    .await
    .map_err(|e| format!("plan join: {e}"))??;

    // Make sure renders/ exists before ffmpeg tries to write into it.
    if let Some(parent) = spec.output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create renders dir: {e}"))?;
    }

    let output_path = spec.output_path.to_string_lossy().into_owned();
    let total_duration_s = spec.total_duration_s;
    let render_limitations = spec.limitations.clone();
    let job_id = state
        .render_jobs
        .start(spec)
        .await
        .map_err(|e| format!("start: {e}"))?;

    Ok(RenderJobInfo {
        job_id: job_id.to_string(),
        output_path,
        total_duration_s,
        render_limitations,
    })
}

/// Read the latest status snapshot for a render. Frontend polls this
/// every 500ms while the job is non-terminal.
#[tauri::command]
pub async fn poll_timeline_render(
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<JobStatus, String> {
    let id = awidat_render::JobId(job_id);
    state
        .render_jobs
        .status(&id)
        .await
        .map_err(|e| format!("poll: {e}"))
}

/// Cancel an in-flight render. Idempotent — already-terminal jobs
/// return Ok.
#[tauri::command]
pub async fn cancel_timeline_render(
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<(), String> {
    let id = awidat_render::JobId(job_id);
    state
        .render_jobs
        .cancel(&id)
        .await
        .map_err(|e| format!("cancel: {e}"))
}
