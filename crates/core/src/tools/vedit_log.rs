//! `vedit_log` tool — last N commits, newest-first. Phase A.
//!
//! The agent reads this when the user asks "what have you done lately?",
//! "show me history", "what's in this project's edit log?". Returns
//! lightweight entries — header + timestamp + hashes — so the agent can
//! list history without pulling every commit's full body into context.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// Default cap on entries returned. Session-scale: a long session
/// might produce 20+ commits; 30 covers most "what did I do today"
/// asks without flooding the agent's context.
const DEFAULT_LIMIT: usize = 30;

/// Hard cap. Older history requires explicit `limit` argument.
const HARD_LIMIT: usize = 200;

/// The `vedit_log` tool.
pub struct VeditLogTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Max entries. Default 30, hard cap 200.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for VeditLogTool {
    fn name(&self) -> &'static str {
        "vedit_log"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_log".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max entries. Default 30, hard cap 200."
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
                "vedit_log: invalid args ({e}). Optional: {{ \"limit\": int }}."
            ))
        })?;
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(HARD_LIMIT);

        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_log: opening repo failed: {e}"))
        })?;

        let entries = vc::log(&repo, limit)
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_log: {e}")))?;

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
                    "full_message": e.full_message,
                    "parents": e.parents,
                })
            })
            .collect();

        let body = serde_json::json!({
            "limit_applied": limit,
            "entry_count": entries_wire.len(),
            "entries": entries_wire,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
List recent vedit commits, newest-first. Each entry: { commit_hash, \
timeline_hash, timestamp, header, full_message, parents }. The header \
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
    async fn empty_log_when_no_commits() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let ctx = ctx_for(dir.path().to_path_buf());
        let out = VeditLogTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_log".into(),
                    args: serde_json::json!({}),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["entry_count"].as_u64(), Some(0));
        assert!(parsed["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn returns_commits_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "First", Some("body 1")).unwrap();
        vc::commit_current_timeline(&repo, "Second", None).unwrap();
        vc::commit_current_timeline(&repo, "Third", Some("body 3")).unwrap();

        let ctx = ctx_for(dir.path().to_path_buf());
        let out = VeditLogTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_log".into(),
                    args: serde_json::json!({}),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let entries = parsed["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["header"].as_str(), Some("Third"));
        assert_eq!(entries[1]["header"].as_str(), Some("Second"));
        assert_eq!(entries[2]["header"].as_str(), Some("First"));
        assert!(
            entries[2]["full_message"]
                .as_str()
                .unwrap()
                .contains("body 1")
        );
    }

    #[tokio::test]
    async fn limit_caps_results() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let repo = vc::open_or_init(dir.path()).unwrap();
        for i in 0..5 {
            vc::commit_current_timeline(&repo, &format!("c{i}"), None).unwrap();
        }
        let ctx = ctx_for(dir.path().to_path_buf());
        let out = VeditLogTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_log".into(),
                    args: serde_json::json!({"limit": 2}),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["entry_count"].as_u64(), Some(2));
    }
}
