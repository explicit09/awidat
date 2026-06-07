//! `vedit_tag` tool — create/list named local vedit checkpoints.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::vc;

/// The `vedit_tag` tool.
pub struct VeditTagTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Tag name to create or update. Omit when listing tags.
    #[serde(default)]
    name: Option<String>,
    /// Ref to tag. Defaults to HEAD.
    #[serde(default)]
    refstr: Option<String>,
    /// List tags instead of writing one.
    #[serde(default)]
    list: bool,
}

#[async_trait]
impl ToolHandler for VeditTagTool {
    fn name(&self) -> &'static str {
        "vedit_tag"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_tag".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Flat tag name to create/update, e.g. 'client-review-v1'."
                    },
                    "refstr": {
                        "type": "string",
                        "description": "Commit hash, short hash, branch name, or HEAD. Defaults to HEAD."
                    },
                    "list": {
                        "type": "boolean",
                        "description": "When true, list existing tags instead of creating one."
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
        let refstr = invocation
            .args
            .get("refstr")
            .and_then(|v| v.as_str())
            .unwrap_or("HEAD");
        vec![ApprovalKey::new(
            "vedit_tag",
            format!("tag:{name}:ref={refstr}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_tag: invalid args ({e}). Optional: {{ \"name\": string, \"refstr\"?: string, \"list\"?: bool }}."
            ))
        })?;
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_tag: opening repo failed: {e}"))
        })?;

        if args.list {
            let tags = vc::list_tags(&repo)
                .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_tag: {e}")))?;
            return Ok(ToolOutput::text(
                serde_json::json!({
                    "tag_count": tags.len(),
                    "tags": tags,
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
                "vedit_tag: name is required unless list=true".into(),
            ));
        };
        let tag = vc::tag_ref(&repo, name, args.refstr.as_deref())
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_tag: {e}")))?;
        Ok(ToolOutput::text(
            serde_json::json!({
                "name": tag.name,
                "target": tag.target,
                "next_step": "Use vedit_show or vedit_diff with this tag name to inspect the checkpoint.",
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Create or list local named vedit checkpoints. Tags are stable labels \
under `.vedit/refs/tags/` that point at commits. They do not switch \
HEAD, create branches, or merge anything. Use this for names like \
`client-review-v1` or `before-tightening-pass`.\
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
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_minimal_otio(project_root: &std::path::Path) {
        let value = serde_json::json!({
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
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn creates_and_lists_tag() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path());
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "Initial", None).unwrap();

        let create = VeditTagTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_tag".into(),
                    args: serde_json::json!({"name": "client-review-v1"}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&create.content).unwrap();
        assert_eq!(parsed["name"].as_str(), Some("client-review-v1"));

        let list = VeditTagTool
            .handle(
                ToolInvocation {
                    call_id: "c2".into(),
                    name: "vedit_tag".into(),
                    args: serde_json::json!({"list": true}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&list.content).unwrap();
        assert_eq!(parsed["tag_count"].as_u64(), Some(1));
    }
}
