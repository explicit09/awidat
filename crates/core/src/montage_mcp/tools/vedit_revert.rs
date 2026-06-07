//! `vedit_revert` — restore the working timeline to a prior vedit
//! commit/ref, optionally recording the restore as a new commit.
//! Ported from `crates/core/src/tools/vedit_revert.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_revert`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditRevertArgs {
    /// Commit hash, short hash, branch name, or HEAD to restore from.
    #[serde(alias = "ref")]
    pub refstr: String,
    /// Whether to create an audit commit after restoring the working
    /// timeline. Defaults to true so the restore is visible in history.
    #[serde(default = "default_commit")]
    pub commit: bool,
    /// Optional commit header when `commit` is true.
    #[serde(default)]
    pub header: Option<String>,
    /// Optional reasoning body when `commit` is true.
    #[serde(default)]
    pub reasoning: Option<String>,
}

fn default_commit() -> bool {
    true
}

pub fn run(args: VeditRevertArgs, ctx: McpToolCtx) -> Result<String, String> {
    let refstr = args.refstr.trim();
    if refstr.is_empty() {
        return Err(
            "vedit_revert: empty refstr. Pass a commit hash, short hash, branch name, or HEAD."
                .into(),
        );
    }

    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_revert: opening repo failed: {e}"))?;
    let restored =
        vc::restore_working_timeline(&repo, refstr).map_err(|e| format!("vedit_revert: {e}"))?;

    let commit_outcome = if args.commit {
        let header = args
            .header
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!("Restore timeline to {}", short_hash(&restored.commit_hash))
            });
        let reasoning = args.reasoning.as_deref().or(Some(
            "Restored project.otio.json to a prior vedit snapshot at the user's request.",
        ));
        Some(
            vc::commit_current_timeline(&repo, &header, reasoning)
                .map_err(|e| format!("vedit_revert: {e}"))?,
        )
    } else {
        None
    };

    let body = serde_json::json!({
        "restored_ref": restored.requested_ref,
        "restored_commit_hash": restored.commit_hash,
        "restored_timeline_hash": restored.timeline_hash,
        "project_otio": repo.project_otio_path().display().to_string(),
        "audit_commit": commit_outcome.map(|out| serde_json::json!({
            "commit_hash": out.commit_hash,
            "timeline_hash": out.timeline_hash,
            "message": out.message,
        })),
        "next_step": "Use vedit_diff to inspect the restore, or reload the timeline in the UI if needed.",
    });
    Ok(body.to_string())
}

fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(7)
        .collect()
}

pub const DESCRIPTION: &str = "\
Restore the project's working `project.otio.json` to a prior vedit \
commit/ref. This is the safe product-level undo path for edit history: \
it reads the timeline snapshot stored at the requested ref and writes \
that snapshot back to the current project.\
\n\nBy default, `commit=true`, so the restore itself is recorded as a new \
commit and appears in `vedit_log`. Set `commit=false` only when the \
user explicitly asks to inspect or stage a restore without saving it.\
\n\nThis is not a branch checkout and not a merge. It does not switch HEAD \
or create branches; it restores the working timeline file to a known \
snapshot.\
";
