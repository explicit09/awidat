//! Media-pane Tauri commands.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use montage_desktop_protocol::{
    MediaCacheArtifactStatus, MediaCacheReadiness, MediaDecodeBackend, MediaFailureReason,
    MediaProcessingProgress, MediaReadinessEntry, MediaReadinessSnapshot, MediaReadinessState,
    PlayableArtifact, PlayableArtifactKind,
};
use montage_proto::index::AssetId;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::state::{MediaServerInner, MontageState};

/// One playable proxy. The frontend feeds `proxy_path` into
/// `convertFileSrc()` to get a `<video>`-compatible URL.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyEntry {
    /// Stem of the asset (no extension). Stable per asset; the
    /// frontend uses this as a label and as a stable React key.
    pub stem: String,
    /// Absolute path to the proxy mp4 on disk. Pass directly to
    /// `convertFileSrc()` on the frontend.
    pub proxy_path: String,
    /// File size in bytes. Useful for "is this proxy actually
    /// finished generating yet?" checks (zero or partial = still
    /// being written).
    pub size_bytes: u64,
}

/// One source asset discovered under the project's `raw/` directory.
#[derive(Debug, Clone, Serialize)]
pub struct SourceMediaEntry {
    /// Stable project-relative asset id, e.g. `raw/interview.mov`.
    pub id: String,
    /// Display name including extension.
    pub name: String,
    /// Absolute source path on disk.
    pub path: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Place a source media asset from the bin onto the timeline. This is
/// the explicit editor path for "imported but not yet used" sources.
#[tauri::command]
pub async fn insert_media_on_timeline(
    app: AppHandle,
    state: State<'_, MontageState>,
    asset_id: String,
    at_s: Option<f64>,
) -> Result<bool, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    if asset_id.contains("..") || asset_id.starts_with('/') || asset_id.starts_with('\\') {
        return Err("asset id must be project-relative".into());
    }
    let asset_path = project_root.join(&asset_id);
    let requested = std::fs::canonicalize(&asset_path)
        .map_err(|e| format!("source media does not exist: {e}"))?;
    let raw_dir = std::fs::canonicalize(project_root.join("raw"))
        .map_err(|e| format!("raw/ is not available: {e}"))?;
    if !requested.starts_with(&raw_dir) || !is_preview_media_file(&requested) {
        return Err("asset is not a source media file in this project's raw/ directory".into());
    }

    let probe = montage_render::probe_media(&requested)
        .await
        .map_err(|e| format!("probe source media: {e}"))?;
    let inserted = match at_s {
        Some(at_s) => {
            crate::commands::auto_insert::insert_media_at(&project_root, &requested, &probe, at_s)
                .await?
        }
        None => {
            crate::commands::auto_insert::append_media(&project_root, &requested, &probe).await?
        }
    };
    if inserted {
        crate::events::emit_timeline_changed(&app, &project_root);
        // Kick off a proxy transcode for the just-placed asset if it
        // doesn't already have a fresh 1080p proxy on disk. Without
        // this the timeline preview shows "Generating preview…"
        // forever for any clip whose proxy isn't already cached —
        // the only other code path that runs `transcode_single_asset
        // _in_project` is the import flow, which only fires for
        // fresh imports, not for assets the user is re-adding to
        // the timeline. Idempotent (proxy_is_fresh short-circuits)
        // so it's safe to fire on every insert.
        let project_root_for_task = project_root.clone();
        let asset_for_task = requested.clone();
        let app_for_task = app.clone();
        tokio::spawn(async move {
            let state = app_for_task.state::<MontageState>();
            match crate::commands::transcode::transcode_single_asset_in_project(
                &app_for_task,
                &state,
                &project_root_for_task,
                &asset_for_task,
            )
            .await
            {
                Ok(Some(_)) => {
                    crate::events::emit_timeline_changed(&app_for_task, &project_root_for_task);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        asset = %asset_for_task.display(),
                        "insert-time proxy transcode failed; timeline preview will stay empty for this clip",
                    );
                }
            }
        });
    }
    Ok(inserted)
}

/// Return every source media file currently sitting under
/// `<project>/raw/`. Unlike [`list_proxies`], this reflects the actual
/// project assets before transcoding/indexing has produced sidecars.
#[tauri::command]
pub async fn list_source_media(
    state: State<'_, MontageState>,
) -> Result<Vec<SourceMediaEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let files = tokio::task::spawn_blocking(move || {
        montage_index::media_files::collect_project_media_files(
            &project_root,
            montage_index::media_files::MediaScanOptions {
                include_raw: true,
                include_renders: false,
                max_files: None,
            },
        )
    })
    .await
    .map_err(|e| format!("scan source media join: {e}"))?
    .map_err(|e| format!("scan source media: {e}"))?;

    Ok(files
        .into_iter()
        .map(|file| SourceMediaEntry {
            id: file.project_relative_path,
            name: file
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("media")
                .to_string(),
            path: file.path.to_string_lossy().into_owned(),
            size_bytes: file.size_bytes,
        })
        .collect())
}

/// Return every proxy currently sitting in
/// `<project>/.montage/proxies/`. Empty list when no project is
/// loaded or the dir doesn't exist yet (first import will create
/// it). Sorted by stem so order is stable across calls.
#[tauri::command]
pub async fn list_proxies(state: State<'_, MontageState>) -> Result<Vec<ProxyEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let dir = project_root.join(".montage").join("proxies");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("read proxies dir: {e}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("read proxies entry: {e}"))?
    {
        let path = entry.path();
        if !is_proxy_file(&path) {
            continue;
        }
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        out.push(ProxyEntry {
            stem,
            proxy_path: path.to_string_lossy().into_owned(),
            size_bytes: meta.len(),
        });
    }
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(out)
}

/// Return the shared media-readiness truth for every source asset in
/// the current project. This is read-only: it reports source/proxy/
/// sidecar state from disk and never starts transcodes or indexers.
#[tauri::command]
pub async fn read_media_readiness(
    state: State<'_, MontageState>,
) -> Result<MediaReadinessSnapshot, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    tokio::task::spawn_blocking(move || build_media_readiness_snapshot_at(&project_root))
        .await
        .map_err(|e| format!("media readiness join: {e}"))?
}

