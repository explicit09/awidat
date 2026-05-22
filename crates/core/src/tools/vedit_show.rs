//! `vedit_show` tool — inspect one vedit commit and its local diff.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_show` tool.
pub struct VeditShowTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Commit hash, short hash, tag, branch, or HEAD.
    refstr: String,
}

#[async_trait]
impl ToolHandler for VeditShowTool {
    fn name(&self) -> &'static str {
        "vedit_show"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_show".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["refstr"],
                "properties": {
                    "refstr": {
                        "type": "string",
                        "description": "Commit hash, short hash, tag, branch, or HEAD."
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
                "vedit_show: invalid args ({e}). Required: {{ \"refstr\": string }}."
            ))
        })?;
        let refstr = args.refstr.trim();
        if refstr.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "vedit_show: refstr cannot be empty".into(),
            ));
        }
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_show: opening repo failed: {e}"))
        })?;
        let details = vc::show_commit(&repo, refstr)
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_show: {e}")))?;
        let change_count = details.diff.len();
        let diff = details.diff;
        let body = serde_json::json!({
            "commit_hash": details.commit_hash,
            "timeline_hash": details.timeline_hash,
            "timestamp": details.timestamp,
            "header": details.header,
            "action_metadata": details.action_metadata,
            "full_message": details.full_message,
            "parents": details.parents,
            "diff": {
                "from": diff.from_ref,
                "to": diff.to_ref,
                "change_count": change_count,
                "structural_changes": diff.changes,
                "animation_changes": diff.animation_changes,
            }
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Show one vedit commit with its message, hashes, parents, and semantic \
diff from the first parent to that commit. Use this for deep-diving a \
history entry without listing the full log again.\
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

    fn write_otio(project_root: &std::path::Path, duration: f64) {
        let value = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{
                        "OTIO_SCHEMA": "Clip.2",
                        "name": "shot-a",
                        "source_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0.0, "rate": 24.0},
                            "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": duration, "rate": 24.0}
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }]
                }]
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn shows_commit_with_parent_diff() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();

        let out = VeditShowTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_show".into(),
                    args: serde_json::json!({"refstr": second.commit_hash}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["header"].as_str(), Some("Trim shot-a"));
        assert_eq!(parsed["diff"]["change_count"].as_u64(), Some(1));
    }
}
