//! Filmstrip thumbnail generation: one JPEG per second of source-time
//! per asset, dropped under `<project>/.montage/thumbnails/<stem>-<hash>/`.
//! The timeline canvas tiles these inside each clip.
//!
//! Idempotency: if any `frame-*.jpg` exists in the target dir AND its
//! mtime is at-or-after the source asset's mtime, we skip. Mirrors
//! `proxy_is_fresh` from `transcode.rs` — same trade-off, same
//! reasoning (a stale strip is "looks slightly off", not "wrong cut").

use std::path::{Path, PathBuf};

use montage_desktop_protocol::{Id, JobKind};
use tauri::{AppHandle, State};

use crate::commands::media::thumbnails_dir_for;
use crate::events::JobEmitter;
use crate::state::MontageState;

/// Generate (or refresh) filmstrip thumbnails for one asset under a
/// specific project. Used by the post-import chain so a project switch
/// can't redirect work into the wrong project. Returns the dir holding
/// the generated frames on success.
pub async fn generate_thumbnails_for_asset_in_project(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: &Path,
    asset_path: &Path,
) -> Result<PathBuf, String> {
    let dir = thumbnails_dir_for(project_root, asset_path);
    if thumbnails_are_fresh(asset_path, &dir) {
        return Ok(dir);
    }
    run_one(app, state, asset_path, &dir).await?;
    Ok(dir)
}

