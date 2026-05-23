//! Project-level preview cache summary.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::media::{proxy_path_for, thumbnails_dir_for, waveform_path_for};
use crate::commands::transcode::{collect_media, proxy_is_fresh};
use crate::state::AwidatState;

/// Artifact status for preview cache components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewArtifactStatus {
    /// File exists and is fresh relative to the source asset.
    Fresh,
    /// Usable artifact exists.
    Ready,
    /// File exists but is older than the source asset.
    Stale,
    /// Expected artifact is missing.
    Missing,
    /// Waveform sidecar exists but declares no audio buckets.
    Empty,
}

/// Count summary for a preview artifact family.
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
    /// Empty waveform sidecar count.
    pub empty_count: usize,
}

/// Per-asset preview cache state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheEntry {
    /// Source asset path.
    pub asset_path: String,
    /// Expected proxy path.
    pub proxy_path: String,
    /// Expected thumbnails directory.
    pub thumbnails_dir: String,
    /// Expected waveform sidecar path.
    pub waveform_path: String,
    /// Proxy state.
    pub proxy: PreviewArtifactStatus,
    /// Thumbnail state.
    pub thumbnails: PreviewArtifactStatus,
    /// Waveform state.
    pub waveform: PreviewArtifactStatus,
    /// True when all visual-preview artifacts are usable.
    pub preview_ready: bool,
}

/// Regeneration work needed to make one asset preview-ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshCandidate {
    /// Source asset path.
    pub asset_path: String,
    /// Proxy needs generation or refresh.
    pub refresh_proxy: bool,
    /// Thumbnails need generation or refresh.
    pub refresh_thumbnails: bool,
    /// Waveform sidecar needs generation or refresh.
    pub refresh_waveform: bool,
    /// Stable machine-readable reasons.
    pub reasons: Vec<String>,
}

/// Aggregate preview-cache generation work for the current project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshWork {
    /// Assets requiring any preview-cache generation or refresh.
    pub asset_count: usize,
    /// Proxies requiring generation or refresh.
    pub proxy_count: usize,
    /// Thumbnail sets requiring generation or refresh.
    pub thumbnails_count: usize,
    /// Waveform sidecars requiring generation or refresh.
    pub waveform_count: usize,
}

/// One concrete artifact generation/refresh task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshTask {
    /// Stable task id for deduping and progress correlation.
    pub task_id: String,
    /// Source asset path.
    pub asset_path: String,
    /// Artifact family: proxy, thumbnails, or waveform.
    pub artifact_kind: String,
    /// Artifact path to create or refresh.
    pub artifact_path: String,
    /// Current artifact status.
    pub status: PreviewArtifactStatus,
    /// Relative scheduling cost; higher tasks should be budgeted as more expensive.
    pub estimated_weight: u32,
    /// Stable machine-readable reason.
    pub reason: String,
}

/// Project-level preview cache summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheSummary {
    /// Project root used for the scan.
    pub project_root: String,
    /// Number of source media assets.
    pub asset_count: usize,
    /// Assets with fresh proxy, ready thumbnails, and ready/empty waveform status.
    pub ready_asset_count: usize,
    /// Proxy counts.
    pub proxy: PreviewArtifactCounts,
    /// Thumbnail counts.
    pub thumbnails: PreviewArtifactCounts,
    /// Waveform counts.
    pub waveforms: PreviewArtifactCounts,
    /// Ordered per-asset entries.
    pub entries: Vec<PreviewCacheEntry>,
    /// Ordered assets requiring preview-cache generation or refresh.
    pub refresh_candidates: Vec<PreviewCacheRefreshCandidate>,
    /// Ordered per-artifact work items for preview-cache scheduling.
    pub refresh_tasks: Vec<PreviewCacheRefreshTask>,
    /// Aggregate refresh work for scheduling/progress UI.
    pub refresh_work: PreviewCacheRefreshWork,
}

