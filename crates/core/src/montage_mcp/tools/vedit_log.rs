//! `vedit_log` — last N vedit commits, newest-first. Ported from
//! `crates/core/src/tools/vedit_log.rs` to the in-process MCP server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

/// Default cap on entries returned. Session-scale: a long session
/// might produce 20+ commits; 30 covers most "what did I do today"
/// asks without flooding the agent's context.
const DEFAULT_LIMIT: usize = 30;

/// Hard cap. Older history requires explicit `limit` argument.
const HARD_LIMIT: usize = 200;

/// Arguments to `vedit_log`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditLogArgs {
    /// Max entries. Default 30, hard cap 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn run(args: VeditLogArgs, ctx: McpToolCtx) -> Result<String, String> {
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(HARD_LIMIT);

    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_log: opening repo failed: {e}"))?;

    let entries = vc::log(&repo, limit).map_err(|e| format!("vedit_log: {e}"))?;

    // Project to the wire shape — the agent gets header + hashes
    // (no full body by default; that's a separate "show this commit"
    // call when we add one).
    let entries_wire: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "commit_hash": e.commit_hash,
                "timeline_hash": e.timeline_hash,
                "timestamp": e.timestamp,
                "header": e.header,
                "action_metadata": e.action_metadata,
                "full_message": e.full_message,
                "action_metadata": vc::action_metadata_from_message(&e.full_message),
                "parents": e.parents,
            })
        })
        .collect();

    let body = serde_json::json!({
        "limit_applied": limit,
        "entry_count": entries_wire.len(),
        "entries": entries_wire,
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
List recent vedit commits, newest-first. Each entry: { commit_hash, \
timeline_hash, timestamp, header, full_message, action_metadata, parents }. The header \
is the first line of the commit message — what to show in compact \
listings; full_message is the body for deep dives.\
\n\nUse this for: 'what's the edit history?', 'show me the last few \
commits', 'when did we make X change?'. The session-start branch is \
a stable reference point — `vedit_diff` defaults to comparing \
session-start..HEAD if you want changes within this session.\
\n\nDefault limit=30, hard cap 200. The agent should not request \
limit=200 unless the user explicitly asks for full history; smaller \
limits keep context focused.\
\n\nReturns an empty entries list when the repo has no commits yet \
(brand-new project). Surface that as 'no commits yet — try \
`vedit_commit` to save the current version.'\
";