fn is_proxy_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
}

fn build_media_readiness_snapshot_at(
    project_root: &Path,
) -> Result<MediaReadinessSnapshot, String> {
    let raw_dir = project_root.join("raw");
    let assets = if raw_dir.is_dir() {
        crate::commands::transcode::collect_media(&raw_dir)
            .map_err(|e| format!("scan raw/: {e}"))?
    } else {
        Vec::new()
    };
    let entries = assets
        .iter()
        .map(|asset| media_readiness_entry(project_root, asset))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MediaReadinessSnapshot {
        project_id: project_root
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned),
        generated_at_ms: now_ms(),
        entries,
    })
}

fn media_readiness_entry(
    project_root: &Path,
    asset_path: &Path,
) -> Result<MediaReadinessEntry, String> {
    let asset_id = asset_path
        .strip_prefix(project_root)
        .unwrap_or(asset_path)
        .to_string_lossy()
        .replace('\\', "/");
    let display_name = asset_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("media")
        .to_string();
    let source_meta = std::fs::metadata(asset_path).ok();
    let source_exists = source_meta.as_ref().is_some_and(|meta| meta.is_file());
    let proxies_dir = project_root.join(".montage").join("proxies");
    let proxy_path = proxy_path_for(&proxies_dir, asset_path);
    let proxy_pending = proxy_pending_path(&proxy_path);
    let proxy_status = cache_file_status(asset_path, &proxy_path, Some(&proxy_pending));

    let cache = MediaCacheReadiness {
        proxy: proxy_status,
        compatibility_media: proxy_status,
        thumbnails: thumbnails_status(project_root, &asset_id),
        waveform: waveform_status(project_root, &asset_id),
        transcript: index_sidecar_status(project_root, "whisper", &asset_id),
        captions: index_sidecar_status(project_root, "whisper", &asset_id),
        scenes: index_sidecar_status(project_root, "scenedetect", &asset_id),
        face_detection: index_sidecar_status(project_root, "face", &asset_id),
        color_analysis: index_sidecar_status(project_root, "color-analysis", &asset_id),
        motion_analysis: sidecar_file_status(
            asset_path,
            &motion_path_for(project_root, asset_path),
        ),
        audio_analysis: index_sidecar_status(project_root, "audio-energy", &asset_id),
        silence_detection: sidecar_file_status(
            asset_path,
            &silences_path_for(project_root, asset_path),
        ),
    };
    let duration_s = probe_duration_s_sync(asset_path);
    let transcript_progress = if matches!(
        cache.transcript,
        MediaCacheArtifactStatus::Ready | MediaCacheArtifactStatus::Stale
    ) {
        None
    } else {
        duration_s.and_then(whispercpp_progress_from_active_work_dir)
    };

    let mut failures = Vec::new();
    if !source_exists {
        failures.push(MediaFailureReason::SourceMissing);
    }
    if matches!(proxy_status, MediaCacheArtifactStatus::Missing) {
        failures.push(MediaFailureReason::ProxyMissing);
    }

    let playable = if matches!(proxy_status, MediaCacheArtifactStatus::Ready) {
        Some(playable_artifact(
            PlayableArtifactKind::Proxy,
            Some(proxy_path.to_string_lossy().into_owned()),
            Some("mp4".into()),
            duration_s,
        ))
    } else if source_exists {
        Some(playable_artifact(
            PlayableArtifactKind::Source,
            Some(asset_path.to_string_lossy().into_owned()),
            asset_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase()),
            duration_s,
        ))
    } else {
        None
    };

    let state = readiness_state(source_exists, proxy_status, playable.is_some(), &cache);
    Ok(MediaReadinessEntry {
        asset_id,
        display_name,
        source_path: source_exists.then(|| asset_path.to_string_lossy().into_owned()),
        state,
        playable,
        cache,
        failures,
        transcript_progress,
        duration_s,
        source_size_bytes: source_meta.map(|meta| meta.len()),
        updated_at_ms: source_updated_at_ms(asset_path),
    })
}

const WHISPERCPP_CHUNK_S: f64 = 30.0;
const WHISPERCPP_OVERLAP_S: f64 = 2.0;
const WHISPERCPP_ACTIVE_WORK_DIR_MAX_AGE_S: u64 = 60;

fn whispercpp_progress_from_active_work_dir(duration_s: f64) -> Option<MediaProcessingProgress> {
    let work_dir = newest_whispercpp_work_dir()?;
    whispercpp_progress_from_work_dir(&work_dir, duration_s)
}

fn newest_whispercpp_work_dir() -> Option<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let entries = std::fs::read_dir(temp_dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("montage-whispercpp-") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            let age = SystemTime::now().duration_since(modified).ok()?;
            if age.as_secs() > WHISPERCPP_ACTIVE_WORK_DIR_MAX_AGE_S {
                return None;
            }
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn whispercpp_progress_from_work_dir(
    work_dir: &Path,
    duration_s: f64,
) -> Option<MediaProcessingProgress> {
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return None;
    }
    let step_s = WHISPERCPP_CHUNK_S - WHISPERCPP_OVERLAP_S;
    if step_s <= 0.0 {
        return None;
    }
    let total_units = (duration_s / step_s).ceil() as u32;
    if total_units == 0 {
        return None;
    }

    let mut completed_units = 0u32;
    let mut max_wav_index = None;
    for entry in std::fs::read_dir(work_dir).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(index) = chunk_index(file_name, ".json") {
            completed_units = completed_units.saturating_add(1);
            max_wav_index = Some(max_wav_index.unwrap_or(index).max(index));
        } else if let Some(index) = chunk_index(file_name, ".wav") {
            max_wav_index = Some(max_wav_index.unwrap_or(index).max(index));
        }
    }
    if completed_units == 0 && max_wav_index.is_none() {
        return None;
    }

    let completed_units = completed_units.min(total_units);
    let current_unit = max_wav_index
        .map(|index| index.saturating_add(1).min(total_units))
        .or_else(|| Some(completed_units.saturating_add(1).min(total_units)));
    let finalizing = completed_units >= total_units;
    let percent = if finalizing {
        Some(99)
    } else {
        Some(((completed_units as f64 / total_units as f64) * 100.0).round() as u8)
    };
    let label = if finalizing {
        "finalizing transcript".to_string()
    } else {
        format!(
            "transcribing chunk {} / {}",
            current_unit.unwrap_or(completed_units.saturating_add(1)),
            total_units
        )
    };

    Some(MediaProcessingProgress {
        label,
        completed_units,
        total_units,
        current_unit,
        unit: "chunks".into(),
        percent,
    })
}

