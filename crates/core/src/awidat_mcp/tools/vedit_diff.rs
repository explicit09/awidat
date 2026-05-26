//! `vedit_diff` — structured diff between two refs.
//! Ported from `crates/core/src/tools/vedit_diff.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_diff`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditDiffArgs {
    /// From-ref. Default `"session-start"` (the branch awidat stamps
    /// at session open). Pass an explicit ref (branch name, full
    /// hash, or short hash) to compare against an earlier point.
    #[serde(default)]
    pub from: Option<String>,
    /// To-ref. Default `"HEAD"`.
    #[serde(default)]
    pub to: Option<String>,
}

pub fn run(args: VeditDiffArgs, ctx: McpToolCtx) -> Result<String, String> {
    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_diff: opening repo failed: {e}"))?;

    let diff = vc::diff_refs(&repo, args.from.as_deref(), args.to.as_deref())
        .map_err(|e| format!("vedit_diff: {e}"))?;

    let body = serde_json::json!({
        "from": diff.from_ref,
        "to": diff.to_ref,
        "change_count": diff.len(),
        "structural_change_count": diff.changes.len(),
        "structural_changes_empty": diff.changes.is_empty(),
        "changes": diff.changes,
        "animation_change_count": diff.animation_changes.len(),
        "animation_changes": diff.animation_changes,
        "note": if diff.is_empty() {
            "Empty structural diff. If you committed metadata-only changes (e.g. agent reasoning updates), they show up in vedit_log but not here."
        } else {
            ""
        },
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Show the structured diff between two refs. Default: \
`from='session-start'`, `to='HEAD'` — i.e. everything that's changed \
since this session began.\
\n\nReturns: { from, to, change_count, structural_changes_empty, \
changes: [...] }. The `changes` array is a list of structured \
operations: TrackAdded, TrackRemoved, Trimmed, Moved, Added, Removed, \
EffectsChanged, Replaced, TransitionAdded, TransitionRemoved. Each \
entry carries enough context to render as English prose.\
\n\nUse this when the user asks: 'what did you change?', 'show me \
the diff', 'what's different from when I started?'. The structural \
diff is computed from the OTIO model — if no clips moved or trimmed, \
it's empty even if the agent rewrote the reasoning fields. \
Metadata-only commits show up in `vedit_log` but produce empty \
structural diffs.\
\n\nPass explicit refs to compare arbitrary points: branch names, \
full hashes, or short hashes (>= 4 hex chars). Unknown refs surface \
as a clear error.\
";
