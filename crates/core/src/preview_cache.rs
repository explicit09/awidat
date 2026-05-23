//! Agent-visible project preview-cache readiness.
//!
//! This mirrors the desktop preview cache contract without depending
//! on Tauri code: proxies, filmstrip thumbnails, and waveform sidecars
//! are summarized from disk so agents can plan refresh work before
//! expensive preview or editorial operations.

use std::path::{Path, PathBuf};

use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use serde::{Deserialize, Serialize};

use crate::proxy::{proxy_is_fresh, proxy_path_for};

/// Preview-cache artifact state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewArtifactStatus {
    /// Proxy exists and is at least as new as the source asset.
    Fresh,
    /// Thumbnail or waveform artifact exists and is usable.
    Ready,
    /// Artifact exists but is older than the source asset.
    Stale,
    /// Expected artifact is missing.
    Missing,
    /// Waveform exists but declares no audio buckets.
    Empty,
}

/// Count summary for one preview-cache artifact family.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PreviewArtifactCounts {
    /// Fresh proxy count.
    pub fresh_count: usize,
    /// Ready thumbnail or waveform count.
    pub ready_count: usize,
    /// Stale artifact count.
    pub stale_count: usize,
    /// Missing artifact count.
    pub missing_count: usize,
    /// Empty waveform count.
    pub empty_count: usize,
}

/// One concrete artifact generation/refresh task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshTask {
    /// Stable task id.
    pub task_id: String,
    /// Project-relative source asset id.
    pub asset_id: String,
    /// Absolute artifact path.
    pub artifact_path: String,
    /// Artifact family: proxy, thumbnails, or waveform.
    pub artifact_kind: String,
    /// Current artifact status.
    pub status: PreviewArtifactStatus,
    /// Relative scheduling cost.
    pub estimated_weight: u32,
    /// Stable machine-readable reason.
    pub reason: String,
}

/// Aggregate preview-cache refresh work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshWork {
    /// Assets requiring any artifact refresh.
    pub asset_count: usize,
    /// Proxies requiring generation or refresh.
    pub proxy_count: usize,
    /// Thumbnail sets requiring generation or refresh.
    pub thumbnails_count: usize,
    /// Waveform sidecars requiring generation or refresh.
    pub waveform_count: usize,
}

/// Artifact-family and task-limit options for preview-cache refresh planning.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PreviewCacheSelectionOptions {
    /// Optional project-relative asset id to select.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Include proxy refresh tasks.
    #[serde(default = "default_true")]
    pub proxy: bool,
    /// Include thumbnail refresh tasks.
    #[serde(default = "default_true")]
    pub thumbnails: bool,
    /// Include waveform refresh tasks.
    #[serde(default = "default_true")]
    pub waveform: bool,
    /// Optional maximum number of selected tasks.
    #[serde(default)]
    pub max_tasks: Option<usize>,
    /// Persist the selected refresh plan as a project-local lifecycle artifact.
    #[serde(default)]
    pub persist_refresh_plan: bool,
}

impl Default for PreviewCacheSelectionOptions {
    fn default() -> Self {
        Self {
            asset_id: None,
            proxy: true,
            thumbnails: true,
            waveform: true,
            max_tasks: None,
            persist_refresh_plan: false,
        }
    }
}

/// Selected preview-cache refresh work for a read-only plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshSelection {
    /// Tasks selected by asset, artifact-family, and limit filters.
    pub selected_refresh_tasks: Vec<PreviewCacheRefreshTask>,
    /// Aggregate work for selected tasks.
    pub selected_refresh_work: PreviewCacheRefreshWork,
    /// Number of selected tasks.
    pub selected_task_count: usize,
    /// Refresh tasks not selected by asset, artifact-family, or limit filters.
    pub skipped_task_count: usize,
}

/// Agent-facing execution contract for selected preview-cache refresh work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshExecutionContract {
    /// Concrete executor that can run the selected refresh tasks.
    pub executor: String,
    /// Current execution state for this plan.
    pub status: String,
    /// Artifact policy for this response.
    pub artifact_policy: String,
    /// Number of selected tasks.
    pub selected_task_count: usize,
    /// Aggregate work for selected tasks.
    pub selected_refresh_work: PreviewCacheRefreshWork,
    /// Stable selected task ids in execution order.
    pub selected_task_ids: Vec<String>,
}

