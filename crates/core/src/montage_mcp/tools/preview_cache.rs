//! `preview_cache` — report project preview-cache readiness and a
//! bounded read-only refresh plan across proxies, thumbnails, and
//! waveform sidecars. Ported from
//! `crates/core/src/tools/preview_cache.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `preview_cache_status`. Mirrors
/// `crate::preview_cache::PreviewCacheSelectionOptions`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PreviewCacheArgs {
    /// Optional project-relative asset id to select, such as raw/a.mov.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Include proxy refresh tasks in the selected plan.
    #[serde(default = "default_true")]
    pub proxy: bool,
    /// Include thumbnail refresh tasks in the selected plan.
    #[serde(default = "default_true")]
    pub thumbnails: bool,
    /// Include waveform refresh tasks in the selected plan.
    #[serde(default = "default_true")]
    pub waveform: bool,
    /// Optional maximum number of selected refresh tasks.
    #[serde(default)]
    pub max_tasks: Option<usize>,
    /// Persist the selected refresh plan to
    /// `.montage/preview-cache/refresh-plan.json` without generating media.
    #[serde(default)]
    pub persist_refresh_plan: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct PreviewCacheStatusResponse {
    #[serde(flatten)]
    summary: crate::preview_cache::PreviewCacheSummary,
    #[serde(flatten)]
    selection: crate::preview_cache::PreviewCacheRefreshSelection,
    refresh_execution: crate::preview_cache::PreviewCacheRefreshExecutionContract,
    refresh_lifecycle: Option<crate::preview_cache::PreviewCacheRefreshLifecycle>,
}

/// Run `preview_cache_status`. Returns the JSON body as `Ok(String)`.
pub fn run(args: PreviewCacheArgs, ctx: McpToolCtx) -> Result<String, String> {
    let options = crate::preview_cache::PreviewCacheSelectionOptions {
        asset_id: args.asset_id,
        proxy: args.proxy,
        thumbnails: args.thumbnails,
        waveform: args.waveform,
        max_tasks: args.max_tasks,
        persist_refresh_plan: args.persist_refresh_plan,
    };
    let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
        .map_err(|e| format!("preview_cache_status: unable to scan preview cache: {e}"))?;
    let selection = crate::preview_cache::select_preview_cache_refresh_tasks(&summary, &options);
    let refresh_execution =
        crate::preview_cache::preview_cache_refresh_execution_contract(&selection);
    let refresh_lifecycle = if options.persist_refresh_plan {
        Some(
            crate::preview_cache::write_preview_cache_refresh_lifecycle(
                &ctx.project_root,
                &selection,
            )
            .map_err(|e| format!("preview_cache_status: write refresh lifecycle: {e}"))?,
        )
    } else {
        None
    };
    let response = PreviewCacheStatusResponse {
        summary,
        selection,
        refresh_execution,
        refresh_lifecycle,
    };
    serde_json::to_string(&response)
        .map_err(|e| format!("preview_cache_status: serialize summary: {e}"))
}

pub const DESCRIPTION: &str = "Report project preview-cache readiness and a bounded read-only refresh plan across proxies, thumbnails, and waveform sidecars.";
