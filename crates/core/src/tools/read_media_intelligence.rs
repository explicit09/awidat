//! `read_media_intelligence` tool - durable progressive intelligence state.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Read progressive per-asset media intelligence state.
pub struct ReadMediaIntelligenceTool;

#[derive(Debug, Deserialize)]
struct ReadMediaIntelligenceArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadMediaIntelligenceResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    ready_asset_count: usize,
    processing_asset_count: usize,
    blocked_asset_count: usize,
    offline_asset_count: usize,
    summary_for_agent: String,
    package: awidat_proto::professional::MediaIntelligencePackage,
}

#[async_trait]
impl ToolHandler for ReadMediaIntelligenceTool {
    fn name(&self) -> &'static str {
        "read_media_intelligence"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project-relative source asset id, such as raw/interview.mov. Omit to inspect every raw project asset."
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
        let args: ReadMediaIntelligenceArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "read_media_intelligence: invalid args ({e}). Expected optional asset_id string."
                ))
            })?;
        let mut package =
            crate::media_intelligence::build_media_intelligence_package(&ctx.project_root)
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "read_media_intelligence: unable to scan project media: {e}"
                    ))
                })?;

        if let Some(asset_id) = args.asset_id.as_deref() {
            package.assets.retain(|asset| asset.asset_id == asset_id);
            if package.assets.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "read_media_intelligence: asset '{asset_id}' was not found under project raw media. Use list_assets/import_media first, or relink the source before inspecting progressive state."
                )));
            }
        }

        let ready_asset_count = count_state(
            &package,
            awidat_proto::professional::MediaIntelligenceAggregateState::Ready,
        );
        let processing_asset_count = count_state(
            &package,
            awidat_proto::professional::MediaIntelligenceAggregateState::Processing,
        );
        let blocked_asset_count = count_state(
            &package,
            awidat_proto::professional::MediaIntelligenceAggregateState::Blocked,
        );
        let offline_asset_count = count_state(
            &package,
            awidat_proto::professional::MediaIntelligenceAggregateState::Offline,
        );
        let response = ReadMediaIntelligenceResponse {
            status: response_status(
                processing_asset_count,
                blocked_asset_count,
                offline_asset_count,
            ),
            project_root: ctx.project_root.to_string_lossy().into_owned(),
            asset_count: package.assets.len(),
            ready_asset_count,
            processing_asset_count,
            blocked_asset_count,
            offline_asset_count,
            summary_for_agent: summarize_for_agent(
                package.assets.len(),
                ready_asset_count,
                processing_asset_count,
                blocked_asset_count,
                offline_asset_count,
            ),
            package,
        };
        let body = serde_json::to_string(&response).map_err(|e| {
            FunctionCallError::Fatal(format!("read_media_intelligence serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
}

fn count_state(
    package: &awidat_proto::professional::MediaIntelligencePackage,
    state: awidat_proto::professional::MediaIntelligenceAggregateState,
) -> usize {
    package
        .assets
        .iter()
        .filter(|asset| asset.aggregate_state == state)
        .count()
}

fn response_status(processing: usize, blocked: usize, offline: usize) -> &'static str {
    if offline > 0 || blocked > 0 {
        "blocked"
    } else if processing > 0 {
        "processing"
    } else {
        "ok"
    }
}

fn summarize_for_agent(
    total: usize,
    ready: usize,
    processing: usize,
    blocked: usize,
    offline: usize,
) -> String {
    if total == 0 {
        return "No raw media assets were found. Import media before building progressive intelligence."
            .into();
    }
    if offline > 0 || blocked > 0 {
        return format!(
            "{offline} offline and {blocked} blocked asset(s) need repair before downstream intelligence can be trusted."
        );
    }
    if processing > 0 {
        return format!("{processing} of {total} asset(s) have processing layers in flight.");
    }
    format!("{ready} of {total} asset(s) have every intelligence layer ready.")
}

const DESCRIPTION: &str = "\
Read the progressive intelligence state machine for raw media assets without \
side effects. Returns independent layer readiness for source, proxy, waveform, \
transcript, speakers, scenes, topics, moments, clip candidates, and b-roll, \
plus aggregate state and next actions.";

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
    async fn read_media_intelligence_reports_layer_state() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"media").unwrap();

        let output = ReadMediaIntelligenceTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "read_media_intelligence".into(),
                    args: serde_json::json!({ "asset_id": "raw/a.mov" }),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["asset_count"], 1);
        assert_eq!(body["package"]["assets"][0]["asset_id"], "raw/a.mov");
        assert_eq!(body["package"]["assets"][0]["layers"][0]["kind"], "source");
    }
}
