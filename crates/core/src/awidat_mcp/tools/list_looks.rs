//! `list_looks` — agent-facing catalog of named color-corrector
//! looks. Ported in step 5 from `crates/core/src/tools/list_looks.rs`.
//! Reads `skills/color-corrector/looks.toml` under the project root
//! and returns its entries as structured JSON.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Project-relative path to the catalog. Kept in sync with
/// `scripts/look_region_plan.py:CATALOG_PATH` and the skill's
/// `SKILL.md` pointer.
const CATALOG_RELATIVE_PATH: &str = "skills/color-corrector/looks.toml";

/// `list_looks` takes no arguments.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListLooksArgs {}

#[derive(Debug, Deserialize, Serialize)]
struct CatalogFile {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    catalog_name: Option<String>,
    // TOML uses `[[look]]` arrays-of-tables, but JSON consumers
    // expect a plural `looks` key. Rename only on deserialize so
    // the structured output stays human-friendly.
    #[serde(default, rename(deserialize = "look"))]
    looks: Vec<CatalogLook>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CatalogLook {
    id: String,
    display_name: String,
    description: String,
    default_input_space: String,
    default_output_space: String,
    default_size: u32,
    recommended_strength_min: f64,
    recommended_strength_max: f64,
    #[serde(default)]
    tags: Vec<String>,
}

pub fn run(_args: ListLooksArgs, ctx: McpToolCtx) -> Result<String, String> {
    let catalog = load_catalog(&ctx.project_root)?;
    serde_json::to_string_pretty(&catalog)
        .map_err(|e| format!("list_looks: catalog serialize failed: {e}"))
}

fn load_catalog(project_root: &Path) -> Result<CatalogFile, String> {
    let path: PathBuf = project_root.join(CATALOG_RELATIVE_PATH);
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("list_looks: catalog unreadable at {}: {e}", path.display()))?;
    toml::from_str::<CatalogFile>(&raw).map_err(|e| {
        format!(
            "list_looks: catalog at {} is not valid TOML: {e}",
            path.display()
        )
    })
}

pub const DESCRIPTION: &str = "\
Returns the agent-facing catalog of named color-corrector looks. \
Each entry includes id, display_name, description, \
default_input_space, default_output_space, default_size, \
recommended_strength_min, recommended_strength_max, and tags. Use \
this before composing an `awidat.lut` or `awidat.color_pipeline` \
effect to pick a look that's compatible with the clip's input \
space and apply a sensible strength.";