fn chunk_index(file_name: &str, suffix: &str) -> Option<u32> {
    let number = file_name.strip_prefix("chunk-")?.strip_suffix(suffix)?;
    number.parse().ok()
}

fn probe_duration_s_sync(asset_path: &Path) -> Option<f64> {
    let bin = montage_render::ffprobe_path().ok()?;
    let output = Command::new(bin)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(asset_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout.trim().parse::<f64>().ok()
}

fn readiness_state(
    source_exists: bool,
    proxy_status: MediaCacheArtifactStatus,
    has_playable: bool,
    cache: &MediaCacheReadiness,
) -> MediaReadinessState {
    if !source_exists {
        return MediaReadinessState::Offline;
    }
    if any_cache_status(cache, MediaCacheArtifactStatus::Failed) {
        return MediaReadinessState::Failed;
    }
    if matches!(proxy_status, MediaCacheArtifactStatus::Pending) {
        return MediaReadinessState::Processing;
    }
    if !has_playable {
        return MediaReadinessState::Blocked;
    }
    if all_cache_ready_or_skipped(cache) {
        MediaReadinessState::Ready
    } else {
        MediaReadinessState::Partial
    }
}

fn playable_artifact(
    kind: PlayableArtifactKind,
    path: Option<String>,
    container: Option<String>,
    duration_s: Option<f64>,
) -> PlayableArtifact {
    PlayableArtifact {
        kind,
        path,
        backend: MediaDecodeBackend::Webkit,
        container,
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        duration_s,
    }
}

fn all_cache_ready_or_skipped(cache: &MediaCacheReadiness) -> bool {
    cache_statuses(cache).into_iter().all(|status| {
        matches!(
            status,
            MediaCacheArtifactStatus::Ready | MediaCacheArtifactStatus::Skipped
        )
    })
}

fn any_cache_status(cache: &MediaCacheReadiness, target: MediaCacheArtifactStatus) -> bool {
    cache_statuses(cache)
        .into_iter()
        .any(|status| status == target)
}

fn cache_statuses(cache: &MediaCacheReadiness) -> [MediaCacheArtifactStatus; 12] {
    [
        cache.proxy,
        cache.compatibility_media,
        cache.thumbnails,
        cache.waveform,
        cache.transcript,
        cache.captions,
        cache.scenes,
        cache.face_detection,
        cache.color_analysis,
        cache.motion_analysis,
        cache.audio_analysis,
        cache.silence_detection,
    ]
}

fn thumbnails_status(project_root: &Path, asset_id: &str) -> MediaCacheArtifactStatus {
    if thumbnails_dir_for_asset_id(project_root, asset_id).is_some() {
        MediaCacheArtifactStatus::Ready
    } else {
        MediaCacheArtifactStatus::Missing
    }
}

fn waveform_status(project_root: &Path, asset_id: &str) -> MediaCacheArtifactStatus {
    if waveform_path_for_asset_id(project_root, asset_id).is_some() {
        MediaCacheArtifactStatus::Ready
    } else {
        MediaCacheArtifactStatus::Missing
    }
}

fn index_sidecar_status(
    project_root: &Path,
    indexer: &str,
    asset_id: &str,
) -> MediaCacheArtifactStatus {
    let asset = AssetId::new(asset_id.to_string());
    let Ok(path) = montage_index::sidecar_io::sidecar_path(project_root, indexer, &asset) else {
        return MediaCacheArtifactStatus::Missing;
    };
    sidecar_file_status(&project_root.join(asset_id), &path)
}

fn sidecar_file_status(asset_path: &Path, sidecar_path: &Path) -> MediaCacheArtifactStatus {
    cache_file_status(asset_path, sidecar_path, None)
}

fn cache_file_status(
    asset_path: &Path,
    artifact_path: &Path,
    pending_path: Option<&Path>,
) -> MediaCacheArtifactStatus {
    if pending_path.is_some_and(|path| path.is_file()) {
        return MediaCacheArtifactStatus::Pending;
    }
    if !artifact_path.is_file() {
        return MediaCacheArtifactStatus::Missing;
    }
    if artifact_is_fresh(asset_path, artifact_path) {
        MediaCacheArtifactStatus::Ready
    } else {
        MediaCacheArtifactStatus::Stale
    }
}

fn artifact_is_fresh(asset_path: &Path, artifact_path: &Path) -> bool {
    let artifact_meta = match std::fs::metadata(artifact_path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    let asset_meta = match std::fs::metadata(asset_path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    let (Ok(artifact_mtime), Ok(asset_mtime)) = (artifact_meta.modified(), asset_meta.modified())
    else {
        return false;
    };
    artifact_mtime >= asset_mtime
}

fn proxy_pending_path(proxy_path: &Path) -> PathBuf {
    let mut raw = proxy_path.as_os_str().to_os_string();
    raw.push(".pending");
    PathBuf::from(raw)
}

fn source_updated_at_ms(asset_path: &Path) -> Option<u64> {
    std::fs::metadata(asset_path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(system_time_ms)
}

fn now_ms() -> u64 {
    system_time_ms(SystemTime::now()).unwrap_or(0)
}

fn system_time_ms(time: SystemTime) -> Option<u64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis().try_into().unwrap_or(u64::MAX))
}

/// Tauri command that resolves the absolute path on disk for a
/// given asset stem, IF the proxy exists. Returns `None` otherwise.
/// Used by the frontend to ask "is the proxy for this asset ready?"
/// without listing every proxy.
#[tauri::command]
pub async fn proxy_path_for_stem(
    state: State<'_, MontageState>,
    stem: String,
) -> Result<Option<String>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(None),
    };
    let candidate: PathBuf = project_root
        .join(".montage")
        .join("proxies")
        .join(format!("{stem}.mp4"));
    Ok(if candidate.is_file() {
        Some(candidate.to_string_lossy().into_owned())
    } else {
        None
    })
}

