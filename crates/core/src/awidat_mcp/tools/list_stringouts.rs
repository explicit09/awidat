//! `list_stringouts` — enumerate ordered select-collections.
//! Ported in step 5 from `crates/core/src/tools/list_stringouts.rs`.

use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListStringoutsArgs {}

pub fn run(_args: ListStringoutsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("list_stringouts: unable to read project: {e}"))?;
    let stringouts: Vec<_> = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .map(|meta| meta.stringouts.clone())
        .unwrap_or_default();

    let total = stringouts.len();
    let mut out = format!("total={total}\n");
    for (i, s) in stringouts.iter().enumerate() {
        let idx = i + 1;
        let name = s.name.as_deref().unwrap_or("");
        out.push_str(&format!(
            "{idx:>3}. id={id} name=\"{name}\" items={count}\n",
            idx = idx,
            id = s.id,
            name = name,
            count = s.select_ids.len(),
        ));
    }
    Ok(out)
}

pub const DESCRIPTION: &str = "\
List the project's named stringouts (ordered select-collections). \
Each stringout has a stable id, an optional display name, and a count \
of ordered select ids it references. Use `create_stringout` to add a \
new one without disturbing existing ones — projects support multiple \
stringouts in parallel (e.g. per arc, alt-cut, cold-open).";