/// Durable preview-cache refresh lifecycle record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshLifecycle {
    /// Absolute lifecycle artifact path.
    pub path: String,
    /// Current lifecycle status.
    pub status: String,
    /// Artifact policy for the persisted plan.
    pub artifact_policy: String,
    /// Number of selected tasks.
    pub selected_task_count: usize,
    /// Aggregate work for selected tasks.
    pub selected_refresh_work: PreviewCacheRefreshWork,
    /// Stable selected task ids in execution order.
    pub selected_task_ids: Vec<String>,
}

/// Per-asset preview-cache state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheEntry {
    /// Project-relative source asset id.
    pub asset_id: String,
    /// Absolute source asset path.
    pub asset_path: String,
    /// Expected proxy path.
    pub proxy_path: String,
    /// Expected thumbnails directory.
    pub thumbnails_dir: String,
    /// Expected waveform sidecar path.
    pub waveform_path: String,
    /// Proxy status.
    pub proxy: PreviewArtifactStatus,
    /// Thumbnail status.
    pub thumbnails: PreviewArtifactStatus,
    /// Waveform status.
    pub waveform: PreviewArtifactStatus,
    /// True when all preview artifacts are usable.
    pub preview_ready: bool,
}

/// Project-level preview-cache summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheSummary {
    /// Project root used for the scan.
    pub project_root: String,
    /// Number of source media assets.
    pub asset_count: usize,
    /// Number of preview-ready assets.
    pub ready_asset_count: usize,
    /// Proxy counts.
    pub proxy: PreviewArtifactCounts,
    /// Thumbnail counts.
    pub thumbnails: PreviewArtifactCounts,
    /// Waveform counts.
    pub waveforms: PreviewArtifactCounts,
    /// Per-asset entries.
    pub entries: Vec<PreviewCacheEntry>,
    /// Per-artifact refresh tasks.
    pub refresh_tasks: Vec<PreviewCacheRefreshTask>,
    /// Aggregate refresh work.
    pub refresh_work: PreviewCacheRefreshWork,
}

/// Build a project-level preview-cache summary.
pub fn build_preview_cache_summary(project_root: &Path) -> std::io::Result<PreviewCacheSummary> {
    let mut files = collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: None,
        },
    )?;
    files.sort_by(|left, right| left.project_relative_path.cmp(&right.project_relative_path));

    let mut proxy = PreviewArtifactCounts::default();
    let mut thumbnails = PreviewArtifactCounts::default();
    let mut waveforms = PreviewArtifactCounts::default();
    let mut entries = Vec::new();
    let mut refresh_tasks = Vec::new();
    let mut ready_asset_count = 0usize;

    for file in files {
        let proxy_path = proxy_path_for(project_root, &file.path);
        let thumbnails_dir = thumbnails_dir_for(project_root, &file.path);
        let waveform_path = waveform_path_for(project_root, &file.path);
        let proxy_status = proxy_status(&file.path, &proxy_path);
        let thumbnail_status =
            timestamped_presence_status(&file.path, &thumbnails_dir, has_thumbnail_frame);
        let waveform_status = waveform_status(&file.path, &waveform_path);

        bump_proxy_count(&mut proxy, proxy_status);
        bump_ready_family_count(&mut thumbnails, thumbnail_status);
        bump_waveform_count(&mut waveforms, waveform_status);

        if proxy_status == PreviewArtifactStatus::Fresh
            && thumbnail_status == PreviewArtifactStatus::Ready
            && matches!(
                waveform_status,
                PreviewArtifactStatus::Ready | PreviewArtifactStatus::Empty
            )
        {
            ready_asset_count += 1;
        }
        push_refresh_tasks(
            &mut refresh_tasks,
            &file.project_relative_path,
            (&proxy_path, proxy_status),
            (&thumbnails_dir, thumbnail_status),
            (&waveform_path, waveform_status),
        );
        entries.push(PreviewCacheEntry {
            asset_id: file.project_relative_path,
            asset_path: file.path.to_string_lossy().into_owned(),
            proxy_path: proxy_path.to_string_lossy().into_owned(),
            thumbnails_dir: thumbnails_dir.to_string_lossy().into_owned(),
            waveform_path: waveform_path.to_string_lossy().into_owned(),
            proxy: proxy_status,
            thumbnails: thumbnail_status,
            waveform: waveform_status,
            preview_ready: proxy_status == PreviewArtifactStatus::Fresh
                && thumbnail_status == PreviewArtifactStatus::Ready
                && matches!(
                    waveform_status,
                    PreviewArtifactStatus::Ready | PreviewArtifactStatus::Empty
                ),
        });
    }

    let refresh_work = refresh_work_from_tasks(&refresh_tasks);
    Ok(PreviewCacheSummary {
        project_root: project_root.to_string_lossy().into_owned(),
        asset_count: entries.len(),
        ready_asset_count,
        proxy,
        thumbnails,
        waveforms,
        entries,
        refresh_tasks,
        refresh_work,
    })
}

