//! Read-only generated-media planning tool.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::tools::find_generated_broll_opportunities::asset_requests_for_moment;

/// Read-only planner for generated-media prompt candidates.
pub struct PlanGeneratedMediaTool;

#[derive(Debug, Deserialize)]
struct Args {
    moment: String,
    #[serde(default)]
    style: Option<String>,
}

#[async_trait]
impl ToolHandler for PlanGeneratedMediaTool {
    fn name(&self) -> &'static str {
        "plan_generated_media"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["moment"],
                "properties": {
                    "moment": { "type": "string" },
                    "style": { "type": "string" }
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
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_generated_media: invalid args ({e}). Required: moment."
            ))
        })?;
        let moment = args.moment.trim();
        if moment.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_generated_media: moment is empty.".into(),
            ));
        }
        let style = args.style.as_deref().unwrap_or("natural editorial");
        let asset_requests = asset_requests_for_moment(moment, moment);
        let candidates = vec![
            format!(
                "{style} b-roll cutaway illustrating {moment}, steady camera, realistic lighting"
            ),
            format!("{style} establishing shot for {moment}, no text, no logos"),
            format!("{style} detail shot that supports {moment}, documentary tone"),
        ];
        Ok(ToolOutput::text(
            serde_json::json!({
                "workflow_purpose": "broll",
                "artifact_kind": "video",
                "recommended_provider": "openrouter",
                "candidates": candidates,
                "options": {
                    "providers": ["openrouter", "mock"],
                    "default_model": crate::generated_media::openrouter::DEFAULT_VIDEO_MODEL,
                    "generation_modes": ["text_to_video"],
                    "note": "Use openrouter for US-accessible video generation. Use mock for offline tests."
                },
                "asset_requests": asset_requests,
                "fallback_note": "If requested assets are unavailable, use a generic unbranded prompt and avoid exact logos, readable UI text, or unapproved likeness."
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Read-only planner for generated b-roll. Returns deterministic prompt \
candidates and provider/mode options without writing the registry.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolHandler;

    #[tokio::test]
    async fn planner_returns_deterministic_broll_candidates() {
        let output = PlanGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "plan_generated_media".into(),
                    args: serde_json::json!({
                        "moment": "founder describes a crowded market",
                        "style": "documentary"
                    }),
                },
                crate::generated_media::test_support::ctx_at(tempfile::tempdir().unwrap().path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["workflow_purpose"], "broll");
        assert_eq!(body["artifact_kind"], "video");
        assert_eq!(body["recommended_provider"], "openrouter");
        assert_eq!(body["candidates"].as_array().unwrap().len(), 3);
        assert!(
            body["candidates"][0]
                .as_str()
                .unwrap()
                .contains("crowded market")
        );
    }

    #[tokio::test]
    async fn planner_surfaces_product_demo_asset_requests() {
        let output = PlanGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "plan_generated_media".into(),
                    args: serde_json::json!({
                        "moment": "product demo showing the app dashboard workflow"
                    }),
                },
                crate::generated_media::test_support::ctx_at(tempfile::tempdir().unwrap().path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let requests = body["asset_requests"].as_array().unwrap();
        assert!(
            requests
                .iter()
                .any(|request| request["kind"] == "screenshot")
        );
        assert!(requests.iter().any(|request| request["kind"] == "logo"));
    }
}
