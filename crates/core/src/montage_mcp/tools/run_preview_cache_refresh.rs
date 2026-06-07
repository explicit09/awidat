//! `run_preview_cache_refresh` — execute the persisted preview-cache
//! refresh lifecycle using the ffmpeg-backed executor. Ported from
//! `crates/core/src/tools/run_preview_cache_refresh.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `run_preview_cache_refresh`. Mirrors
/// `crate::preview_cache::PreviewCacheSelectionOptions`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RunPreviewCacheRefreshArgs {
    /// Optional project-relative asset id to select, such as raw/a.mov.
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
    /// Optional maximum number of selected refresh tasks per invocation.
    #[serde(default)]
    pub max_tasks: Option<usize>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct RunPreviewCacheRefreshResponse {
    lifecycle: crate::preview_cache::PreviewCacheRefreshLifecycle,
    /// True when the executor was invoked. False when no tasks were
    /// selected (the lifecycle is reported but the caller can decide
    /// whether to retry).
    executed: bool,
}

/// Run `run_preview_cache_refresh`. Returns a JSON body as `Ok(String)`.
pub async fn run(args: RunPreviewCacheRefreshArgs, ctx: McpToolCtx) -> Result<String, String> {
    let options = crate::preview_cache::PreviewCacheSelectionOptions {
        asset_id: args.asset_id,
        proxy: args.proxy,
        thumbnails: args.thumbnails,
        waveform: args.waveform,
        max_tasks: args.max_tasks,
        persist_refresh_plan: false,
    };
    let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
        .map_err(|e| format!("run_preview_cache_refresh: unable to scan preview cache: {e}"))?;
    let selection = crate::preview_cache::select_preview_cache_refresh_tasks(&summary, &options);

    if selection.selected_task_count == 0 {
        let lifecycle = crate::preview_cache::write_preview_cache_refresh_lifecycle(
            &ctx.project_root,
            &selection,
        )
        .map_err(|e| format!("run_preview_cache_refresh: write empty lifecycle: {e}"))?;
        return serde_json::to_string(&RunPreviewCacheRefreshResponse {
            lifecycle,
            executed: false,
        })
        .map_err(|e| format!("run_preview_cache_refresh: serialize lifecycle: {e}"));
    }

    let executor = crate::preview_refresh_executor::FfmpegPreviewRefreshExecutor::new(
        ctx.project_root.clone(),
    );
    let lifecycle =
        crate::preview_cache::run_preview_cache_refresh(&ctx.project_root, &selection, &executor)
            .await
            .map_err(|e| format!("run_preview_cache_refresh: {e}"))?;

    serde_json::to_string(&RunPreviewCacheRefreshResponse {
        lifecycle,
        executed: true,
    })
    .map_err(|e| format!("run_preview_cache_refresh: serialize lifecycle: {e}"))
}

pub const DESCRIPTION: &str = "Run the persisted preview-cache refresh lifecycle to completion using the ffmpeg-backed executor. Resumes from any prior pending/in-progress tasks and skips already-completed tasks.";
