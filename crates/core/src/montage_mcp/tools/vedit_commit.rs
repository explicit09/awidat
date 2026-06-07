//! `vedit_commit` — snapshot the project's `project.otio.json` as an
//! explicit vedit checkpoint. Ported from
//! `crates/core/src/tools/vedit_commit.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::vc;

/// Arguments to `vedit_commit`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VeditCommitArgs {
    /// One-line imperative header. The format is canonical: short,
    /// no trailing period — same convention as good git commit
    /// messages.
    pub header: String,
    /// Optional reasoning body. Free text; explains the editorial
    /// decisions behind the change.
    #[serde(default)]
    pub reasoning: Option<String>,
}

pub fn run(args: VeditCommitArgs, ctx: McpToolCtx) -> Result<String, String> {
    let header = args.header.trim();
    if header.is_empty() {
        return Err(
            "vedit_commit: empty header. Pass a short imperative line like \
             \"Trim drone_shot -1.8s\"."
                .into(),
        );
    }

    let repo = vc::open_or_init(&ctx.project_root)
        .map_err(|e| format!("vedit_commit: opening repo failed: {e}"))?;
    let outcome = vc::commit_current_timeline(&repo, header, args.reasoning.as_deref())
        .map_err(|e| format!("vedit_commit: {e}"))?;

    let body = serde_json::json!({
        "commit_hash": outcome.commit_hash,
        "timeline_hash": outcome.timeline_hash,
        "message": outcome.message,
        "next_step": "Use vedit_diff to see changes since session-start, or vedit_log to list recent commits.",
    });
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
Snapshot the project's current `project.otio.json` as a vedit commit. \
Use this when the user asks to save this version, mark a checkpoint, \
or commit a session of work.\
\n\nThe commit message format is canonical: a one-line imperative \
header (e.g. \"Trim drone_shot -1.8s; insert b-roll at 12.4s\") plus \
an optional reasoning body explaining your editorial decisions. The \
header is what `vedit_log` shows; the body is for deep dives. Both \
together become an audit trail of the agent's editorial judgment over \
time.\
\n\nReturns the new commit hash + the timeline-content hash. Two \
commits with identical timeline content share a timeline-hash even \
though their commit hashes differ (timestamp).\
\n\nIdempotent only on a per-content basis: re-committing identical \
project content writes a new commit object (different timestamp) but \
the underlying timeline isn't duplicated. There is no current \
behavior for skipping no-op commits — the agent should not call this \
unless something has actually changed since the last commit.\
\n\nAccepted apply_edl envelopes already auto-commit through the apply \
pipeline. Use this tool for explicit save points, named checkpoints, \
or metadata-only commits the user asked to preserve.\
";
