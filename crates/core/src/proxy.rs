//! Shared proxy cache helpers for agent-callable media operations.
//!
//! Proxy filenames follow `{stem}-{PROXY_SCHEMA_TAG}-{8xhash}.mp4` so the
//! agent-side helpers stay byte-compatible with the desktop transcoder's
//! own `commands/media.rs::proxy_path_for` (both hash the absolute asset
//! path via FNV-1a 32-bit and embed the same render-side schema tag).
//! Bump `montage_render::PROXY_SCHEMA_TAG` to invalidate old proxies.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use montage_index::media_files::{MediaScanOptions, collect_project_media_files};
use montage_render::{DEFAULT_PROXY_TIMEOUT, PROXY_SCHEMA_TAG};
use serde::Serialize;

/// Age past which a `.pending` proxy artifact is treated as abandoned
/// rather than actively in-flight (R11). `transcode_proxy` itself now
/// bounds a single attempt to `montage_render::DEFAULT_PROXY_TIMEOUT`
/// (overridable via `MONTAGE_PROXY_TIMEOUT_SECS`) and removes the
/// `.pending` file on that timeout — so a `.pending` file surviving
/// past 2x that bound means the writer crashed, was killed, or the
/// process died mid-write without running its own cleanup (e.g. a
/// desktop force-quit or OOM-kill), not that a legitimate transcode is
/// still running. 2x leaves headroom for `DEFAULT_PROXY_TIMEOUT`
/// overrides and slow filesystems flushing the rename.
pub fn stale_pending_age_threshold() -> Duration {
    DEFAULT_PROXY_TIMEOUT * 2
}

/// Agent-visible proxy cache status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyStatus {
    /// Proxy exists and is at least as new as the source asset.
    Fresh,
    /// Proxy exists but is older than the source asset.
    Stale,
    /// Proxy does not exist.
    Missing,
    /// Pending proxy artifact exists and is recent enough to plausibly
    /// be an active transcode.
    Pending,
    /// A `.pending` artifact exists but is older than
    /// [`stale_pending_age_threshold`] — the writer is presumed dead.
    /// Callers should treat this like `Missing` (safe to regenerate)
    /// rather than waiting on it forever.
    StalePending,
}

/// Status for one source asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProxyStatusEntry {
    /// Project-relative asset id.
    pub asset_id: String,
    /// Absolute source asset path.
    pub asset_path: String,
    /// Absolute proxy path.
    pub proxy_path: String,
    /// Current proxy status.
    pub status: ProxyStatus,
    /// Proxy size when a proxy artifact exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Return the stable proxy path for a source asset. Matches the desktop
/// transcoder's `commands/media.rs::proxy_path_for` byte-for-byte so the
/// agent's reads land on the same files the desktop writes.
pub fn proxy_path_for(project_root: &Path, asset_path: &Path) -> PathBuf {
    let proxies_dir = project_root.join(".montage").join("proxies");
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("asset");
    proxies_dir.join(format!(
        "{stem}-{}-{:08x}.mp4",
        PROXY_SCHEMA_TAG,
        stable_path_hash(asset_path)
    ))
}

/// Return the pending path used while ffmpeg writes a proxy.
pub fn proxy_pending_path(proxy_path: &Path) -> PathBuf {
    let mut raw = proxy_path.as_os_str().to_os_string();
    raw.push(".pending");
    PathBuf::from(raw)
}

/// Build status for one source asset.
pub fn proxy_status_for(
    project_root: &Path,
    asset_id: &str,
    asset_path: &Path,
) -> ProxyStatusEntry {
    let proxy_path = proxy_path_for(project_root, asset_path);
    let pending_path = proxy_pending_path(&proxy_path);
    let (status, size_bytes) = if let Ok(meta) = std::fs::metadata(&pending_path) {
        let status = if pending_is_stale(&meta) {
            ProxyStatus::StalePending
        } else {
            ProxyStatus::Pending
        };
        (status, Some(meta.len()))
    } else {
        match std::fs::metadata(&proxy_path) {
            Ok(meta) if proxy_is_fresh(asset_path, &proxy_path) => {
                (ProxyStatus::Fresh, Some(meta.len()))
            }
            Ok(meta) => (ProxyStatus::Stale, Some(meta.len())),
            Err(_) => (ProxyStatus::Missing, None),
        }
    };
    ProxyStatusEntry {
        asset_id: asset_id.to_string(),
        asset_path: asset_path.to_string_lossy().into_owned(),
        proxy_path: proxy_path.to_string_lossy().into_owned(),
        status,
        size_bytes,
    }
}

