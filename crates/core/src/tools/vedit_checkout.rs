//! `vedit_checkout` tool — switch the working timeline to a branch.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_checkout` tool.
pub struct VeditCheckoutTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Existing branch/alternate to check out.
    branch: String,
}

#[async_trait]
impl ToolHandler for VeditCheckoutTool {
    fn name(&self) -> &'static str {
        "vedit_checkout"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_checkout".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["branch"],
                "properties": {
                    "branch": {
                        "type": "string",
                        "description": "Existing branch/alternate name to switch to."
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
        let branch = invocation
            .args
            .get("branch")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-branch>");
        vec![ApprovalKey::new(
            "vedit_checkout",
            format!("checkout:{branch}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_checkout: invalid args ({e}). Required: {{ \"branch\": string }}."
            ))
        })?;
        let branch = args.branch.trim();
        if branch.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "vedit_checkout: branch cannot be empty".into(),
            ));
        }

        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_checkout: opening repo failed: {e}"))
        })?;
        let checkout = vc::checkout_branch(&repo, branch)
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_checkout: {e}")))?;
        Ok(ToolOutput::text(
            serde_json::json!({
                "branch": checkout.branch,
                "commit_hash": checkout.commit_hash,
                "timeline_hash": checkout.timeline_hash,
                "project_otio": repo.project_otio_path().display().to_string(),
                "next_step": "Reload the timeline if needed, then continue edits on this branch. Use vedit_branch(list=true) to confirm the active branch.",
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Switch HEAD to an existing local vedit branch and restore \
`project.otio.json` to that branch's committed timeline snapshot. This \
is branch checkout for alternate cuts; it is not a merge and it does \
not create an audit commit by itself.\
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

    fn write_minimal_otio(project_root: &std::path::Path, name: &str) {
        let value = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": name,
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn checks_out_branch_and_restores_working_timeline() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path(), "v1");
        let repo = vc::open_or_init(dir.path()).unwrap();
        let first = vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        vc::create_branch(&repo, "alt-tight", Some(&first.commit_hash)).unwrap();
        write_minimal_otio(dir.path(), "v2");
        vc::commit_current_timeline(&repo, "Main update", None).unwrap();

        let out = VeditCheckoutTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_checkout".into(),
                    args: serde_json::json!({"branch": "alt-tight"}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["branch"].as_str(), Some("alt-tight"));

        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("project.otio.json")).unwrap())
                .unwrap();
        assert_eq!(restored["name"].as_str(), Some("v1"));
    }
}