/// Return a localhost streaming URL for project media. Unlike Tauri's
/// `asset:` URL, this preserves audio in WKWebView; unlike a blob URL,
/// it does not load multi-GB media into WebKit memory.
#[tauri::command]
pub async fn media_url_for_path(
    state: State<'_, MontageState>,
    path: String,
) -> Result<String, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let requested =
        std::fs::canonicalize(&path).map_err(|e| format!("media path does not exist: {e}"))?;
    if !is_project_media_path(&project_root, &requested) {
        return Err("media path is outside this project's media directories".into());
    }

    let (port, files) = ensure_media_server(&state)?;
    let token = media_token(&requested);
    files
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?
        .insert(token.clone(), requested);
    Ok(format!("http://127.0.0.1:{port}/media/{token}"))
}

/// Streaming URL for a recent-project tile on the Landing screen.
///
/// Unlike [`media_url_for_path`], this does NOT use the globally-loaded
/// project (the Landing screen shows when no project is open, so
/// `project_root` is `None` — see `close_project`). The caller passes the
/// tile's own project root explicitly, and the requested media is validated
/// against THAT project's cache dirs (`.montage/proxies` or
/// `.montage/thumbnails`). The media server is per-file-token, not
/// per-project, so it serves fine without a loaded project.
#[tauri::command]
pub async fn project_preview_url(
    state: State<'_, MontageState>,
    project_path: String,
    media_path: String,
) -> Result<String, String> {
    let project_root = PathBuf::from(&project_path);
    if !project_root.join("project.otio.json").is_file() {
        return Err("not a montage project".into());
    }
    let requested = std::fs::canonicalize(&media_path)
        .map_err(|e| format!("media path does not exist: {e}"))?;
    if !is_project_preview_path(&project_root, &requested) {
        return Err("media path is outside this project's preview directories".into());
    }

    let (port, files) = ensure_media_server(&state)?;
    let token = media_token(&requested);
    files
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?
        .insert(token.clone(), requested);
    Ok(format!("http://127.0.0.1:{port}/media/{token}"))
}

/// Path validation for project-manager preview tiles. Accepts the same
/// proxy/raw media as [`is_project_media_path`], plus the filmstrip
/// thumbnail JPEGs under `.montage/thumbnails/` (which the tile uses as
/// the image fallback when no proxy exists).
fn is_project_preview_path(project_root: &Path, requested: &Path) -> bool {
    if is_project_media_path(project_root, requested) {
        return true;
    }
    if let Ok(thumbs_dir) = std::fs::canonicalize(project_root.join(".montage").join("thumbnails"))
    {
        if requested.starts_with(&thumbs_dir)
            && requested
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
        {
            return true;
        }
    }
    false
}

fn is_project_media_path(project_root: &Path, requested: &Path) -> bool {
    if let Ok(proxies_dir) = std::fs::canonicalize(project_root.join(".montage").join("proxies")) {
        if requested.starts_with(&proxies_dir) && is_proxy_file(requested) {
            return true;
        }
    }
    let Ok(raw_dir) = std::fs::canonicalize(project_root.join("raw")) else {
        return false;
    };
    requested.starts_with(raw_dir) && is_preview_media_file(requested)
}

fn is_preview_media_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "mp4" | "m4v" | "mov" | "webm" | "mp3" | "wav" | "m4a"
                )
            })
            .unwrap_or(false)
}

fn ensure_media_server(
    state: &State<'_, MontageState>,
) -> Result<(u16, Arc<StdMutex<HashMap<String, PathBuf>>>), String> {
    let mut slot = state
        .media_server
        .inner
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?;
    if let Some(inner) = slot.as_ref() {
        return Ok((inner.port, inner.files.clone()));
    }

    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind media server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("media server addr: {e}"))?
        .port();
    let files = Arc::new(StdMutex::new(HashMap::<String, PathBuf>::new()));
    let files_for_thread = files.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let files = files_for_thread.clone();
            thread::spawn(move || handle_media_connection(stream, files));
        }
    });

    *slot = Some(MediaServerInner {
        port,
        files: files.clone(),
    });
    Ok((port, files))
}

pub(crate) fn clear_media_server_files(state: &crate::state::MontageState) -> Result<(), String> {
    let slot = state
        .media_server
        .inner
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?;
    if let Some(inner) = slot.as_ref() {
        inner
            .files
            .lock()
            .map_err(|_| "media server lock poisoned".to_string())?
            .clear();
    }
    Ok(())
}

fn handle_media_connection(mut stream: TcpStream, files: Arc<StdMutex<HashMap<String, PathBuf>>>) {
    let mut req = [0_u8; 8192];
    let Ok(n) = stream.read(&mut req) else {
        return;
    };
    if n == 0 {
        return;
    }
    let req = String::from_utf8_lossy(&req[..n]);
    let mut lines = req.lines();
    let Some(first) = lines.next() else {
        return;
    };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method != "GET" && method != "HEAD" {
        write_simple_response(&mut stream, "405 Method Not Allowed", "text/plain", b"");
        return;
    }
    let Some(token) = path.strip_prefix("/media/") else {
        write_simple_response(&mut stream, "404 Not Found", "text/plain", b"");
        return;
    };
    let file_path = match files.lock().ok().and_then(|m| m.get(token).cloned()) {
        Some(p) => p,
        None => {
            write_simple_response(&mut stream, "404 Not Found", "text/plain", b"");
            return;
        }
    };
    let range = lines.find_map(parse_range_header);
    serve_file(&mut stream, &file_path, range, method == "HEAD");
}

enum RangeSpec {
    From { start: u64, end: Option<u64> },
    Suffix { len: u64 },
}

