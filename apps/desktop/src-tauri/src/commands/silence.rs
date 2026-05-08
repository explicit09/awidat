//! Silence detection: per-asset silence ranges written as a JSON
//! sidecar at `<project>/.awidat/silences/<stem>-<hash>.json`. The
//! `find_dead_air` editorial tool reads the sidecar to surface
//! silence-trim Notes.
//!
//! Idempotency: mtime-based, same shape as proxy + waveform. A stale
//! sidecar is "looks slightly off", not "wrong cut" — we don't
//! sha-the-source on every import.
//!
//! Format: a top-level JSON object
//! `{ "ranges": [{ start_s, end_s, db_floor }, ...], threshold_db,
//! min_duration_s }`. Wrapping the array in an object lets us add
//! fields (e.g. `analyzed_at`, `algorithm_version`) without breaking
//! the find_dead_air tool's deserializer. Empty `ranges: []` means
//! "no silences at this threshold" or "asset has no audio" — both
//! are valid.

use std::path::{Path, PathBuf};

use awidat_desktop_protocol::{Id, JobKind};
use awidat_render::SilenceRange;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::commands::media::silences_path_for;
use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

/// Default silence threshold in dBFS. Picked for podcast audio:
/// typical noise floor is `-50 to -60` dBFS, conversational speech
/// peaks around `-15 to -25`. `-40` is well below speech but above
/// most rooms' noise floors.
const DEFAULT_THRESHOLD_DB: f64 = -40.0;

/// Default minimum silence duration. Below this (~breath beats),
/// silence is part of natural speech rhythm and the `find_dead_air`
/// tool won't surface a Note. Phase 1 uses `0.6` so the sidecar
/// captures plenty of candidates; the tool's threshold (~1.5s)
/// filters tighter at query time.
const DEFAULT_MIN_DURATION_S: f64 = 0.6;

/// On-disk sidecar shape. Wrapped in a struct so future fields land
/// behind a serde-friendly schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilenceSidecar {
    /// Detected silence ranges in source-time, ordered by start_s.
    pub ranges: Vec<SilenceRangeRecord>,
    /// Threshold the detector ran against, for the find_dead_air
    /// tool to know whether the cached sidecar matches its needs.
    pub threshold_db: f64,
    /// Minimum duration the detector required, ditto.
    pub min_duration_s: f64,
}

/// Serializable mirror of [`SilenceRange`] — the renderer crate's
/// type isn't `Serialize` (it's a process-internal value), so we
/// translate at the sidecar boundary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SilenceRangeRecord {
    /// Source-media seconds where silence began.
    pub start_s: f64,
    /// Source-media seconds where silence ended.
    pub end_s: f64,
    /// dBFS noise floor during the range.
    pub db_floor: f64,
}

impl From<SilenceRange> for SilenceRangeRecord {
    fn from(r: SilenceRange) -> Self {
        Self {
            start_s: r.start_s,
            end_s: r.end_s,
            db_floor: r.db_floor,
        }
    }
}

/// Generate (or refresh) the silence sidecar for one asset under a
/// specific project. Used by the post-import chain. Returns the
/// sidecar path on success.
pub async fn generate_silences_for_asset_in_project(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    project_root: &Path,
    asset_path: &Path,
) -> Result<PathBuf, String> {
    let sidecar = silences_path_for(project_root, asset_path);
    if sidecar_is_fresh(asset_path, &sidecar) {
        return Ok(sidecar);
    }
    run_one(app, state, asset_path, &sidecar).await?;
    Ok(sidecar)
}

/// Drive one ffmpeg run end-to-end with a live job card.
async fn run_one(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    asset: &Path,
    sidecar: &Path,
) -> Result<(), String> {
    let job_id = Id::new(format!(
        "silences-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(state, &job_id).await;
    let asset_label = asset
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::Silences,
        format!("scanning silences in {asset_label}"),
    );

    let result = awidat_render::generate_silences(
        asset,
        DEFAULT_THRESHOLD_DB,
        DEFAULT_MIN_DURATION_S,
        cancel.clone(),
    )
    .await;

    unregister_job(state, &job_id).await;

    match result {
        Ok(ranges) => {
            if let Some(parent) = sidecar.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    let msg = format!("create silences dir: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            }
            let payload = SilenceSidecar {
                ranges: ranges
                    .iter()
                    .copied()
                    .map(SilenceRangeRecord::from)
                    .collect(),
                threshold_db: DEFAULT_THRESHOLD_DB,
                min_duration_s: DEFAULT_MIN_DURATION_S,
            };
            let json = match serde_json::to_string(&payload) {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("serialize silences: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            };
            if let Err(e) = tokio::fs::write(sidecar, json).await {
                let msg = format!("write silences sidecar: {e}");
                emitter.err(msg.clone());
                return Err(msg);
            }
            let count = payload.ranges.len();
            emitter.ok(Some(format!(
                "silences ready: {asset_label} ({count} range{})",
                if count == 1 { "" } else { "s" },
            )));
            Ok(())
        }
        Err(awidat_render::FfmpegError::NonZero { stderr_tail, .. }) if cancel.is_cancelled() => {
            let _ = stderr_tail;
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            let msg = format!("silences: {e}");
            emitter.err(msg.clone());
            Err(msg)
        }
    }
}

/// True iff `sidecar` exists and its mtime is at-or-after `asset`'s.
fn sidecar_is_fresh(asset: &Path, sidecar: &Path) -> bool {
    let sidecar_meta = match std::fs::metadata(sidecar) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let asset_meta = match std::fs::metadata(asset) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let (Ok(sidecar_mtime), Ok(asset_mtime)) = (sidecar_meta.modified(), asset_meta.modified())
    else {
        return false;
    };
    sidecar_mtime >= asset_mtime
}

/// Read the silences sidecar at `path`. Errors when the path doesn't
/// exist or isn't valid JSON.
#[tauri::command]
pub async fn read_silences(path: String) -> Result<SilenceSidecar, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("read silences sidecar: {e}"))?;
    let parsed: SilenceSidecar =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse silences sidecar: {e}"))?;
    Ok(parsed)
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

async fn unregister_job(state: &State<'_, AwidatState>, id: &Id) {
    state.jobs.lock().await.remove(&id.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_stale_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp3");
        std::fs::write(&asset, b"x").unwrap();
        assert!(!sidecar_is_fresh(&asset, &dir.path().join("nope.json")));
    }

    #[test]
    fn sidecar_fresh_when_newer() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp3");
        std::fs::write(&asset, b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let sidecar = dir.path().join("a.json");
        std::fs::write(&sidecar, b"{}").unwrap();
        assert!(sidecar_is_fresh(&asset, &sidecar));
    }

    #[test]
    fn silence_record_roundtrips_through_json() {
        let payload = SilenceSidecar {
            ranges: vec![SilenceRangeRecord {
                start_s: 1.5,
                end_s: 4.0,
                db_floor: -45.0,
            }],
            threshold_db: -40.0,
            min_duration_s: 0.6,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: SilenceSidecar = serde_json::from_str(&s).unwrap();
        assert_eq!(back.ranges.len(), 1);
        assert!((back.ranges[0].start_s - 1.5).abs() < 1e-9);
        assert!((back.ranges[0].end_s - 4.0).abs() < 1e-9);
        assert!((back.threshold_db - -40.0).abs() < 1e-9);
    }
}
