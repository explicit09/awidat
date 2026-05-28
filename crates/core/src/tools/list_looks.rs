//! `list_looks` — agent-facing catalog of named color-corrector looks.
//!
//! Reads `skills/color-corrector/looks.toml` (the Stage 4 catalog) and
//! returns its entries as structured JSON. The agent uses this to
//! enumerate options without having to know the on-disk path or
//! parse TOML itself; the Python planner consumes the same file at
//! `look_region_plan.py` boot.
//!
//! No filtering / pagination knobs: the catalog is small (under
//! 100 looks) and re-running the tool is cheap.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Project-relative path to the catalog. Kept in sync with
/// `scripts/look_region_plan.py:CATALOG_PATH` and the skill's
/// `SKILL.md` pointer.
const CATALOG_RELATIVE_PATH: &str = "skills/color-corrector/looks.toml";

/// The `list_looks` tool.
pub struct ListLooksTool;

#[derive(Debug, Deserialize)]
struct Args {}

#[derive(Debug, Deserialize, Serialize)]
struct CatalogFile {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    catalog_name: Option<String>,
    // TOML uses `[[look]]` arrays-of-tables, but JSON consumers
    // expect a plural `looks` key. Rename only on deserialize so
    // the structured output stays human-friendly.
    #[serde(default, rename(deserialize = "look"))]
    looks: Vec<CatalogLook>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CatalogLook {
    id: String,
    display_name: String,
    description: String,
    default_input_space: String,
    default_output_space: String,
    default_size: u32,
    recommended_strength_min: f64,
    recommended_strength_max: f64,
    #[serde(default)]
    tags: Vec<String>,
}

#[async_trait]
impl ToolHandler for ListLooksTool {
    fn name(&self) -> &'static str {
        "list_looks"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_looks".into(),
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
        let _: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!("list_looks: invalid args ({e})"))
        })?;
        let catalog = load_catalog(&ctx.project_root)?;
        let json = serde_json::to_string_pretty(&catalog).map_err(|e| {
            FunctionCallError::RespondToModel(format!("list_looks: catalog serialize failed: {e}"))
        })?;
        Ok(ToolOutput::text(json))
    }
}

fn load_catalog(project_root: &std::path::Path) -> Result<CatalogFile, FunctionCallError> {
    let path: PathBuf = project_root.join(CATALOG_RELATIVE_PATH);
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "list_looks: catalog unreadable at {}: {e}",
            path.display()
        ))
    })?;
    toml::from_str::<CatalogFile>(&raw).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "list_looks: catalog at {} is not valid TOML: {e}",
            path.display()
        ))
    })
}

const DESCRIPTION: &str = "\
Returns the agent-facing catalog of named color-corrector looks. \
Each entry includes id, display_name, description, \
default_input_space, default_output_space, default_size, \
recommended_strength_min, recommended_strength_max, and tags. Use \
this before composing an `awidat.lut` or `awidat.color_pipeline` \
effect to pick a look that's compatible with the clip's input \
space and apply a sensible strength.";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use serde_json::json;
    use std::path::Path;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
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
            name: "list_looks".into(),
            args,
        }
    }

    fn write_minimal_catalog(dir: &Path) {
        std::fs::create_dir_all(dir.join("skills/color-corrector")).unwrap();
        std::fs::write(
            dir.join("skills/color-corrector/looks.toml"),
            r#"
schema_version = 1
catalog_name = "test"

[[look]]
id = "natural_balance"
display_name = "Natural Balance"
description = "Subtle baseline."
default_input_space = "rec709_g24"
default_output_space = "rec709_g24"
default_size = 17
recommended_strength_min = 0.7
recommended_strength_max = 1.0
tags = ["safe", "default"]
"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn returns_catalog_json() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_catalog(dir.path());
        let out = ListLooksTool
            .handle(invoke(json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let looks = body["looks"].as_array().expect("looks array");
        assert_eq!(looks.len(), 1);
        assert_eq!(looks[0]["id"], "natural_balance");
        assert_eq!(looks[0]["default_input_space"], "rec709_g24");
    }

    #[tokio::test]
    async fn missing_catalog_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ListLooksTool
            .handle(invoke(json!({})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(ref msg) if msg.contains("unreadable")),
            "expected respond-to-model error for missing catalog, got {err:?}",
        );
    }

    #[tokio::test]
    async fn malformed_toml_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("skills/color-corrector")).unwrap();
        std::fs::write(
            dir.path().join("skills/color-corrector/looks.toml"),
            "this = is not [valid toml",
        )
        .unwrap();
        let err = ListLooksTool
            .handle(invoke(json!({})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(ref msg) if msg.contains("not valid TOML")),
            "expected TOML parse error message, got {err:?}",
        );
    }
}