/// Artifact families to refresh. Defaults refresh every stale/missing family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewCacheRefreshOptions {
    /// Optional project-relative asset id to refresh, such as `raw/a.mov`.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Refresh proxy media.
    #[serde(default = "default_true")]
    pub proxy: bool,
    /// Refresh filmstrip thumbnails.
    #[serde(default = "default_true")]
    pub thumbnails: bool,
    /// Refresh waveform sidecars.
    #[serde(default = "default_true")]
    pub waveform: bool,
    /// Optional maximum number of artifact tasks to run.
    #[serde(default)]
    pub max_tasks: Option<usize>,
    /// When true, return the plan without generating artifacts.
    #[serde(default)]
    pub dry_run: bool,
}

impl Default for PreviewCacheRefreshOptions {
    fn default() -> Self {
        Self {
            asset_id: None,
            proxy: true,
            thumbnails: true,
            waveform: true,
            max_tasks: None,
            dry_run: false,
        }
    }
}

/// Concrete preview-cache refresh plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshPlan {
    /// True when the command should only report planned work.
    pub dry_run: bool,
    /// Tasks selected by the requested artifact families and limit.
    pub tasks: Vec<PreviewCacheRefreshTask>,
    /// Number of selected tasks.
    pub planned_task_count: usize,
    /// Refresh tasks not selected because of family filters or max_tasks.
    pub skipped_task_count: usize,
}

/// Per-task execution result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshTaskResult {
    /// Planned task.
    pub task: PreviewCacheRefreshTask,
    /// Execution state: planned, completed, failed, or skipped.
    pub status: String,
    /// Optional generated artifact path.
    pub output_path: Option<String>,
    /// Optional failure or skip reason.
    pub message: Option<String>,
}

/// Preview-cache refresh command result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewCacheRefreshReport {
    /// Plan used by the command.
    pub plan: PreviewCacheRefreshPlan,
    /// Per-task outcomes.
    pub results: Vec<PreviewCacheRefreshTaskResult>,
    /// Completed artifact task count.
    pub completed_count: usize,
    /// Failed artifact task count.
    pub failed_count: usize,
}

fn default_true() -> bool {
    true
}

/// Return a project-level preview cache summary for the loaded project.
#[tauri::command]
pub async fn preview_cache_summary(
    state: State<'_, AwidatState>,
) -> Result<PreviewCacheSummary, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    build_preview_cache_summary(&project_root)
}

/// Generate or refresh missing/stale preview-cache artifacts.
#[tauri::command]
pub async fn preview_cache_refresh(
    app: AppHandle,
    state: State<'_, AwidatState>,
    options: Option<PreviewCacheRefreshOptions>,
) -> Result<PreviewCacheRefreshReport, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let options = options.unwrap_or_default();
    let plan = build_preview_cache_refresh_plan(&project_root, options)?;
    if plan.dry_run {
        let results = plan
            .tasks
            .iter()
            .cloned()
            .map(|task| PreviewCacheRefreshTaskResult {
                task,
                status: "planned".into(),
                output_path: None,
                message: None,
            })
            .collect();
        return Ok(PreviewCacheRefreshReport {
            plan,
            results,
            completed_count: 0,
            failed_count: 0,
        });
    }

    execute_preview_cache_refresh_plan(app, state, &project_root, plan).await
}

fn build_preview_cache_refresh_plan(
    project_root: &Path,
    options: PreviewCacheRefreshOptions,
) -> Result<PreviewCacheRefreshPlan, String> {
    validate_asset_id_filter(options.asset_id.as_deref())?;
    let summary = build_preview_cache_summary(project_root)?;
    let mut skipped_task_count = 0usize;
    let mut tasks = Vec::new();
    for task in summary.refresh_tasks {
        if !task_asset_enabled(project_root, &task, options.asset_id.as_deref())
            || !task_family_enabled(&task.artifact_kind, &options)
        {
            skipped_task_count += 1;
            continue;
        }
        if options
            .max_tasks
            .is_some_and(|max_tasks| tasks.len() >= max_tasks)
        {
            skipped_task_count += 1;
            continue;
        }
        tasks.push(task);
    }
    Ok(PreviewCacheRefreshPlan {
        dry_run: options.dry_run,
        planned_task_count: tasks.len(),
        skipped_task_count,
        tasks,
    })
}

