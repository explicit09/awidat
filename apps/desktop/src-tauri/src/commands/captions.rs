//! Caption sidecar export — writes `<project>/renders/captions.srt`
//! and `captions.vtt` from the timeline's transcript cues.
//!
//! The cues are remapped to *timeline-time*, not raw source-time, so
//! cuts/trims/inserts the user has made in the Edit screen flow into
//! the exported sidecar. Implementation reuses
//! `awidat_core::tools::export_package::collect_timeline_cues` which
//! is the same algorithm `export_package` relies on for its package
//! exports.
//!
//! Captions on the Deliver page are sidecar-only (SRT + VTT files
//! the user uploads to YouTube/TikTok/etc. separately). Burn-in
//! captions happen in the Edit / agent flow, not here.

use std::path::PathBuf;

use awidat_core::tools::export_package::collect_timeline_cues;
use awidat_proto::project::Project;
use awidat_proto::subtitle::{format_srt, format_vtt};
use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

/// Reply from `export_caption_sidecars`: the two sidecar paths the
/// frontend can reveal in Finder.
#[derive(Debug, Clone, Serialize)]
pub struct CaptionSidecarPaths {
    /// Absolute path of the written SRT file.
    pub srt_path: String,
    /// Absolute path of the written VTT file.
    pub vtt_path: String,
    /// Number of cues emitted.
    pub cue_count: usize,
}

/// Write `captions.srt` + `captions.vtt` under `<project>/renders/`
/// from the current timeline. Returns the absolute paths so the
/// frontend can show "Open in Finder" actions.
///
/// Errors when:
///   - no project is loaded
///   - the timeline produced zero cues (no transcribed clips on
///     video tracks AND no editable subtitle tracks)
#[tauri::command]
pub async fn export_caption_sidecars(
    state: State<'_, AwidatState>,
) -> Result<CaptionSidecarPaths, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    // Project::read + collect_timeline_cues are sync — keep them off
    // the runtime worker so a multi-track timeline doesn't block the
    // event loop. The collected cues hold owned strings; safe to send.
    let project_root_for_task = project_root.clone();
    let (cues, srt_path, vtt_path) =
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let project = Project::read(&project_root_for_task)
                .map_err(|e| format!("read project: {e}"))?;
            let cues = collect_timeline_cues(&project_root_for_task, &project)
                .map_err(|e| format!("collect cues: {e}"))?;
            let renders_dir = project_root_for_task.join("renders");
            std::fs::create_dir_all(&renders_dir)
                .map_err(|e| format!("create renders dir: {e}"))?;
            let srt_path = renders_dir.join("captions.srt");
            let vtt_path = renders_dir.join("captions.vtt");
            Ok((cues, srt_path, vtt_path))
        })
        .await
        .map_err(|e| format!("join: {e}"))??;

    if cues.is_empty() {
        return Err(
            "no caption cues found — transcribe the source media first \
             or author an editable subtitle track."
                .into(),
        );
    }

    let srt_text = format_srt(&cues);
    let vtt_text = format_vtt(&cues);
    write_sidecar(&srt_path, &srt_text).await?;
    write_sidecar(&vtt_path, &vtt_text).await?;

    Ok(CaptionSidecarPaths {
        srt_path: srt_path.to_string_lossy().into_owned(),
        vtt_path: vtt_path.to_string_lossy().into_owned(),
        cue_count: cues.len(),
    })
}

async fn write_sidecar(path: &PathBuf, contents: &str) -> Result<(), String> {
    tokio::fs::write(path, contents)
        .await
        .map_err(|e| format!("write {}: {e}", path.display()))
}
