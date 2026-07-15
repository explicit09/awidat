//! `set_picture_lock` — lock or unlock picture for department handoffs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `set_picture_lock`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SetPictureLockArgs {
    /// True to lock picture; false to unlock.
    pub locked: bool,
    /// Optional reason recorded in the lock file.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Run `set_picture_lock`.
pub fn run(args: SetPictureLockArgs, ctx: McpToolCtx) -> Result<String, String> {
    let state = crate::picture_lock::set_picture_lock(
        &ctx.project_root,
        args.locked,
        args.reason.or_else(|| {
            Some(if args.locked {
                "locked via set_picture_lock".into()
            } else {
                "unlocked via set_picture_lock".into()
            })
        }),
    )?;
    let body = serde_json::json!({
        "locked": state.locked,
        "reason": state.reason,
        "updated_at": state.updated_at.to_rfc3339(),
    });
    serde_json::to_string(&body).map_err(|e| format!("set_picture_lock serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Set or clear picture lock for the project. When locked, apply_edl rejects \
ops that trim, split, move, retime, or otherwise restructure picture so \
sound/color/graphics passes cannot silently reopen the cut. Call with \
locked=true after picture gates pass (or user confirms picture lock); \
locked=false only when the user explicitly asks to reopen picture.\
";