fn validate_asset_id_filter(asset_id: Option<&str>) -> Result<(), String> {
    let Some(asset_id) = asset_id else {
        return Ok(());
    };
    let path = Path::new(asset_id);
    if asset_id.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(
            "preview_cache_refresh: asset_id must be a non-empty project-relative path".into(),
        );
    }
    Ok(())
}

fn task_asset_enabled(
    project_root: &Path,
    task: &PreviewCacheRefreshTask,
    asset_id: Option<&str>,
) -> bool {
    let Some(asset_id) = asset_id else {
        return true;
    };
    Path::new(&task.asset_path)
        .strip_prefix(project_root)
        .ok()
        .and_then(|path| path.to_str())
        == Some(asset_id)
}

fn task_family_enabled(family: &str, options: &PreviewCacheRefreshOptions) -> bool {
    match family {
        "proxy" => options.proxy,
        "thumbnails" => options.thumbnails,
        "waveform" => options.waveform,
        _ => false,
    }
}

async fn execute_preview_cache_refresh_plan(
    app: AppHandle,
    state: State<'_, AwidatState>,
    project_root: &Path,
    plan: PreviewCacheRefreshPlan,
) -> Result<PreviewCacheRefreshReport, String> {
    let mut results = Vec::new();
    let mut completed_count = 0usize;
    let mut failed_count = 0usize;
    for task in plan.tasks.iter().cloned() {
        let result = execute_preview_cache_task(&app, &state, project_root, task).await;
        if result.status == "completed" {
            completed_count += 1;
        }
        if result.status == "failed" {
            failed_count += 1;
        }
        results.push(result);
    }
    Ok(PreviewCacheRefreshReport {
        plan,
        results,
        completed_count,
        failed_count,
    })
}

async fn execute_preview_cache_task(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    project_root: &Path,
    task: PreviewCacheRefreshTask,
) -> PreviewCacheRefreshTaskResult {
    let asset = Path::new(&task.asset_path);
    let result = match task.artifact_kind.as_str() {
        "proxy" => crate::commands::transcode::transcode_single_asset_in_project(
            app,
            state,
            project_root,
            asset,
        )
        .await
        .map(|path| path.map(|path| path.to_string_lossy().into_owned())),
        "thumbnails" => crate::commands::thumbnail::generate_thumbnails_for_asset_in_project(
            app,
            state,
            project_root,
            asset,
        )
        .await
        .map(|path| Some(path.to_string_lossy().into_owned())),
        "waveform" => crate::commands::waveform::generate_waveform_for_asset_in_project(
            app,
            state,
            project_root,
            asset,
        )
        .await
        .map(|path| path.map(|path| path.to_string_lossy().into_owned())),
        _ => Err(format!(
            "unsupported preview cache artifact kind: {}",
            task.artifact_kind
        )),
    };
    match result {
        Ok(output_path) => PreviewCacheRefreshTaskResult {
            task,
            status: "completed".into(),
            output_path,
            message: None,
        },
        Err(message) => PreviewCacheRefreshTaskResult {
            task,
            status: "failed".into(),
            output_path: None,
            message: Some(message),
        },
    }
}