fn parse_range_header(line: &str) -> Option<RangeSpec> {
    let value = line
        .strip_prefix("Range: ")
        .or_else(|| line.strip_prefix("range: "))?;
    let range = value.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start, end) = range.split_once('-')?;
    if start.is_empty() {
        return Some(RangeSpec::Suffix {
            len: end.parse::<u64>().ok()?,
        });
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse::<u64>().ok()?)
    };
    Some(RangeSpec::From { start, end })
}

fn serve_file(stream: &mut TcpStream, path: &Path, range: Option<RangeSpec>, head_only: bool) {
    let Ok(mut file) = File::open(path) else {
        write_simple_response(stream, "404 Not Found", "text/plain", b"");
        return;
    };
    let Ok(meta) = file.metadata() else {
        write_simple_response(stream, "500 Internal Server Error", "text/plain", b"");
        return;
    };
    let len = meta.len();
    if len == 0 {
        write_simple_response(stream, "416 Range Not Satisfiable", "text/plain", b"");
        return;
    }
    let (status, start, end) = match range {
        Some(RangeSpec::From { start, end }) if start < len => {
            let end = end.unwrap_or(len - 1).min(len - 1);
            ("206 Partial Content", start, end)
        }
        Some(RangeSpec::Suffix { len: suffix_len }) if suffix_len > 0 => {
            let suffix_len = suffix_len.min(len);
            ("206 Partial Content", len - suffix_len, len - 1)
        }
        Some(_) => {
            let header = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\n\r\n"
            );
            let _ = stream.write_all(header.as_bytes());
            return;
        }
        None => ("200 OK", 0, len - 1),
    };
    let content_len = end - start + 1;
    let content_type = preview_media_content_type(path);
    // Proxy files are content-hashed and schema-tagged in their
    // filenames (`<stem>-1440q20-<hash>.mp4`), so their bytes never
    // change under a given URL — let the webview cache them. The
    // all-keyframe proxy's moov atom alone is several MB; `no-store`
    // forced a full re-download of it on every project open, which
    // dominated first-frame latency. Non-proxy media (raw sources,
    // mutable thumbnails) keeps `no-store`.
    let cache_control = if path.components().any(|c| c.as_os_str() == "proxies") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: {cache_control}\r\nContent-Length: {content_len}\r\nConnection: close\r\n"
    );
    if status.starts_with("206") {
        header.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
    }
    header.push_str("\r\n");
    if stream.write_all(header.as_bytes()).is_err() || head_only {
        return;
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut remaining = content_len;
    let mut buf = [0_u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let Ok(read) = file.read(&mut buf[..want]) else {
            break;
        };
        if read == 0 {
            break;
        }
        if stream.write_all(&buf[..read]).is_err() {
            break;
        }
        remaining -= read as u64;
    }
}

fn preview_media_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        // Filmstrip thumbnail JPEGs served as the Landing tile's image
        // fallback when a project has no proxy. Without these cases the
        // file is served as video/mp4 and the `<img>` fails to decode.
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "video/mp4",
    }
}

fn write_simple_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn media_token(path: &Path) -> String {
    format!("{:016x}", stable_path_hash64(path))
}

/// Compute the absolute proxy-mp4 path for an asset path.
///
/// Used by the transcoder when generating proxies AND by the
/// timeline flattener when telling the frontend which mp4 to play
/// for each clip's segment. Both paths must agree byte-for-byte —
/// hence one shared helper rather than parallel implementations.
///
/// The proxy filename is `<asset-stem>-<schema>-<hash>.mp4`. The hash
/// is FNV-1a over the asset's absolute path string and disambiguates
/// two raw/ files that share the same stem in nested subdirectories.
/// The schema tag (e.g. `1440q20` = 1440p at CRF 20) intentionally
/// invalidates older proxies whenever we change quality targets —
/// older `*-1080p-*` files become orphans on the next orphan scan and
/// get cleaned up. Callers must pass the absolute path (not project-
/// relative) — feeding in a relative path produces a different hash
/// and the resulting proxy path won't match the one the transcoder
/// wrote.
pub fn proxy_path_for(proxies_dir: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    proxies_dir.join(format!(
        "{stem}-{}-{:08x}.mp4",
        montage_render::PROXY_SCHEMA_TAG,
        stable_path_hash(asset_abs_path)
    ))
}

fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn stable_path_hash64(path: &Path) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    hash
}

/// Resolve the absolute proxy path for a project-relative asset id
/// (e.g. `raw/foo.MOV`). Returns `Some(path)` if the proxy exists on
/// disk, `None` otherwise. Mirrors `proxy_is_fresh`-aware lookup
/// without checking mtime — for the live preview we accept any proxy
/// the transcoder has finished writing; staleness is rare and the
/// post-import chain refreshes proxies whenever a raw file changes.
pub fn proxy_path_for_asset_id(project_root: &Path, asset_id: &str) -> Option<String> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let proxies_dir = project_root.join(".montage").join("proxies");
    let proxy = proxy_path_for(&proxies_dir, &abs);
    proxy
        .is_file()
        .then(|| proxy.to_string_lossy().into_owned())
}

/// Compute the absolute thumbnails-directory path for an asset path.
///
/// Mirrors [`proxy_path_for`]: per-asset, content-disambiguated by the
/// FNV-1a hash of the absolute source path so two `foo.mov` files in
/// different `raw/` subdirs don't share a dir. Generation lands files
/// named `frame-NNNN.jpg` inside this dir; the timeline canvas reads
/// them by walking the dir at paint time.
pub fn thumbnails_dir_for(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root
        .join(".montage")
        .join("thumbnails")
        .join(format!("{stem}-{:08x}", stable_path_hash(asset_abs_path)))
}

/// Compute the absolute waveform-sidecar path for an asset path.
/// Mirrors [`thumbnails_dir_for`]: per-asset, content-disambiguated
/// by the FNV-1a hash of the absolute source path. The sidecar
/// holds JSON `{ "buckets": [...] }`; see
/// `commands::waveform::WaveformSidecar`.
pub fn waveform_path_for(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root
        .join(".montage")
        .join("waveforms")
        .join(format!(
            "{stem}-{:08x}.json",
            stable_path_hash(asset_abs_path)
        ))
}