/// Select a bounded, read-only preview-cache refresh plan from a summary.
pub fn select_preview_cache_refresh_tasks(
    summary: &PreviewCacheSummary,
    options: &PreviewCacheSelectionOptions,
) -> PreviewCacheRefreshSelection {
    let mut skipped_task_count = 0usize;
    let mut selected_refresh_tasks = Vec::new();
    for task in &summary.refresh_tasks {
        if options
            .asset_id
            .as_ref()
            .is_some_and(|asset_id| task.asset_id != *asset_id)
            || !task_family_enabled(&task.artifact_kind, options)
            || options
                .max_tasks
                .is_some_and(|max_tasks| selected_refresh_tasks.len() >= max_tasks)
        {
            skipped_task_count += 1;
            continue;
        }
        selected_refresh_tasks.push(task.clone());
    }
    PreviewCacheRefreshSelection {
        selected_refresh_work: refresh_work_from_tasks(&selected_refresh_tasks),
        selected_task_count: selected_refresh_tasks.len(),
        skipped_task_count,
        selected_refresh_tasks,
    }
}

/// Build the execution contract for a selected preview-cache refresh plan.
pub fn preview_cache_refresh_execution_contract(
    selection: &PreviewCacheRefreshSelection,
) -> PreviewCacheRefreshExecutionContract {
    PreviewCacheRefreshExecutionContract {
        executor: "desktop_preview_cache_refresh".into(),
        status: if selection.selected_task_count > 0 {
            "ready_to_start".into()
        } else {
            "nothing_to_refresh".into()
        },
        artifact_policy: "no_render_job_started".into(),
        selected_task_count: selection.selected_task_count,
        selected_refresh_work: selection.selected_refresh_work.clone(),
        selected_task_ids: selection
            .selected_refresh_tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect(),
    }
}

/// Persist selected preview-cache refresh work as a project-local lifecycle artifact.
pub fn write_preview_cache_refresh_lifecycle(
    project_root: &Path,
    selection: &PreviewCacheRefreshSelection,
) -> std::io::Result<PreviewCacheRefreshLifecycle> {
    let path = preview_cache_refresh_lifecycle_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lifecycle = PreviewCacheRefreshLifecycle {
        path: path.to_string_lossy().into_owned(),
        status: "planned".into(),
        artifact_policy: "no_render_job_started".into(),
        selected_task_count: selection.selected_task_count,
        selected_refresh_work: selection.selected_refresh_work.clone(),
        selected_task_ids: selection
            .selected_refresh_tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect(),
    };
    let body = serde_json::to_vec_pretty(&lifecycle)?;
    std::fs::write(&path, body)?;
    Ok(lifecycle)
}

/// Project-local path for the durable preview-cache refresh lifecycle.
pub fn preview_cache_refresh_lifecycle_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".awidat")
        .join("preview-cache")
        .join("refresh-plan.json")
}

