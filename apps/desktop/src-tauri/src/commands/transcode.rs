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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use awidat_desktop_protocol::{Id, JobKind};
use awidat_render::{TranscodeProgress, TranscodeProgressCallback};
use serde::Serialize;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::media::proxy_path_for;
use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

/// Proxy cache status for one expected or discovered artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyCacheStatus {
    /// Proxy exists and is at least as new as the source asset.
    Fresh,
    /// Proxy exists but is older than the source asset.
    Stale,
    /// Source asset exists but its expected proxy is missing.
    Missing,
    /// Proxy file exists without a matching raw asset.
    Orphan,
    /// Pending proxy file from an incomplete transcode exists.
    Pending,
}

/// One row in the proxy cache lifecycle manifest.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyCacheEntry {
    /// Source asset path when the entry maps to a raw asset.
    pub asset_path: Option<String>,
    /// Proxy or pending-proxy artifact path.
    pub proxy_path: String,
    /// Lifecycle status.
    pub status: ProxyCacheStatus,
    /// File size when the proxy artifact exists.
    pub size_bytes: Option<u64>,
}

/// Auditable proxy cache lifecycle report.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyCacheManifest {
    /// Project root used for the scan.
    pub project_root: String,
    /// Number of media assets under `raw/`.
    pub asset_count: usize,
    /// Number of fresh proxies.
    pub fresh_count: usize,
    /// Number of stale proxies.
    pub stale_count: usize,
    /// Number of missing proxies.
    pub missing_count: usize,
    /// Number of orphan proxy files.
    pub orphan_count: usize,
    /// Number of pending proxy files.
    pub pending_count: usize,
    /// Ordered manifest entries.
    pub entries: Vec<ProxyCacheEntry>,
}

/// Dry-run cleanup candidate for proxy cache artifacts.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyCleanupCandidate {
    /// Artifact path that would be removed.
    pub path: String,
    /// Cleanup reason.
    pub reason: ProxyCacheStatus,
    /// File size at scan time.
    pub size_bytes: u64,
}

/// Dry-run proxy cleanup report.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyCacheCleanupReport {
    /// Always true; cleanup is currently report-only.
    pub dry_run: bool,
    /// Full lifecycle manifest used for the cleanup plan.
    pub manifest: ProxyCacheManifest,
    /// Stale, orphan, and pending artifacts that are safe cleanup candidates.
    pub delete_candidates: Vec<ProxyCleanupCandidate>,
}

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
        match awidat_render::probe_media(&asset).await {
            Ok(probe) if probe.has_video => {}
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(error = %e, asset = %asset.display(), "probe failed; skipping proxy");
                continue;
            }
        }
        let proxy_path = proxy_path_for(&proxies_dir, &asset);
        if proxy_is_fresh(&asset, &proxy_path) {
            continue;
        }
        transcode_one(&app, &state, &asset, &proxy_path).await?;
        generated += 1;
    }
    Ok(generated)
}

/// Return a dry-run proxy lifecycle report for the loaded project.
#[tauri::command]
pub async fn proxy_cache_lifecycle_report(
    state: State<'_, AwidatState>,
) -> Result<ProxyCacheCleanupReport, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    plan_proxy_cache_cleanup(&project_root)
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
    match awidat_render::probe_media(asset_path).await {
        Ok(probe) if probe.has_video => {}
        Ok(_) => return Ok(None),
        Err(e) => return Err(format!("probe for transcode: {e}")),
    }
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

    let pending_path = proxy_pending_path(proxy_path);
    let result =
        awidat_render::transcode_proxy(asset, &pending_path, Some(cb), cancel.clone()).await;

    unregister_job(state, &job_id).await;
    let emitter = done_rx
        .await
        .map_err(|_| "transcode emitter task crashed".to_string())?;

    match result {
        Ok(()) => {
            if proxy_path.is_file() {
                let _ = tokio::fs::remove_file(proxy_path).await;
            }
            tokio::fs::rename(&pending_path, proxy_path)
                .await
                .map_err(|e| format!("finalize proxy: {e}"))?;
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

fn proxy_pending_path(proxy_path: &Path) -> PathBuf {
    let mut raw = proxy_path.as_os_str().to_os_string();
    raw.push(".pending");
    PathBuf::from(raw)
}

fn is_pending_proxy_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".mp4.pending"))
}

