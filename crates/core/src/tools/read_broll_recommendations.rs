//! `read_broll_recommendations` tool - scored B-roll opportunities.

use async_trait::async_trait;
use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use serde::Deserialize;
use serde::Serialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::broll_recommendations::build_broll_recommendation_package;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::understanding::build_understanding_package;

/// Read scored B-roll recommendations derived from fused understanding.
pub struct ReadBrollRecommendationsTool;

#[derive(Debug, Deserialize)]
struct ReadBrollRecommendationsArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    asset_id: Option<String>,
    /// Optional minimum recommendation score from 0..1.
    #[serde(default)]
    min_score: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ReadBrollRecommendationsResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    recommendation_count: usize,
    summary_for_agent: String,
    recommendations: awidat_proto::professional::BrollRecommendationPackage,
}

#[async_trait]
impl ToolHandler for ReadBrollRecommendationsTool {
    fn name(&self) -> &'static str {
        "read_broll_recommendations"
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
                    },
                    "min_score": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 1,
                        "description": "Optional minimum recommendation score from 0 to 1."
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
        let args: ReadBrollRecommendationsArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "read_broll_recommendations: invalid args ({e}). Expected optional asset_id string and min_score number."
                ))
            })?;
        let min_score = args.min_score.unwrap_or(0.0);
        if !(0.0..=1.0).contains(&min_score) {
            return Err(FunctionCallError::RespondToModel(
                "read_broll_recommendations: min_score must be between 0 and 1.".into(),
            ));
        }

        let asset_ids = asset_ids(&ctx.project_root, args.asset_id.as_deref())?;
        let understanding = build_understanding_package(&ctx.project_root, &asset_ids);
        let mut recommendations = build_broll_recommendation_package(&understanding.assets);
        filter_by_min_score(&mut recommendations, min_score);
        let recommendation_count = recommendations
            .assets
            .iter()
            .map(|asset| asset.recommendations.len())
            .sum();
        let response = ReadBrollRecommendationsResponse {
            status: "ok",
            project_root: ctx.project_root.to_string_lossy().into_owned(),
            asset_count: recommendations.assets.len(),
            recommendation_count,
            summary_for_agent: summarize_for_agent(
                recommendations.assets.len(),
                recommendation_count,
                min_score,
            ),
            recommendations,
        };
        let body = serde_json::to_string(&response).map_err(|e| {
            FunctionCallError::Fatal(format!(
                "read_broll_recommendations serialization failed: {e}"
            ))
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
            "read_broll_recommendations: unable to scan raw media: {e}"
        ))
    })?;
    Ok(files
        .into_iter()
        .map(|file| file.project_relative_path)
        .collect())
}

fn filter_by_min_score(
    package: &mut awidat_proto::professional::BrollRecommendationPackage,
    min_score: f64,
) {
    for asset in &mut package.assets {
        asset
            .recommendations
            .retain(|recommendation| recommendation.score >= min_score);
    }
}

fn summarize_for_agent(asset_count: usize, recommendation_count: usize, min_score: f64) -> String {
    if asset_count == 0 {
        return "No raw assets were found. Import media and run indexing before reading B-roll recommendations."
            .into();
    }
    format!(
        "{asset_count} asset(s), {recommendation_count} B-roll recommendation(s) at min_score {min_score:.2}."
    )
}

const DESCRIPTION: &str = "\
Read scored B-roll recommendations derived from fused understanding without \
side effects. Returns category, confidence score, asset strategy, insertion \
plan, rationale, score breakdown, and source evidence ids for review.";

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
    async fn read_broll_recommendations_returns_filtered_asset() {
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
                        {
                            "moment_id": "m1",
                            "kind": "explanation",
                            "start_s": 1.0,
                            "end_s": 10.0,
                            "score": 0.9,
                            "text": "Retention improved by 42 percent after the new pipeline."
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let whisper = dir.path().join("index/whisper/raw/a.mov.json");
        std::fs::create_dir_all(whisper.parent().unwrap()).unwrap();
        std::fs::write(
            whisper,
            serde_json::to_vec(&serde_json::json!({
                "data": {
                    "segments": [
                        {
                            "start_s": 1.0,
                            "end_s": 10.0,
                            "text": "Retention improved by 42 percent after the new pipeline."
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let output = ReadBrollRecommendationsTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "read_broll_recommendations".into(),
                    args: serde_json::json!({"asset_id": "raw/a.mov", "min_score": 0.7}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["asset_count"], 1);
        assert_eq!(body["recommendation_count"], 1);
        assert_eq!(
            body["recommendations"]["assets"][0]["recommendations"][0]["category"],
            "statistic"
        );
    }

    #[tokio::test]
    async fn read_broll_recommendations_rejects_invalid_min_score() {
        let dir = tempfile::tempdir().unwrap();
        let err = ReadBrollRecommendationsTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "read_broll_recommendations".into(),
                    args: serde_json::json!({"min_score": 2.0}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("min_score must be between 0 and 1")
        );
    }
}
