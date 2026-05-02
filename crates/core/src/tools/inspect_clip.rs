//! `inspect_clip` tool — codec/dims/duration/audio summary for an asset.
//!
//! Per `PLAN.md` §6.1, returns "codec/dims/duration/audio waveform
//! thumbnail. Bounded output." We project from sidecars (whisper for
//! duration/language/speakers, scenedetect for fps/duration, audio-energy
//! for loudness/silences). The waveform thumbnail is deferred to v1.5
//! (we'd need an image-return shape — see `view_frame` in phase 4D).

use async_trait::async_trait;
use awidat_index::{SidecarError, read_sidecar};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `inspect_clip` tool.
pub struct InspectClipTool;

#[derive(Debug, Deserialize)]
struct InspectClipArgs {
    /// Project-relative asset id, OR a clip uuid (resolution by uuid is
    /// stubbed for now — week 5+ when timelines have real clips).
    asset_id: String,
}

#[async_trait]
impl ToolHandler for InspectClipTool {
    fn name(&self) -> &'static str {
        "inspect_clip"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "inspect_clip".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Project-relative path of the source asset, e.g. \"raw/ep-014.mp4\"."
                    }
                },
                "required": ["asset_id"]
            }),
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
        let args: InspectClipArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "inspect_clip: invalid args ({e}). Required: {{ \"asset_id\": <str> }}."
            ))
        })?;

        let asset = AssetId::new(args.asset_id);
        let mut overview = serde_json::Map::new();
        overview.insert("asset_id".into(), serde_json::json!(asset.to_string()));

        let mut any = false;

        if let Some(scene) = try_read(&ctx.project_root, "scenedetect", &asset)? {
            let data = scene.get("data").cloned().unwrap_or_default();
            if let Some(v) = data.get("frame_rate") { overview.insert("frame_rate".into(), v.clone()); }
            if let Some(v) = data.get("duration_s") { overview.insert("duration_s".into(), v.clone()); }
            if let Some(shots) = data.get("shots").and_then(|v| v.as_array()) {
                overview.insert("shot_count".into(), serde_json::json!(shots.len()));
            }
            any = true;
        }

        if let Some(audio) = try_read(&ctx.project_root, "audio-energy", &asset)? {
            let data = audio.get("data").cloned().unwrap_or_default();
            // audio-energy may have duration too — prefer it if scenedetect didn't fire.
            if !overview.contains_key("duration_s") {
                if let Some(v) = data.get("duration_s") { overview.insert("duration_s".into(), v.clone()); }
            }
            if let Some(v) = data.get("sample_rate") { overview.insert("audio_sample_rate".into(), v.clone()); }
            if let Some(v) = data.get("loudness_integrated_lufs") { overview.insert("loudness_integrated_lufs".into(), v.clone()); }
            if let Some(silences) = data.get("silences").and_then(|v| v.as_array()) {
                overview.insert("silence_count".into(), serde_json::json!(silences.len()));
            }
            any = true;
        }

        if let Some(whisper) = try_read(&ctx.project_root, "whisper", &asset)? {
            let data = whisper.get("data").cloned().unwrap_or_default();
            if let Some(v) = data.get("language") { overview.insert("language".into(), v.clone()); }
            if let Some(v) = data.get("speakers") { overview.insert("speakers".into(), v.clone()); }
            if let Some(v) = data.get("model") { overview.insert("whisper_model".into(), v.clone()); }
            if let Some(segments) = data.get("segments").and_then(|v| v.as_array()) {
                overview.insert("segment_count".into(), serde_json::json!(segments.len()));
            }
            any = true;
        }

        if !any {
            return Err(FunctionCallError::RespondToModel(format!(
                "inspect_clip: no indexer sidecars found for '{asset}'. Run \
                 `awidat index` first."
            )));
        }

        let body = serde_json::to_string(&serde_json::Value::Object(overview))
            .map_err(|e| FunctionCallError::Fatal(e.to_string()))?;
        Ok(ToolOutput::text(body))
    }
}

fn try_read(
    root: &std::path::Path,
    indexer: &str,
    asset: &AssetId,
) -> Result<Option<serde_json::Value>, FunctionCallError> {
    match read_sidecar(root, indexer, asset) {
        Ok(v) => Ok(Some(v)),
        Err(SidecarError::NotFound { .. }) => Ok(None),
        Err(e) => Err(FunctionCallError::RespondToModel(e.to_string())),
    }
}

const DESCRIPTION: &str = "\
Get a one-page metadata summary for an asset: duration, frame_rate, audio \
sample rate, loudness, language, speakers, shot count, segment count. \
Aggregates from whichever indexer sidecars exist (whisper / scenedetect / \
audio-energy). Use this before deciding which sidecar to read in detail \
via `read_index`.\
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
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "inspect_clip".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, body: serde_json::Value) {
        let p = root.join("index").join(indexer).join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn aggregates_across_indexers() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(dir.path(), "scenedetect", "raw/x.mp4",
            serde_json::json!({"data": {"frame_rate": 24.0, "duration_s": 120.0, "shots": [1,2,3,4]}}));
        write_sidecar(dir.path(), "audio-energy", "raw/x.mp4",
            serde_json::json!({"data": {"sample_rate": 48000, "loudness_integrated_lufs": -16.5, "silences": [{}]}}));
        write_sidecar(dir.path(), "whisper", "raw/x.mp4",
            serde_json::json!({"data": {"language": "en", "speakers": ["A","B"], "segments": [{},{},{}]}}));

        let out = InspectClipTool
            .handle(invoke(serde_json::json!({"asset_id": "raw/x.mp4"})), ctx_at(dir.path()))
            .await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["frame_rate"], 24.0);
        assert_eq!(v["duration_s"], 120.0);
        assert_eq!(v["shot_count"], 4);
        assert_eq!(v["language"], "en");
        assert_eq!(v["segment_count"], 3);
        assert_eq!(v["loudness_integrated_lufs"], -16.5);
    }

    #[tokio::test]
    async fn no_sidecars_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = InspectClipTool
            .handle(invoke(serde_json::json!({"asset_id": "raw/missing.mp4"})), ctx_at(dir.path()))
            .await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("no indexer sidecars")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }
}
