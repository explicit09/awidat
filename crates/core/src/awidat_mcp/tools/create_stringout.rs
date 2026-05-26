//! `create_stringout` — append a new named stringout. Ported from
//! `crates/core/src/tools/create_stringout.rs` to the in-process MCP
//! server in step 5 of the codex-harness migration.
//!
//! Mutating: writes the project's awidat metadata.

use awidat_proto::professional::Stringout;
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::media_catalog_mutation::ensure_awidat_metadata;

/// Arguments to `create_stringout`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CreateStringoutArgs {
    /// Stable stringout id. Required.
    pub id: String,
    /// Optional display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Ordered select ids the stringout points at.
    #[serde(default)]
    pub items: Vec<String>,
}

/// Run `create_stringout` against the project resolved from
/// [`McpToolCtx`]. Returns the response JSON as `Ok(String)`;
/// validation / read / write errors return `Err(String)`.
pub fn run(args: CreateStringoutArgs, ctx: McpToolCtx) -> Result<String, String> {
    let id = args.id.trim();
    if id.is_empty() {
        return Err("create_stringout: id must not be empty".into());
    }
    let id = id.to_string();

    let mut project = Project::read(&ctx.project_root)
        .map_err(|e| format!("create_stringout: unable to read project: {e}"))?;

    let meta = ensure_awidat_metadata(&mut project.timeline);
    if meta.stringouts.iter().any(|s| s.id == id) {
        return Err(format!(
            "create_stringout: stringout {id} already exists"
        ));
    }
    let stringout = Stringout {
        id: id.clone(),
        name: args.name,
        select_ids: args.items,
    };
    let item_count = stringout.select_ids.len();
    meta.stringouts.push(stringout);

    project
        .write(&ctx.project_root)
        .map_err(|e| format!("create_stringout: unable to write project: {e}"))?;

    Ok(serde_json::json!({
        "status": "created",
        "id": id,
        "items": item_count,
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Create a new named stringout (ordered select-collection) in the \
project. Projects support multiple parallel stringouts (per arc, \
alt-cut, cold-open) — calling this never replaces an existing one. \
'id' is required; 'name' is an optional display label; 'items' is an \
ordered list of select ids the stringout points at. Returns an error \
if a stringout with that id already exists.\
";
