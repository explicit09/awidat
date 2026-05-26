//! `vedit_diff` tool — structured diff between two refs. Phase A.
//!
//! The agent uses this to answer "what changed in this session?" or
//! "what did you do between commit X and now?" without re-reading the
//! whole timeline. Default: `from = session-start`, `to = HEAD`,
//! which surfaces every accepted edit in the current session.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_diff` tool.
pub struct VeditDiffTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// From-ref. Default `"session-start"` (the branch awidat stamps
    /// at session open). Pass an explicit ref (branch name, full
    /// hash, or short hash) to compare against an earlier point.
    #[serde(default)]
    from: Option<String>,
    /// To-ref. Default `"HEAD"`.
    #[serde(default)]
    to: Option<String>,
}

#[async_trait]
impl ToolHandler for VeditDiffTool {
    fn name(&self) -> &'static str {
        "vedit_diff"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_diff".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "From-ref. Default 'session-start'."
                    },
                    "to": {
                        "type": "string",
                        "description": "To-ref. Default 'HEAD'."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_diff: invalid args ({e}). Both fields optional; default is session-start..HEAD."
            ))
        })?;

        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_diff: opening repo failed: {e}"))
        })?;

        let diff = vc::diff_refs(&repo, args.from.as_deref(), args.to.as_deref())
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_diff: {e}")))?;

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
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx_for(project_root: std::path::PathBuf) -> ToolContext {
        use std::sync::Arc;
        use tokio::sync::broadcast;
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root,
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_minimal_otio(project_root: &std::path::Path) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&v).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn empty_diff_when_no_commits() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let ctx = ctx_for(dir.path().to_path_buf());
        let out = VeditDiffTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_diff".into(),
                    args: serde_json::json!({}),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["change_count"].as_u64(), Some(0));
        assert_eq!(parsed["structural_changes_empty"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn unknown_ref_surfaces_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        // Stamp at least one commit so the repo isn't fresh — otherwise
        // the test conflates "no commits" with "bad ref."
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "first", None).unwrap();

        let ctx = ctx_for(dir.path().to_path_buf());
        let err = VeditDiffTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_diff".into(),
                    args: serde_json::json!({"from": "no-such-ref"}),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("no-such-ref")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
