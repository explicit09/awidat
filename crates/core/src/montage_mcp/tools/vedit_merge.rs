//! `vedit_merge` — execute bounded local vedit branch merges. Ported
//! from `crates/core/src/tools/vedit_merge.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_merge`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditMergeArgs {
    /// Source branch, tag, commit hash, or ref to merge.
    pub source: String,
    /// Target branch/ref to receive the merge. Defaults to HEAD.
    #[serde(default)]
    pub target: Option<String>,
}

pub fn run(args: VeditMergeArgs, ctx: McpToolCtx) -> Result<String, String> {
    let _mutation = vc::lock_timeline_mutation(&ctx.project_root)
        .map_err(|e| format!("vedit_merge: lock timeline mutation failed: {e}"))?;
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_merge: opening repo failed: {e}"))?;
    let outcome = vc::merge_refs(&repo, &args.source, args.target.as_deref())
        .map_err(|e| format!("vedit_merge: {e}"))?;
    let body = serde_json::json!({
        "commit_hash": outcome.commit_hash,
        "timeline_hash": outcome.timeline_hash,
        "message": outcome.message,
        "source_ref": outcome.source_ref,
        "target_ref": outcome.target_ref,
        "source_commit": outcome.source_commit,
        "target_commit": outcome.target_commit,
        "merge_base": outcome.merge_base,
        "parents": outcome.parents,
        "parent_count": outcome.parents.len(),
        "source_changed_clip_ids": outcome.source_changed_clip_ids,
        "target_changed_clip_ids": outcome.target_changed_clip_ids,
        "note": "Merged after bounded preflight confirmed non-overlapping changed clip/media ids.",
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Merge a source vedit ref into a target branch/ref using Montage's bounded \
non-overlapping clip-id rule. The merge first runs the same preflight as \
vedit_merge_preflight. If any changed clip/media ids overlap, it refuses \
the merge and reports a conflict. On success it writes project.otio.json, \
advances the target branch, and creates a two-parent merge commit.\
";
