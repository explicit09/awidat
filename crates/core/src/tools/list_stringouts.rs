//! `list_stringouts` tool — enumerate ordered select-collections.
//!
//! Slice C2 (wave3-bin-aware). A project can now keep multiple
//! [`awidat_proto::professional::Stringout`] entries (e.g. one per
//! arc, cold-open, alt-cut) instead of being limited to a single
//! ordered list. This tool returns them in the order they appear in
//! [`awidat_proto::awidat_meta::AwidatTimelineMetadata::stringouts`].
//!
//! Read-only.

use async_trait::async_trait;
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `list_stringouts` tool.
pub struct ListStringoutsTool;

#[derive(Debug, Deserialize)]
struct ListStringoutsArgs {}

#[async_trait]
impl ToolHandler for ListStringoutsTool {
    fn name(&self) -> &'static str {
        "list_stringouts"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_stringouts".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
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
        let _args: ListStringoutsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_stringouts: invalid args ({e}). No arguments expected."
            ))
        })?;

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_stringouts: unable to read project: {e}"
            ))
        })?;
        let stringouts: Vec<_> = project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .map(|meta| meta.stringouts.clone())
            .unwrap_or_default();

        let total = stringouts.len();
        let mut out = format!("total={total}\n");
        for (i, s) in stringouts.iter().enumerate() {
            let idx = i + 1;
            let name = s.name.as_deref().unwrap_or("");
            out.push_str(&format!(
                "{idx:>3}. id={id} name=\"{name}\" items={count}\n",
                idx = idx,
                id = s.id,
                name = name,
                count = s.select_ids.len(),
            ));
        }
        Ok(ToolOutput::text(out))
    }
}

const DESCRIPTION: &str = "\
List the project's named stringouts (ordered select-collections). \
Each stringout has a stable id, an optional display name, and a count \
of ordered select ids it references. Use `create_stringout` to add a \
new one without disturbing existing ones — projects support multiple \
stringouts in parallel (e.g. per arc, alt-cut, cold-open).\
";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use awidat_proto::professional::Stringout;
    use awidat_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;
    use crate::media_catalog_mutation::ensure_awidat_metadata;
    use crate::tool::SandboxMode;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke() -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "list_stringouts".into(),
            args: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn empty_project_shows_total_zero() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let out = ListStringoutsTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("total=0"));
    }

    #[tokio::test]
    async fn preserves_insertion_order_and_item_counts() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let meta = ensure_awidat_metadata(&mut project.timeline);
        meta.stringouts.push(Stringout {
            id: "stringout-a".into(),
            name: Some("Arc A".into()),
            select_ids: vec!["s-1".into(), "s-2".into()],
        });
        meta.stringouts.push(Stringout {
            id: "stringout-b".into(),
            name: None,
            select_ids: vec!["s-3".into()],
        });
        project.write(dir.path()).unwrap();

        let out = ListStringoutsTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("total=2"));
        let a_pos = out.content.find("stringout-a").unwrap();
        let b_pos = out.content.find("stringout-b").unwrap();
        assert!(a_pos < b_pos, "expected insertion order preserved");
        assert!(out.content.contains("items=2"));
        assert!(out.content.contains("items=1"));
    }
}
