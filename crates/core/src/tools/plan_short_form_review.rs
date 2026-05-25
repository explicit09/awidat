//! `plan_short_form_review` agent tool.
//!
//! Read-only long-form to short-form review planner. It ranks complete
//! moments and returns draft EDL packages without applying them.

use async_trait::async_trait;
use awidat_index::{SidecarError, read_sidecar};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::short_form_review::{
    ShortFormReviewInput, ShortFormReviewOptions, build_short_form_review,
};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Read-only planner for ranked short-form review candidates.
pub struct PlanShortFormReviewTool;

#[derive(Debug, Deserialize)]
struct Args {
    asset_id: String,
    #[serde(default)]
    source_width: Option<u32>,
    #[serde(default)]
    source_height: Option<u32>,
    #[serde(default)]
    max_candidates: Option<usize>,
    #[serde(default)]
    max_duration_s: Option<f64>,
}

#[async_trait]
impl ToolHandler for PlanShortFormReviewTool {
    fn name(&self) -> &'static str {
        "plan_short_form_review"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_short_form_review".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Project-relative source asset id, e.g. raw/interview.mov."
                    },
                    "source_width": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional source media width in pixels. Defaults to 1920 if unknown."
                    },
                    "source_height": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional source media height in pixels. Defaults to 1080 if unknown."
                    },
                    "max_candidates": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "description": "Maximum ranked review candidates to return. Default 15."
                    },
                    "max_duration_s": {
                        "type": "number",
                        "minimum": 5,
                        "maximum": 300,
                        "description": "Maximum candidate duration. Default 300 seconds, allowing extended short-form when complete."
                    }
                },
                "required": ["asset_id"]
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
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_short_form_review: invalid args ({e}). Required: asset_id."
            ))
        })?;
        if args.asset_id.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_short_form_review: asset_id must be non-empty.".into(),
            ));
        }

        let asset = AssetId::new(args.asset_id.clone());
        let input = ShortFormReviewInput {
            asset_id: args.asset_id,
            source_width: args.source_width.unwrap_or(1920),
            source_height: args.source_height.unwrap_or(1080),
            transcript: sidecar_data(&ctx, "whisper", &asset)?,
            editorial_moments: sidecar_data(&ctx, "editorial-moments", &asset)?,
            audio_energy: sidecar_data(&ctx, "audio-energy", &asset)?,
            topics: sidecar_data(&ctx, "topic", &asset)?,
            scenes: sidecar_data(&ctx, "scenedetect", &asset)?,
            shot: sidecar_data(&ctx, "shot", &asset)?,
            face: sidecar_data(&ctx, "face", &asset)?,
            gaze: sidecar_data(&ctx, "gaze", &asset)?,
            frame_quality: sidecar_data(&ctx, "frame-quality", &asset)?,
            composition: sidecar_data(&ctx, "composition", &asset)?,
            broll_assets: sidecar_data(&ctx, "broll-candidates", &asset)?,
        };
        let review = build_short_form_review(
            input,
            ShortFormReviewOptions {
                max_candidates: args.max_candidates.unwrap_or(15).min(50),
                max_duration_s: args.max_duration_s.unwrap_or(300.0).min(300.0),
            },
        );
        let body = serde_json::to_string_pretty(&review).map_err(|e| {
            FunctionCallError::Fatal(format!("plan_short_form_review serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
}

fn sidecar_data(
    ctx: &ToolContext,
    indexer: &str,
    asset: &AssetId,
) -> Result<serde_json::Value, FunctionCallError> {
    match read_sidecar(&ctx.project_root, indexer, asset) {
        Ok(sidecar) => Ok(sidecar
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null)),
        Err(SidecarError::NotFound { .. }) => Ok(serde_json::Value::Null),
        Err(err) => Err(FunctionCallError::RespondToModel(format!(
            "plan_short_form_review: failed to read {indexer} sidecar: {err}"
        ))),
    }
}

const DESCRIPTION: &str = "\
Build a read-only long-form to short-form review plan for one asset. \
The tool ranks complete standalone candidate moments, allows extended \
short-form up to five minutes when the idea earns it, recommends B-roll \
by default when support visuals clarify the idea, plans speaker-aware \
9:16 layouts for wide long-form sources, and returns reviewable draft EDL \
packages plus title, caption, platform, confidence, and human review \
actions. It does not apply edits; use apply_edl separately after review so \
autopilot/co-pilot/manual approval behavior remains in control.\
";
