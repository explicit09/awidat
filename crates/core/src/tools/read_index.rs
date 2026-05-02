//! `read_index` tool — read a single sidecar channel for an asset.
//!
//! Per `PLAN.md` §6.1: `read_index(asset, channel)` returns a "succinct
//! shape." We don't return the whole sidecar — channels project into
//! summarized views (cap on per-window count, etc.) so the model isn't
//! drowning in 50K-word transcripts in one tool result.

use async_trait::async_trait;
use awidat_index::{SidecarError, read_sidecar};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Bytes cap on the JSON the tool returns. Keeps token cost predictable
/// even when the underlying sidecar is huge (e.g. a 1h whisper transcript
/// is several MB raw). Larger queries should `walk_indexer` via a
/// follow-up search tool — `read_index` is for "give me X of asset Y".
const RESULT_CAP_BYTES: usize = 8 * 1024;

/// Default count for windowed channels (transcript segments, scenes,
/// short-term loudness). 50 entries fits comfortably in the cap.
const DEFAULT_WINDOW: usize = 50;

/// The `read_index` tool.
pub struct ReadIndexTool;

#[derive(Debug, Deserialize)]
struct ReadIndexArgs {
    asset_id: String,
    channel: String,
    /// Index of the first entry to return (0-based) for windowed channels.
    #[serde(default)]
    offset: Option<usize>,
    /// How many entries to include for windowed channels.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ReadIndexTool {
    fn name(&self) -> &'static str {
        "read_index"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_index".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Project-relative path of the source asset, e.g. \"raw/ep-014.mp4\"."
                    },
                    "channel": {
                        "type": "string",
                        "enum": ["transcript", "scenes", "audio_levels", "topics", "summary"],
                        "description": "Which signal to read. transcript=words+segments; scenes=shot boundaries; audio_levels=RMS+LUFS+silences; topics=topic-segmentation; summary=one-line overview of all channels."
                    },
                    "offset": { "type": "integer", "minimum": 0, "description": "0-based first entry. Default 0." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Max entries. Default 50." }
                },
                "required": ["asset_id", "channel"]
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
        let args: ReadIndexArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "read_index: invalid args ({e}). Required: {{ \"asset_id\": <str>, \
                 \"channel\": \"transcript\"|\"scenes\"|\"audio_levels\"|\"topics\"|\"summary\" }}."
            ))
        })?;

        let indexer = match args.channel.as_str() {
            "transcript" => "whisper",
            "scenes" => "scenedetect",
            "audio_levels" => "audio-energy",
            "topics" => "topic",
            "summary" => return summary(&ctx.project_root, &args.asset_id),
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "read_index: channel '{other}' not recognized. Use one of: \
                     transcript, scenes, audio_levels, topics, summary."
                )));
            }
        };

        let asset_id = AssetId::new(args.asset_id);
        let sidecar = read_sidecar(&ctx.project_root, indexer, &asset_id).map_err(map_err)?;
        let projected = project_channel(&args.channel, &sidecar, args.offset.unwrap_or(0), args.limit.unwrap_or(DEFAULT_WINDOW));
        let body = serde_json::to_string(&projected).map_err(|e| {
            FunctionCallError::Fatal(format!("read_index serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(cap_size(&body)))
    }
}

fn map_err(e: SidecarError) -> FunctionCallError {
    FunctionCallError::RespondToModel(e.to_string())
}

/// Project the requested channel into a model-friendly shape with offset/limit.
fn project_channel(
    channel: &str,
    sidecar: &serde_json::Value,
    offset: usize,
    limit: usize,
) -> serde_json::Value {
    let data = sidecar.get("data").cloned().unwrap_or(serde_json::Value::Null);
    match channel {
        "transcript" => {
            let segments = data.get("segments").cloned().unwrap_or(serde_json::Value::Array(vec![]));
            let speakers = data.get("speakers").cloned();
            let language = data.get("language").cloned();
            let total = segments.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&segments, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "language": language,
                "speakers": speakers,
                "segments": windowed,
                "total_segments": total,
                "offset": offset,
                "limit": limit,
            })
        }
        "scenes" => {
            let shots = data.get("shots").cloned().unwrap_or(serde_json::Value::Array(vec![]));
            let total = shots.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&shots, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "frame_rate": data.get("frame_rate"),
                "duration_s": data.get("duration_s"),
                "shots": windowed,
                "total_shots": total,
                "offset": offset, "limit": limit,
            })
        }
        "audio_levels" => {
            // For audio levels we don't ship every 100ms RMS sample (would
            // blow the cap on a 1h file). Send just the silences + LUFS
            // overview unless windowing the rms windows.
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "duration_s": data.get("duration_s"),
                "loudness_integrated_lufs": data.get("loudness_integrated_lufs"),
                "silences": data.get("silences"),
                "silence_relative_lu": data.get("silence_relative_lu"),
                "loudness_short_term_count": data
                    .get("loudness_short_term")
                    .and_then(|v| v.as_array())
                    .map(Vec::len)
                    .unwrap_or(0),
                "windows_count": data
                    .get("windows")
                    .and_then(|v| v.as_array())
                    .map(Vec::len)
                    .unwrap_or(0),
            })
        }
        "topics" => {
            let topics = data.get("topics").cloned().unwrap_or(serde_json::Value::Array(vec![]));
            let total = topics.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&topics, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "labeler": data.get("labeler"),
                "topics": windowed,
                "total_topics": total,
                "offset": offset, "limit": limit,
            })
        }
        _ => sidecar.clone(),
    }
}