/// Build proxy status for all raw media files in the project.
pub fn proxy_status_for_project(project_root: &Path) -> std::io::Result<Vec<ProxyStatusEntry>> {
    collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: None,
        },
    )
    .map(|files| {
        files
            .into_iter()
            .map(|file| proxy_status_for(project_root, &file.project_relative_path, &file.path))
            .collect()
    })
}

/// True when a proxy exists and is not older than the source.
pub fn proxy_is_fresh(asset_path: &Path, proxy_path: &Path) -> bool {
    let Ok(proxy_mtime) = modified(proxy_path) else {
        return false;
    };
    let Ok(asset_mtime) = modified(asset_path) else {
        return false;
    };
    proxy_mtime >= asset_mtime
}

fn modified(path: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(path)?.modified()
}

/// True when a `.pending` file's mtime is older than
/// [`stale_pending_age_threshold`]. Unknown/unreadable mtime (rare —
/// e.g. a filesystem without mtime support) errs toward "not stale" so
/// we never prune a possibly-active transcode on a metadata read
/// failure; the file will get another chance to age out on the next
/// scan once mtime is readable.
fn pending_is_stale(meta: &std::fs::Metadata) -> bool {
    let Ok(modified) = meta.modified() else {
        return false;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age > stale_pending_age_threshold(),
        Err(_) => false, // clock skew (mtime in the future) — not stale.
    }
}

fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn proxy_path_is_stable_and_under_montage() {
        let root = Path::new("/project");
        let asset = Path::new("/project/raw/camera.mov");

        let first = proxy_path_for(root, asset);
        let second = proxy_path_for(root, asset);

        assert_eq!(first, second);
        let needle = format!(".montage/proxies/camera-{}-", PROXY_SCHEMA_TAG);
        assert!(
            first.to_string_lossy().contains(&needle),
            "expected proxy path to contain `{needle}`, got `{}`",
            first.display()
        );
    }

    #[test]
    fn proxy_status_reports_missing_and_pending() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/camera.mov");
        write(&asset, b"media");

        let missing = proxy_status_for(dir.path(), "raw/camera.mov", &asset);
        assert_eq!(missing.status, ProxyStatus::Missing);
        assert_eq!(missing.size_bytes, None);

        let pending_path = proxy_pending_path(&PathBuf::from(&missing.proxy_path));
        write(&pending_path, b"pending");

        let pending = proxy_status_for(dir.path(), "raw/camera.mov", &asset);
        assert_eq!(pending.status, ProxyStatus::Pending);
        assert_eq!(pending.size_bytes, Some(7));
    }

    #[test]
    fn proxy_status_reports_stale_pending_past_the_age_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/camera.mov");
        write(&asset, b"media");

        let proxy_path = proxy_path_for(dir.path(), &asset);
        let pending_path = proxy_pending_path(&proxy_path);
        write(&pending_path, b"partial");

        // Fresh pending: recent mtime, plausibly in-flight.
        let entry = proxy_status_for(dir.path(), "raw/camera.mov", &asset);
        assert_eq!(entry.status, ProxyStatus::Pending);

        // Backdate the .pending file's mtime past the abandonment
        // threshold without sleeping for real (the default threshold is
        // 60 real minutes) — `File::set_modified` is stable since Rust
        // 1.75, no extra dependency needed.
        let file = std::fs::File::options()
            .write(true)
            .open(&pending_path)
            .unwrap();
        let backdated =
            SystemTime::now() - (stale_pending_age_threshold() + Duration::from_secs(60));
        file.set_modified(backdated).unwrap();

        let entry = proxy_status_for(dir.path(), "raw/camera.mov", &asset);
        assert_eq!(
            entry.status,
            ProxyStatus::StalePending,
            "a .pending file past the age threshold must be reported as abandoned, \
             not left as Pending forever (R11)"
        );
        // Size is still reported so a caller can see what it's about to
        // replace/prune.
        assert_eq!(entry.size_bytes, Some(7));
    }

    #[test]
    fn proxy_status_for_project_scans_raw_assets() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("raw/a.mov"), b"a");
        write(&dir.path().join("renders/out.mov"), b"render");

        let entries = proxy_status_for_project(dir.path()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].asset_id, "raw/a.mov");
    }
}
