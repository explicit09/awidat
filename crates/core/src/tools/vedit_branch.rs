//! `vedit_branch` tool — create/list local vedit alternates.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_branch` tool.
pub struct VeditBranchTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Branch name to create. Omit when listing branches.
    #[serde(default)]
    name: Option<String>,
    /// Ref the branch should start at. Defaults to HEAD.
    #[serde(default)]
    start_ref: Option<String>,
    /// List branches instead of writing one.
    #[serde(default)]
    list: bool,
}

#[async_trait]
impl ToolHandler for VeditBranchTool {
    fn name(&self) -> &'static str {
        "vedit_branch"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_branch".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Flat branch/alternate name to create, e.g. 'alt-tight'."
                    },
                    "start_ref": {
                        "type": "string",
                        "description": "Commit hash, short hash, tag, branch, or HEAD. Defaults to HEAD."
                    },
                    "list": {
                        "type": "boolean",
                        "description": "When true, list existing branches instead of creating one."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
        !invocation
            .args
            .get("list")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        if !self.is_mutating(invocation) {
            return Vec::new();
        }
        let name = invocation
            .args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-name>");
        let start_ref = invocation
            .args
            .get("start_ref")
            .and_then(|v| v.as_str())
            .unwrap_or("HEAD");
        vec![ApprovalKey::new(
            "vedit_branch",
            format!("branch:{name}:start={start_ref}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_branch: invalid args ({e}). Optional: {{ \"name\": string, \"start_ref\"?: string, \"list\"?: bool }}."
            ))
        })?;
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_branch: opening repo failed: {e}"))
        })?;

        if args.list {
            let branches = vc::list_branches(&repo)
                .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_branch: {e}")))?;
            return Ok(ToolOutput::text(
                serde_json::json!({
                    "branch_count": branches.len(),
                    "branches": branches,
                })
                .to_string(),
            ));
        }

        let Some(name) = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Err(FunctionCallError::RespondToModel(
                "vedit_branch: name is required unless list=true".into(),
            ));
        };
        let branch = vc::create_branch(&repo, name, args.start_ref.as_deref())
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_branch: {e}")))?;
        Ok(ToolOutput::text(
            serde_json::json!({
                "name": branch.name,
                "target": branch.target,
                "is_current": branch.is_current,
                "next_step": "Use vedit_checkout with this branch name to switch the working timeline to the alternate.",
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Create or list local vedit branches/alternates. A branch is a movable \
ref under `.vedit/refs/heads/` that can hold an alternate cut. Creating \
a branch does not switch HEAD or merge anything; use `vedit_checkout` \
to switch the working timeline to an existing branch.\
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
    async fn creates_and_lists_branch() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path(), "v1");
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "Initial", None).unwrap();

        let create = VeditBranchTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_branch".into(),
                    args: serde_json::json!({"name": "alt-tight"}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&create.content).unwrap();
        assert_eq!(parsed["name"].as_str(), Some("alt-tight"));

        let list = VeditBranchTool
            .handle(
                ToolInvocation {
                    call_id: "c2".into(),
                    name: "vedit_branch".into(),
                    args: serde_json::json!({"list": true}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&list.content).unwrap();
        assert_eq!(parsed["branch_count"].as_u64(), Some(2));
    }
}