fn build_preview_cache_summary(project_root: &Path) -> Result<PreviewCacheSummary, String> {
    let raw_dir = project_root.join("raw");
    let mut assets = if raw_dir.is_dir() {
        collect_media(&raw_dir).map_err(|e| format!("scan raw/: {e}"))?
    } else {
        Vec::new()
    };
    assets.sort();

    let proxies_dir = project_root.join(".awidat").join("proxies");
    let mut proxy_counts = PreviewArtifactCounts::default();
    let mut thumbnail_counts = PreviewArtifactCounts::default();
    let mut waveform_counts = PreviewArtifactCounts::default();
    let mut entries = Vec::new();
    let mut refresh_candidates = Vec::new();
    let mut refresh_tasks = Vec::new();
    let mut ready_asset_count = 0;

    for asset in assets {
        let proxy_path = proxy_path_for(&proxies_dir, &asset);
        let thumbnails_dir = thumbnails_dir_for(project_root, &asset);
        let waveform_path = waveform_path_for(project_root, &asset);
        let proxy = proxy_status(&asset, &proxy_path);
        let thumbnails = timestamped_presence_status(&asset, &thumbnails_dir, has_thumbnail_frame);
        let waveform = waveform_status(&asset, &waveform_path);

        bump_proxy_count(&mut proxy_counts, proxy);
        bump_ready_family_count(&mut thumbnail_counts, thumbnails);
        bump_waveform_count(&mut waveform_counts, waveform);

        let preview_ready = proxy == PreviewArtifactStatus::Fresh
            && thumbnails == PreviewArtifactStatus::Ready
            && matches!(
                waveform,
                PreviewArtifactStatus::Ready | PreviewArtifactStatus::Empty
            );
        if preview_ready {
            ready_asset_count += 1;
        }
        if let Some(candidate) = refresh_candidate_for_asset(&asset, proxy, thumbnails, waveform) {
            refresh_candidates.push(candidate);
        }
        refresh_tasks.extend(refresh_tasks_for_asset(
            &asset,
            (&proxy_path, proxy),
            (&thumbnails_dir, thumbnails),
            (&waveform_path, waveform),
        ));
        entries.push(PreviewCacheEntry {
            asset_path: asset.to_string_lossy().into_owned(),
            proxy_path: proxy_path.to_string_lossy().into_owned(),
            thumbnails_dir: thumbnails_dir.to_string_lossy().into_owned(),
            waveform_path: waveform_path.to_string_lossy().into_owned(),
            proxy,
            thumbnails,
            waveform,
            preview_ready,
        });
    }

    Ok(PreviewCacheSummary {
        project_root: project_root.to_string_lossy().into_owned(),
        asset_count: entries.len(),
        ready_asset_count,
        proxy: proxy_counts,
        thumbnails: thumbnail_counts,
        waveforms: waveform_counts,
        refresh_work: refresh_work_from_candidates(&refresh_candidates),
        entries,
        refresh_candidates,
        refresh_tasks,
    })
}

fn proxy_status(asset: &Path, proxy_path: &Path) -> PreviewArtifactStatus {
    if proxy_is_fresh(asset, proxy_path) {
        PreviewArtifactStatus::Fresh
    } else if proxy_path.is_file() {
        PreviewArtifactStatus::Stale
    } else {
        PreviewArtifactStatus::Missing
    }
}

fn timestamped_presence_status(
    asset: &Path,
    path: &Path,
    presence_check: fn(&Path) -> bool,
) -> PreviewArtifactStatus {
    if !presence_check(path) {
        return PreviewArtifactStatus::Missing;
    }
    if artifact_is_fresh(asset, path) {
        PreviewArtifactStatus::Ready
    } else {
        PreviewArtifactStatus::Stale
    }
}

fn waveform_status(asset: &Path, path: &Path) -> PreviewArtifactStatus {
    if !path.is_file() {
        return PreviewArtifactStatus::Missing;
    }
    if !artifact_is_fresh(asset, path) {
        return PreviewArtifactStatus::Stale;
    }
    if waveform_has_non_empty_buckets(path) {
        PreviewArtifactStatus::Ready
    } else {
        PreviewArtifactStatus::Empty
    }
}

fn refresh_candidate_for_asset(
    asset: &Path,
    proxy: PreviewArtifactStatus,
    thumbnails: PreviewArtifactStatus,
    waveform: PreviewArtifactStatus,
) -> Option<PreviewCacheRefreshCandidate> {
    let mut reasons = Vec::new();
    let refresh_proxy = push_refresh_reason(&mut reasons, "proxy", proxy);
    let refresh_thumbnails = push_refresh_reason(&mut reasons, "thumbnails", thumbnails);
    let refresh_waveform = push_refresh_reason(&mut reasons, "waveform", waveform);

    if reasons.is_empty() {
        return None;
    }
    Some(PreviewCacheRefreshCandidate {
        asset_path: asset.to_string_lossy().into_owned(),
        refresh_proxy,
        refresh_thumbnails,
        refresh_waveform,
        reasons,
    })
}

