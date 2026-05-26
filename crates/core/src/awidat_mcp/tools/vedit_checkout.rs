//! `vedit_checkout` — switch the working timeline to a branch.
//! Ported from `crates/core/src/tools/vedit_checkout.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_checkout`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditCheckoutArgs {
    /// Existing branch/alternate to check out.
    pub branch: String,
}

pub fn run(args: VeditCheckoutArgs, ctx: McpToolCtx) -> Result<String, String> {
    let branch = args.branch.trim();
    if branch.is_empty() {
        return Err("vedit_checkout: branch cannot be empty".into());
    }

    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_checkout: opening repo failed: {e}"))?;
    let checkout =
        vc::checkout_branch(&repo, branch).map_err(|e| format!("vedit_checkout: {e}"))?;
    Ok(serde_json::json!({
        "branch": checkout.branch,
        "commit_hash": checkout.commit_hash,
        "timeline_hash": checkout.timeline_hash,
        "project_otio": repo.project_otio_path().display().to_string(),
        "next_step": "Reload the timeline if needed, then continue edits on this branch. Use vedit_branch(list=true) to confirm the active branch.",
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Switch HEAD to an existing local vedit branch and restore \
`project.otio.json` to that branch's committed timeline snapshot. This \
is branch checkout for alternate cuts; it is not a merge and it does \
not create an audit commit by itself.\
";