/// Compute the desktop-compatible thumbnails directory for an asset.
pub fn thumbnails_dir_for(project_root: &Path, asset_path: &Path) -> PathBuf {
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("asset");
    project_root
        .join(".awidat")
        .join("thumbnails")
        .join(format!("{stem}-{:08x}", stable_path_hash(asset_path)))
}

/// Compute the desktop-compatible waveform sidecar path for an asset.
pub fn waveform_path_for(project_root: &Path, asset_path: &Path) -> PathBuf {
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("asset");
    project_root
        .join(".awidat")
        .join("waveforms")
        .join(format!("{stem}-{:08x}.json", stable_path_hash(asset_path)))
}

fn proxy_status(asset_path: &Path, proxy_path: &Path) -> PreviewArtifactStatus {
    if proxy_is_fresh(asset_path, proxy_path) {
        PreviewArtifactStatus::Fresh
    } else if proxy_path.is_file() {
        PreviewArtifactStatus::Stale
    } else {
        PreviewArtifactStatus::Missing
    }
}

fn timestamped_presence_status(
    asset_path: &Path,
    artifact_path: &Path,
    presence_check: fn(&Path) -> bool,
) -> PreviewArtifactStatus {
    if !presence_check(artifact_path) {
        return PreviewArtifactStatus::Missing;
    }
    if artifact_is_fresh(asset_path, artifact_path) {
        PreviewArtifactStatus::Ready
    } else {
        PreviewArtifactStatus::Stale
    }
}

fn waveform_status(asset_path: &Path, waveform_path: &Path) -> PreviewArtifactStatus {
    if !waveform_path.is_file() {
        return PreviewArtifactStatus::Missing;
    }
    if !artifact_is_fresh(asset_path, waveform_path) {
        return PreviewArtifactStatus::Stale;
    }
    if waveform_has_non_empty_buckets(waveform_path) {
        PreviewArtifactStatus::Ready
    } else {
        PreviewArtifactStatus::Empty
    }
}

fn push_refresh_tasks(
    tasks: &mut Vec<PreviewCacheRefreshTask>,
    asset_id: &str,
    proxy: (&Path, PreviewArtifactStatus),
    thumbnails: (&Path, PreviewArtifactStatus),
    waveform: (&Path, PreviewArtifactStatus),
) {
    push_refresh_task(tasks, asset_id, "proxy", proxy.0, proxy.1);
    push_refresh_task(tasks, asset_id, "thumbnails", thumbnails.0, thumbnails.1);
    push_refresh_task(tasks, asset_id, "waveform", waveform.0, waveform.1);
}

fn push_refresh_task(
    tasks: &mut Vec<PreviewCacheRefreshTask>,
    asset_id: &str,
    family: &str,
    artifact_path: &Path,
    status: PreviewArtifactStatus,
) {
    let suffix = match status {
        PreviewArtifactStatus::Missing => "missing",
        PreviewArtifactStatus::Stale => "stale",
        PreviewArtifactStatus::Fresh
        | PreviewArtifactStatus::Ready
        | PreviewArtifactStatus::Empty => return,
    };
    let reason = format!("{family}_{suffix}");
    tasks.push(PreviewCacheRefreshTask {
        task_id: format!("{family}:{asset_id}:{reason}"),
        asset_id: asset_id.into(),
        artifact_path: artifact_path.to_string_lossy().into_owned(),
        artifact_kind: family.into(),
        status,
        estimated_weight: refresh_task_weight(family, status),
        reason,
    });
}

fn refresh_task_weight(family: &str, status: PreviewArtifactStatus) -> u32 {
    let base = match family {
        "proxy" => 3,
        "thumbnails" => 2,
        "waveform" => 1,
        _ => 1,
    };
    match status {
        PreviewArtifactStatus::Stale => base + 1,
        PreviewArtifactStatus::Missing => base,
        PreviewArtifactStatus::Fresh
        | PreviewArtifactStatus::Ready
        | PreviewArtifactStatus::Empty => 0,
    }
}

