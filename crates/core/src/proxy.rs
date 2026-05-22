//! Shared proxy cache helpers for agent-callable media operations.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use serde::Serialize;

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
    /// Pending proxy artifact exists.
    Pending,
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

/// Return the stable proxy path for a source asset.
pub fn proxy_path_for(project_root: &Path, asset_path: &Path) -> PathBuf {
    let proxies_dir = project_root.join(".awidat").join("proxies");
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("asset");
    proxies_dir.join(format!(
        "{stem}-1080p-{:016x}.mp4",
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
    let (status, size_bytes) = if pending_path.is_file() {
        (
            ProxyStatus::Pending,
            std::fs::metadata(&pending_path).map(|meta| meta.len()).ok(),
        )
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

fn stable_path_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
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
    fn proxy_path_is_stable_and_under_awidat() {
        let root = Path::new("/project");
        let asset = Path::new("/project/raw/camera.mov");

        let first = proxy_path_for(root, asset);
        let second = proxy_path_for(root, asset);

        assert_eq!(first, second);
        assert!(
            first
                .to_string_lossy()
                .contains(".awidat/proxies/camera-1080p-")
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
    fn proxy_status_for_project_scans_raw_assets() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("raw/a.mov"), b"a");
        write(&dir.path().join("renders/out.mov"), b"render");

        let entries = proxy_status_for_project(dir.path()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].asset_id, "raw/a.mov");
    }
}