fn refresh_work_from_candidates(
    candidates: &[PreviewCacheRefreshCandidate],
) -> PreviewCacheRefreshWork {
    PreviewCacheRefreshWork {
        asset_count: candidates.len(),
        proxy_count: candidates
            .iter()
            .filter(|candidate| candidate.refresh_proxy)
            .count(),
        thumbnails_count: candidates
            .iter()
            .filter(|candidate| candidate.refresh_thumbnails)
            .count(),
        waveform_count: candidates
            .iter()
            .filter(|candidate| candidate.refresh_waveform)
            .count(),
    }
}

fn refresh_tasks_for_asset(
    asset: &Path,
    proxy: (&Path, PreviewArtifactStatus),
    thumbnails: (&Path, PreviewArtifactStatus),
    waveform: (&Path, PreviewArtifactStatus),
) -> Vec<PreviewCacheRefreshTask> {
    let mut tasks = Vec::new();
    push_refresh_task(&mut tasks, asset, "proxy", proxy.0, proxy.1);
    push_refresh_task(&mut tasks, asset, "thumbnails", thumbnails.0, thumbnails.1);
    push_refresh_task(&mut tasks, asset, "waveform", waveform.0, waveform.1);
    tasks
}

fn push_refresh_task(
    tasks: &mut Vec<PreviewCacheRefreshTask>,
    asset: &Path,
    family: &str,
    artifact_path: &Path,
    status: PreviewArtifactStatus,
) {
    let suffix = match status {
        PreviewArtifactStatus::Missing => "missing",
        PreviewArtifactStatus::Stale => "stale",
        PreviewArtifactStatus::Fresh
        | PreviewArtifactStatus::Ready
        | PreviewArtifactStatus::Empty => {
            return;
        }
    };
    let reason = format!("{family}_{suffix}");
    tasks.push(PreviewCacheRefreshTask {
        task_id: refresh_task_id(asset, family, &reason),
        asset_path: asset.to_string_lossy().into_owned(),
        artifact_kind: family.into(),
        artifact_path: artifact_path.to_string_lossy().into_owned(),
        status,
        estimated_weight: refresh_task_weight(family, status),
        reason,
    });
}

