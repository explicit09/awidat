//! Media-pane Tauri commands.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::state::{AwidatState, MediaServerInner};

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
    state: State<'_, AwidatState>,
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

    let probe = awidat_render::probe_media(&requested)
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
            let state = app_for_task.state::<AwidatState>();
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
    state: State<'_, AwidatState>,
) -> Result<Vec<SourceMediaEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let files = tokio::task::spawn_blocking(move || {
        awidat_index::media_files::collect_project_media_files(
            &project_root,
            awidat_index::media_files::MediaScanOptions {
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
/// `<project>/.awidat/proxies/`. Empty list when no project is
/// loaded or the dir doesn't exist yet (first import will create
/// it). Sorted by stem so order is stable across calls.
#[tauri::command]
pub async fn list_proxies(state: State<'_, AwidatState>) -> Result<Vec<ProxyEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let dir = project_root.join(".awidat").join("proxies");
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

fn is_proxy_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
}

/// Tauri command that resolves the absolute path on disk for a
/// given asset stem, IF the proxy exists. Returns `None` otherwise.
/// Used by the frontend to ask "is the proxy for this asset ready?"
/// without listing every proxy.
#[tauri::command]
pub async fn proxy_path_for_stem(
    state: State<'_, AwidatState>,
    stem: String,
) -> Result<Option<String>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(None),
    };
    let candidate: PathBuf = project_root
        .join(".awidat")
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
    state: State<'_, AwidatState>,
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

fn is_project_media_path(project_root: &Path, requested: &Path) -> bool {
    if let Ok(proxies_dir) = std::fs::canonicalize(project_root.join(".awidat").join("proxies")) {
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
    state: &State<'_, AwidatState>,
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

pub(crate) fn clear_media_server_files(state: &crate::state::AwidatState) -> Result<(), String> {
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
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nContent-Length: {content_len}\r\nConnection: close\r\n"
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
/// The proxy filename is `<asset-stem>-1080p-<hash>.mp4`. The hash is
/// FNV-1a over the asset's absolute path string and disambiguates two
/// raw/ files that share the same stem in nested subdirectories. The
/// quality marker intentionally invalidates older 720p proxies without
/// probing every cache entry. Callers must pass the absolute path (not
/// project-relative) — feeding in a relative path produces a different
/// hash and the resulting proxy path won't match the one the transcoder
/// wrote.
pub fn proxy_path_for(proxies_dir: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    proxies_dir.join(format!(
        "{stem}-1080p-{:08x}.mp4",
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
    let proxies_dir = project_root.join(".awidat").join("proxies");
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
        .join(".awidat")
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
    project_root.join(".awidat").join("waveforms").join(format!(
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
    project_root.join(".awidat").join("silences").join(format!(
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
    project_root.join(".awidat").join("motion").join(format!(
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
fn walk_external_refs(
    value: &mut serde_json::Value,
    old: &str,
    new: &str,
    changed: &mut usize,
) {
    match value {
        serde_json::Value::Object(map) => {
            let is_external = map
                .get("OTIO_SCHEMA")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.starts_with("ExternalReference"));
            if is_external
                && map.get("target_url").and_then(|t| t.as_str()) == Some(old)
            {
                map.insert(
                    "target_url".into(),
                    serde_json::Value::String(new.into()),
                );
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
    state: State<'_, AwidatState>,
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
    let serialized = serde_json::to_vec_pretty(&json)
        .map_err(|e| format!("serialize otio: {e}"))?;
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
        let proxies = PathBuf::from("/tmp/proj/.awidat/proxies");
        let a = proxy_path_for(&proxies, &PathBuf::from("/tmp/proj/raw/a/foo.mov"));
        let b = proxy_path_for(&proxies, &PathBuf::from("/tmp/proj/raw/b/foo.mov"));
        assert_ne!(a, b);
    }

    #[test]
    fn proxy_path_replaces_extension_with_mp4() {
        let proxies = PathBuf::from("/tmp/proj/.awidat/proxies");
        let asset = PathBuf::from("/tmp/proj/raw/foo.mov");
        let p = proxy_path_for(&proxies, &asset);
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(
            p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with("foo-1080p-")
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

        let proxies_dir = dir.path().join(".awidat").join("proxies");
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
    fn media_token_is_stable_for_same_path() {
        let path = PathBuf::from("/tmp/proj/.awidat/proxies/foo.mp4");

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
        let proxies_dir = dir.path().join(".awidat").join("proxies");
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
