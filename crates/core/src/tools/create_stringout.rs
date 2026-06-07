//! `create_stringout` tool — append a new named stringout.
//!
//! Slice C2 (wave3-bin-aware). Persists a new
//! [`montage_proto::professional::Stringout`] entry into
//! [`montage_proto::montage_meta::MontageTimelineMetadata::stringouts`]
//! without disturbing any existing ones. Multiple parallel stringouts
//! (per arc, alt-cut, cold-open, etc.) are now first-class.
//!
//! Mutating tool. Approval-keyed by stringout id.

use async_trait::async_trait;
use montage_proto::professional::Stringout;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::media_catalog_mutation::ensure_montage_metadata;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `create_stringout` tool.
pub struct CreateStringoutTool;

#[derive(Debug, Deserialize)]
struct CreateStringoutArgs {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Ordered select ids the stringout points at.
    #[serde(default)]
    items: Vec<String>,
}

#[async_trait]
impl ToolHandler for CreateStringoutTool {
    fn name(&self) -> &'static str {
        "create_stringout"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "create_stringout".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {"type": "string", "description": "Stable stringout id."},
                    "name": {"type": "string", "description": "Optional display name."},
                    "items": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Ordered select ids."
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
        vec![ApprovalKey::new(
            self.name(),
            format!("stringout:{}", invocation.args),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: CreateStringoutArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "create_stringout: invalid args ({e}). 'id' is required."
            ))
        })?;
        let id = args.id.trim();
        if id.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "create_stringout: id must not be empty".into(),
            ));
        }
        let id = id.to_string();

        let mut project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "create_stringout: unable to read project: {e}"
            ))
        })?;

        let meta = ensure_montage_metadata(&mut project.timeline);
        if meta.stringouts.iter().any(|s| s.id == id) {
            return Err(FunctionCallError::RespondToModel(format!(
                "create_stringout: stringout {id} already exists"
            )));
        }
        let stringout = Stringout {
            id: id.clone(),
            name: args.name,
            select_ids: args.items,
        };
        let item_count = stringout.select_ids.len();
        meta.stringouts.push(stringout);

        project.write(&ctx.project_root).map_err(|e| {
            FunctionCallError::Fatal(format!("create_stringout: unable to write project: {e}"))
        })?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "status": "created",
                "id": id,
                "items": item_count,
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Create a new named stringout (ordered select-collection) in the \
project. Projects support multiple parallel stringouts (per arc, \
alt-cut, cold-open) — calling this never replaces an existing one. \
'id' is required; 'name' is an optional display label; 'items' is an \
ordered list of select ids the stringout points at. Returns an error \
if a stringout with that id already exists.\
";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use montage_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;
    use crate::tool::SandboxMode;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "create_stringout".into(),
            args,
        }
    }

    #[tokio::test]
    async fn creates_stringout_and_persists_to_metadata() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();

        CreateStringoutTool
            .handle(
                invoke(serde_json::json!({
                    "id": "stringout-a",
                    "name": "Arc A",
                    "items": ["s-1", "s-2"]
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let stringouts = &project
            .timeline
            .metadata
            .montage
            .as_ref()
            .unwrap()
            .stringouts;
        assert_eq!(stringouts.len(), 1);
        assert_eq!(stringouts[0].id, "stringout-a");
        assert_eq!(stringouts[0].name.as_deref(), Some("Arc A"));
        assert_eq!(stringouts[0].select_ids, vec!["s-1", "s-2"]);
    }

    #[tokio::test]
    async fn rejects_duplicate_id() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        CreateStringoutTool
            .handle(invoke(serde_json::json!({"id": "x"})), ctx_at(dir.path()))
            .await
            .unwrap();
        let err = CreateStringoutTool
            .handle(invoke(serde_json::json!({"id": "x"})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            FunctionCallError::RespondToModel(m) if m.contains("already exists")
        ));
    }
}