fn build_proxy_cache_manifest(project_root: &Path) -> Result<ProxyCacheManifest, String> {
    let raw_dir = project_root.join("raw");
    let proxies_dir = project_root.join(".awidat").join("proxies");
    let assets = if raw_dir.is_dir() {
        collect_media(&raw_dir).map_err(|e| format!("scan raw/: {e}"))?
    } else {
        Vec::new()
    };
    let mut entries = Vec::new();
    let mut expected = HashSet::new();
    let mut fresh_count = 0;
    let mut stale_count = 0;
    let mut missing_count = 0;
    let mut orphan_count = 0;
    let mut pending_count = 0;

    for asset in &assets {
        let proxy_path = proxy_path_for(&proxies_dir, asset);
        expected.insert(proxy_path.clone());
        let (status, size_bytes) = match std::fs::metadata(&proxy_path) {
            Ok(meta) if proxy_is_fresh(asset, &proxy_path) => {
                fresh_count += 1;
                (ProxyCacheStatus::Fresh, Some(meta.len()))
            }
            Ok(meta) => {
                stale_count += 1;
                (ProxyCacheStatus::Stale, Some(meta.len()))
            }
            Err(_) => {
                missing_count += 1;
                (ProxyCacheStatus::Missing, None)
            }
        };
        entries.push(ProxyCacheEntry {
            asset_path: Some(asset.to_string_lossy().into_owned()),
            proxy_path: proxy_path.to_string_lossy().into_owned(),
            status,
            size_bytes,
        });
    }

    if proxies_dir.is_dir() {
        let proxy_entries =
            std::fs::read_dir(&proxies_dir).map_err(|e| format!("read proxies dir: {e}"))?;
        for entry in proxy_entries.flatten() {
            let path = entry.path();
            if is_pending_proxy_file(&path) {
                pending_count += 1;
                entries.push(ProxyCacheEntry {
                    asset_path: None,
                    proxy_path: path.to_string_lossy().into_owned(),
                    status: ProxyCacheStatus::Pending,
                    size_bytes: entry.metadata().ok().map(|meta| meta.len()),
                });
            } else if path.is_file() && is_proxy_artifact(&path) && !expected.contains(&path) {
                orphan_count += 1;
                entries.push(ProxyCacheEntry {
                    asset_path: None,
                    proxy_path: path.to_string_lossy().into_owned(),
                    status: ProxyCacheStatus::Orphan,
                    size_bytes: entry.metadata().ok().map(|meta| meta.len()),
                });
            }
        }
    }

    entries.sort_by(|a, b| a.proxy_path.cmp(&b.proxy_path));
    Ok(ProxyCacheManifest {
        project_root: project_root.to_string_lossy().into_owned(),
        asset_count: assets.len(),
        fresh_count,
        stale_count,
        missing_count,
        orphan_count,
        pending_count,
        entries,
    })
}

fn plan_proxy_cache_cleanup(project_root: &Path) -> Result<ProxyCacheCleanupReport, String> {
    let manifest = build_proxy_cache_manifest(project_root)?;
    let delete_candidates = manifest
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.status,
                ProxyCacheStatus::Stale | ProxyCacheStatus::Orphan | ProxyCacheStatus::Pending
            )
        })
        .filter_map(|entry| {
            Some(ProxyCleanupCandidate {
                path: entry.proxy_path.clone(),
                reason: entry.status,
                size_bytes: entry.size_bytes?,
            })
        })
        .collect();
    Ok(ProxyCacheCleanupReport {
        dry_run: true,
        manifest,
        delete_candidates,
    })
}

