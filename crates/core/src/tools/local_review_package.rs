//! `author_local_review_package` tool — create a local review package
//! from a rendered output path and the latest vedit commit metadata.
//!
//! This is intentionally local-first: it writes a JSON artifact under
//! `<project>/.awidat/review-packages/` and does not attempt third-party
//! sync or ingestion.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::review::build_local_review_package;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Build and persist a local review package.
pub struct LocalReviewPackageTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Path to a rendered review asset (absolute or project-relative).
    render_path: String,
    /// Optional package tags.
    #[serde(default)]
    tags: Vec<String>,
}

#[async_trait]
impl ToolHandler for LocalReviewPackageTool {
    fn name(&self) -> &'static str {
        "author_local_review_package"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "author_local_review_package".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "render_path": {
                        "type": "string",
                        "description": "Rendered review asset to reference. Absolute or project-relative to `.awidat` project root."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for discoverability and manual filtering."
                    }
                },
                "required": ["render_path"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "author_local_review_package: invalid args ({e}). Required: {{ \"render_path\": string }}. Optional: tags."
            ))
        })?;
        if args.render_path.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "author_local_review_package: render_path cannot be empty".into(),
            ));
        }

        let package = build_local_review_package(&ctx.project_root, &args.render_path, args.tags)
            .map_err(|e| {
            FunctionCallError::RespondToModel(format!("author_local_review_package: {e}"))
        })?;
        let body = serde_json::to_string_pretty(&package).map_err(|e| {
            FunctionCallError::Fatal(format!(
                "author_local_review_package: failed to serialize package: {e}"
            ))
        })?;
        Ok(ToolOutput::text(body))
    }
}

const DESCRIPTION: &str = "\
Author a local review package from a rendered output path. The package links that \
asset to the latest vedit commit, including commit header, commit hash, timeline hash, \
generated time, tags, and the commit reasoning body. The package is written as JSON \
under `<project>/.awidat/review-packages/` and returned as a JSON object.
\
If you are handing off a review render to a collaborator, use this tool before \
you share the file manually; third-party review APIs are intentionally not part of \
this local-only flow.
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolContext, ToolInvocation};
    use std::sync::Arc;
    use tokio::sync::broadcast;

    use std::path::PathBuf;
    use tempfile::tempdir;
    use tokio::fs;

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

    async fn commit_and_render(project_root: &std::path::Path) -> PathBuf {
        use crate::vc;
        write_minimal_otio(project_root);
        let repo = vc::open_or_init(project_root).unwrap();
        vc::commit_current_timeline(
            &repo,
            "Test review package header",
            Some("Package for testing"),
        )
        .unwrap();
        let render = project_root.join("renders").join("review.mp4");
        fs::create_dir_all(render.parent().unwrap()).await.unwrap();
        fs::write(&render, b"render asset").await.unwrap();
        render
    }

    fn ctx_at(project_root: std::path::PathBuf) -> ToolContext {
        let (events_tx, _) = broadcast::channel(16);
        ToolContext {
            project_root,
            events_tx,
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

    #[tokio::test]
    async fn returns_a_written_review_package_on_success() {
        let dir = tempdir().unwrap();
        let render = commit_and_render(dir.path()).await;

        let ctx = ctx_at(dir.path().to_path_buf());

        let out = LocalReviewPackageTool
            .handle(
                ToolInvocation {
                    call_id: "call".into(),
                    name: "author_local_review_package".into(),
                    args: serde_json::json!({
                        "render_path": render.display().to_string(),
                        "tags": ["review", "handoff", "review"]
                    }),
                },
                ctx,
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            parsed["commit_header"].as_str().unwrap(),
            "Test review package header"
        );
        assert_eq!(parsed["tags"][0].as_str().unwrap(), "review");
        assert_eq!(parsed["tags"].as_array().unwrap().len(), 2);
        assert_eq!(
            parsed["render_path"].as_str().unwrap(),
            render.to_str().unwrap()
        );
        assert!(parsed["package_path"].as_str().unwrap().contains("review-"));
        assert!(!parsed["reasoning_body"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn requires_render_path() {
        let dir = tempdir().unwrap();
        write_minimal_otio(dir.path());

        let ctx = ctx_at(dir.path().to_path_buf());

        let err = LocalReviewPackageTool
            .handle(
                ToolInvocation {
                    call_id: "call".into(),
                    name: "author_local_review_package".into(),
                    args: serde_json::json!({
                        "render_path": "   "
                    }),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            crate::FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("render_path cannot be empty"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
