//! `start_indexing` — run the configured indexer pipeline over the
//! project's `raw/` assets. Ported from
//! `crates/core/src/tools/start_indexing.rs` to the in-process MCP
//! server. The original `ctx.job_manager` / event broadcasting hooks
//! are dropped — the agent awaits the dispatcher inline and gets the
//! report when it finishes.

use montage_config::Config;
use montage_index::media_files::collect_raw_media_inputs;
use montage_index::{AssetInput, PairOutcome};
use montage_mcp::ClientInfo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `start_indexing`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct StartIndexingArgs {
    /// Optional indexer-name filter. If non-empty, only indexers whose
    /// `name` matches one of these run. Default: all configured
    /// indexers.
    #[serde(default)]
    pub indexers: Vec<String>,
}

pub async fn run(args: StartIndexingArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project_root = ctx.project_root.clone();
    let config = Config::load(Some(&project_root))
        .map_err(|e| format!("start_indexing: load config: {e}"))?;
    let mut servers: Vec<_> = config.indexers().cloned().collect();
    if servers.is_empty() {
        return Err("start_indexing: no indexers configured. \
             Add `[[mcp.servers]]` entries with kind = \"indexer\" \
             to <project>/.montage/config.toml or ~/.config/montage/config.toml."
            .into());
    }
    if !args.indexers.is_empty() {
        servers.retain(|s| args.indexers.iter().any(|n| n == &s.name));
        if servers.is_empty() {
            return Err(format!(
                "start_indexing: indexers filter {:?} matched none of the \
                 configured indexers. Configured: {}",
                args.indexers,
                config
                    .indexers()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let assets = collect_assets(&project_root)
        .map_err(|e| format!("start_indexing: scan {}/raw: {e}", project_root.display()))?;
    if assets.is_empty() {
        return Err(format!(
            "start_indexing: no assets to index. \
             Drop source files under {}/raw or use the desktop's Import.",
            project_root.display()
        ));
    }

    let client_info = ClientInfo {
        name: "montage-agent".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };

    let concurrency = std::env::var("MONTAGE_INDEX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2);

    let report = montage_index::run(
        &project_root,
        &servers,
        &assets,
        client_info,
        concurrency,
        // We pass `None` for progress — the report at the end is what
        // the model sees. The desktop's separate index_project command
        // emits per-pair progress to its protocol pipe; this tool's
        // caller is the agent loop, not the GUI.
        None,
    )
    .await
    .map_err(|e| format!("start_indexing: dispatcher: {e}"))?;

    Ok(format_report(&report, &servers, &assets))
}

fn format_report(
    report: &montage_index::IndexReport,
    servers: &[montage_config::McpServer],
    assets: &[AssetInput],
) -> String {
    let (skipped, wrote, failed, dep_skipped) = report.counts();
    let mut out = String::new();
    out.push_str(&format!(
        "indexed {} asset(s) with {} indexer(s):\n",
        assets.len(),
        servers.len()
    ));
    out.push_str(&format!(
        "  {wrote} wrote, {skipped} skipped (sha unchanged), \
         {failed} failed, {dep_skipped} blocked-by-dep\n"
    ));
    if report.has_failures() {
        out.push_str("\nfailures:\n");
        for o in &report.outcomes {
            if let PairOutcome::Failed {
                indexer,
                asset,
                message,
                ..
            } = o
            {
                out.push_str(&format!("  ✗ {indexer} · {asset}: {message}\n"));
            }
        }
    }
    out
}

fn collect_assets(project_root: &std::path::Path) -> std::io::Result<Vec<AssetInput>> {
    collect_raw_media_inputs(project_root)
}

pub const DESCRIPTION: &str = "\
Run the configured indexers (whisper transcription, scene detection, \
audio energy, editorial moments, etc) over every media file in the \
project's raw/ dir. Returns the summary report inline once finished. \
The dispatcher is sha-keyed — re-running on already-indexed assets \
is a fast no-op, so it's safe to call any time you suspect sidecars \
might be stale. Pass an optional `indexers` filter (e.g. ['whisper']) \
to re-run only specific producers — useful when iterating on a single \
indexer's tuning. WARNING: indexing a fresh asset can take 20+ minutes \
for hour-long video; only call when the user has asked for an editorial \
operation that needs the sidecars and view_episode shows they're missing. \
Don't proactively re-index already-indexed projects.";
