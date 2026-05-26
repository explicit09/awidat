//! `list_assets` — paginated listing of project assets.
//! Ported in step 5 from `crates/core/src/tools/list_assets.rs`.
//!
//! 1-indexed offset, default limit 25, hard cap 100, per-entry
//! truncation at 500B, "More than N entries found" footer when
//! truncated. Scope: `raw` (source media), `renders` (engine outputs),
//! or `all` (both).
//!
//! Optional bin filter: when `bin` is provided, the result is restricted
//! to filesystem-discovered assets whose `AssetRecord` in the project
//! catalog has a matching `bin_id`. A bin id of the synthetic form
//! `role:<role>` (e.g. `role:audio`) filters on the asset's role rather
//! than a user-defined bin — the same virtual buckets surfaced by
//! `list_bins`. Unknown bin ids return zero results (not an error).

use std::path::Path;

use awidat_index::media_files::{MediaFile, MediaScanOptions, collect_project_media_files};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::awidat_mcp::tools::list_bins::{ROLE_BIN_PREFIX, role_id_for_record};

/// Default limit. Codex `list_dir.rs:31-33`.
const DEFAULT_LIMIT: usize = 25;
/// Hard cap; queries asking for more get clamped + a footer.
const MAX_LIMIT: usize = 100;
/// Per-entry text cap. Codex `list_dir.rs:24` `MAX_ENTRY_LENGTH`.
const MAX_ENTRY_LENGTH: usize = 500;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListAssetsArgs {
    /// `raw` | `renders` | `all`. Default `all`.
    #[serde(default)]
    pub scope: Option<String>,
    /// 1-indexed first entry to return. Default 1.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Max entries (default 25, hard cap 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional bin id filter. User-defined bin id from the project
    /// catalog, or the synthetic form `role:<role>` for a built-in role
    /// bucket (see `list_bins`).
    #[serde(default)]
    pub bin: Option<String>,
}

pub fn run(args: ListAssetsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let scope = args.scope.as_deref().unwrap_or("all").to_string();
    let offset = args.offset.unwrap_or(1);
    if offset == 0 {
        return Err("list_assets: offset must be a 1-indexed entry number (≥ 1)".into());
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

    let entries = match scope.as_str() {
        "raw" => collect(
            &ctx.project_root,
            MediaScanOptions {
                include_raw: true,
                include_renders: false,
                max_files: None,
            },
        ),
        "renders" => collect(
            &ctx.project_root,
            MediaScanOptions {
                include_raw: false,
                include_renders: true,
                max_files: None,
            },
        ),
        "all" => collect(
            &ctx.project_root,
            MediaScanOptions {
                include_raw: true,
                include_renders: true,
                max_files: None,
            },
        ),
        other => {
            return Err(format!(
                "list_assets: scope '{other}' not recognized. Use one of: raw, renders, all."
            ));
        }
    };

    // Stable sort by (scope, relative_path).
    let mut entries =
        entries.map_err(|e| format!("list_assets: unable to scan assets: {e}"))?;
    entries.sort_by(|a, b| (&a.scope, &a.rel_path).cmp(&(&b.scope, &b.rel_path)));

    // Optional catalog-backed bin filter. Read the project lazily —
    // only when a `bin` arg was provided.
    if let Some(bin) = args.bin.as_deref() {
        entries = filter_by_bin(&ctx.project_root, entries, bin)?;
    }

    let total = entries.len();
    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let visible = &entries[start..end];

    let mut out = String::new();
    match args.bin.as_deref() {
        Some(bin) => out.push_str(&format!(
            "scope={scope} bin={bin} total={total} offset={offset} limit={limit}\n"
        )),
        None => out.push_str(&format!(
            "scope={scope} total={total} offset={offset} limit={limit}\n"
        )),
    }
    for (i, e) in visible.iter().enumerate() {
        let idx = offset + i;
        let line = format!(
            "{idx:>4}. [{scope}] {rel}  ({size})",
            scope = e.scope,
            rel = e.rel_path,
            size = human_size(e.size_bytes)
        );
        out.push_str(&truncate(line, MAX_ENTRY_LENGTH));
        out.push('\n');
    }
    if end < total {
        out.push_str(&format!(
            "More than {limit} entries found ({remaining} additional). Re-call with offset={next}.\n",
            remaining = total - end,
            next = end + 1,
        ));
    }
    Ok(out)
}

#[derive(Debug)]
struct AssetEntry {
    scope: String,
    rel_path: String,
    size_bytes: u64,
}

fn collect(root: &Path, options: MediaScanOptions) -> std::io::Result<Vec<AssetEntry>> {
    collect_project_media_files(root, options)
        .map(|files| files.into_iter().map(AssetEntry::from).collect())
}

impl From<MediaFile> for AssetEntry {
    fn from(file: MediaFile) -> Self {
        let scope_prefix = format!("{}/", file.scope);
        let rel_path = file
            .project_relative_path
            .strip_prefix(&scope_prefix)
            .unwrap_or(&file.project_relative_path)
            .to_string();
        Self {
            scope: file.scope,
            rel_path,
            size_bytes: file.size_bytes,
        }
    }
}

/// Filter entries against the project's durable asset catalog.
fn filter_by_bin(
    project_root: &Path,
    entries: Vec<AssetEntry>,
    bin: &str,
) -> Result<Vec<AssetEntry>, String> {
    let project = Project::read(project_root)
        .map_err(|e| format!("list_assets: unable to read project for bin filter: {e}"))?;
    let catalog = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|meta| meta.asset_catalog.as_ref());
    let Some(catalog) = catalog else {
        // No catalog ⇒ nothing satisfies the filter.
        return Ok(Vec::new());
    };

    let is_role_filter = bin.starts_with(ROLE_BIN_PREFIX);
    let kept = entries
        .into_iter()
        .filter(|entry| {
            let asset_id = format!("{}/{}", entry.scope, entry.rel_path);
            let Some(record) = catalog.assets.iter().find(|r| r.id == asset_id) else {
                return false;
            };
            if is_role_filter {
                role_id_for_record(record) == bin
            } else {
                record.bin_id.as_deref() == Some(bin)
            }
        })
        .collect();
    Ok(kept)
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn truncate(s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub const DESCRIPTION: &str = "\
List source assets and renders in the project. Returns a numbered, \
paginated list with file sizes. Scope: 'raw' (source media), 'renders' \
(engine outputs), 'all' (both, default). 1-indexed offset, default \
limit 25, hard cap 100. Use this to discover what's available before \
calling `inspect_clip` or `read_index` on a specific asset.";
