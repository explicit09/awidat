//! Agent-visible project preview-cache readiness.
//!
//! This mirrors the desktop preview cache contract without depending
//! on Tauri code: proxies, filmstrip thumbnails, and waveform sidecars
//! are summarized from disk so agents can plan refresh work before
//! expensive preview or editorial operations.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use serde::{Deserialize, Serialize};

use crate::proxy::{proxy_is_fresh, proxy_path_for};

/// Preview-cache artifact state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

// ---------------------------------------------------------------------------
// Per-task lifecycle state
// ---------------------------------------------------------------------------

/// Per-task lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PreviewCacheRefreshTaskStatus {
    /// Task has not yet started.
    #[default]
    Pending,
    /// Task is currently executing.
    InProgress,
    /// Task finished successfully.
    Completed,
    /// Task finished with an error.
    Failed,
    /// Task was intentionally skipped.
    Skipped,
}

/// Per-task lifecycle state, persisted alongside the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewCacheRefreshTaskState {
    /// Stable task identifier.
    pub task_id: String,
    /// Artifact family: proxy, thumbnails, or waveform.
    pub artifact_kind: String,
    /// Project-relative source asset id.
    pub asset_id: String,
    /// Absolute artifact path.
    pub artifact_path: String,
    /// Current execution status of this task.
    pub status: PreviewCacheRefreshTaskStatus,
    /// When this task started executing (ms since epoch).
    pub started_at_ms: Option<u64>,
    /// When this task finished executing (ms since epoch).
    pub finished_at_ms: Option<u64>,
    /// Error message if the task failed.
    pub error_message: Option<String>,
}

/// Durable preview-cache refresh lifecycle record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewCacheRefreshLifecycle {
    /// Absolute lifecycle artifact path.
    pub path: String,
    /// Current lifecycle status (aggregate).
    pub status: String,
    /// Artifact policy for the persisted plan.
    pub artifact_policy: String,
    /// Number of selected tasks.
    pub selected_task_count: usize,
    /// Aggregate work for selected tasks.
    pub selected_refresh_work: PreviewCacheRefreshWork,
    /// Stable selected task ids in execution order.
    pub selected_task_ids: Vec<String>,
    /// Per-task states.
    #[serde(default)]
    pub tasks: Vec<PreviewCacheRefreshTaskState>,
    /// When this lifecycle run started (ms since epoch).
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    /// When this lifecycle run finished (ms since epoch).
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the preview-cache refresh driver.
#[derive(Debug, thiserror::Error)]
pub enum PreviewRefreshError {
    /// The executor returned an error for the given task.
    #[error("executor failed for task {task_id}: {message}")]
    Executor {
        /// The task that failed.
        task_id: String,
        /// Human-readable failure message.
        message: String,
    },
    /// An I/O error occurred while reading or persisting the lifecycle.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The lifecycle artifact exists but cannot be deserialized.
    #[error("lifecycle artifact corrupt: {0}")]
    Corrupt(String),
    /// A lifecycle run is already in progress and was started recently.
    #[error("lifecycle already running; started_at_ms={started_at_ms}")]
    Busy {
        /// The timestamp (ms since epoch) when the running lifecycle started.
        started_at_ms: u64,
    },
}

// ---------------------------------------------------------------------------
// Executor trait
// ---------------------------------------------------------------------------

/// Drives the execution of a single preview-cache refresh task.
#[async_trait::async_trait]
pub trait PreviewRefreshExecutor: Send + Sync {
    /// Execute the given refresh task, returning `Ok(())` on success or a
    /// `PreviewRefreshError` on failure.
    async fn execute(&self, task: &PreviewCacheRefreshTask) -> Result<(), PreviewRefreshError>;
}

// ---------------------------------------------------------------------------
// Per-asset preview-cache state
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

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
///
/// Existing artifacts at the same path are overwritten. For resume semantics,
/// use `run_preview_cache_refresh` instead.
pub fn write_preview_cache_refresh_lifecycle(
    project_root: &Path,
    selection: &PreviewCacheRefreshSelection,
) -> std::io::Result<PreviewCacheRefreshLifecycle> {
    let path = preview_cache_refresh_lifecycle_path(project_root);
    let tasks = tasks_from_selection(selection);
    let lifecycle = PreviewCacheRefreshLifecycle {
        path: path.to_string_lossy().into_owned(),
        status: derive_aggregate_status(&tasks),
        artifact_policy: "no_render_job_started".into(),
        selected_task_count: selection.selected_task_count,
        selected_refresh_work: selection.selected_refresh_work.clone(),
        selected_task_ids: selection
            .selected_refresh_tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect(),
        tasks,
        started_at_ms: None,
        finished_at_ms: None,
    };
    persist_lifecycle_io(&lifecycle)?;
    Ok(lifecycle)
}

