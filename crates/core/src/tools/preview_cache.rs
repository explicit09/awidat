//! Agent-callable preview-cache readiness tool.

use async_trait::async_trait;
use serde::Serialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Report proxy, thumbnail, and waveform preview-cache readiness.
pub struct PreviewCacheStatusTool;

#[derive(Debug, Serialize)]
struct PreviewCacheStatusResponse {
    #[serde(flatten)]
    summary: crate::preview_cache::PreviewCacheSummary,
    #[serde(flatten)]
    selection: crate::preview_cache::PreviewCacheRefreshSelection,
    refresh_execution: crate::preview_cache::PreviewCacheRefreshExecutionContract,
    refresh_lifecycle: Option<crate::preview_cache::PreviewCacheRefreshLifecycle>,
}

#[async_trait]
impl ToolHandler for PreviewCacheStatusTool {
    fn name(&self) -> &'static str {
        "preview_cache_status"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: "Report project preview-cache readiness and a bounded read-only refresh plan across proxies, thumbnails, and waveform sidecars.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project-relative asset id to select, such as raw/a.mov."
                    },
                    "proxy": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include proxy refresh tasks in the selected plan."
                    },
                    "thumbnails": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include thumbnail refresh tasks in the selected plan."
                    },
                    "waveform": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include waveform refresh tasks in the selected plan."
                    },
                    "max_tasks": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Optional maximum number of selected refresh tasks."
                    },
                    "persist_refresh_plan": {
                        "type": "boolean",
                        "default": false,
                        "description": "Persist the selected refresh plan to .awidat/preview-cache/refresh-plan.json without generating media."
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
        let options: crate::preview_cache::PreviewCacheSelectionOptions =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "preview_cache_status: invalid arguments: {e}"
                ))
            })?;
        let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "preview_cache_status: unable to scan preview cache: {e}"
                ))
            })?;
        let selection =
            crate::preview_cache::select_preview_cache_refresh_tasks(&summary, &options);
        let refresh_execution =
            crate::preview_cache::preview_cache_refresh_execution_contract(&selection);
        let refresh_lifecycle = if options.persist_refresh_plan {
            Some(
                crate::preview_cache::write_preview_cache_refresh_lifecycle(
                    &ctx.project_root,
                    &selection,
                )
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "preview_cache_status: write refresh lifecycle: {e}"
                    ))
                })?,
            )
        } else {
            None
        };
        let response = PreviewCacheStatusResponse {
            summary,
            selection,
            refresh_execution,
            refresh_lifecycle,
        };
        let body = serde_json::to_string(&response).map_err(|e| {
            FunctionCallError::Fatal(format!("preview_cache_status: serialize summary: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
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

    #[tokio::test]
    async fn preview_cache_status_reports_refresh_work() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let output = PreviewCacheStatusTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "preview_cache_status".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["asset_count"], 1);
        assert_eq!(body["refresh_work"]["proxy_count"], 1);
        assert_eq!(body["refresh_work"]["thumbnails_count"], 1);
        assert_eq!(body["refresh_work"]["waveform_count"], 1);
        assert_eq!(body["refresh_tasks"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn preview_cache_status_returns_bounded_selected_refresh_plan() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let output = PreviewCacheStatusTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "preview_cache_status".into(),
                    args: serde_json::json!({
                        "asset_id": "raw/a.mov",
                        "proxy": false,
                        "thumbnails": true,
                        "waveform": false,
                        "max_tasks": 1
                    }),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["refresh_tasks"].as_array().unwrap().len(), 3);
        assert_eq!(body["selected_task_count"], 1);
        assert_eq!(body["skipped_task_count"], 2);
        assert_eq!(body["selected_refresh_work"]["asset_count"], 1);
        assert_eq!(body["selected_refresh_work"]["proxy_count"], 0);
        assert_eq!(body["selected_refresh_work"]["thumbnails_count"], 1);
        assert_eq!(body["selected_refresh_work"]["waveform_count"], 0);
        assert_eq!(
            body["selected_refresh_tasks"][0]["artifact_kind"],
            "thumbnails"
        );
        assert_eq!(body["selected_refresh_tasks"][0]["asset_id"], "raw/a.mov");
    }

    #[tokio::test]
    async fn preview_cache_status_reports_executable_refresh_contract() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let output = PreviewCacheStatusTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "preview_cache_status".into(),
                    args: serde_json::json!({
                        "asset_id": "raw/a.mov",
                        "proxy": true,
                        "thumbnails": false,
                        "waveform": false,
                        "max_tasks": 1
                    }),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["selected_task_count"], 1);
        assert_eq!(
            body["refresh_execution"]["executor"],
            "desktop_preview_cache_refresh"
        );
        assert_eq!(body["refresh_execution"]["status"], "ready_to_start");
        assert_eq!(
            body["refresh_execution"]["artifact_policy"],
            "no_render_job_started"
        );
        assert_eq!(body["refresh_execution"]["selected_task_count"], 1);
        assert_eq!(
            body["refresh_execution"]["selected_refresh_work"]["proxy_count"],
            1
        );
        assert_eq!(
            body["refresh_execution"]["selected_task_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn preview_cache_status_can_persist_refresh_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let output = PreviewCacheStatusTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "preview_cache_status".into(),
                    args: serde_json::json!({
                        "asset_id": "raw/a.mov",
                        "max_tasks": 2,
                        "persist_refresh_plan": true
                    }),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let lifecycle_path = dir
            .path()
            .join(".awidat")
            .join("preview-cache")
            .join("refresh-plan.json");

        assert!(lifecycle_path.is_file());
        assert_eq!(body["refresh_lifecycle"]["status"], "planned");
        assert_eq!(
            body["refresh_lifecycle"]["artifact_policy"],
            "no_render_job_started"
        );
        assert_eq!(body["refresh_lifecycle"]["selected_task_count"], 2);
        assert_eq!(
            body["refresh_lifecycle"]["path"].as_str(),
            Some(lifecycle_path.to_string_lossy().as_ref())
        );
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(lifecycle_path).unwrap()).unwrap();
        assert_eq!(persisted["selected_task_count"], 2);
        assert_eq!(persisted["selected_task_ids"].as_array().unwrap().len(), 2);
    }
}