/// Compute the absolute silence-sidecar path for an asset path.
/// Mirrors [`waveform_path_for`]: per-asset, content-disambiguated
/// by the FNV-1a hash of the absolute source path. The sidecar
/// holds JSON `{ "ranges": [{ start_s, end_s, db_floor }, ...],
/// threshold_db, min_duration_s }`; see
/// `commands::silence::SilenceSidecar`.
pub fn silences_path_for(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root.join(".montage").join("silences").join(format!(
        "{stem}-{:08x}.json",
        stable_path_hash(asset_abs_path)
    ))
}

/// Compute the absolute motion-sidecar path. Sidecar holds JSON
/// `{ "samples_per_second": 1, "magnitudes": [f32; ...] }`; see
/// `commands::motion::MotionSidecar`. Phase 2's continuity engine
/// reads it to detect mid-motion cuts.
pub fn motion_path_for(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root.join(".montage").join("motion").join(format!(
        "{stem}-{:08x}.json",
        stable_path_hash(asset_abs_path)
    ))
}

/// Resolve the motion-sidecar path for a project-relative asset id.
/// Returns `Some(path)` when the sidecar exists on disk; `None`
/// otherwise. The continuity tool tolerates absence — a missing
/// sidecar means the motion rule abstains rather than blocks.
#[allow(dead_code)]
pub fn motion_path_for_asset_id(project_root: &Path, asset_id: &str) -> Option<String> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let sidecar = motion_path_for(project_root, &abs);
    sidecar
        .is_file()
        .then(|| sidecar.to_string_lossy().into_owned())
}

/// Resolve the absolute silence-sidecar path for a project-relative
/// asset id (e.g. `raw/foo.mp3`). Returns `Some(path)` if the
/// sidecar exists on disk; `None` otherwise. Unlike the waveform
/// helper, an empty `ranges: []` is a valid result (the asset has
/// no detected silence, or has no audio stream — both are useful
/// to the find_dead_air tool, which short-circuits on either).
#[allow(dead_code)]
pub fn silences_path_for_asset_id(project_root: &Path, asset_id: &str) -> Option<String> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let sidecar = silences_path_for(project_root, &abs);
    sidecar
        .is_file()
        .then(|| sidecar.to_string_lossy().into_owned())
}

/// Resolve the absolute waveform-sidecar path for a project-relative
/// asset id (e.g. `raw/foo.mp3`). Returns `Some(path)` if the
/// sidecar exists AND its on-disk JSON contains a non-empty
/// `buckets` array (so the frontend only sees real waveforms, not
/// in-progress writes or "no audio stream" placeholders). `None`
/// otherwise.
pub fn waveform_path_for_asset_id(project_root: &Path, asset_id: &str) -> Option<String> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let sidecar = waveform_path_for(project_root, &abs);
    if !sidecar.is_file() {
        return None;
    }
    // Cheap content sniff — same shape as `commands::waveform::has_buckets`
    // but inlined here to avoid a circular dep. We treat any JSON
    // matching `"buckets":[]` (whitespace-stripped) as empty.
    let Ok(contents) = std::fs::read_to_string(&sidecar) else {
        return None;
    };
    let compact: String = contents
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(64)
        .collect();
    if compact.contains("\"buckets\":[]") {
        return None;
    }
    Some(sidecar.to_string_lossy().into_owned())
}

/// Resolve the absolute thumbnails-dir path for a project-relative
/// asset id (e.g. `raw/foo.MOV`). Returns `Some(path)` if the dir
/// exists AND has at least one `frame-*.jpg` in it (so the frontend
/// only sees fully-generated strips, not in-progress or empty dirs).
/// `None` otherwise.
pub fn thumbnails_dir_for_asset_id(project_root: &Path, asset_id: &str) -> Option<String> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let dir = thumbnails_dir_for(project_root, &abs);
    if !dir.is_dir() {
        return None;
    }
    // Cheap "is non-empty" check — read just the first entry. We don't
    // need the exact count here; the frontend reads the dir itself
    // when it draws.
    let has_frame = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("frame-"));
    has_frame.then(|| dir.to_string_lossy().into_owned())
}

/// Recursively walk a JSON value and rewrite every `ExternalReference`
/// node whose `target_url` matches `old` to use `new`. `changed` is
/// incremented for each rewrite so callers can refuse no-op writes.
fn walk_external_refs(value: &mut serde_json::Value, old: &str, new: &str, changed: &mut usize) {
    match value {
        serde_json::Value::Object(map) => {
            let is_external = map
                .get("OTIO_SCHEMA")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.starts_with("ExternalReference"));
            if is_external && map.get("target_url").and_then(|t| t.as_str()) == Some(old) {
                map.insert("target_url".into(), serde_json::Value::String(new.into()));
                *changed += 1;
            }
            for child in map.values_mut() {
                walk_external_refs(child, old, new, changed);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                walk_external_refs(child, old, new, changed);
            }
        }
        _ => {}
    }
}