/// Drive one ffmpeg run end-to-end with a live job card.
async fn run_one(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    asset: &Path,
    output_dir: &Path,
) -> Result<(), String> {
    let job_id = Id::new(format!(
        "thumbnails-{}",
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
        JobKind::Thumbnails,
        format!("extracting filmstrip for {asset_label}"),
    );

    // ffmpeg with `-progress` is overkill for a thumbnails pass that
    // typically completes in seconds; we just emit a single
    // indeterminate tick so the card shows movement, then resolve.
    emitter.progress(None, format!("{asset_label}: working…"));

    let result = montage_render::generate_thumbnails(asset, output_dir, cancel.clone()).await;

    state.unregister_job(&job_id.0).await;

    match result {
        Ok(()) => {
            emitter.ok(Some(format!("filmstrip ready: {asset_label}")));
            Ok(())
        }
        Err(montage_render::FfmpegError::NonZero { stderr_tail, .. }) if cancel.is_cancelled() => {
            let _ = stderr_tail;
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            let msg = format!("thumbnails: {e}");
            emitter.err(msg.clone());
            Err(msg)
        }
    }
}

/// Resolve an untrusted, project-relative asset id against the project
/// root. Rejects ids that could escape the root (absolute paths, `..`,
/// backslashes, empty segments) before any filesystem access.
fn resolve_asset_in_root(project_root: &Path, asset_id: &str) -> Result<PathBuf, String> {
    if !montage_proto::index::is_safe_relative_asset_path(asset_id) {
        return Err(format!("invalid asset id: {asset_id:?}"));
    }
    Ok(project_root.join(asset_id))
}

/// True iff `dir` resolves inside `root` after symlink resolution. False
/// when either path can't be canonicalized (missing dir → caller already
/// handled; treat as outside).
fn dir_is_within_root(root: &Path, dir: &Path) -> bool {
    match (root.canonicalize(), dir.canonicalize()) {
        (Ok(root), Ok(dir)) => dir.starts_with(&root),
        _ => false,
    }
}

/// True iff `dir` exists, contains at least one `frame-*.jpg`, AND that
/// frame's mtime is at-or-after the source's. Inversely: false on
/// missing-dir, empty-dir, or stale-strip.
fn thumbnails_are_fresh(asset: &Path, dir: &Path) -> bool {
    let asset_meta = match std::fs::metadata(asset) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let Ok(asset_mtime) = asset_meta.modified() else {
        return false;
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("frame-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(frame_mtime) = meta.modified() else {
            continue;
        };
        // Any single up-to-date frame is enough — ffmpeg writes them in
        // order, so the first match implies the rest are fresh too.
        return frame_mtime >= asset_mtime;
    }
    false
}

/// List the absolute paths of every `frame-*.jpg` in a thumbnails
/// dir, sorted lexicographically (which lines up with frame-NNNN
/// numeric order). Returned paths are suitable for `convertFileSrc()`
/// on the frontend. Empty list when the dir doesn't exist or is empty.
///
/// `dir` MUST be a path the frontend already has from the protocol
/// (e.g. `TimelineItem::Clip.thumbnail_dir`). Thumbnail dirs always live
/// under `<project>/.montage/thumbnails/`, so we confine `dir` to the
/// loaded project root (post-symlink-resolution) as defense in depth on
/// top of the asset-protocol scope.
#[tauri::command]
pub async fn list_thumbnail_frames(
    state: State<'_, MontageState>,
    dir: String,
) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(&dir);
    if !path.is_dir() {
        return Ok(Vec::new());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    if !dir_is_within_root(&project_root, path) {
        return Err(format!("thumbnails dir outside project root: {dir:?}"));
    }
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|e| format!("read thumbnails dir: {e}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("read thumbnails entry: {e}"))?
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("frame-") || !name.ends_with(".jpg") {
            continue;
        }
        out.push(entry.path().to_string_lossy().into_owned());
    }
    out.sort();
    Ok(out)
}

/// Tauri command exposed for the unusual case where the frontend wants
/// to refresh thumbnails on demand for one asset (not currently wired
/// to any UI; here for parity with `proxy_path_for_stem` and as the
/// canonical entry point if a manual "regenerate filmstrip" affordance
/// shows up).
#[tauri::command]
pub async fn generate_thumbnails_for_asset(
    app: AppHandle,
    state: State<'_, MontageState>,
    asset_id: String,
) -> Result<Option<String>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let abs = resolve_asset_in_root(&project_root, &asset_id)?;
    if !abs.is_file() {
        return Ok(None);
    }
    let dir = generate_thumbnails_for_asset_in_project(&app, &state, &project_root, &abs).await?;
    Ok(Some(dir.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod confinement_tests {
    use super::*;

    #[test]
    fn resolve_asset_in_root_accepts_plain_relative_ids() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_asset_in_root(root, "media/clip.mp4").unwrap(),
            PathBuf::from("/proj/media/clip.mp4")
        );
    }

    #[test]
    fn resolve_asset_in_root_rejects_parent_traversal() {
        assert!(resolve_asset_in_root(Path::new("/proj"), "../secret").is_err());
        assert!(resolve_asset_in_root(Path::new("/proj"), "a/../../secret").is_err());
    }

    #[test]
    fn resolve_asset_in_root_rejects_absolute_paths() {
        assert!(resolve_asset_in_root(Path::new("/proj"), "/etc/passwd").is_err());
    }

    #[test]
    fn resolve_asset_in_root_rejects_backslashes_and_empty() {
        assert!(resolve_asset_in_root(Path::new("/proj"), "a\\b").is_err());
        assert!(resolve_asset_in_root(Path::new("/proj"), "").is_err());
    }

    #[test]
    fn dir_is_within_root_accepts_subdir() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join(".montage/thumbnails/x");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(dir_is_within_root(root.path(), &sub));
    }

    #[test]
    fn dir_is_within_root_rejects_outside_dir() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        assert!(!dir_is_within_root(root.path(), other.path()));
    }

    #[test]
    fn dir_is_within_root_rejects_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(!dir_is_within_root(root.path(), &link));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_stale_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        assert!(!thumbnails_are_fresh(&asset, &dir.path().join("nope")));
    }

    #[test]
    fn thumbnails_stale_when_dir_empty() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        let thumbs = dir.path().join("thumbs");
        std::fs::create_dir_all(&thumbs).unwrap();
        assert!(!thumbnails_are_fresh(&asset, &thumbs));
    }

    #[test]
    fn thumbnails_stale_when_dir_has_only_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        let thumbs = dir.path().join("thumbs");
        std::fs::create_dir_all(&thumbs).unwrap();
        std::fs::write(thumbs.join(".DS_Store"), b"y").unwrap();
        assert!(!thumbnails_are_fresh(&asset, &thumbs));
    }

    #[test]
    fn thumbnails_fresh_when_frame_newer() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("a.mp4");
        std::fs::write(&asset, b"x").unwrap();
        // Sleep so mtimes differ at filesystem resolution.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let thumbs = dir.path().join("thumbs");
        std::fs::create_dir_all(&thumbs).unwrap();
        std::fs::write(thumbs.join("frame-0001.jpg"), b"y").unwrap();
        assert!(thumbnails_are_fresh(&asset, &thumbs));
    }
}