fn window(value: &serde_json::Value, offset: usize, limit: usize) -> serde_json::Value {
    match value.as_array() {
        Some(arr) => {
            let end = (offset + limit).min(arr.len());
            let start = offset.min(arr.len());
            serde_json::Value::Array(arr[start..end].to_vec())
        }
        None => value.clone(),
    }
}

fn cap_size(s: &str) -> String {
    if s.len() <= RESULT_CAP_BYTES {
        return s.to_string();
    }
    let head = &s[..RESULT_CAP_BYTES];
    format!("{head}\n[truncated to {RESULT_CAP_BYTES} bytes; raise --offset to page]")
}

fn summary(project_root: &std::path::Path, asset_id: &str) -> Result<ToolOutput, FunctionCallError> {
    let asset = AssetId::new(asset_id.to_string());
    let mut summary = serde_json::Map::new();
    for (channel, indexer) in [
        ("transcript", "whisper"),
        ("scenes", "scenedetect"),
        ("audio_levels", "audio-energy"),
        ("topics", "topic"),
    ] {
        match read_sidecar(project_root, indexer, &asset) {
            Ok(v) => {
                let projected = project_channel(channel, &v, 0, 0);
                let mut entry = serde_json::Map::new();
                if let Some(v) = projected.get("language") { entry.insert("language".into(), v.clone()); }
                if let Some(v) = projected.get("speakers") { entry.insert("speakers".into(), v.clone()); }
                if let Some(v) = projected.get("total_segments") { entry.insert("total_segments".into(), v.clone()); }
                if let Some(v) = projected.get("total_shots") { entry.insert("total_shots".into(), v.clone()); }
                if let Some(v) = projected.get("duration_s") { entry.insert("duration_s".into(), v.clone()); }
                if let Some(v) = projected.get("loudness_integrated_lufs") { entry.insert("loudness_integrated_lufs".into(), v.clone()); }
                if let Some(v) = projected.get("topics") { entry.insert("topics_count".into(), serde_json::json!(v.as_array().map(Vec::len).unwrap_or(0))); }
                summary.insert(channel.into(), serde_json::Value::Object(entry));
            }
            Err(SidecarError::NotFound { .. }) => {
                summary.insert(channel.into(), serde_json::json!({"available": false}));
            }
            Err(other) => {
                summary.insert(
                    channel.into(),
                    serde_json::json!({"error": other.to_string()}),
                );
            }
        }
    }
    let body = serde_json::to_string(&serde_json::Value::Object(summary))
        .map_err(|e| FunctionCallError::Fatal(e.to_string()))?;
    Ok(ToolOutput::text(cap_size(&body)))
}

const DESCRIPTION: &str = "\
Read one channel of the footage index for an asset. Channels: \
'transcript' (whisper words+segments), 'scenes' (shot boundaries), \
'audio_levels' (LUFS + silences), 'topics' (topic segmentation), \
'summary' (one-line overview of all channels). Windowed channels accept \
offset+limit (default 0+50). Result is capped at 8KB; page via offset \
when truncated.\
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
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "read_index".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, body: serde_json::Value) {
        let p = root.join("index").join(indexer).join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn transcript_channel_paginates() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "whisper",
            "raw/x.wav",
            serde_json::json!({
                "indexer": "whisper", "asset_id": "raw/x.wav",
                "data": {
                    "language": "en",
                    "speakers": [],
                    "segments": (0..120).map(|i| serde_json::json!({"text": format!("seg {i}"), "start_s": i, "end_s": i+1})).collect::<Vec<_>>(),
                }
            }),
        );
        let out = ReadIndexTool
            .handle(
                invoke(serde_json::json!({
                    "asset_id": "raw/x.wav",
                    "channel": "transcript",
                    "offset": 50,
                    "limit": 10,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["language"], "en");
        assert_eq!(body["total_segments"], 120);
        let segs = body["segments"].as_array().unwrap();
        assert_eq!(segs.len(), 10);
        assert_eq!(segs[0]["text"], "seg 50");
    }

    #[tokio::test]
    async fn missing_sidecar_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ReadIndexTool
            .handle(
                invoke(serde_json::json!({
                    "asset_id": "raw/missing.wav",
                    "channel": "transcript"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("no sidecar"));
                assert!(msg.contains("awidat index --indexer whisper"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_channel_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ReadIndexTool
            .handle(
                invoke(serde_json::json!({
                    "asset_id": "raw/x.wav",
                    "channel": "bogus"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("'bogus'")));
    }

    #[tokio::test]
    async fn summary_aggregates_available_channels() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "whisper",
            "raw/x.wav",
            serde_json::json!({
                "indexer": "whisper", "asset_id": "raw/x.wav",
                "data": {"language": "en", "segments": [{"text":"hi","start_s":0,"end_s":1}]}
            }),
        );
        // No scenedetect sidecar; should report available=false.
        let out = ReadIndexTool
            .handle(
                invoke(serde_json::json!({
                    "asset_id": "raw/x.wav",
                    "channel": "summary"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(body["transcript"]["language"], "en");
        assert_eq!(body["scenes"]["available"], false);
    }

    #[test]
    fn is_not_mutating() {
        let inv = invoke(serde_json::json!({}));
        assert!(!ReadIndexTool.is_mutating(&inv));
    }
}
