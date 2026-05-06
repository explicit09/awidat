//! Proxy transcoding: 720p H.264 all-keyframe mp4 under
//! `<project>/.awidat/proxies/` for every asset in `raw/` that
//! doesn't already have an up-to-date proxy. The live preview pane
//! scrubs against the proxy, never the original.
//!
//! Idempotency: if `<asset>.mp4`'s mtime under proxies/ is newer
//! than the source's mtime, we skip it. The check is mtime-based
//! rather than sha-based because proxies aren't load-bearing for
//! correctness — a stale proxy is "looks slightly off" not "wrong
//! cut" — and sha-of-source on every import would double the
//! import wait time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use awidat_desktop_protocol::{Id, JobKind};
use awidat_render::{TranscodeProgress, TranscodeProgressCallback};
use tauri::{AppHandle, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::media::proxy_path_for;
use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

/// Generate (or refresh) proxies for every media file under
/// `<project>/raw/`. Emits one `Item::Job` (`JobKind::Transcode`)
/// per asset transcoded; assets whose proxies are already up to
/// date are skipped silently.
///
/// Returns the number of proxies actually generated. Zero is a
/// valid success — it means the project was already proxied.
#[tauri::command]
pub async fn transcode_project_proxies(
    app: AppHandle,
    state: State<'_, AwidatState>,
) -> Result<usize, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return Ok(0);
    }
    let proxies_dir = project_root.join(".awidat").join("proxies");
    tokio::fs::create_dir_all(&proxies_dir)
        .await
        .map_err(|e| format!("create proxies dir: {e}"))?;

    let assets = collect_media(&raw_dir).map_err(|e| format!("scan raw/: {e}"))?;
    let mut generated = 0usize;
    for asset in assets {
        let proxy_path = proxy_path_for(&proxies_dir, &asset);
        if proxy_is_fresh(&asset, &proxy_path) {
            continue;
        }
        transcode_one(&app, &state, &asset, &proxy_path).await?;
        generated += 1;
    }
    Ok(generated)
}

/// Transcode one asset into the proxy directory for a specific project.
/// Used by the post-import chain so project switches cannot redirect
/// work into whichever project is current by the time the background
/// task wakes up.
pub async fn transcode_single_asset_in_project(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    project_root: &Path,
    asset_path: &Path,
) -> Result<Option<PathBuf>, String> {
    let proxies_dir = project_root.join(".awidat").join("proxies");
    tokio::fs::create_dir_all(&proxies_dir)
        .await
        .map_err(|e| format!("create proxies dir: {e}"))?;
    let proxy_path = proxy_path_for(&proxies_dir, asset_path);
    if proxy_is_fresh(asset_path, &proxy_path) {
        return Ok(Some(proxy_path));
    }
    transcode_one(app, state, asset_path, &proxy_path).await?;
    Ok(Some(proxy_path))
}

/// Run one ffmpeg transcode end-to-end with a live job card.
async fn transcode_one(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    asset: &Path,
    proxy_path: &Path,
) -> Result<(), String> {
    let job_id = Id::new(format!(
        "transcode-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(state, &job_id).await;
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::Transcode,
        format!(
            "transcoding proxy for {}",
            asset.file_name().unwrap_or_default().to_string_lossy()
        ),
    );

    // Forward `TranscodeProgress` events into the JobEmitter.
    let (tx, mut rx) = mpsc::unbounded_channel::<TranscodeProgress>();
    let cb: TranscodeProgressCallback = Arc::new(move |evt| {
        let _ = tx.send(evt);
    });

    // Forwarder task — owns the emitter.
    let asset_for_status = asset
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let emitter_for_task = emitter;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<JobEmitter>();
    tokio::spawn(async move {
        while let Some(evt) = rx.recv().await {
            match evt {
                TranscodeProgress::Started { total_duration_s } => {
                    let label = match total_duration_s {
                        Some(d) => format!("starting (source {d:.1}s)"),
                        None => "starting (duration unknown)".into(),
                    };
                    emitter_for_task.progress(Some(0), label);
                }
                TranscodeProgress::Tick { percent, line: _ } => {
                    let status = match percent {
                        Some(p) => format!("{asset_for_status}: {p}%"),
                        None => format!("{asset_for_status}: working…"),
                    };
                    emitter_for_task.progress(percent, status);
                }
            }
        }
        let _ = done_tx.send(emitter_for_task);
    });

    let result = awidat_render::transcode_proxy(asset, proxy_path, Some(cb), cancel.clone()).await;

    unregister_job(state, &job_id).await;
    let emitter = done_rx
        .await
        .map_err(|_| "transcode emitter task crashed".to_string())?;

    match result {
        Ok(()) => {
            emitter.ok(Some(format!(
                "proxy ready: {}",
                proxy_path.file_name().unwrap_or_default().to_string_lossy()
            )));
            Ok(())
        }
        Err(awidat_render::FfmpegError::NonZero { stderr_tail, .. }) if cancel.is_cancelled() => {
            // Cancelled is reported as NonZero with stderr_tail =
            // "cancelled" (see transcode_proxy).
            let _ = stderr_tail;
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            let msg = format!("transcode: {e}");
            emitter.err(msg.clone());
            Err(msg)
        }
    }
}

/// True iff `proxy` exists and its mtime is at-or-after `asset`'s.
/// Inversely: false on missing-proxy OR stale-proxy.
fn proxy_is_fresh(asset: &Path, proxy: &Path) -> bool {
    let proxy_meta = match std::fs::metadata(proxy) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let asset_meta = match std::fs::metadata(asset) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let (Ok(proxy_mtime), Ok(asset_mtime)) = (proxy_meta.modified(), asset_meta.modified()) else {
        return false;
    };
    proxy_mtime >= asset_mtime
}

fn collect_media(raw_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(raw_dir, &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.is_file() && looks_like_media(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Whitelist of file extensions we treat as media for transcoding.
/// We don't want to try to transcode a stray .DS_Store or a .json
/// sidecar. ffmpeg can read more than this list — but extending it
/// requires a real decision per format, not just "yolo, try it."
fn looks_like_media(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp4" | "mov" | "mkv" | "webm" | "m4v" | "avi" | "flv" | "wmv" | "mpg" | "mpeg"
    )
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
    fn proxy_is_stale_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        let proxy = dir.path().join("a-proxy.mp4");
        assert!(!proxy_is_fresh(&asset, &proxy));
    }

    #[test]
    fn proxy_is_fresh_when_newer() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        // Sleep just enough so mtimes differ at filesystem resolution.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let proxy = dir.path().join("a-proxy.mp4");
        std::fs::write(&proxy, b"x").unwrap();
        assert!(proxy_is_fresh(&asset, &proxy));
    }

    #[test]
    fn looks_like_media_recognises_common_extensions() {
        assert!(looks_like_media(Path::new("a.mp4")));
        assert!(looks_like_media(Path::new("a.MOV")));
        assert!(looks_like_media(Path::new("a.webm")));
        assert!(!looks_like_media(Path::new("a.json")));
        assert!(!looks_like_media(Path::new("Makefile")));
    }
}
