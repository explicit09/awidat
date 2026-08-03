//! `manage_assets` — agent-callable asset catalog and source review
//! mutators. Ported from `crates/core/src/tools/manage_assets.rs` to
//! the in-process MCP server in step 5 of the codex-harness migration.
//!
//! The original file exposes six distinct tools to the model. Each is
//! kept as its own ported `run` function with its own argument struct;
//! the rmcp wrappers in `montage_mcp::mod` register one `#[tool]` per
//! sub-tool, all annotated `destructive_hint = true`.

use montage_proto::professional::{SelectDecision, SourceRange, SourceSelect};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::media_catalog_mutation::{
    CatalogMutationError, create_bin, ensure_asset_catalog, ensure_montage_metadata,
    move_asset_to_bin,
};
use crate::montage_mcp::context::McpToolCtx;

// ------- Args -------

/// Arguments to `create_bin`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CreateBinArgs {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// Arguments to `move_to_bin`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct MoveToBinArgs {
    pub asset_id: String,
    #[serde(default)]
    pub bin_id: Option<String>,
}

/// Arguments to `rename_asset`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RenameAssetArgs {
    pub asset_id: String,
    pub label: String,
}

/// Arguments to `tag_asset`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TagAssetArgs {
    pub asset_id: String,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

/// Arguments to `rate_asset`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RateAssetArgs {
    pub asset_id: String,
    pub rating: u8,
}

/// Arguments to `mark_select`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct MarkSelectArgs {
    pub asset_id: String,
    pub start_s: f64,
    pub end_s: f64,
    /// One of: "select", "reject", "maybe", "favorite". Default
    /// "select".
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

// ------- Run functions -------

/// Run `create_bin`.
pub fn run_create_bin(args: CreateBinArgs, ctx: McpToolCtx) -> Result<String, String> {
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        create_bin(meta, args.id, args.name, args.parent_id).map_err(catalog_error)?;
        Ok(serde_json::json!({"status": "created"}))
    })
}

/// Run `move_to_bin`.
pub fn run_move_to_bin(args: MoveToBinArgs, ctx: McpToolCtx) -> Result<String, String> {
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        move_asset_to_bin(meta, &args.asset_id, args.bin_id).map_err(catalog_error)?;
        Ok(serde_json::json!({"status": "moved", "asset_id": args.asset_id}))
    })
}

/// Run `rename_asset`.
pub fn run_rename_asset(args: RenameAssetArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.label.trim().is_empty() {
        return Err("rename_asset: label must not be empty".into());
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| format!("asset {} does not exist", args.asset_id))?;
        asset.label = Some(args.label);
        Ok(serde_json::json!({"status": "renamed", "asset_id": args.asset_id}))
    })
}

/// Run `tag_asset`.
pub fn run_tag_asset(args: TagAssetArgs, ctx: McpToolCtx) -> Result<String, String> {
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| format!("asset {} does not exist", args.asset_id))?;
        asset
            .tags
            .retain(|tag| !args.remove.iter().any(|remove| remove == tag));
        for tag in args.add {
            let tag = tag.trim();
            if !tag.is_empty() && !asset.tags.iter().any(|existing| existing == tag) {
                asset.tags.push(tag.to_string());
            }
        }
        asset.tags.sort();
        Ok(serde_json::json!({
            "status": "tagged",
            "asset_id": args.asset_id,
            "tags": asset.tags.clone()
        }))
    })
}

/// Run `rate_asset`.
pub fn run_rate_asset(args: RateAssetArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.rating > 5 {
        return Err("rate_asset: rating must be in 0..=5".into());
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| format!("asset {} does not exist", args.asset_id))?;
        asset.rating = Some(args.rating);
        Ok(serde_json::json!({
            "status": "rated",
            "asset_id": args.asset_id,
            "rating": args.rating
        }))
    })
}

/// Run `mark_select`.
pub fn run_mark_select(args: MarkSelectArgs, ctx: McpToolCtx) -> Result<String, String> {
    let range = SourceRange {
        start_s: args.start_s,
        end_s: args.end_s,
    };
    if !range.is_valid() {
        return Err("mark_select: start_s must be finite and before end_s".into());
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_montage_metadata(&mut project.timeline);
        {
            let catalog = ensure_asset_catalog(meta);
            if !catalog.assets.iter().any(|asset| asset.id == args.asset_id) {
                return Err(format!("asset {} does not exist", args.asset_id));
            }
        }
        let decision = match args.decision.as_deref() {
            None => SelectDecision::default(),
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "select" => SelectDecision::Select,
                "reject" => SelectDecision::Reject,
                "maybe" => SelectDecision::Maybe,
                "favorite" => SelectDecision::Favorite,
                other => {
                    return Err(format!(
                        "mark_select: unknown decision {other:?}. Use select, reject, maybe, or favorite."
                    ));
                }
            },
        };
        let id = args.id.unwrap_or_else(|| {
            format!(
                "select-{}-{:.3}-{:.3}-{:?}",
                slug(&args.asset_id),
                range.start_s,
                range.end_s,
                decision
            )
            .to_ascii_lowercase()
        });
        let select = SourceSelect {
            id: id.clone(),
            asset_id: args.asset_id.clone(),
            range,
            decision,
            reason: args.reason,
            notes: args.notes,
            ..SourceSelect::default()
        };
        match meta.selects.iter_mut().find(|select| select.id == id) {
            Some(existing) => *existing = select,
            None => meta.selects.push(select),
        }
        let catalog = ensure_asset_catalog(meta);
        if let Some(asset) = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            && !asset
                .usage
                .select_ids
                .iter()
                .any(|select_id| select_id == &id)
        {
            asset.usage.select_ids.push(id.clone());
            asset.usage.select_ids.sort();
        }
        Ok(serde_json::json!({"status": "marked", "select_id": id}))
    })
}

// ------- Helpers -------

fn mutate_project(
    ctx: &McpToolCtx,
    mutate: impl FnOnce(&mut Project) -> Result<serde_json::Value, String>,
) -> Result<String, String> {
    let _mutation = crate::vc::lock_timeline_mutation(&ctx.project_root)
        .map_err(|e| format!("manage_assets: lock timeline mutation: {e}"))?;
    let mut project = Project::read(&ctx.project_root)
        .map_err(|e| format!("manage_assets: unable to read project: {e}"))?;
    let body = mutate(&mut project)?;
    project
        .write(&ctx.project_root)
        .map_err(|e| format!("manage_assets: unable to write project: {e}"))?;
    Ok(body.to_string())
}

fn catalog_error(error: CatalogMutationError) -> String {
    error.to_string()
}

fn slug(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

// ------- Descriptions -------

pub const DESCRIPTION_CREATE_BIN: &str =
    "Create a durable user/agent asset bin in the project catalog.";

pub const DESCRIPTION_MOVE_TO_BIN: &str =
    "Move a catalog asset into a durable bin, or clear its bin.";

pub const DESCRIPTION_RENAME_ASSET: &str =
    "Set the human-readable label for a catalog asset without renaming the source file.";

pub const DESCRIPTION_TAG_ASSET: &str = "Add or remove durable tags on a catalog asset.";

pub const DESCRIPTION_RATE_ASSET: &str = "Set an asset rating from 0 to 5.";

pub const DESCRIPTION_MARK_SELECT: &str =
    "Create or update a durable source select for an asset range.";
