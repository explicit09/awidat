//! `read_understanding` tool - fused understanding and clip candidates.

use async_trait::async_trait;
use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use serde::Deserialize;
use serde::Serialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::clip_candidates::build_clip_candidate_package;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::understanding::build_understanding_package;

/// Read fused understanding and reviewable clip candidates.
pub struct ReadUnderstandingTool;

#[derive(Debug, Deserialize)]
struct ReadUnderstandingArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadUnderstandingResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    moment_count: usize,
    clip_candidate_count: usize,
    summary_for_agent: String,
    understanding: awidat_proto::professional::UnderstandingPackage,
    clip_candidates: awidat_proto::professional::ClipCandidatePackage,
}

#[async_trait]
impl ToolHandler for ReadUnderstandingTool {
    fn name(&self) -> &'static str {
        "read_understanding"
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
        let args: ReadUnderstandingArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "read_understanding: invalid args ({e}). Expected optional asset_id string."
            ))
        })?;
        let asset_ids = asset_ids(&ctx.project_root, args.asset_id.as_deref())?;
        let understanding = build_understanding_package(&ctx.project_root, &asset_ids);
        let clip_candidates = build_clip_candidate_package(&understanding.assets);
        let moment_count = understanding
            .assets
            .iter()
            .map(|asset| asset.moments.len())
            .sum();
        let clip_candidate_count = clip_candidates
            .assets
            .iter()
            .map(|asset| asset.candidates.len())
            .sum();
        let response = ReadUnderstandingResponse {
            status: "ok",
            project_root: ctx.project_root.to_string_lossy().into_owned(),
            asset_count: understanding.assets.len(),
            moment_count,
            clip_candidate_count,
            summary_for_agent: summarize_for_agent(
                understanding.assets.len(),
                moment_count,
                clip_candidate_count,
            ),
            understanding,
            clip_candidates,
        };
        let body = serde_json::to_string(&response).map_err(|e| {
            FunctionCallError::Fatal(format!("read_understanding serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
}

fn asset_ids(
    project_root: &std::path::Path,
    only_asset: Option<&str>,
) -> Result<Vec<String>, FunctionCallError> {
    if let Some(asset_id) = only_asset {
        return Ok(vec![asset_id.to_string()]);
    }
    let files = collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: None,
        },
    )
    .map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "read_understanding: unable to scan raw media: {e}"
        ))
    })?;
    Ok(files
        .into_iter()
        .map(|file| file.project_relative_path)
        .collect())
}

fn summarize_for_agent(asset_count: usize, moment_count: usize, candidate_count: usize) -> String {
    if asset_count == 0 {
        return "No raw assets were found. Import media and run indexing before reading understanding."
            .into();
    }
    format!(
        "{asset_count} asset(s), {moment_count} fused moment(s), {candidate_count} reviewable clip candidate(s)."
    )
}

const DESCRIPTION: &str = "\
Read consolidated scene/moment understanding and derived short-form clip \
candidates without side effects. Fuses existing transcript, scene, topic, \
audio-energy, and editorial-moment sidecars, then returns reviewable clip \
candidates with scores, explanations, evidence ids, and one-click assembly \
metadata.";

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
    async fn read_understanding_returns_filtered_asset() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw/a.mov");
        std::fs::create_dir_all(raw.parent().unwrap()).unwrap();
        std::fs::write(raw, b"media").unwrap();
        let path = dir.path().join("index/editorial-moments/raw/a.mov.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "data": {
                    "moments": [
                        {"moment_id": "m1", "kind": "hook", "start_s": 1.0, "end_s": 20.0, "score": 0.9}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let output = ReadUnderstandingTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "read_understanding".into(),
                    args: serde_json::json!({"asset_id": "raw/a.mov"}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["asset_count"], 1);
        assert_eq!(body["moment_count"], 1);
        assert_eq!(body["clip_candidate_count"], 1);
        assert_eq!(body["understanding"]["assets"][0]["asset_id"], "raw/a.mov");
    }
}
