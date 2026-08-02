//! Audio waveform extraction: per-asset peak amplitudes written as a
//! JSON sidecar at `<project>/.montage/waveforms/<stem>-<hash>.json`.
//! The timeline canvas reads the sidecar via `read_waveform` and
//! draws a centered amplitude line under each audio clip.
//!
//! Idempotency: mtime-based, same shape as proxy + thumbnails. A
//! stale sidecar is "looks slightly off", not "wrong cut" — we don't
//! sha-the-source on every import.
//!
//! Format: a top-level JSON object
//! `{ "buckets": [f32, ...], "duration_s": f64 }`. We wrap the array in
//! an object so future-us can add fields without breaking the frontend's
//! deserializer; `duration_s` (the audio span the buckets cover) is
//! `#[serde(default)]` so sidecars written before it existed still parse.
//! Empty `buckets: []` means "asset has no audio stream"; the frontend
//! treats it the same as a missing sidecar.

use std::path::{Path, PathBuf};

use montage_desktop_protocol::{Id, JobKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::media::waveform_path_for;
use crate::events::JobEmitter;
use crate::state::MontageState;

/// On-disk sidecar shape. Wrapped in a struct so future fields land
/// behind a serde-friendly schema; the frontend mirrors this in
/// `waveformCache.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformSidecar {
    /// Peak amplitudes in `[0.0, 1.0]`, one per bucket. Empty when
    /// the asset has no audio stream.
    pub buckets: Vec<f32>,
    /// Total audio duration (seconds) the buckets span. The timeline
    /// uses this to map a trimmed clip's source range onto the bucket
    /// array. Optional + defaulted so sidecars written before this field
    /// existed still parse (the frontend falls back when it's missing).
    #[serde(default)]
    pub duration_s: f64,
}

/// Default bucket count per asset. ~4 KB JSON; resampled to display
/// width by the canvas at draw time.
const DEFAULT_BUCKET_COUNT: usize = 2048;

/// Generate (or refresh) the waveform sidecar for one asset under a
/// specific project. Used by the post-import chain. Returns the
/// sidecar path on success, or `None` if the asset has no audio.
pub async fn generate_waveform_for_asset_in_project(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: &Path,
    asset_path: &Path,
) -> Result<Option<PathBuf>, String> {
    let sidecar = waveform_path_for(project_root, asset_path);
    if sidecar_is_fresh(asset_path, &sidecar) {
        return Ok(if has_buckets(&sidecar) {
            Some(sidecar)
        } else {
            None
        });
    }
    run_one(app, state, asset_path, &sidecar).await
}

/// Drive one ffmpeg run end-to-end with a live job card.
async fn run_one(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    asset: &Path,
    sidecar: &Path,
) -> Result<Option<PathBuf>, String> {
    let job_id = Id::new(format!(
        "waveform-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = state.register_job(&job_id.0).await;
    let asset_label = asset
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::Waveform,
        format!("scanning audio for {asset_label}"),
    );

    let result =
        montage_render::generate_waveform(asset, DEFAULT_BUCKET_COUNT, cancel.clone()).await;

    state.unregister_job(&job_id.0).await;

    match result {
        Ok(wave) => {
            // Write the sidecar even when buckets is empty — the
            // empty case is "asset has no audio" and the mtime-fresh
            // path needs *some* file to skip on next import.
            if let Some(parent) = sidecar.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    let msg = format!("create waveforms dir: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            }
            let is_empty = wave.buckets.is_empty();
            let payload = WaveformSidecar {
                buckets: wave.buckets,
                duration_s: wave.duration_s,
            };
            let json = match serde_json::to_string(&payload) {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("serialize waveform: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            };
            if let Err(e) = tokio::fs::write(sidecar, json).await {
                let msg = format!("write waveform sidecar: {e}");
                emitter.err(msg.clone());
                return Err(msg);
            }
            if is_empty {
                emitter.ok(Some(format!("no audio in {asset_label}")));
                Ok(None)
            } else {
                emitter.ok(Some(format!("waveform ready: {asset_label}")));
                Ok(Some(sidecar.to_path_buf()))
            }
        }
        Err(montage_render::FfmpegError::NonZero { stderr_tail, .. }) if cancel.is_cancelled() => {
            let _ = stderr_tail;
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            let msg = format!("waveform: {e}");
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

/// True iff the sidecar holds non-empty buckets. Cheap parse — read
/// the first ~256 bytes and look for `"buckets":[]`. We don't
/// fully deserialize because callers that want the buckets call
/// [`read_waveform`].
fn has_buckets(sidecar: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(sidecar) else {
        return false;
    };
    // Strip whitespace; check for the literal empty-array marker.
    let compact: String = contents
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(64)
        .collect();
    !compact.contains("\"buckets\":[]")
}

/// Frontend-facing waveform payload: peaks plus the duration they span.
#[derive(Debug, Clone, Serialize)]
pub struct WaveformData {
    pub buckets: Vec<f32>,
    /// Audio duration (seconds) the buckets cover. `0.0` for legacy
    /// sidecars written before the field existed — the frontend treats
    /// non-positive as "unknown" and falls back.
    pub duration_s: f64,
}

/// Read the waveform sidecar at `path` and return the buckets plus the
/// audio duration they span. Errors when the path doesn't exist or isn't
/// valid JSON.
#[tauri::command]
pub async fn read_waveform(path: String) -> Result<WaveformData, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("read waveform sidecar: {e}"))?;
    let parsed: WaveformSidecar = montage_proto::serde_robust::from_json_slice(&bytes)
        .map_err(|e| format!("parse waveform sidecar: {e}"))?;
    Ok(WaveformData {
        buckets: parsed.buckets,
        duration_s: parsed.duration_s,
    })
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
    fn has_buckets_recognises_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, br#"{ "buckets": [] }"#).unwrap();
        assert!(!has_buckets(&p));
    }

    #[test]
    fn has_buckets_recognises_non_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, br#"{"buckets":[0.1,0.2,0.3]}"#).unwrap();
        assert!(has_buckets(&p));
    }

    #[test]
    fn sidecar_parses_with_duration() {
        let parsed: WaveformSidecar = montage_proto::serde_robust::from_json_str(
            r#"{"buckets":[0.1,0.2],"duration_s":12.5}"#,
        )
        .unwrap();
        assert_eq!(parsed.buckets.len(), 2);
        assert!((parsed.duration_s - 12.5).abs() < 1e-9);
    }

    #[test]
    fn legacy_sidecar_without_duration_still_parses() {
        // Sidecars written before duration_s existed must still load;
        // duration defaults to 0.0 (the frontend treats that as unknown).
        let parsed: WaveformSidecar =
            montage_proto::serde_robust::from_json_str(r#"{"buckets":[0.1,0.2,0.3]}"#).unwrap();
        assert_eq!(parsed.buckets.len(), 3);
        assert_eq!(parsed.duration_s, 0.0);
    }
}
