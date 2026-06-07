//! `vedit_branch` — create/list local vedit alternates.
//! Ported from `crates/core/src/tools/vedit_branch.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_branch`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditBranchArgs {
    /// Branch name to create. Omit when listing branches.
    #[serde(default)]
    pub name: Option<String>,
    /// Ref the branch should start at. Defaults to HEAD.
    #[serde(default)]
    pub start_ref: Option<String>,
    /// List branches instead of writing one.
    #[serde(default)]
    pub list: bool,
}

pub fn run(args: VeditBranchArgs, ctx: McpToolCtx) -> Result<String, String> {
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_branch: opening repo failed: {e}"))?;

    if args.list {
        let branches = vc::list_branches(&repo).map_err(|e| format!("vedit_branch: {e}"))?;
        return Ok(serde_json::json!({
            "branch_count": branches.len(),
            "branches": branches,
        })
        .to_string());
    }

    let Some(name) = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err("vedit_branch: name is required unless list=true".into());
    };
    let branch = vc::create_branch(&repo, name, args.start_ref.as_deref())
        .map_err(|e| format!("vedit_branch: {e}"))?;
    Ok(serde_json::json!({
        "name": branch.name,
        "target": branch.target,
        "is_current": branch.is_current,
        "next_step": "Use vedit_checkout with this branch name to switch the working timeline to the alternate.",
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Create or list local vedit branches/alternates. A branch is a movable \
ref under `.vedit/refs/heads/` that can hold an alternate cut. Creating \
a branch does not switch HEAD or merge anything; use `vedit_checkout` \
to switch the working timeline to an existing branch.\
";
