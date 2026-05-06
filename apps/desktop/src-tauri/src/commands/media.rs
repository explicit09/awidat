//! Media-pane Tauri commands. Currently just listing proxies; the
//! preview itself is served via Tauri's built-in `asset:` protocol
//! whose scope was opened to the project's proxies dir by
//! `set_project_root` / `init_project`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

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
    proxies_dir.join(format!("{stem}-{:08x}.mp4", stable_path_hash(asset_abs_path)))
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
    proxy.is_file().then(|| proxy.to_string_lossy().into_owned())
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
    project_root
        .join(".awidat")
        .join("waveforms")
        .join(format!("{stem}-{:08x}.json", stable_path_hash(asset_abs_path)))
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
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("frame-")
        });
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