fn refresh_work_from_tasks(tasks: &[PreviewCacheRefreshTask]) -> PreviewCacheRefreshWork {
    let asset_count = tasks
        .iter()
        .map(|task| task.asset_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    PreviewCacheRefreshWork {
        asset_count,
        proxy_count: tasks
            .iter()
            .filter(|task| task.artifact_kind == "proxy")
            .count(),
        thumbnails_count: tasks
            .iter()
            .filter(|task| task.artifact_kind == "thumbnails")
            .count(),
        waveform_count: tasks
            .iter()
            .filter(|task| task.artifact_kind == "waveform")
            .count(),
    }
}

fn task_family_enabled(family: &str, options: &PreviewCacheSelectionOptions) -> bool {
    match family {
        "proxy" => options.proxy,
        "thumbnails" => options.thumbnails,
        "waveform" => options.waveform,
        _ => false,
    }
}

fn default_true() -> bool {
    true
}

fn artifact_is_fresh(asset_path: &Path, artifact_path: &Path) -> bool {
    let Some(artifact_meta) = freshest_metadata(artifact_path) else {
        return false;
    };
    let Ok(asset_meta) = std::fs::metadata(asset_path) else {
        return false;
    };
    let (Ok(artifact_mtime), Ok(asset_mtime)) = (artifact_meta.modified(), asset_meta.modified())
    else {
        return false;
    };
    artifact_mtime >= asset_mtime
}

fn freshest_metadata(path: &Path) -> Option<std::fs::Metadata> {
    if path.is_file() {
        return std::fs::metadata(path).ok();
    }
    if !path.is_dir() {
        return None;
    }
    std::fs::read_dir(path)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .max_by_key(|metadata| metadata.modified().ok())
}

fn has_thumbnail_frame(path: &Path) -> bool {
    path.is_dir()
        && std::fs::read_dir(path)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("frame-"))
}

fn waveform_has_non_empty_buckets(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let compact = contents
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .take(64)
        .collect::<String>();
    !compact.contains("\"buckets\":[]")
}

fn bump_proxy_count(counts: &mut PreviewArtifactCounts, status: PreviewArtifactStatus) {
    match status {
        PreviewArtifactStatus::Fresh => counts.fresh_count += 1,
        PreviewArtifactStatus::Stale => counts.stale_count += 1,
        PreviewArtifactStatus::Missing => counts.missing_count += 1,
        PreviewArtifactStatus::Ready | PreviewArtifactStatus::Empty => {}
    }
}

fn bump_ready_family_count(counts: &mut PreviewArtifactCounts, status: PreviewArtifactStatus) {
    match status {
        PreviewArtifactStatus::Ready => counts.ready_count += 1,
        PreviewArtifactStatus::Stale => counts.stale_count += 1,
        PreviewArtifactStatus::Missing => counts.missing_count += 1,
        PreviewArtifactStatus::Fresh | PreviewArtifactStatus::Empty => {}
    }
}

fn bump_waveform_count(counts: &mut PreviewArtifactCounts, status: PreviewArtifactStatus) {
    match status {
        PreviewArtifactStatus::Ready => counts.ready_count += 1,
        PreviewArtifactStatus::Stale => counts.stale_count += 1,
        PreviewArtifactStatus::Missing => counts.missing_count += 1,
        PreviewArtifactStatus::Empty => counts.empty_count += 1,
        PreviewArtifactStatus::Fresh => {}
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

    #[test]
    fn preview_cache_summary_reports_proxy_thumbnail_and_waveform_work() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let summary = build_preview_cache_summary(dir.path()).unwrap();

        assert_eq!(summary.asset_count, 1);
        assert_eq!(summary.ready_asset_count, 0);
        assert_eq!(summary.proxy.missing_count, 1);
        assert_eq!(summary.thumbnails.missing_count, 1);
        assert_eq!(summary.waveforms.missing_count, 1);
        assert_eq!(summary.refresh_work.asset_count, 1);
        assert_eq!(summary.refresh_work.proxy_count, 1);
        assert_eq!(summary.refresh_work.thumbnails_count, 1);
        assert_eq!(summary.refresh_work.waveform_count, 1);
        assert_eq!(summary.refresh_tasks.len(), 3);
    }
}
