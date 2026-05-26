//! `vedit_show` — inspect one vedit commit and its local diff.
//! Ported from `crates/core/src/tools/vedit_show.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_show`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditShowArgs {
    /// Commit hash, short hash, tag, branch, or HEAD.
    pub refstr: String,
}

pub fn run(args: VeditShowArgs, ctx: McpToolCtx) -> Result<String, String> {
    let refstr = args.refstr.trim();
    if refstr.is_empty() {
        return Err("vedit_show: refstr cannot be empty".into());
    }
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_show: opening repo failed: {e}"))?;
    let details = vc::show_commit(&repo, refstr).map_err(|e| format!("vedit_show: {e}"))?;
    let change_count = details.diff.len();
    let diff = details.diff;
    let body = serde_json::json!({
        "commit_hash": details.commit_hash,
        "timeline_hash": details.timeline_hash,
        "timestamp": details.timestamp,
        "header": details.header,
        "action_metadata": details.action_metadata,
        "full_message": details.full_message,
        "action_metadata": vc::action_metadata_from_message(&details.full_message),
        "parents": details.parents,
        "diff": {
            "from": diff.from_ref,
            "to": diff.to_ref,
            "change_count": change_count,
            "structural_changes": diff.changes,
            "animation_changes": diff.animation_changes,
        }
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Show one vedit commit with its message, hashes, parents, and semantic \
diff from the first parent to that commit. Use this for deep-diving a \
history entry without listing the full log again.\
";
