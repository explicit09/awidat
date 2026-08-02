//! Per-second motion-magnitude sampling. Per-asset JSON sidecar at
//! `<project>/.montage/motion/<stem>-<hash>.json` consumed by Phase
//! 2's continuity engine to detect mid-motion cuts.
//!
//! Idempotency: mtime-based, same shape as proxy / waveform /
//! silences. A stale signal is "looks slightly off", not "wrong
//! cut" — we don't sha-the-source on every import.
//!
//! Format: `{ "samples_per_second": 1, "magnitudes": [f32, ...] }`.
//! `samples_per_second` is recorded so future-us can ship higher-
//! resolution signals (e.g. 2/sec for action footage) without
//! breaking older readers.

use std::path::{Path, PathBuf};

use montage_desktop_protocol::{Id, JobKind};
use montage_render::MotionSignal;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::media::motion_path_for;
use crate::events::JobEmitter;
use crate::state::MontageState;

/// Default samples-per-second. v1 ships 1 — adequate for podcast +
/// long-form spoken content. Action footage may want 2-4 in a
/// future tweak; the sidecar shape carries the value so consumers
/// can scale their lookups accordingly.
const DEFAULT_SAMPLES_PER_SECOND: u32 = 1;

/// On-disk sidecar shape. Wrapped in a struct so future fields
/// (algorithm version, calibration data) land behind a serde
/// boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionSidecar {
    /// How many magnitude samples per second of source-time.
    /// v1 always emits 1.
    pub samples_per_second: u32,
    /// Mean motion magnitude per sample, in `[0.0, 1.0]`.
    pub magnitudes: Vec<f32>,
}

/// Generate (or refresh) the motion sidecar for one asset under a
/// specific project. Used by the post-import chain. Returns the
/// sidecar path on success.
pub async fn generate_motion_for_asset_in_project(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: &Path,
    asset_path: &Path,
) -> Result<PathBuf, String> {
    generate_motion_for_asset_in_project_inner(app, state.inner(), project_root, asset_path).await
}

/// `&MontageState` variant of [`generate_motion_for_asset_in_project`],
/// so callers without a Tauri `State` handle (e.g. background tasks
/// spawned with `tokio::join!` that need to outlive the `State`'s
/// borrow lifetime) can invoke the same machinery.
pub async fn generate_motion_for_asset_in_project_inner(
    app: &AppHandle,
    state: &MontageState,
    project_root: &Path,
    asset_path: &Path,
) -> Result<PathBuf, String> {
    let sidecar = motion_path_for(project_root, asset_path);
    if sidecar_is_fresh(asset_path, &sidecar) {
        return Ok(sidecar);
    }
    run_one(app, state, asset_path, &sidecar).await?;
    Ok(sidecar)
}

async fn run_one(
    app: &AppHandle,
    state: &MontageState,
    asset: &Path,
    sidecar: &Path,
) -> Result<(), String> {
    let job_id = Id::new(format!(
        "motion-{}",
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
        JobKind::Motion,
        format!("sampling motion in {asset_label}"),
    );

    let result = montage_render::generate_motion_signal(asset, cancel.clone()).await;
    state.unregister_job(&job_id.0).await;

    match result {
        Ok(magnitudes) => {
            if let Some(parent) = sidecar.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    let msg = format!("create motion dir: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            }
            let payload = MotionSidecar {
                samples_per_second: DEFAULT_SAMPLES_PER_SECOND,
                magnitudes: magnitudes_to_f32(magnitudes),
            };
            let count = payload.magnitudes.len();
            let json = match serde_json::to_string(&payload) {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("serialize motion: {e}");
                    emitter.err(msg.clone());
                    return Err(msg);
                }
            };
            if let Err(e) = tokio::fs::write(sidecar, json).await {
                let msg = format!("write motion sidecar: {e}");
                emitter.err(msg.clone());
                return Err(msg);
            }
            emitter.ok(Some(format!(
                "motion ready: {asset_label} ({count} sample{})",
                if count == 1 { "" } else { "s" },
            )));
            Ok(())
        }
        Err(montage_render::FfmpegError::NonZero { stderr_tail, .. }) if cancel.is_cancelled() => {
            let _ = stderr_tail;
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            let msg = format!("motion: {e}");
            emitter.err(msg.clone());
            Err(msg)
        }
    }
}

/// Type alias dance: `MotionSignal` is `Vec<f32>` from the renderer
/// crate, but we accept it as a struct field for shape-stability.
fn magnitudes_to_f32(s: MotionSignal) -> Vec<f32> {
    s
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_stale_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        assert!(!sidecar_is_fresh(&asset, &dir.path().join("nope.json")));
    }

    #[test]
    fn sidecar_fresh_when_newer() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let sidecar = dir.path().join("a.json");
        std::fs::write(&sidecar, b"{}").unwrap();
        assert!(sidecar_is_fresh(&asset, &sidecar));
    }

    #[test]
    fn motion_sidecar_roundtrips_through_json() {
        let payload = MotionSidecar {
            samples_per_second: 1,
            magnitudes: vec![0.0, 0.12, 0.55, 0.9, 0.05],
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: MotionSidecar = montage_proto::serde_robust::from_json_str(&s).unwrap();
        assert_eq!(back.samples_per_second, 1);
        assert_eq!(back.magnitudes.len(), 5);
        assert!((back.magnitudes[2] - 0.55).abs() < 1e-6);
    }
}