fn refresh_task_id(asset: &Path, family: &str, reason: &str) -> String {
    format!("{}:{}:{}", family, asset.to_string_lossy(), reason)
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

fn push_refresh_reason(
    reasons: &mut Vec<String>,
    family: &str,
    status: PreviewArtifactStatus,
) -> bool {
    let suffix = match status {
        PreviewArtifactStatus::Missing => "missing",
        PreviewArtifactStatus::Stale => "stale",
        PreviewArtifactStatus::Fresh
        | PreviewArtifactStatus::Ready
        | PreviewArtifactStatus::Empty => {
            return false;
        }
    };
    reasons.push(format!("{family}_{suffix}"));
    true
}

fn artifact_is_fresh(asset: &Path, artifact: &Path) -> bool {
    let artifact_meta = match freshest_metadata(artifact) {
        Some(metadata) => metadata,
        None => return false,
    };
    let asset_meta = match std::fs::metadata(asset) {
        Ok(metadata) => metadata,
        Err(_) => return false,
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
        .filter(|c| !c.is_whitespace())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::media::{proxy_path_for, thumbnails_dir_for, waveform_path_for};

    #[test]
    fn preview_cache_summary_counts_ready_and_missing_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir)?;
        let ready_asset = raw_dir.join("ready.mov");
        let missing_asset = raw_dir.join("missing.mov");
        std::fs::write(&ready_asset, b"ready")?;
        std::fs::write(&missing_asset, b"missing")?;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&proxies_dir)?;
        std::fs::write(proxy_path_for(&proxies_dir, &ready_asset), b"proxy")?;

        let thumbnails_dir = thumbnails_dir_for(dir.path(), &ready_asset);
        std::fs::create_dir_all(&thumbnails_dir)?;
        std::fs::write(thumbnails_dir.join("frame-0001.jpg"), b"thumb")?;

        let waveform_path = waveform_path_for(dir.path(), &ready_asset);
        let waveform_parent = waveform_path
            .parent()
            .ok_or("waveform path missing parent")?;
        std::fs::create_dir_all(waveform_parent)?;
        std::fs::write(&waveform_path, br#"{"buckets":[0.1,0.2]}"#)?;

        let summary = build_preview_cache_summary(dir.path())?;

        assert_eq!(summary.asset_count, 2);
        assert_eq!(summary.ready_asset_count, 1);
        assert_eq!(summary.proxy.fresh_count, 1);
        assert_eq!(summary.proxy.missing_count, 1);
        assert_eq!(summary.thumbnails.ready_count, 1);
        assert_eq!(summary.thumbnails.missing_count, 1);
        assert_eq!(summary.waveforms.ready_count, 1);
        assert_eq!(summary.waveforms.missing_count, 1);
        assert_eq!(summary.refresh_work.proxy_count, 1);
        assert_eq!(summary.refresh_work.thumbnails_count, 1);
        assert_eq!(summary.refresh_work.waveform_count, 1);
        assert_eq!(summary.refresh_work.asset_count, 1);
        assert_eq!(summary.refresh_candidates.len(), 1);
        assert_eq!(summary.refresh_tasks.len(), 3);
        assert!(summary.refresh_tasks.iter().any(|task| {
            task.asset_path.ends_with("missing.mov")
                && task.artifact_kind == "proxy"
                && task.task_id.starts_with("proxy:")
                && task.artifact_path.ends_with(".mp4")
                && task.status == PreviewArtifactStatus::Missing
                && task.estimated_weight == 3
                && task.reason == "proxy_missing"
        }));
        let refresh = &summary.refresh_candidates[0];
        assert!(refresh.asset_path.ends_with("missing.mov"));
        assert!(refresh.refresh_proxy);
        assert!(refresh.refresh_thumbnails);
        assert!(refresh.refresh_waveform);
        assert!(
            refresh
                .reasons
                .iter()
                .any(|reason| reason == "proxy_missing")
        );
        assert!(
            refresh
                .reasons
                .iter()
                .any(|reason| reason == "thumbnails_missing")
        );
        assert!(
            refresh
                .reasons
                .iter()
                .any(|reason| reason == "waveform_missing")
        );
        assert!(summary.entries.iter().any(|entry| {
            entry.asset_path.ends_with("ready.mov")
                && entry.proxy == PreviewArtifactStatus::Fresh
                && entry.thumbnails == PreviewArtifactStatus::Ready
                && entry.waveform == PreviewArtifactStatus::Ready
        }));
        Ok(())
    }

    #[test]
    fn preview_cache_summary_lists_stale_refresh_tasks() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir)?;
        let asset = raw_dir.join("stale.mov");
        std::fs::write(&asset, b"old-source")?;

        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&proxies_dir)?;
        let proxy = proxy_path_for(&proxies_dir, &asset);
        std::fs::write(&proxy, b"old-proxy")?;

        let thumbnails_dir = thumbnails_dir_for(dir.path(), &asset);
        std::fs::create_dir_all(&thumbnails_dir)?;
        std::fs::write(thumbnails_dir.join("frame-0001.jpg"), b"old-thumb")?;

        let waveform_path = waveform_path_for(dir.path(), &asset);
        let waveform_parent = waveform_path
            .parent()
            .ok_or("waveform path missing parent")?;
        std::fs::create_dir_all(waveform_parent)?;
        std::fs::write(&waveform_path, br#"{"buckets":[0.1]}"#)?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&asset, b"newer-source")?;

        let summary = build_preview_cache_summary(dir.path())?;

        assert_eq!(summary.ready_asset_count, 0);
        assert_eq!(summary.proxy.stale_count, 1);
        assert_eq!(summary.thumbnails.stale_count, 1);
        assert_eq!(summary.waveforms.stale_count, 1);
        assert_eq!(summary.refresh_work.asset_count, 1);
        assert_eq!(summary.refresh_work.proxy_count, 1);
        assert_eq!(summary.refresh_work.thumbnails_count, 1);
        assert_eq!(summary.refresh_work.waveform_count, 1);
        assert_eq!(summary.refresh_tasks.len(), 3);
        for kind in ["proxy", "thumbnails", "waveform"] {
            assert!(summary.refresh_tasks.iter().any(|task| {
                task.asset_path.ends_with("stale.mov")
                    && task.artifact_kind == kind
                    && task.task_id.starts_with(&format!("{kind}:"))
                    && task.status == PreviewArtifactStatus::Stale
                    && task.estimated_weight > 0
                    && task.reason == format!("{kind}_stale")
            }));
        }
        Ok(())
    }

    #[test]
    fn preview_cache_refresh_plan_limits_and_filters_tasks()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir)?;
        let missing_asset = raw_dir.join("missing.mov");
        let ready_asset = raw_dir.join("ready.mov");
        std::fs::write(&missing_asset, b"missing")?;
        std::fs::write(&ready_asset, b"ready")?;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let proxies_dir = dir.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&proxies_dir)?;
        std::fs::write(proxy_path_for(&proxies_dir, &ready_asset), b"proxy")?;
        let thumbnails_dir = thumbnails_dir_for(dir.path(), &ready_asset);
        std::fs::create_dir_all(&thumbnails_dir)?;
        std::fs::write(thumbnails_dir.join("frame-0001.jpg"), b"thumb")?;
        let waveform_path = waveform_path_for(dir.path(), &ready_asset);
        let waveform_parent = waveform_path
            .parent()
            .ok_or("waveform path missing parent")?;
        std::fs::create_dir_all(waveform_parent)?;
        std::fs::write(&waveform_path, br#"{"buckets":[0.1]}"#)?;

        let plan = build_preview_cache_refresh_plan(
            dir.path(),
            PreviewCacheRefreshOptions {
                asset_id: None,
                proxy: true,
                thumbnails: false,
                waveform: true,
                max_tasks: Some(2),
                dry_run: true,
            },
        )?;

        assert!(plan.dry_run);
        assert_eq!(plan.planned_task_count, 2);
        assert_eq!(plan.skipped_task_count, 1);
        assert_eq!(plan.tasks.len(), 2);
        assert!(plan.tasks.iter().all(|task| {
            task.asset_path.ends_with("missing.mov")
                && (task.artifact_kind == "proxy" || task.artifact_kind == "waveform")
        }));
        assert!(plan.tasks.iter().any(|task| task.artifact_kind == "proxy"));
        assert!(
            plan.tasks
                .iter()
                .any(|task| task.artifact_kind == "waveform")
        );
        Ok(())
    }

    #[test]
    fn preview_cache_refresh_plan_can_filter_by_project_asset_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir)?;
        std::fs::write(raw_dir.join("a.mov"), b"a")?;
        std::fs::write(raw_dir.join("b.mov"), b"b")?;

        let plan = build_preview_cache_refresh_plan(
            dir.path(),
            PreviewCacheRefreshOptions {
                asset_id: Some("raw/b.mov".into()),
                proxy: true,
                thumbnails: true,
                waveform: true,
                max_tasks: None,
                dry_run: true,
            },
        )?;

        assert_eq!(plan.planned_task_count, 3);
        assert_eq!(plan.skipped_task_count, 3);
        assert!(
            plan.tasks
                .iter()
                .all(|task| task.asset_path.ends_with("raw/b.mov"))
        );
        Ok(())
    }
}
