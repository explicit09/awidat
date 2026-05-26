//! Agent-callable preview-cache refresh executor.
//!
//! Mutating sibling of `preview_cache_status`: this tool actually runs the
//! persisted lifecycle through `PreviewRefreshExecutor`, persisting per-task
//! state transitions to `.awidat/preview-cache/refresh-plan.json` as it goes.
//! Resume semantics are inherited from `run_preview_cache_refresh`: re-running
//! the tool against the same project picks up Pending/InProgress tasks and
//! skips already-`Completed`/`Skipped` ones.

use async_trait::async_trait;
use serde::Serialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Tool surface for executing the persisted preview-cache refresh lifecycle.
pub struct RunPreviewCacheRefreshTool;

#[derive(Debug, Serialize)]
struct RunPreviewCacheRefreshResponse {
    lifecycle: crate::preview_cache::PreviewCacheRefreshLifecycle,
    /// True when the executor was invoked. False when no tasks were selected
    /// (the lifecycle is reported but the caller can decide whether to retry).
    executed: bool,
}

#[async_trait]
impl ToolHandler for RunPreviewCacheRefreshTool {
    fn name(&self) -> &'static str {
        "run_preview_cache_refresh"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description:
                "Run the persisted preview-cache refresh lifecycle to completion using the ffmpeg-backed executor. Resumes from any prior pending/in-progress tasks and skips already-completed tasks."
                    .into(),
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
                        "description": "Include proxy refresh tasks."
                    },
                    "thumbnails": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include thumbnail refresh tasks."
                    },
                    "waveform": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include waveform refresh tasks."
                    },
                    "max_tasks": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Optional maximum number of selected refresh tasks per invocation."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn capability_metadata(
        &self,
        invocation: &ToolInvocation,
    ) -> crate::capability_metadata::CapabilityMetadata {
        let mut metadata = crate::capability_metadata::CapabilityMetadata::for_tool_name(
            self.name(),
            self.is_mutating(invocation),
        );
        metadata.preview_supported = crate::capability_metadata::SupportLevel::NotSupported;
        metadata.export_supported = crate::capability_metadata::SupportLevel::Supported;
        metadata.approval_required = true;
        metadata.side_effects = vec![
            "runs ffmpeg per refresh task to generate proxies, thumbnails, or waveform sidecars"
                .into(),
            "atomically updates `.awidat/preview-cache/refresh-plan.json` with per-task state"
                .into(),
        ];
        metadata.known_limitations = vec![
            "cross-process file locking is not yet implemented; concurrent invocations rely on the in-process busy guard"
                .into(),
        ];
        metadata
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let options: crate::preview_cache::PreviewCacheSelectionOptions =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "run_preview_cache_refresh: invalid arguments: {e}"
                ))
            })?;
        let summary = crate::preview_cache::build_preview_cache_summary(&ctx.project_root)
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "run_preview_cache_refresh: unable to scan preview cache: {e}"
                ))
            })?;
        let selection =
            crate::preview_cache::select_preview_cache_refresh_tasks(&summary, &options);

        if selection.selected_task_count == 0 {
            let lifecycle = crate::preview_cache::write_preview_cache_refresh_lifecycle(
                &ctx.project_root,
                &selection,
            )
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "run_preview_cache_refresh: write empty lifecycle: {e}"
                ))
            })?;
            let body = serde_json::to_string(&RunPreviewCacheRefreshResponse {
                lifecycle,
                executed: false,
            })
            .map_err(|e| {
                FunctionCallError::Fatal(format!(
                    "run_preview_cache_refresh: serialize lifecycle: {e}"
                ))
            })?;
            return Ok(ToolOutput::text(body));
        }

        let executor = crate::preview_refresh_executor::FfmpegPreviewRefreshExecutor::new(
            ctx.project_root.clone(),
        );
        let lifecycle = crate::preview_cache::run_preview_cache_refresh(
            &ctx.project_root,
            &selection,
            &executor,
        )
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!("run_preview_cache_refresh: {e}"))
        })?;

        let body = serde_json::to_string(&RunPreviewCacheRefreshResponse {
            lifecycle,
            executed: true,
        })
        .map_err(|e| {
            FunctionCallError::Fatal(format!(
                "run_preview_cache_refresh: serialize lifecycle: {e}"
            ))
        })?;
        Ok(ToolOutput::text(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schema_advertises_expected_properties() {
        let tool = RunPreviewCacheRefreshTool;
        let schema = tool.schema();
        assert_eq!(schema.name, "run_preview_cache_refresh");
        let props = schema.input_schema["properties"]
            .as_object()
            .expect("input_schema must declare properties");
        for key in ["asset_id", "proxy", "thumbnails", "waveform", "max_tasks"] {
            assert!(props.contains_key(key), "schema missing property: {key}");
        }
    }

    #[test]
    fn tool_advertises_mutating_default_with_approval() {
        let tool = RunPreviewCacheRefreshTool;
        let invocation = ToolInvocation {
            call_id: "test".into(),
            name: tool.name().into(),
            args: serde_json::json!({}),
        };
        assert!(tool.is_mutating(&invocation));
        let metadata = tool.capability_metadata(&invocation);
        assert!(metadata.approval_required);
        assert!(metadata.graph_mutates);
        assert!(
            metadata
                .known_limitations
                .iter()
                .any(|note| note.contains("cross-process file locking"))
        );
    }
}
