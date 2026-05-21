//! Shared project media discovery for raw assets, renders, and repair scans.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use awidat_proto::index::AssetId;

use crate::AssetInput;

/// One project media file discovered by the shared scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFile {
    /// Absolute file path on disk.
    pub path: PathBuf,
    /// Project-relative path with forward slashes.
    pub project_relative_path: String,
    /// Top-level scope where the file was found, e.g. `raw` or `renders`.
    pub scope: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Options for project media discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaScanOptions {
    /// Include source media under `raw/`.
    pub include_raw: bool,
    /// Include rendered outputs under `renders/`.
    pub include_renders: bool,
    /// Maximum files to return. `None` means no cap.
    pub max_files: Option<usize>,
}

impl Default for MediaScanOptions {
    fn default() -> Self {
        Self {
            include_raw: true,
            include_renders: false,
            max_files: None,
        }
    }
}

/// Collect media files from the configured project scopes.
pub fn collect_project_media_files(
    project_root: &Path,
    options: MediaScanOptions,
) -> std::io::Result<Vec<MediaFile>> {
    let mut out = Vec::new();
    if options.include_raw {
        collect_scope(project_root, "raw", options.max_files, &mut out)?;
    }
    if options.include_renders {
        collect_scope(project_root, "renders", options.max_files, &mut out)?;
    }
    out.sort_by(|a, b| a.project_relative_path.cmp(&b.project_relative_path));
    Ok(out)
}

/// Collect source media under `raw/` as dispatcher-ready asset inputs.
pub fn collect_raw_media_inputs(project_root: &Path) -> std::io::Result<Vec<AssetInput>> {
    collect_project_media_files(project_root, MediaScanOptions::default()).map(|files| {
        files
            .into_iter()
            .map(|file| AssetInput {
                id: AssetId::new(file.project_relative_path),
                path: file.path,
            })
            .collect()
    })
}

/// Return true when a path has a media/source-support extension Awidat knows how to catalog.
pub fn is_media_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp4"
            | "mov"
            | "m4v"
            | "mkv"
            | "webm"
            | "avi"
            | "wav"
            | "mp3"
            | "m4a"
            | "aac"
            | "flac"
            | "ogg"
            | "aif"
            | "aiff"
            | "png"
            | "jpg"
            | "jpeg"
            | "tif"
            | "tiff"
            | "webp"
            | "heic"
            | "psd"
            | "ai"
            | "svg"
            | "srt"
            | "vtt"
            | "itt"
            | "stl"
            | "cube"
            | "cdl"
            | "json"
            | "xml"
            | "otio"
            | "edl"
            | "ale"
    )
}

/// Return true when `dir` is a generated or VCS directory that media scans must skip.
pub fn is_ignored_scan_dir(project_root: &Path, dir: &Path) -> bool {
    if dir == project_root {
        return false;
    }
    let Ok(relative) = dir.strip_prefix(project_root) else {
        return true;
    };
    relative.components().next().is_some_and(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name == OsStr::new("index")
                    || name == OsStr::new(".awidat")
                    || name == OsStr::new(".git")
                    || name == OsStr::new("target")
        )
    })
}

fn collect_scope(
    project_root: &Path,
    scope: &str,
    max_files: Option<usize>,
    out: &mut Vec<MediaFile>,
) -> std::io::Result<()> {
    let root = project_root.join(scope);
    if !root.is_dir() {
        return Ok(());
    }
    walk(project_root, &root, scope, max_files, out)
}

fn walk(
    project_root: &Path,
    dir: &Path,
    scope: &str,
    max_files: Option<usize>,
    out: &mut Vec<MediaFile>,
) -> std::io::Result<()> {
    if max_files.is_some_and(|max| out.len() >= max) || is_ignored_scan_dir(project_root, dir) {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        if max_files.is_some_and(|max| out.len() >= max) {
            return Ok(());
        }
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            walk(project_root, &path, scope, max_files, out)?;
        } else if meta.is_file() && is_media_path(&path) {
            let Ok(relative) = path.strip_prefix(project_root) else {
                continue;
            };
            let project_relative_path = relative.to_string_lossy().replace('\\', "/");
            out.push(MediaFile {
                path,
                project_relative_path,
                scope: scope.to_string(),
                size_bytes: meta.len(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: impl AsRef<Path>) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    fn relative_paths(root: &Path, files: &[MediaFile]) -> Vec<String> {
        files
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn collect_project_media_skips_generated_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("raw/a.mp4"));
        write(dir.path().join("renders/out.mp4"));
        write(dir.path().join(".awidat/proxies/a.mp4"));
        write(dir.path().join("index/whisper/a.json"));

        let found = collect_project_media_files(dir.path(), MediaScanOptions::default()).unwrap();

        assert_eq!(relative_paths(dir.path(), &found), vec!["raw/a.mp4"]);
    }

    #[test]
    fn collect_raw_media_inputs_returns_project_relative_ids() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("raw/camera/a.mov"));
        write(dir.path().join("raw/camera/notes.txt"));

        let found = collect_raw_media_inputs(dir.path()).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.to_string(), "raw/camera/a.mov");
        assert_eq!(found[0].path, dir.path().join("raw/camera/a.mov"));
    }

    #[test]
    fn collect_project_media_can_scan_renders_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("raw/a.mp4"));
        write(dir.path().join("renders/out.mp4"));

        let found = collect_project_media_files(
            dir.path(),
            MediaScanOptions {
                include_renders: true,
                ..MediaScanOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            relative_paths(dir.path(), &found),
            vec!["raw/a.mp4", "renders/out.mp4"]
        );
    }
}
