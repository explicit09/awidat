//! `awidat index` subcommand: load config, build the asset list, dispatch
//! the indexer pool via `awidat-index`, print a summary.
//!
//! Asset discovery: when `--asset` is omitted we walk `<project>/raw/`
//! recursively and treat every regular file as a candidate. Filter by
//! suffix is intentionally NOT done here — the agent's library and the
//! indexer ecosystem decide what's a media file (an image-only indexer
//! will skip a `.wav`, etc.).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use awidat_config::{Config, McpServer};
use awidat_index::{AssetInput, IndexReport, PairOutcome, run as dispatch};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;

pub fn run(
    project_root: &Path,
    explicit_assets: Vec<PathBuf>,
    indexer_filter: Vec<String>,
    concurrency: usize,
) -> Result<()> {
    if !project_root.is_dir() {
        return Err(anyhow!(
            "project root '{}' is not a directory",
            project_root.display()
        ));
    }
    let config = Config::load(Some(project_root))
        .with_context(|| "failed to load awidat config (global and/or project)")?;

    let mut servers: Vec<McpServer> = config.indexers().cloned().collect();
    if servers.is_empty() {
        return Err(anyhow!(
            "no indexers configured. Add `[[mcp.servers]]` entries with kind = \"indexer\" \
             to your `<project>/.awidat/config.toml` or `~/.config/awidat/config.toml`."
        ));
    }
    if !indexer_filter.is_empty() {
        servers.retain(|s| indexer_filter.iter().any(|n| n == &s.name));
        if servers.is_empty() {
            return Err(anyhow!(
                "no configured indexers match --indexer filter {indexer_filter:?}"
            ));
        }
    }

    let assets = collect_assets(project_root, &explicit_assets)?;
    if assets.is_empty() {
        let raw = project_root.join("raw");
        return Err(anyhow!(
            "no assets to index. Drop source files under '{}' or pass --asset.",
            raw.display()
        ));
    }

    println!(
        "Indexing {} asset(s) with {} indexer(s):",
        assets.len(),
        servers.len()
    );
    for s in &servers {
        println!("  - {}", s.name);
    }
    for a in &assets {
        println!("  · {}", a.id);
    }
    println!();

    let client_info = ClientInfo {
        name: "awidat".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let report: IndexReport = runtime
        .block_on(dispatch(
            project_root,
            &servers,
            &assets,
            client_info,
            concurrency,
        ))
        .context("indexer dispatcher failed")?;

    print_report(&report);
    if report.has_failures() {
        Err(anyhow!(
            "{} indexer pair(s) failed; see lines above",
            report.failures().count()
        ))
    } else {
        Ok(())
    }
}

fn collect_assets(
    project_root: &Path,
    explicit: &[PathBuf],
) -> Result<Vec<AssetInput>> {
    let mut out = Vec::new();
    if !explicit.is_empty() {
        for p in explicit {
            let abs = if p.is_absolute() {
                p.clone()
            } else {
                project_root.join(p)
            };
            let id = derive_asset_id(project_root, &abs);
            out.push(AssetInput {
                id: AssetId::new(id),
                path: abs,
            });
        }
        return Ok(out);
    }

    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return Ok(out);
    }
    walk_assets(project_root, &raw_dir, &mut out)?;
    Ok(out)
}

fn walk_assets(
    project_root: &Path,
    dir: &Path,
    out: &mut Vec<AssetInput>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_assets(project_root, &path, out)?;
        } else if path.is_file() {
            let id = derive_asset_id(project_root, &path);
            out.push(AssetInput {
                id: AssetId::new(id),
                path,
            });
        }
    }
    Ok(())
}

fn derive_asset_id(project_root: &Path, abs: &Path) -> String {
    abs.strip_prefix(project_root)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/")
}

fn print_report(report: &IndexReport) {
    let (skipped, wrote, failed) = report.counts();
    println!("Index summary: {wrote} wrote, {skipped} skipped, {failed} failed");
    for outcome in &report.outcomes {
        match outcome {
            PairOutcome::Wrote { indexer, asset, path } => {
                println!("  ✓ {indexer:>14}  {asset}  →  {}", path.display());
            }
            PairOutcome::Skipped { indexer, asset } => {
                println!("  · {indexer:>14}  {asset}  (sha unchanged)");
            }
            PairOutcome::Failed {
                indexer,
                asset,
                message,
            } => {
                println!("  ✗ {indexer:>14}  {asset}  FAILED: {message}");
            }
        }
    }
}
