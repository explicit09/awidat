//! `vedit_commit` tool — snapshot the project's `project.otio.json`
//! as an explicit vedit checkpoint.
//!
//! Accepted edit envelopes already auto-commit through the apply
//! pipeline. The agent calls this only when the user asks to save
//! or commit an explicit checkpoint.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_commit` tool.
pub struct VeditCommitTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// One-line imperative header. The format is canonical: short,
    /// no trailing period — same convention as good git commit
    /// messages. Example: "Trim drone_shot_04 -1.8s; insert b-roll
    /// cover at skyline reference".
    header: String,
    /// Optional reasoning body. Free text; explains the editorial
    /// decisions behind the change. Phase B's apply-pipeline auto-
    /// committer will populate this from the turn's reasoning;
    /// Phase A callers (the agent acting on a user "save" request)
    /// pass it explicitly.
    #[serde(default)]
    reasoning: Option<String>,
}

#[async_trait]
impl ToolHandler for VeditCommitTool {
    fn name(&self) -> &'static str {
        "vedit_commit"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_commit".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["header"],
                "properties": {
                    "header": {
                        "type": "string",
                        "description": "One-line imperative header. Short, no trailing period."
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Optional body explaining the editorial decisions."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let header = invocation
            .args
            .get("header")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-header>")
            .trim();
        vec![ApprovalKey::new(
            "vedit_commit",
            format!(
                "checkpoint:{}",
                crate::tool::summarize_args(&serde_json::json!({ "header": header }))
            ),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_commit: invalid args ({e}). Required: {{ \"header\": string, \"reasoning\"?: string }}."
            ))
        })?;
        let header = args.header.trim();
        if header.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "vedit_commit: empty header. Pass a short imperative line like \
                 \"Trim drone_shot -1.8s\"."
                    .into(),
            ));
        }

        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_commit: opening repo failed: {e}"))
        })?;
        let outcome = vc::commit_current_timeline(&repo, header, args.reasoning.as_deref())
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_commit: {e}")))?;

        let body = serde_json::json!({
            "commit_hash": outcome.commit_hash,
            "timeline_hash": outcome.timeline_hash,
            "message": outcome.message,
            "next_step": "Use vedit_diff to see changes since session-start, or vedit_log to list recent commits.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
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
    async fn returns_commit_hash_in_response_body() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let ctx = ctx_for(dir.path().to_path_buf());

        let out = VeditCommitTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_commit".into(),
                    args: serde_json::json!({
                        "header": "Initial commit",
                        "reasoning": "Brand-new project."
                    }),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(
            parsed["commit_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            parsed["timeline_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            parsed["message"]
                .as_str()
                .unwrap()
                .contains("Initial commit")
        );
        assert!(
            parsed["message"]
                .as_str()
                .unwrap()
                .contains("Agent reasoning: Brand-new project.")
        );
    }

    #[tokio::test]
    async fn empty_header_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let ctx = ctx_for(dir.path().to_path_buf());

        let err = VeditCommitTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_commit".into(),
                    args: serde_json::json!({"header": ""}),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("empty header")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_project_otio_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        // No project.otio.json written.
        let ctx = ctx_for(dir.path().to_path_buf());
        let err = VeditCommitTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_commit".into(),
                    args: serde_json::json!({"header": "x"}),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("project.otio.json") || msg.contains("project timeline"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
