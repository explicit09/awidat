//! `vedit_merge_preflight` — check branch merge overlap without
//! merging. Ported from `crates/core/src/tools/vedit_merge_preflight.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_merge_preflight`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditMergePreflightArgs {
    /// Source branch, tag, commit hash, or ref that would be merged.
    pub source: String,
    /// Target branch, tag, commit hash, or ref. Defaults to HEAD.
    #[serde(default)]
    pub target: Option<String>,
}

pub fn run(args: VeditMergePreflightArgs, ctx: McpToolCtx) -> Result<String, String> {
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_merge_preflight: opening repo failed: {e}"))?;
    let preflight = vc::merge_preflight(&repo, &args.source, args.target.as_deref())
        .map_err(|e| format!("vedit_merge_preflight: {e}"))?;
    let body = serde_json::json!({
        "source_ref": preflight.source_ref,
        "target_ref": preflight.target_ref,
        "source_commit": preflight.source_commit,
        "target_commit": preflight.target_commit,
        "merge_base": preflight.merge_base,
        "is_mergeable": preflight.is_mergeable,
        "source_changed_clip_ids": preflight.source_changed_clip_ids,
        "target_changed_clip_ids": preflight.target_changed_clip_ids,
        "overlapping_clip_ids": preflight.overlapping_clip_ids,
        "source_change_count": preflight.source_change_count,
        "target_change_count": preflight.target_change_count,
        "next_step": if preflight.is_mergeable {
            "A future bounded merge can proceed under the non-overlapping clip-id rule; this preflight is read-only and does not merge refs."
        } else {
            "Resolve the overlapping clip ids manually; this preflight is read-only and does not merge refs."
        },
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Check whether a source ref can be safely merged into a target ref under \
Awidat's proposed bounded merge rule: both sides must have changed \
non-overlapping clip/media identifiers since their common ancestor. \
This tool is read-only; it does not checkout, merge, resolve conflicts, \
or modify refs.\
";