/// Load an existing lifecycle artifact if present.
///
/// Returns `Ok(None)` if the file does not exist.
/// Returns `Err(PreviewRefreshError::Corrupt)` if the file exists but cannot be parsed.
pub fn read_preview_cache_refresh_lifecycle(
    project_root: &Path,
) -> Result<Option<PreviewCacheRefreshLifecycle>, PreviewRefreshError> {
    let path = preview_cache_refresh_lifecycle_path(project_root);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    serde_json::from_slice::<PreviewCacheRefreshLifecycle>(&bytes)
        .map(Some)
        .map_err(|err| PreviewRefreshError::Corrupt(err.to_string()))
}

/// Run the refresh against an executor with resume semantics.
///
/// 1. Load existing lifecycle if present; otherwise build one from `selection`.
/// 2. If the loaded lifecycle status is "in_progress" and started_at_ms is within
///    the last 5 minutes, return `PreviewRefreshError::Busy`.
/// 3. Iterate tasks in order. For each Pending or InProgress task:
///    - Mark InProgress, persist, call `executor.execute`.
///    - On Ok: mark Completed, persist.
///    - On Err: mark Failed with error_message, persist. Continue iterating.
/// 4. After the loop, set finished_at_ms if all tasks are terminal, recompute
///    aggregate status, persist atomically. Return the final lifecycle.
pub async fn run_preview_cache_refresh(
    project_root: &Path,
    selection: &PreviewCacheRefreshSelection,
    executor: &dyn PreviewRefreshExecutor,
) -> Result<PreviewCacheRefreshLifecycle, PreviewRefreshError> {
    // --- 1. Load or build lifecycle ---
    let mut lifecycle = match read_preview_cache_refresh_lifecycle(project_root)? {
        Some(existing) => merge_lifecycle_with_selection(existing, selection, project_root),
        None => {
            let tasks = tasks_from_selection(selection);
            let path = preview_cache_refresh_lifecycle_path(project_root);
            PreviewCacheRefreshLifecycle {
                path: path.to_string_lossy().into_owned(),
                status: derive_aggregate_status(&tasks),
                artifact_policy: "no_render_job_started".into(),
                selected_task_count: selection.selected_task_count,
                selected_refresh_work: selection.selected_refresh_work.clone(),
                selected_task_ids: selection
                    .selected_refresh_tasks
                    .iter()
                    .map(|task| task.task_id.clone())
                    .collect(),
                tasks,
                started_at_ms: None,
                finished_at_ms: None,
            }
        }
    };

    // --- 2. Busy guard ---
    if lifecycle.status == "in_progress" {
        if let Some(started_at_ms) = lifecycle.started_at_ms {
            let now_ms = now_ms();
            let five_min_ms: u64 = 5 * 60 * 1_000;
            if now_ms.saturating_sub(started_at_ms) < five_min_ms {
                return Err(PreviewRefreshError::Busy { started_at_ms });
            }
        }
    }

    // Build a lookup map from task_id -> PreviewCacheRefreshTask for the executor.
    let task_lookup: std::collections::HashMap<String, &PreviewCacheRefreshTask> = selection
        .selected_refresh_tasks
        .iter()
        .map(|t| (t.task_id.clone(), t))
        .collect();

    // --- 3. Iterate tasks ---
    for i in 0..lifecycle.tasks.len() {
        let status = lifecycle.tasks[i].status.clone();
        if !matches!(
            status,
            PreviewCacheRefreshTaskStatus::Pending | PreviewCacheRefreshTaskStatus::InProgress
        ) {
            continue;
        }

        let task_id = lifecycle.tasks[i].task_id.clone();

        // Mark InProgress.
        let run_start_ms = now_ms();
        lifecycle.tasks[i].status = PreviewCacheRefreshTaskStatus::InProgress;
        lifecycle.tasks[i].started_at_ms = Some(run_start_ms);
        if lifecycle.started_at_ms.is_none() {
            lifecycle.started_at_ms = Some(run_start_ms);
        }
        lifecycle.status = derive_aggregate_status(&lifecycle.tasks);
        persist_lifecycle_io(&lifecycle)?;

        // Resolve the work definition from the selection.
        let result = match task_lookup.get(&task_id) {
            Some(work_task) => executor.execute(work_task).await,
            None => {
                // Task not in current selection — skip it gracefully.
                Err(PreviewRefreshError::Executor {
                    task_id: task_id.clone(),
                    message: "task not found in current selection".into(),
                })
            }
        };

        let finish_ms = now_ms();
        match result {
            Ok(()) => {
                lifecycle.tasks[i].status = PreviewCacheRefreshTaskStatus::Completed;
                lifecycle.tasks[i].finished_at_ms = Some(finish_ms);
                lifecycle.tasks[i].error_message = None;
            }
            Err(err) => {
                lifecycle.tasks[i].status = PreviewCacheRefreshTaskStatus::Failed;
                lifecycle.tasks[i].finished_at_ms = Some(finish_ms);
                lifecycle.tasks[i].error_message = Some(err.to_string());
            }
        }
        lifecycle.status = derive_aggregate_status(&lifecycle.tasks);
        persist_lifecycle_io(&lifecycle)?;
    }

    // --- 4. Finalize ---
    let all_terminal = lifecycle.tasks.iter().all(|t| {
        matches!(
            t.status,
            PreviewCacheRefreshTaskStatus::Completed
                | PreviewCacheRefreshTaskStatus::Failed
                | PreviewCacheRefreshTaskStatus::Skipped
        )
    });
    if all_terminal {
        lifecycle.finished_at_ms = Some(now_ms());
    }
    lifecycle.status = derive_aggregate_status(&lifecycle.tasks);
    persist_lifecycle_io(&lifecycle)?;

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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return current time as milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Derive the aggregate lifecycle status string from the current task states.
///
/// Rules (in priority order):
/// - `"planned"`     — all tasks are Pending (nothing started)
/// - `"in_progress"` — at least one task is InProgress
/// - `"completed"`   — no Pending/InProgress/Failed tasks remain
/// - `"failed"`      — no Pending or InProgress tasks, at least one Failed
/// - `"partial"`     — no InProgress tasks, some Pending remain alongside other statuses
fn derive_aggregate_status(tasks: &[PreviewCacheRefreshTaskState]) -> String {
    if tasks.is_empty() {
        return "planned".into();
    }

    let has_in_progress = tasks
        .iter()
        .any(|t| t.status == PreviewCacheRefreshTaskStatus::InProgress);
    if has_in_progress {
        return "in_progress".into();
    }

    let has_pending = tasks
        .iter()
        .any(|t| t.status == PreviewCacheRefreshTaskStatus::Pending);
    let has_failed = tasks
        .iter()
        .any(|t| t.status == PreviewCacheRefreshTaskStatus::Failed);

    // All pending and nothing started yet.
    if has_pending && !has_failed {
        let has_completed_or_skipped = tasks.iter().any(|t| {
            matches!(
                t.status,
                PreviewCacheRefreshTaskStatus::Completed | PreviewCacheRefreshTaskStatus::Skipped
            )
        });
        if !has_completed_or_skipped {
            return "planned".into();
        }
        // Some pending + some completed/skipped = partial.
        return "partial".into();
    }

    // Pending tasks remain alongside failures — resumable.
    if has_pending && has_failed {
        return "partial".into();
    }

    // No pending, no in_progress.
    if !has_failed {
        return "completed".into();
    }

    // No pending, no in_progress, at least one failed.
    "failed".into()
}

/// Build per-task states from a selection (all Pending).
fn tasks_from_selection(
    selection: &PreviewCacheRefreshSelection,
) -> Vec<PreviewCacheRefreshTaskState> {
    selection
        .selected_refresh_tasks
        .iter()
        .map(|task| PreviewCacheRefreshTaskState {
            task_id: task.task_id.clone(),
            artifact_kind: task.artifact_kind.clone(),
            asset_id: task.asset_id.clone(),
            artifact_path: task.artifact_path.clone(),
            status: PreviewCacheRefreshTaskStatus::Pending,
            started_at_ms: None,
            finished_at_ms: None,
            error_message: None,
        })
        .collect()
}

/// Merge an existing lifecycle with the current selection, preserving completed
/// task states and appending any new tasks as Pending.
fn merge_lifecycle_with_selection(
    mut existing: PreviewCacheRefreshLifecycle,
    selection: &PreviewCacheRefreshSelection,
    project_root: &Path,
) -> PreviewCacheRefreshLifecycle {
    // Index existing task states by task_id.
    let existing_ids: std::collections::HashSet<String> =
        existing.tasks.iter().map(|t| t.task_id.clone()).collect();

    // Append tasks from the selection that are not already in the lifecycle.
    for task in &selection.selected_refresh_tasks {
        if !existing_ids.contains(&task.task_id) {
            existing.tasks.push(PreviewCacheRefreshTaskState {
                task_id: task.task_id.clone(),
                artifact_kind: task.artifact_kind.clone(),
                asset_id: task.asset_id.clone(),
                artifact_path: task.artifact_path.clone(),
                status: PreviewCacheRefreshTaskStatus::Pending,
                started_at_ms: None,
                finished_at_ms: None,
                error_message: None,
            });
        }
    }

    // Refresh aggregate fields to reflect the current selection.
    existing.selected_task_count = selection.selected_task_count;
    existing.selected_refresh_work = selection.selected_refresh_work.clone();
    existing.selected_task_ids = selection
        .selected_refresh_tasks
        .iter()
        .map(|t| t.task_id.clone())
        .collect();
    existing.path = preview_cache_refresh_lifecycle_path(project_root)
        .to_string_lossy()
        .into_owned();
    existing.status = derive_aggregate_status(&existing.tasks);
    existing
}

/// Atomically persist a lifecycle artifact: write to `.tmp`, then rename.
fn persist_lifecycle_io(lifecycle: &PreviewCacheRefreshLifecycle) -> std::io::Result<()> {
    let path = PathBuf::from(&lifecycle.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(lifecycle)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(&body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // -----------------------------------------------------------------------
    // Test executor
    // -----------------------------------------------------------------------

    struct RecordingExecutor {
        outcomes: Mutex<HashMap<String, Result<(), &'static str>>>,
        calls: Mutex<Vec<String>>,
    }

    impl RecordingExecutor {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                outcomes: Mutex::new(HashMap::new()),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn set_outcome(&self, task_id: &str, outcome: Result<(), &'static str>) {
            self.outcomes
                .lock()
                .unwrap()
                .insert(task_id.to_owned(), outcome);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl PreviewRefreshExecutor for RecordingExecutor {
        async fn execute(&self, task: &PreviewCacheRefreshTask) -> Result<(), PreviewRefreshError> {
            self.calls.lock().unwrap().push(task.task_id.clone());
            match self
                .outcomes
                .lock()
                .unwrap()
                .get(&task.task_id)
                .copied()
                .unwrap_or(Ok(()))
            {
                Ok(()) => Ok(()),
                Err(msg) => Err(PreviewRefreshError::Executor {
                    task_id: task.task_id.clone(),
                    message: msg.into(),
                }),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a synthetic `PreviewCacheRefreshSelection` with N tasks.
    fn make_selection_with_tasks(task_ids: &[&str]) -> PreviewCacheRefreshSelection {
        let tasks: Vec<PreviewCacheRefreshTask> = task_ids
            .iter()
            .map(|id| PreviewCacheRefreshTask {
                task_id: id.to_string(),
                asset_id: format!("asset_{id}"),
                artifact_path: format!("/tmp/artifact_{id}"),
                artifact_kind: "proxy".into(),
                status: PreviewArtifactStatus::Missing,
                estimated_weight: 3,
                reason: "proxy_missing".into(),
            })
            .collect();
        let work = refresh_work_from_tasks(&tasks);
        let count = tasks.len();
        PreviewCacheRefreshSelection {
            selected_refresh_work: work,
            selected_task_count: count,
            skipped_task_count: 0,
            selected_refresh_tasks: tasks,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: all tasks succeed → all Completed, aggregate "completed"
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_refresh_completes_when_all_tasks_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let executor = RecordingExecutor::new();
        let selection = make_selection_with_tasks(&["t1", "t2"]);

        let lifecycle = run_preview_cache_refresh(dir.path(), &selection, executor.as_ref())
            .await
            .unwrap();

        assert_eq!(lifecycle.status, "completed");
        assert_eq!(lifecycle.tasks.len(), 2);
        for task in &lifecycle.tasks {
            assert_eq!(task.status, PreviewCacheRefreshTaskStatus::Completed);
            assert!(task.started_at_ms.is_some());
            assert!(task.finished_at_ms.is_some());
            assert!(task.error_message.is_none());
        }
        assert!(lifecycle.finished_at_ms.is_some());

        // Verify artifact was persisted.
        let persisted = read_preview_cache_refresh_lifecycle(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, "completed");
        assert_eq!(executor.calls(), vec!["t1", "t2"]);
    }

    // -----------------------------------------------------------------------
    // Test 2: second task fails → continues, aggregate "failed"
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_refresh_isolates_failures_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let executor = RecordingExecutor::new();
        executor.set_outcome("t2", Err("boom"));
        let selection = make_selection_with_tasks(&["t1", "t2"]);

        let lifecycle = run_preview_cache_refresh(dir.path(), &selection, executor.as_ref())
            .await
            .unwrap();

        assert_eq!(lifecycle.status, "failed");

        let t1 = lifecycle.tasks.iter().find(|t| t.task_id == "t1").unwrap();
        assert_eq!(t1.status, PreviewCacheRefreshTaskStatus::Completed);
        assert!(t1.error_message.is_none());

        let t2 = lifecycle.tasks.iter().find(|t| t.task_id == "t2").unwrap();
        assert_eq!(t2.status, PreviewCacheRefreshTaskStatus::Failed);
        assert!(t2.error_message.as_deref().unwrap().contains("boom"));
        assert!(lifecycle.finished_at_ms.is_some());

        // Both were called.
        assert_eq!(executor.calls(), vec!["t1", "t2"]);
    }

    // -----------------------------------------------------------------------
    // Test 3: resume skips already-Completed tasks
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_refresh_resumes_skipping_completed_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let selection = make_selection_with_tasks(&["t1", "t2"]);

        // Pre-write a lifecycle where t1 is already Completed.
        let path = preview_cache_refresh_lifecycle_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original_started: u64 = 1_000_000;
        let original_finished: u64 = 1_000_500;
        let pre_lifecycle = PreviewCacheRefreshLifecycle {
            path: path.to_string_lossy().into_owned(),
            status: "partial".into(),
            artifact_policy: "no_render_job_started".into(),
            selected_task_count: 2,
            selected_refresh_work: selection.selected_refresh_work.clone(),
            selected_task_ids: vec!["t1".into(), "t2".into()],
            tasks: vec![
                PreviewCacheRefreshTaskState {
                    task_id: "t1".into(),
                    artifact_kind: "proxy".into(),
                    asset_id: "asset_t1".into(),
                    artifact_path: "/tmp/artifact_t1".into(),
                    status: PreviewCacheRefreshTaskStatus::Completed,
                    started_at_ms: Some(original_started),
                    finished_at_ms: Some(original_finished),
                    error_message: None,
                },
                PreviewCacheRefreshTaskState {
                    task_id: "t2".into(),
                    artifact_kind: "proxy".into(),
                    asset_id: "asset_t2".into(),
                    artifact_path: "/tmp/artifact_t2".into(),
                    status: PreviewCacheRefreshTaskStatus::Pending,
                    started_at_ms: None,
                    finished_at_ms: None,
                    error_message: None,
                },
            ],
            started_at_ms: None,
            finished_at_ms: None,
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&pre_lifecycle).unwrap()).unwrap();

        let executor = RecordingExecutor::new();
        let lifecycle = run_preview_cache_refresh(dir.path(), &selection, executor.as_ref())
            .await
            .unwrap();

        // Only t2 should have been dispatched.
        assert_eq!(executor.calls(), vec!["t2"]);
        assert_eq!(lifecycle.status, "completed");

        // t1 timestamps must be the originals.
        let t1 = lifecycle.tasks.iter().find(|t| t.task_id == "t1").unwrap();
        assert_eq!(t1.status, PreviewCacheRefreshTaskStatus::Completed);
        assert_eq!(t1.started_at_ms, Some(original_started));
        assert_eq!(t1.finished_at_ms, Some(original_finished));
    }

    // -----------------------------------------------------------------------
    // Test 4: new tasks in selection are appended
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_refresh_appends_new_tasks_from_selection() {
        let dir = tempfile::tempdir().unwrap();
        let selection = make_selection_with_tasks(&["task_A", "task_B"]);

        // Pre-write a lifecycle with only task_A Completed.
        let path = preview_cache_refresh_lifecycle_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original_started: u64 = 2_000_000;
        let original_finished: u64 = 2_000_900;
        let pre_lifecycle = PreviewCacheRefreshLifecycle {
            path: path.to_string_lossy().into_owned(),
            status: "completed".into(),
            artifact_policy: "no_render_job_started".into(),
            selected_task_count: 1,
            selected_refresh_work: PreviewCacheRefreshWork {
                asset_count: 1,
                proxy_count: 1,
                thumbnails_count: 0,
                waveform_count: 0,
            },
            selected_task_ids: vec!["task_A".into()],
            tasks: vec![PreviewCacheRefreshTaskState {
                task_id: "task_A".into(),
                artifact_kind: "proxy".into(),
                asset_id: "asset_task_A".into(),
                artifact_path: "/tmp/artifact_task_A".into(),
                status: PreviewCacheRefreshTaskStatus::Completed,
                started_at_ms: Some(original_started),
                finished_at_ms: Some(original_finished),
                error_message: None,
            }],
            started_at_ms: Some(original_started),
            finished_at_ms: Some(original_finished),
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&pre_lifecycle).unwrap()).unwrap();

        let executor = RecordingExecutor::new();
        let lifecycle = run_preview_cache_refresh(dir.path(), &selection, executor.as_ref())
            .await
            .unwrap();

        // Both tasks in the final lifecycle.
        assert_eq!(lifecycle.tasks.len(), 2);
        assert_eq!(lifecycle.status, "completed");

        // task_A preserved with original timestamps.
        let a = lifecycle
            .tasks
            .iter()
            .find(|t| t.task_id == "task_A")
            .unwrap();
        assert_eq!(a.status, PreviewCacheRefreshTaskStatus::Completed);
        assert_eq!(a.started_at_ms, Some(original_started));
        assert_eq!(a.finished_at_ms, Some(original_finished));

        // task_B was dispatched and completed with new timestamps.
        let b = lifecycle
            .tasks
            .iter()
            .find(|t| t.task_id == "task_B")
            .unwrap();
        assert_eq!(b.status, PreviewCacheRefreshTaskStatus::Completed);
        assert!(b.started_at_ms.unwrap() > original_finished);

        // Only task_B was called.
        assert_eq!(executor.calls(), vec!["task_B"]);
    }

    // -----------------------------------------------------------------------
    // Test 5: Busy guard fires when in_progress within 5 minutes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_refresh_rejects_busy_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let selection = make_selection_with_tasks(&["t1"]);

        // Pre-write a lifecycle that is "in_progress" with started_at_ms = now - 60s.
        let path = preview_cache_refresh_lifecycle_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let recent_start_ms = now_ms() - 60_000; // 60 seconds ago
        let pre_lifecycle = PreviewCacheRefreshLifecycle {
            path: path.to_string_lossy().into_owned(),
            status: "in_progress".into(),
            artifact_policy: "no_render_job_started".into(),
            selected_task_count: 1,
            selected_refresh_work: selection.selected_refresh_work.clone(),
            selected_task_ids: vec!["t1".into()],
            tasks: vec![PreviewCacheRefreshTaskState {
                task_id: "t1".into(),
                artifact_kind: "proxy".into(),
                asset_id: "asset_t1".into(),
                artifact_path: "/tmp/artifact_t1".into(),
                status: PreviewCacheRefreshTaskStatus::InProgress,
                started_at_ms: Some(recent_start_ms),
                finished_at_ms: None,
                error_message: None,
            }],
            started_at_ms: Some(recent_start_ms),
            finished_at_ms: None,
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&pre_lifecycle).unwrap()).unwrap();

        let executor = RecordingExecutor::new();
        let result = run_preview_cache_refresh(dir.path(), &selection, executor.as_ref()).await;

        assert!(matches!(result, Err(PreviewRefreshError::Busy { .. })));
        // Executor was never called.
        assert!(executor.calls().is_empty());
    }

    // -----------------------------------------------------------------------
    // Existing test (preserved)
    // -----------------------------------------------------------------------

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
