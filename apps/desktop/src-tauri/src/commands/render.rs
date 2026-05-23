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

use std::path::PathBuf;

use awidat_desktop_protocol::{Id, JobKind};
use awidat_render::{
    JobStatus, ReframeTarget, RenderPlanLimitation, build_timeline_render_spec, reframe_to_target,
};
use serde::Serialize;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

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

/// Reply from `start_reframe_render`: enough info for the frontend to
/// surface the output and (optionally) cancel via the registered job.
#[derive(Debug, Clone, Serialize)]
pub struct ReframeJobInfo {
    /// Opaque job id — pass to `cancel_reframe_render`.
    pub job_id: String,
    /// Where the reframed mp4 will land.
    pub output_path: String,
    /// Target spec width.
    pub width: u32,
    /// Target spec height.
    pub height: u32,
}

/// Reframe an already-rendered master mp4 to a platform-specific
/// target (e.g. 9:16 vertical for TikTok, 1:1 square for Instagram).
///
/// Runs ffmpeg with a center-crop + scale filter. Audio passes through
/// unchanged. Output filename includes the `target_id` so multiple
/// reframes from one master coexist under `<project>/renders/`.
#[tauri::command]
pub async fn start_reframe_render(
    app: AppHandle,
    state: State<'_, AwidatState>,
    master_path: String,
    target_id: String,
    width: u32,
    height: u32,
    video_bitrate_kbps: Option<u32>,
) -> Result<ReframeJobInfo, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let input = PathBuf::from(&master_path);
    if !input.is_file() {
        return Err(format!("master mp4 missing: {}", input.display()));
    }
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("master");
    let safe_target = target_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();
    let output_path = project_root
        .join("renders")
        .join(format!("{stem}-{safe_target}.mp4"));

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create renders dir: {e}"))?;
    }

    let job_id = Id::new(format!(
        "reframe-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(&state, &job_id).await;
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::Render,
        format!("reframe → {target_id} ({width}×{height})"),
    );

    let target = ReframeTarget {
        width,
        height,
        video_bitrate_kbps,
    };

    // Spawn so the command returns immediately with the JobInfo while
    // ffmpeg runs in the background. Progress events flow via a small
    // app-handle + id closure so we don't need to clone the
    // (non-Clone) JobEmitter.
    let output_for_task = output_path.clone();
    let input_for_task = input.clone();
    let cancel_for_task = cancel.clone();
    let app_for_progress = app.clone();
    let id_for_progress = job_id.clone();
    tokio::spawn(async move {
        let progress_cb: awidat_render::TranscodeProgressCallback =
            std::sync::Arc::new(move |p| {
                if let awidat_render::TranscodeProgress::Tick { percent, line } = p {
                    crate::events::emit_item(
                        &app_for_progress,
                        awidat_desktop_protocol::Item::Job {
                            id: id_for_progress.clone(),
                            phase: awidat_desktop_protocol::ItemLifecycle::Delta,
                            job_kind: JobKind::Render,
                            percent,
                            status: line,
                            result: None,
                            output_path: None,
                        },
                    );
                }
            });
        let result = reframe_to_target(
            &input_for_task,
            &output_for_task,
            target,
            Some(progress_cb),
            cancel_for_task,
        )
        .await;
        match result {
            Ok(()) => {
                emitter.ok_with_path(
                    Some(format!("reframe complete")),
                    Some(output_for_task.to_string_lossy().into_owned()),
                );
            }
            Err(awidat_render::FfmpegError::NonZero { stderr_tail, .. })
                if stderr_tail == "cancelled" =>
            {
                emitter.cancelled();
            }
            Err(e) => {
                emitter.err(format!("reframe: {e}"));
            }
        }
    });

    Ok(ReframeJobInfo {
        job_id: job_id.0,
        output_path: output_path.to_string_lossy().into_owned(),
        width,
        height,
    })
}

/// Cancel an in-flight reframe job by its id.
#[tauri::command]
pub async fn cancel_reframe_render(
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<(), String> {
    let mut jobs = state.jobs.lock().await;
    if let Some(handle) = jobs.remove(&job_id) {
        handle.cancel.cancel();
    }
    Ok(())
}

async fn register_job(state: &State<'_, AwidatState>, id: &Id) -> CancellationToken {
    let token = CancellationToken::new();
    state.jobs.lock().await.insert(
        id.0.clone(),
        JobHandle {
            cancel: token.clone(),
        },
    );
    token
}
