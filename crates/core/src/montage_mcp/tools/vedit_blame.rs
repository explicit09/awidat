//! `vedit_blame` — project vedit history onto one clip.
//! Ported from `crates/core/src/tools/vedit_blame.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

const DEFAULT_LIMIT: usize = 200;
const HARD_LIMIT: usize = 500;

/// Arguments to `vedit_blame`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditBlameArgs {
    /// Clip name or media reference to project history onto.
    pub clip_id: String,
    /// Ref where the first-parent walk starts. Defaults to HEAD.
    #[serde(default)]
    pub start_ref: Option<String>,
    /// Max first-parent commits to inspect. Default 200, hard cap 500.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn run(args: VeditBlameArgs, ctx: McpToolCtx) -> Result<String, String> {
    let clip_id = args.clip_id.trim();
    if clip_id.is_empty() {
        return Err("vedit_blame: clip_id cannot be empty".into());
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(HARD_LIMIT);
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_blame: opening repo failed: {e}"))?;
    let entries = vc::blame_clip(&repo, clip_id, args.start_ref.as_deref(), limit)
        .map_err(|e| format!("vedit_blame: {e}"))?;
    let matches = entries
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "commit_hash": entry.commit_hash,
                "timeline_hash": entry.timeline_hash,
                "timestamp": entry.timestamp,
                "header": entry.header,
                "full_message": entry.full_message,
                "parents": entry.parents,
                "structural_changes": entry.changes,
                "animation_changes": entry.animation_changes,
            })
        })
        .collect::<Vec<_>>();
    let body = serde_json::json!({
        "clip_id": clip_id,
        "start_ref": args.start_ref.unwrap_or_else(|| "HEAD".to_string()),
        "limit_applied": limit,
        "match_count": matches.len(),
        "matches": matches,
        "note": if matches.is_empty() {
            "No first-parent commits within the limit touched this clip by name, media reference, or animation target string."
        } else {
            ""
        },
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Project vedit history onto one clip. Walks first-parent history from \
HEAD (or start_ref), computes each commit's semantic diff, and returns \
commits whose changes touch the supplied clip name or media reference. \
This is attribution, not a branch checkout or merge operation.\
";
