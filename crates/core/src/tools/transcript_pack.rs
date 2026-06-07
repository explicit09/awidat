//! `transcript_pack` tool — compact transcript evidence for planning.

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::transcript_pack::{
    DEFAULT_MAX_WORDS_PER_ASSET, MAX_WORDS_PER_ASSET, TranscriptPackOptions,
    build_timeline_transcript_pack, build_transcript_pack,
};

/// Build a compact transcript pack from whisper sidecars.
pub struct TranscriptPackTool;

#[derive(Debug, Deserialize)]
struct TranscriptPackArgs {
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    max_words_per_asset: Option<usize>,
    #[serde(default)]
    include_all_assets: bool,
}

#[async_trait]
impl ToolHandler for TranscriptPackTool {
    fn name(&self) -> &'static str {
        "transcript_pack"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "transcript_pack".into(),
            description: "Build a compact transcript pack from whisper sidecars for speech-led cut, caption, and cleanup planning.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project asset id to include. Omit to pack every whisper transcript sidecar."
                    },
                    "max_words_per_asset": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_WORDS_PER_ASSET,
                        "description": "Maximum word-timing entries per asset. Defaults to 120."
                    },
                    "include_all_assets": {
                        "type": "boolean",
                        "description": "When true, pack matching whisper sidecars without filtering to the current timeline. Defaults to false."
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
        let args: TranscriptPackArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "transcript_pack: invalid args ({e}). All fields are optional."
            ))
        })?;
        let options = TranscriptPackOptions {
            asset_id: args.asset_id,
            max_words_per_asset: args
                .max_words_per_asset
                .or(Some(DEFAULT_MAX_WORDS_PER_ASSET)),
        };
        let pack = if args.include_all_assets {
            build_transcript_pack(&ctx.project_root, options)
        } else {
            let project = Project::read(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!("transcript_pack: read project: {e}"))
            })?;
            build_timeline_transcript_pack(&ctx.project_root, &project.timeline, options)
        }
        .map_err(|e| FunctionCallError::RespondToModel(format!("transcript_pack: {e}")))?;
        let content = serde_json::to_string(&pack).map_err(|e| {
            FunctionCallError::RespondToModel(format!("transcript_pack: encode output: {e}"))
        })?;
        Ok(ToolOutput::text(content))
    }
}

#[cfg(test)]
mod tests {
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
        Timeline, Track, TrackChild, TrackKind,
    };
    use tokio::sync::broadcast;

    use super::*;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invocation(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "transcript_pack".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, asset_id: &str, body: serde_json::Value) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
    }

    fn timeline_with_clip(asset_id: &str, source_start_s: f64, duration_s: f64) -> Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = Timeline::empty("test");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        timeline
    }

    #[tokio::test]
    async fn transcript_pack_tool_returns_bounded_json() {
        let dir = tempfile::tempdir().unwrap();
        montage_proto::project::Project::init(dir.path()).unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            serde_json::json!({
                "data": {
                    "segments": [{"start_s": 0.0, "end_s": 1.0, "text": "alpha beta"}],
                    "words": [
                        {"start_s": 0.0, "end_s": 0.2, "text": "alpha"},
                        {"start_s": 0.2, "end_s": 0.4, "text": "beta"}
                    ]
                }
            }),
        );

        let output = TranscriptPackTool
            .handle(
                invocation(serde_json::json!({
                    "asset_id": "raw/a.mp4",
                    "max_words_per_asset": 1,
                    "include_all_assets": true
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["total_matching_assets"], 1);
        assert_eq!(body["assets"][0]["words"].as_array().unwrap().len(), 1);
        assert_eq!(body["assets"][0]["truncated"], true);
    }

    #[tokio::test]
    async fn transcript_pack_tool_defaults_to_timeline_visible_words() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = montage_proto::project::Project::init(dir.path()).unwrap();
        project.timeline = timeline_with_clip("raw/a.mp4", 1.0, 1.0);
        project.write(dir.path()).unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            serde_json::json!({
                "data": {
                    "words": [
                        {"start_s": 0.0, "end_s": 0.5, "text": "before"},
                        {"start_s": 1.2, "end_s": 1.6, "text": "visible"},
                        {"start_s": 4.0, "end_s": 4.5, "text": "after"}
                    ]
                }
            }),
        );

        let output = TranscriptPackTool
            .handle(invocation(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();

        assert_eq!(body["total_word_count"], 1);
        assert_eq!(body["assets"][0]["text"], "visible");
    }
}
