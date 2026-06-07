//! Single-frame still-image exports for the Deliver page. The Deliver
//! UI exposes two flavors:
//!
//!   * **Cover** — a thumbnail for the published episode (YouTube cover,
//!     podcast art). Written as `<project>/renders/cover.png` so the
//!     filename is stable across re-exports.
//!   * **Custom frame** — a one-off still at a user-chosen timecode.
//!     Written with the timecode in the filename so multiple frames
//!     coexist.
//!
//! Both run through `montage_render::extract_frame` (a single ffmpeg
//! `-ss <t> -frames:v 1` invocation) — no transcoding pipeline.

use std::path::PathBuf;

use montage_render::{ImageFormat, extract_frame};
use serde::Serialize;
use tauri::State;

use crate::state::MontageState;

/// Output kind for the still export.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StillKind {
    /// Cover image (stable filename, overwritten on each export).
    Cover,
    /// Ad-hoc frame at a user-chosen timecode.
    Custom,
}

/// Reply with the absolute output path so the frontend can "Open in
/// Finder" the result.
#[derive(Debug, Clone, Serialize)]
pub struct StillExportInfo {
    /// Absolute path of the written image.
    pub output_path: String,
    /// Image format used.
    pub format: &'static str,
}

/// Extract one frame from `asset_path` at `t_s` seconds (source-time)
/// and write it under `<project>/renders/`.
///
/// `kind == Cover` writes `renders/cover.png`; `kind == Custom` writes
/// `renders/frame-<mmss>.png`. Width/height come from the source —
/// the Deliver page doesn't yet expose a resize control for stills.
#[tauri::command]
pub async fn export_still(
    state: State<'_, MontageState>,
    asset_path: String,
    t_s: f64,
    kind: StillKind,
) -> Result<StillExportInfo, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let asset = PathBuf::from(&asset_path);
    if !asset.is_file() {
        return Err(format!("asset missing: {}", asset.display()));
    }
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(format!("invalid timecode: {t_s}"));
    }

    let bytes = extract_frame(&asset, t_s, ImageFormat::Png, None)
        .await
        .map_err(|e| format!("extract frame: {e}"))?;

    let renders_dir = project_root.join("renders");
    tokio::fs::create_dir_all(&renders_dir)
        .await
        .map_err(|e| format!("create renders dir: {e}"))?;
    let filename = match kind {
        StillKind::Cover => "cover.png".to_string(),
        StillKind::Custom => {
            let mm = (t_s as u64) / 60;
            let ss = (t_s as u64) % 60;
            let ms = ((t_s - t_s.floor()) * 1000.0) as u64;
            format!("frame-{mm:02}{ss:02}{ms:03}.png")
        }
    };
    let output_path = renders_dir.join(filename);
    tokio::fs::write(&output_path, &bytes)
        .await
        .map_err(|e| format!("write {}: {e}", output_path.display()))?;

    Ok(StillExportInfo {
        output_path: output_path.to_string_lossy().into_owned(),
        format: "png",
    })
}
