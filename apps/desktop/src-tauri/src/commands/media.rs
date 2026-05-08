//! Media-pane Tauri commands.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;

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

/// Return a localhost streaming URL for a proxy file. Unlike Tauri's
/// `asset:` URL, this preserves audio in WKWebView; unlike a blob URL,
/// it does not load a multi-GB proxy into WebKit memory.
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
    let proxies_dir = std::fs::canonicalize(project_root.join(".awidat").join("proxies"))
        .map_err(|e| format!("proxies dir unavailable: {e}"))?;
    if !requested.starts_with(&proxies_dir) || !is_proxy_file(&requested) {
        return Err("media path is outside this project's proxy directory".into());
    }

    let (port, files) = ensure_media_server(&state)?;
    let token = media_token(&requested);
    files
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?
        .insert(token.clone(), requested);
    Ok(format!("http://127.0.0.1:{port}/media/{token}"))
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
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nContent-Length: {content_len}\r\nConnection: close\r\n"
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

fn write_simple_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn media_token(path: &Path) -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", since_epoch, stable_path_hash(path))
}

/// Compute the absolute proxy-mp4 path for an asset path.
///
/// Used by the transcoder when generating proxies AND by the
/// timeline flattener when telling the frontend which mp4 to play
/// for each clip's segment. Both paths must agree byte-for-byte —
/// hence one shared helper rather than parallel implementations.
///
/// The proxy filename is `<asset-stem>-<hash>.mp4`. The hash is FNV-1a
/// over the asset's absolute path string and disambiguates two raw/
/// files that share the same stem in nested subdirectories. Callers
/// must pass the absolute path (not project-relative) — feeding in a
/// relative path produces a different hash and the resulting proxy
/// path won't match the one the transcoder wrote.
pub fn proxy_path_for(proxies_dir: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    proxies_dir.join(format!(
        "{stem}-{:08x}.mp4",
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
                .starts_with("foo-")
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
}
