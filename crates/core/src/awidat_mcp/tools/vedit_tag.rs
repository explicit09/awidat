//! `vedit_tag` — create or list named local vedit checkpoints.
//! Ported from `crates/core/src/tools/vedit_tag.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_tag`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditTagArgs {
    /// Flat tag name to create/update, e.g. 'client-review-v1'. Omit
    /// when listing tags.
    #[serde(default)]
    pub name: Option<String>,
    /// Commit hash, short hash, branch name, or HEAD. Defaults to HEAD.
    #[serde(default)]
    pub refstr: Option<String>,
    /// When true, list existing tags instead of creating one.
    #[serde(default)]
    pub list: bool,
}

pub fn run(args: VeditTagArgs, ctx: McpToolCtx) -> Result<String, String> {
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_tag: opening repo failed: {e}"))?;

    if args.list {
        let tags = vc::list_tags(&repo).map_err(|e| format!("vedit_tag: {e}"))?;
        return Ok(serde_json::json!({
            "tag_count": tags.len(),
            "tags": tags,
        })
        .to_string());
    }

    let Some(name) = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err("vedit_tag: name is required unless list=true".into());
    };
    let tag = vc::tag_ref(&repo, name, args.refstr.as_deref())
        .map_err(|e| format!("vedit_tag: {e}"))?;
    Ok(serde_json::json!({
        "name": tag.name,
        "target": tag.target,
        "next_step": "Use vedit_show or vedit_diff with this tag name to inspect the checkpoint.",
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Create or list local named vedit checkpoints. Tags are stable labels \
under `.vedit/refs/tags/` that point at commits. They do not switch \
HEAD, create branches, or merge anything. Use this for names like \
`client-review-v1` or `before-tightening-pass`.\
";
