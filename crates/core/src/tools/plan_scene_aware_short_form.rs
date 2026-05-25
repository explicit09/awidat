//! `plan_scene_aware_short_form` agent tool.
//!
//! Read-only planner for scene-aware vertical short-form edits. The tool
//! gathers existing sidecars, delegates decision logic to
//! `scene_aware_short_form`, and returns structured recommendations plus
//! an EDL fragment for review.

use async_trait::async_trait;
use awidat_index::{SidecarError, read_sidecar};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::scene_aware_short_form::{SceneAwareShortFormInput, build_scene_aware_short_form_plan};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Read-only scene-aware short-form planner.
pub struct PlanSceneAwareShortFormTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Project-relative source asset id, e.g. raw/interview.mov.
    asset_id: String,
    /// Timeline clip uuid/name to use in EDL anchors.
    clip_id: String,
    /// Source media width in pixels.
    source_width: u32,
    /// Source media height in pixels.
    source_height: u32,
}

#[async_trait]
impl ToolHandler for PlanSceneAwareShortFormTool {
    fn name(&self) -> &'static str {
        "plan_scene_aware_short_form"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_scene_aware_short_form".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Project-relative source asset id, e.g. raw/interview.mov."
                    },
                    "clip_id": {
                        "type": "string",
                        "description": "Timeline clip uuid/name to use in reviewable EDL anchors."
                    },
                    "source_width": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Source media width in pixels."
                    },
                    "source_height": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Source media height in pixels."
                    }
                },
                "required": ["asset_id", "clip_id", "source_width", "source_height"]
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
                "plan_scene_aware_short_form: invalid args ({e}). Required: asset_id, clip_id, source_width, source_height."
            ))
        })?;
        if args.asset_id.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_scene_aware_short_form: asset_id must be non-empty.".into(),
            ));
        }
        if args.clip_id.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_scene_aware_short_form: clip_id must be non-empty.".into(),
            ));
        }

        let asset = AssetId::new(args.asset_id.clone());
        let input = SceneAwareShortFormInput {
            asset_id: args.asset_id,
            clip_id: args.clip_id,
            source_width: args.source_width,
            source_height: args.source_height,
            transcript: sidecar_data(&ctx, "whisper", &asset)?,
            topics: sidecar_data(&ctx, "topic", &asset)?,
            editorial_moments: sidecar_data(&ctx, "editorial-moments", &asset)?,
            audio_energy: sidecar_data(&ctx, "audio-energy", &asset)?,
            face: sidecar_data(&ctx, "face", &asset)?,
            gaze: sidecar_data(&ctx, "gaze", &asset)?,
            scenes: sidecar_data(&ctx, "scenedetect", &asset)?,
            shot: sidecar_data(&ctx, "shot", &asset)?,
            frame_quality: sidecar_data(&ctx, "frame-quality", &asset)?,
            composition: sidecar_data(&ctx, "composition", &asset)?,
            clip: sidecar_data(&ctx, "clip", &asset)?,
        };
        let plan = build_scene_aware_short_form_plan(input);
        let body = serde_json::to_string_pretty(&plan)
            .map_err(|e| FunctionCallError::Fatal(format!("plan serialization failed: {e}")))?;
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
            "plan_scene_aware_short_form: failed to read {indexer} sidecar: {err}"
        ))),
    }
}

const DESCRIPTION: &str = "\
Build a read-only scene-aware short-form edit plan for one candidate clip. \
The tool uses existing Awidat evidence sidecars when available: transcript, \
word timings, topics, editorial moments, audio energy, face/gaze, scene and \
shot detection, frame quality, composition, and CLIP metadata. It analyzes \
shot layout, caption safety, negative space, motion intensity, weak visuals, \
and semantic support-visual opportunities, then returns structured \
recommendations with transcript, visual, pacing, safety, and confidence \
reasons plus an EDL fragment. The returned EDL is reviewable and should be \
applied separately with apply_edl only after inspection.\
";

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
            name: "plan_scene_aware_short_form".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, data: serde_json::Value) {
        let path = root
            .join("index")
            .join(indexer)
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": indexer,
                "asset_id": asset,
                "data": data,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn reads_sidecars_and_returns_scene_aware_plan() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/demo.mp4";
        write_sidecar(
            dir.path(),
            "whisper",
            asset,
            serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 1.4,
                    "text": "Watch this",
                    "words": [{"start_s": 0.0, "end_s": 0.4, "text": "Watch"}]
                }]
            }),
        );
        write_sidecar(
            dir.path(),
            "face",
            asset,
            serde_json::json!({
                "per_frame": [{
                    "t_s": 0.5,
                    "faces": [{"confidence": 0.99, "x": 0.36, "y": 0.48, "w": 0.24, "h": 0.34}]
                }]
            }),
        );
        write_sidecar(
            dir.path(),
            "composition",
            asset,
            serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 1.5,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.8}
                }]
            }),
        );

        let out = PlanSceneAwareShortFormTool
            .handle(
                invoke(serde_json::json!({
                    "asset_id": asset,
                    "clip_id": "clip-1",
                    "source_width": 1920,
                    "source_height": 1080
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["caption_plan"][0]["placement"], "upper");
        assert_eq!(body["caption_plan"][0]["word_timings"][0]["text"], "Watch");
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("*** Insert Caption")
        );
    }
}
