//! `find_black_frames` tool - run FFmpeg blackdetect over one source asset.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_PICTURE_BLACK_RATIO_TH: f64 = 0.98;
const DEFAULT_MIN_DURATION_S: f64 = 0.2;

/// The `find_black_frames` tool.
pub struct FindBlackFramesTool;

#[derive(Debug, Deserialize)]
struct FindBlackFramesArgs {
    /// Project-relative source asset path.
    asset: String,
    /// FFmpeg blackdetect picture-black ratio threshold.
    #[serde(default)]
    picture_black_ratio_th: Option<f64>,
    /// Minimum black range duration in seconds.
    #[serde(default)]
    min_duration_s: Option<f64>,
}

#[async_trait]
impl ToolHandler for FindBlackFramesTool {
    fn name(&self) -> &'static str {
        "find_black_frames"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_black_frames".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset": {
                        "type": "string",
                        "description": "Project-relative source asset path, for example raw/interview.mp4."
                    },
                    "picture_black_ratio_th": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "FFmpeg blackdetect picture-black ratio threshold. Default 0.98."
                    },
                    "min_duration_s": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "description": "Minimum black-frame range duration in seconds. Default 0.2."
                    }
                },
                "required": ["asset"]
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
        let args: FindBlackFramesArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "find_black_frames: invalid args ({e}). Required: {{ \"asset\": <project-relative path> }}."
                ))
            })?;
        validate_asset(&args.asset)?;
        let asset_path = ctx.project_root.join(&args.asset);
        if !asset_path.exists() {
            return Err(FunctionCallError::RespondToModel(format!(
                "find_black_frames: asset '{}' not found at {}",
                args.asset,
                asset_path.display()
            )));
        }

        let picture_black_ratio_th = args
            .picture_black_ratio_th
            .unwrap_or(DEFAULT_PICTURE_BLACK_RATIO_TH);
        let min_duration_s = args.min_duration_s.unwrap_or(DEFAULT_MIN_DURATION_S);
        let ranges = awidat_render::generate_black_frames(
            &asset_path,
            picture_black_ratio_th,
            min_duration_s,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_black_frames: ffmpeg blackdetect failed: {e}"
            ))
        })?;

        let body = serde_json::json!({
            "asset": args.asset,
            "picture_black_ratio_th": picture_black_ratio_th,
            "min_duration_s": min_duration_s,
            "ranges": ranges.iter().map(|range| serde_json::json!({
                "start_s": range.start_s,
                "end_s": range.end_s,
                "duration_s": range.duration_s,
            })).collect::<Vec<_>>(),
            "range_count": ranges.len(),
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn validate_asset(asset: &str) -> Result<(), FunctionCallError> {
    if asset.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "find_black_frames: asset must not be empty".into(),
        ));
    }
    if asset.contains("..") {
        return Err(FunctionCallError::RespondToModel(format!(
            "find_black_frames: asset '{asset}' must not contain '..' segments"
        )));
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Detect black-frame ranges in one project source asset using FFmpeg \
blackdetect. Use this as a quality/eval inspection tool after renders or \
before accepting suspicious cuts; it is read-only and returns source-time \
ranges with start_s, end_s, and duration_s.";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

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

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "find_black_frames".into(),
            args,
        }
    }

    #[tokio::test]
    async fn missing_asset_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = FindBlackFramesTool
            .handle(
                invoke(serde_json::json!({"asset": "raw/missing.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            FunctionCallError::RespondToModel(msg) if msg.contains("not found")
        ));
    }

    #[tokio::test]
    async fn dotdot_asset_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = FindBlackFramesTool
            .handle(
                invoke(serde_json::json!({"asset": "../outside.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            FunctionCallError::RespondToModel(msg) if msg.contains("must not contain '..'")
        ));
    }

    #[test]
    fn tool_is_read_only() {
        assert!(!FindBlackFramesTool.is_mutating(&invoke(serde_json::json!({
            "asset": "raw/a.mp4"
        }))));
    }
}