/// Re-point every clip referencing `old_asset_id` at `new_asset_id`
/// in the project's OTIO. Used by the Media-Offline overlay so the
/// user can recover from a moved/renamed raw file without losing
/// their cut. Errors when nothing matches so the UI can surface
/// "no clips needed relinking" instead of silently writing the OTIO
/// back unchanged.
#[tauri::command]
pub async fn relink_missing_asset(
    app: AppHandle,
    state: State<'_, MontageState>,
    old_asset_id: String,
    new_asset_id: String,
) -> Result<usize, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let otio_path = project_root.join("project.otio.json");
    let bytes = tokio::fs::read(&otio_path)
        .await
        .map_err(|e| format!("read otio: {e}"))?;
    let mut json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse otio: {e}"))?;
    let mut changed = 0_usize;
    walk_external_refs(&mut json, &old_asset_id, &new_asset_id, &mut changed);
    if changed == 0 {
        return Err(format!("no ExternalReference matched {old_asset_id}"));
    }
    let serialized =
        serde_json::to_vec_pretty(&json).map_err(|e| format!("serialize otio: {e}"))?;
    tokio::fs::write(&otio_path, serialized)
        .await
        .map_err(|e| format!("write otio: {e}"))?;
    crate::events::emit_timeline_changed(&app, &project_root);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_path_disambiguates_same_stem_assets() {
        let proxies = PathBuf::from("/tmp/proj/.montage/proxies");
        let a = proxy_path_for(&proxies, &PathBuf::from("/tmp/proj/raw/a/foo.mov"));
        let b = proxy_path_for(&proxies, &PathBuf::from("/tmp/proj/raw/b/foo.mov"));
        assert_ne!(a, b);
    }

    #[test]
    fn proxy_path_replaces_extension_with_mp4() {
        let proxies = PathBuf::from("/tmp/proj/.montage/proxies");
        let asset = PathBuf::from("/tmp/proj/raw/foo.mov");
        let p = proxy_path_for(&proxies, &asset);
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("mp4"));
        let stem = p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        assert!(
            stem.starts_with(&format!("foo-{}-", montage_render::PROXY_SCHEMA_TAG)),
            "expected proxy stem to start with foo-<schema>- ; got {stem}"
        );
    }

    #[test]
    fn proxy_path_for_asset_id_returns_none_when_proxy_missing() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("foo.mov");
        std::fs::write(&asset, b"x").unwrap();

        assert!(proxy_path_for_asset_id(dir.path(), "raw/foo.mov").is_none());

        let proxies_dir = dir.path().join(".montage").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let proxy = proxy_path_for(&proxies_dir, &asset);
        std::fs::write(&proxy, b"y").unwrap();

        assert_eq!(
            proxy_path_for_asset_id(dir.path(), "raw/foo.mov").as_deref(),
            Some(proxy.to_string_lossy().as_ref()),
        );
    }

    #[test]
    fn proxy_path_for_asset_id_returns_none_when_asset_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(proxy_path_for_asset_id(dir.path(), "raw/nonexistent.mp4").is_none());
    }

    #[test]
    fn media_readiness_reports_source_fallback_when_proxy_missing() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("clip.mov");
        std::fs::write(&asset, b"source").unwrap();

        let snapshot = build_media_readiness_snapshot_at(dir.path()).unwrap();

        assert_eq!(snapshot.entries.len(), 1);
        let entry = &snapshot.entries[0];
        assert_eq!(entry.asset_id, "raw/clip.mov");
        assert_eq!(entry.state, MediaReadinessState::Partial);
        assert_eq!(entry.cache.proxy, MediaCacheArtifactStatus::Missing);
        assert_eq!(
            entry.playable.as_ref().map(|artifact| artifact.kind),
            Some(PlayableArtifactKind::Source)
        );
        assert!(entry.failures.contains(&MediaFailureReason::ProxyMissing));
    }

    #[test]
    fn media_readiness_prefers_fresh_proxy_and_reports_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("clip.mov");
        std::fs::write(&asset, b"source").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        let proxies_dir = dir.path().join(".montage").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let proxy = proxy_path_for(&proxies_dir, &asset);
        std::fs::write(&proxy, b"proxy").unwrap();

        let thumbnails = thumbnails_dir_for(dir.path(), &asset);
        std::fs::create_dir_all(&thumbnails).unwrap();
        std::fs::write(thumbnails.join("frame-0001.jpg"), b"jpg").unwrap();

        let waveform = waveform_path_for(dir.path(), &asset);
        std::fs::create_dir_all(waveform.parent().unwrap()).unwrap();
        std::fs::write(&waveform, br#"{"buckets":[0.1]}"#).unwrap();

        let motion = motion_path_for(dir.path(), &asset);
        std::fs::create_dir_all(motion.parent().unwrap()).unwrap();
        std::fs::write(&motion, b"{}").unwrap();

        let silences = silences_path_for(dir.path(), &asset);
        std::fs::create_dir_all(silences.parent().unwrap()).unwrap();
        std::fs::write(&silences, b"{}").unwrap();

        for indexer in [
            "whisper",
            "scenedetect",
            "face",
            "color-analysis",
            "audio-energy",
        ] {
            let path = dir
                .path()
                .join("index")
                .join(indexer)
                .join("raw/clip.mov.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }

        let snapshot = build_media_readiness_snapshot_at(dir.path()).unwrap();
        let entry = &snapshot.entries[0];

        assert_eq!(entry.state, MediaReadinessState::Ready);
        assert_eq!(entry.cache.proxy, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.thumbnails, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.waveform, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.transcript, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.captions, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.scenes, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.face_detection, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.color_analysis, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.motion_analysis, MediaCacheArtifactStatus::Ready);
        assert_eq!(entry.cache.audio_analysis, MediaCacheArtifactStatus::Ready);
        assert_eq!(
            entry.cache.silence_detection,
            MediaCacheArtifactStatus::Ready
        );
        assert_eq!(
            entry.playable.as_ref().map(|artifact| artifact.kind),
            Some(PlayableArtifactKind::Proxy)
        );
        assert!(entry.failures.is_empty());
    }

    #[test]
    fn media_readiness_reports_processing_for_pending_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("clip.mov");
        std::fs::write(&asset, b"source").unwrap();

        let proxies_dir = dir.path().join(".montage").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let proxy = proxy_path_for(&proxies_dir, &asset);
        std::fs::write(proxy_pending_path(&proxy), b"partial").unwrap();

        let snapshot = build_media_readiness_snapshot_at(dir.path()).unwrap();
        let entry = &snapshot.entries[0];

        assert_eq!(entry.state, MediaReadinessState::Processing);
        assert_eq!(entry.cache.proxy, MediaCacheArtifactStatus::Pending);
        assert_eq!(
            entry.playable.as_ref().map(|artifact| artifact.kind),
            Some(PlayableArtifactKind::Source)
        );
    }

    #[test]
    fn whispercpp_progress_estimates_completed_chunks() {
        let work_dir = tempfile::tempdir().unwrap();
        std::fs::write(work_dir.path().join("chunk-0000.wav"), b"wav").unwrap();
        std::fs::write(work_dir.path().join("chunk-0000.json"), b"{}").unwrap();
        std::fs::write(work_dir.path().join("chunk-0001.wav"), b"wav").unwrap();
        std::fs::write(work_dir.path().join("chunk-0001.json"), b"{}").unwrap();
        std::fs::write(work_dir.path().join("chunk-0002.wav"), b"wav").unwrap();

        let progress = whispercpp_progress_from_work_dir(work_dir.path(), 4626.8222).unwrap();

        assert_eq!(progress.completed_units, 2);
        assert_eq!(progress.total_units, 166);
        assert_eq!(progress.current_unit, Some(3));
        assert_eq!(progress.unit, "chunks");
        assert_eq!(progress.percent, Some(1));
        assert_eq!(progress.label, "transcribing chunk 3 / 166");
    }

    #[test]
    fn media_token_is_stable_for_same_path() {
        let path = PathBuf::from("/tmp/proj/.montage/proxies/foo.mp4");

        assert_eq!(media_token(&path), media_token(&path));
    }

    #[test]
    fn project_media_path_allows_raw_media_without_proxy_dir() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("foo.mov");
        std::fs::write(&asset, b"x").unwrap();
        let asset = std::fs::canonicalize(asset).unwrap();

        assert!(is_project_media_path(dir.path(), &asset));
    }

    #[test]
    fn project_media_path_allows_proxy_media() {
        let dir = tempfile::tempdir().unwrap();
        let proxies_dir = dir.path().join(".montage").join("proxies");
        std::fs::create_dir_all(&proxies_dir).unwrap();
        let proxy = proxies_dir.join("foo.mp4");
        std::fs::write(&proxy, b"x").unwrap();
        let proxy = std::fs::canonicalize(proxy).unwrap();

        assert!(is_project_media_path(dir.path(), &proxy));
    }

    #[test]
    fn project_media_path_rejects_non_media_raw_files() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("notes.txt");
        std::fs::write(&asset, b"x").unwrap();
        let asset = std::fs::canonicalize(asset).unwrap();

        assert!(!is_project_media_path(dir.path(), &asset));
    }

    #[test]
    fn project_media_path_rejects_outside_files() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();

        assert!(!is_project_media_path(dir.path(), outside.path()));
    }

    #[test]
    fn preview_media_content_type_matches_preview_extension() {
        assert_eq!(
            preview_media_content_type(&PathBuf::from("clip.mov")),
            "video/quicktime"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("clip.webm")),
            "video/webm"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("audio.mp3")),
            "audio/mpeg"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("audio.wav")),
            "audio/wav"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("audio.m4a")),
            "audio/mp4"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("proxy.mp4")),
            "video/mp4"
        );
        // Filmstrip thumbnail frames (Landing image-fallback path) must
        // serve as image/*, not the video/mp4 default.
        assert_eq!(
            preview_media_content_type(&PathBuf::from("frame-0001.jpg")),
            "image/jpeg"
        );
        assert_eq!(
            preview_media_content_type(&PathBuf::from("frame-0001.JPEG")),
            "image/jpeg"
        );
    }

    #[test]
    fn waveform_path_for_asset_id_returns_none_when_no_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("foo.wav");
        std::fs::write(&asset, b"x").unwrap();

        // No sidecar yet → None.
        assert!(waveform_path_for_asset_id(dir.path(), "raw/foo.wav").is_none());

        // Empty buckets → None (treat as "no audio in this asset").
        let sidecar = waveform_path_for(dir.path(), &asset);
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, br#"{"buckets":[]}"#).unwrap();
        assert!(waveform_path_for_asset_id(dir.path(), "raw/foo.wav").is_none());

        // Non-empty buckets → Some(path).
        std::fs::write(&sidecar, br#"{"buckets":[0.1,0.2]}"#).unwrap();
        assert_eq!(
            waveform_path_for_asset_id(dir.path(), "raw/foo.wav").as_deref(),
            Some(sidecar.to_string_lossy().as_ref()),
        );
    }

    #[test]
    fn is_project_media_path_accepts_raw_video_under_raw_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let raw_dir = tmp.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let asset = raw_dir.join("clip.mov");
        std::fs::write(&asset, b"src").unwrap();
        let canonical = std::fs::canonicalize(&asset).unwrap();
        assert!(is_project_media_path(tmp.path(), &canonical));
    }

    #[test]
    fn rewrite_target_url_swaps_external_reference_path() {
        let mut json = serde_json::json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "target_url": "raw/old.MOV"
        });
        let mut changed = 0_usize;
        walk_external_refs(&mut json, "raw/old.MOV", "raw/new.MOV", &mut changed);
        assert_eq!(changed, 1);
        assert_eq!(json["target_url"], "raw/new.MOV");
    }

    #[test]
    fn walk_external_refs_skips_non_matching_target_url() {
        let mut json = serde_json::json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "target_url": "raw/other.MOV"
        });
        let mut changed = 0_usize;
        walk_external_refs(&mut json, "raw/old.MOV", "raw/new.MOV", &mut changed);
        assert_eq!(changed, 0);
        assert_eq!(json["target_url"], "raw/other.MOV");
    }

    #[test]
    fn walk_external_refs_recurses_into_nested_arrays_and_objects() {
        let mut json = serde_json::json!({
            "tracks": {
                "children": [
                    {
                        "OTIO_SCHEMA": "Track.1",
                        "children": [
                            {
                                "OTIO_SCHEMA": "Clip.1",
                                "media_reference": {
                                    "OTIO_SCHEMA": "ExternalReference.1",
                                    "target_url": "raw/old.MOV"
                                }
                            }
                        ]
                    }
                ]
            }
        });
        let mut changed = 0_usize;
        walk_external_refs(&mut json, "raw/old.MOV", "raw/new.MOV", &mut changed);
        assert_eq!(changed, 1);
        assert_eq!(
            json["tracks"]["children"][0]["children"][0]["media_reference"]["target_url"],
            "raw/new.MOV"
        );
    }
}