fn is_proxy_artifact(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
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

/// Whitelist of file extensions we treat as media for scanning. The
/// transcode command probes each asset and only video-bearing files
/// get a proxy.
/// We don't want to try to transcode a stray .DS_Store or a .json
/// sidecar. ffmpeg can read more than this list — but extending it
/// requires a real decision per format, not just "yolo, try it."
fn looks_like_media(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp4"
            | "mov"
            | "mkv"
            | "webm"
            | "m4v"
            | "avi"
            | "flv"
            | "wmv"
            | "mpg"
            | "mpeg"
            | "wav"
            | "mp3"
            | "m4a"
            | "aac"
            | "flac"
            | "aiff"
            | "aif"
            | "ogg"
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

    #[test]
    fn proxy_cache_manifest_reports_fresh_stale_missing_orphan_and_pending() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let fresh_asset = raw_dir.join("fresh.mov");
        let stale_asset = raw_dir.join("stale.mov");
        let missing_asset = raw_dir.join("missing.mov");
        std::fs::write(&fresh_asset, b"fresh").unwrap();
        std::fs::write(&stale_asset, b"stale").unwrap();
        std::fs::write(&missing_asset, b"missing").unwrap();

        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let fresh_proxy = proxy_path_for(&proxies_dir, &fresh_asset);
        let stale_proxy = proxy_path_for(&proxies_dir, &stale_asset);
        std::fs::write(&stale_proxy, b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&fresh_proxy, b"new").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&stale_asset, b"newer-source").unwrap();
        let orphan_proxy = proxies_dir.join("orphan-00000000.mp4");
        std::fs::write(&orphan_proxy, b"orphan").unwrap();
        let pending_proxy = proxies_dir.join("pending-00000000.mp4.pending");
        std::fs::write(&pending_proxy, b"partial").unwrap();

        let manifest = build_proxy_cache_manifest(dir.path()).unwrap();
        assert_eq!(manifest.asset_count, 3);
        assert_eq!(manifest.fresh_count, 1);
        assert_eq!(manifest.stale_count, 1);
        assert_eq!(manifest.missing_count, 1);
        assert_eq!(manifest.orphan_count, 1);
        assert_eq!(manifest.pending_count, 1);
        assert!(manifest.entries.iter().any(|entry| {
            entry
                .asset_path
                .as_deref()
                .is_some_and(|path| path.ends_with("fresh.mov"))
                && entry.status == ProxyCacheStatus::Fresh
        }));
        assert!(manifest.entries.iter().any(|entry| {
            entry
                .asset_path
                .as_deref()
                .is_some_and(|path| path.ends_with("stale.mov"))
                && entry.status == ProxyCacheStatus::Stale
        }));
        assert!(manifest.entries.iter().any(|entry| {
            entry
                .asset_path
                .as_deref()
                .is_some_and(|path| path.ends_with("missing.mov"))
                && entry.status == ProxyCacheStatus::Missing
        }));
        assert!(manifest.entries.iter().any(|entry| {
            entry.proxy_path.ends_with("orphan-00000000.mp4")
                && entry.status == ProxyCacheStatus::Orphan
        }));
        assert!(manifest.entries.iter().any(|entry| {
            entry.proxy_path.ends_with("pending-00000000.mp4.pending")
                && entry.status == ProxyCacheStatus::Pending
        }));
    }

    #[test]
    fn proxy_cache_cleanup_dry_run_lists_deletable_stale_orphan_and_pending_files() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let stale_asset = raw_dir.join("stale.mov");
        std::fs::write(&stale_asset, b"old-source").unwrap();
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let stale_proxy = proxy_path_for(&proxies_dir, &stale_asset);
        std::fs::write(&stale_proxy, b"old-proxy").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&stale_asset, b"new-source").unwrap();
        let orphan_proxy = proxies_dir.join("orphan-00000000.mp4");
        let pending_proxy = proxy_pending_path(&stale_proxy);
        std::fs::write(&orphan_proxy, b"orphan").unwrap();
        std::fs::write(&pending_proxy, b"partial").unwrap();

        let report = plan_proxy_cache_cleanup(dir.path()).unwrap();
        let paths: Vec<&str> = report
            .delete_candidates
            .iter()
            .map(|candidate| candidate.path.as_str())
            .collect();
        assert!(report.dry_run);
        assert_eq!(report.delete_candidates.len(), 3);
        let stale_proxy_name = stale_proxy
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        assert!(paths.iter().any(|path| path.ends_with(stale_proxy_name)));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("orphan-00000000.mp4"))
        );
        assert!(paths.iter().any(|path| path.ends_with(".mp4.pending")));
        assert!(stale_proxy.exists());
        assert!(orphan_proxy.exists());
        assert!(pending_proxy.exists());
    }
}
