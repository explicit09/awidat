//! `vedit_changed_clip_ids` — list clip/media ids touched by a ref diff.
//! Ported from `crates/core/src/tools/vedit_changed_clip_ids.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_changed_clip_ids`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditChangedClipIdsArgs {
    /// From-ref. Default `"session-start"`.
    #[serde(default)]
    pub from: Option<String>,
    /// To-ref. Default `"HEAD"`.
    #[serde(default)]
    pub to: Option<String>,
}

pub fn run(args: VeditChangedClipIdsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_changed_clip_ids: opening repo failed: {e}"))?;
    let diff = vc::diff_refs(&repo, args.from.as_deref(), args.to.as_deref())
        .map_err(|e| format!("vedit_changed_clip_ids: {e}"))?;
    let changed_clip_ids = vc::changed_clip_ids(&diff);
    let body = serde_json::json!({
        "from": diff.from_ref,
        "to": diff.to_ref,
        "changed_clip_count": changed_clip_ids.len(),
        "changed_clip_ids": changed_clip_ids,
        "structural_change_count": diff.changes.len(),
        "animation_change_count": diff.animation_changes.len(),
        "note": if diff.is_empty() {
            "No structural or animation changes touched clip/media identifiers in this ref range."
        } else {
            ""
        },
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
List the sorted clip names, media references, and clip animation targets \
touched by a vedit diff. Default: from='session-start', to='HEAD'. \
This is read-only preflight data for history review or future merge \
conflict checks; it does not checkout, merge, or modify refs.\
";
