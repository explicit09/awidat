//! `read_index` — read one channel of the footage index for an asset.
//! Ported in step 5 from `crates/core/src/tools/read_index.rs`.

use std::path::Path;

use montage_index::{SidecarError, read_sidecar};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Bytes cap on the JSON the tool returns. Keeps token cost predictable
/// even when the underlying sidecar is huge.
const RESULT_CAP_BYTES: usize = 8 * 1024;

/// Default count for windowed channels.
const DEFAULT_WINDOW: usize = 50;

/// Hard cap on `limit`.
const MAX_LIMIT: usize = 300;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReadIndexArgs {
    /// Project-relative path of the source asset, e.g. "raw/ep-014.mp4".
    pub asset_id: String,
    /// Which signal to read. transcript / scenes / audio_levels / beats / topics /
    /// editorial_moments / color / clip / face / gaze / shot / composition /
    /// frame_quality / generated_description / summary.
    pub channel: String,
    /// 0-based first entry for windowed channels.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Max entries for windowed channels. Default 50, hard cap 300.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn run(args: ReadIndexArgs, ctx: McpToolCtx) -> Result<String, String> {
    let indexer = match args.channel.as_str() {
        "transcript" => "whisper",
        "scenes" => "scenedetect",
        "audio_levels" => "audio-energy",
        "beats" => "beats",
        "topics" => "topic",
        "editorial_moments" => "editorial-moments",
        "color" => "color-analysis",
        "clip" => "clip",
        "face" => "face",
        "gaze" => "gaze",
        "shot" => "shot",
        "composition" => "composition",
        "frame_quality" => "frame-quality",
        "generated_description" => "generated-description",
        "summary" => return summary(&ctx.project_root, &args.asset_id),
        other => {
            return Err(format!(
                "read_index: channel '{other}' not recognized. Use one of: \
                 transcript, scenes, audio_levels, beats, topics, editorial_moments, \
                 color, clip, face, gaze, shot, composition, frame_quality, \
                 generated_description, summary."
            ));
        }
    };

    let asset_id = AssetId::new(args.asset_id);
    let sidecar = read_sidecar(&ctx.project_root, indexer, &asset_id).map_err(map_err)?;
    let limit = args.limit.unwrap_or(DEFAULT_WINDOW).min(MAX_LIMIT);
    let projected = project_channel(&args.channel, &sidecar, args.offset.unwrap_or(0), limit);
    let body = serde_json::to_string(&projected)
        .map_err(|e| format!("read_index serialization failed: {e}"))?;
    Ok(cap_size(&body))
}

fn map_err(e: SidecarError) -> String {
    e.to_string()
}

fn project_channel(
    channel: &str,
    sidecar: &serde_json::Value,
    offset: usize,
    limit: usize,
) -> serde_json::Value {
    let data = sidecar
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    match channel {
        "transcript" => {
            let segments = data
                .get("segments")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
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
                "offset": offset, "limit": limit,
            })
        }
        "scenes" => {
            let shots = data
                .get("shots")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
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
        "audio_levels" => serde_json::json!({
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
        }),
        "beats" => {
            let beats = data
                .get("beats")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = beats.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&beats, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "duration_s": data.get("duration_s"),
                "tempo_bpm": data.get("tempo_bpm"),
                "beat_times_s": data.get("beat_times_s"),
                "beats": windowed,
                "total_beats": total,
                "offset": offset, "limit": limit,
            })
        }
        "topics" => {
            let topics = data
                .get("topics")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
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
        "editorial_moments" => {
            let moments = data
                .get("moments")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = moments.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&moments, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "labeler_model": data.get("labeler_model"),
                "topic_segments_processed": data.get("topic_segments_processed"),
                "moments": windowed,
                "total_moments": total,
                "offset": offset, "limit": limit,
            })
        }
        "color" => {
            let per_frame = data
                .get("per_frame")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = per_frame.as_array().map(Vec::len).unwrap_or(0);
            let windowed = window(&per_frame, offset, limit);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "frame_rate_sampled": data.get("frame_rate_sampled"),
                "detect_width": data.get("detect_width"),
                "detect_height": data.get("detect_height"),
                "frame_count": data.get("frame_count"),
                "summary": data.get("summary"),
                "scenes": data.get("scenes"),
                "ignored_frames": data.get("ignored_frames"),
                "per_frame": windowed,
                "total_frames": total,
                "offset": offset, "limit": limit,
            })
        }
        "clip" => serde_json::json!({
            "asset_id": sidecar.get("asset_id"),
            "model": data.get("model"),
            "embedding_dim": data.get("embedding_dim"),
            "embedding_dtype": data.get("embedding_dtype"),
            "frame_rate_sampled": data.get("frame_rate_sampled"),
            "duration_s": data.get("duration_s"),
            "frame_count": data.get("frame_count"),
            "timestamps_s": window(
                data.get("timestamps_s").unwrap_or(&serde_json::Value::Array(vec![])),
                offset, limit,
            ),
            "has_embeddings": data.get("embeddings_b64").and_then(|v| v.as_str()).is_some(),
            "offset": offset, "limit": limit,
        }),
        "face" => {
            let per_frame = data
                .get("per_frame")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = per_frame.as_array().map(Vec::len).unwrap_or(0);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "frame_rate_sampled": data.get("frame_rate_sampled"),
                "duration_s": data.get("duration_s"),
                "detect_width": data.get("detect_width"),
                "detect_height": data.get("detect_height"),
                "frame_count": data.get("frame_count"),
                "faces": data.get("faces"),
                "speaker_to_face": data.get("speaker_to_face"),
                "per_frame": window(&per_frame, offset, limit),
                "total_frames": total,
                "offset": offset, "limit": limit,
            })
        }
        "gaze" => {
            let per_frame = data
                .get("per_frame")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = per_frame.as_array().map(Vec::len).unwrap_or(0);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "frame_rate_sampled": data.get("frame_rate_sampled"),
                "detect_width": data.get("detect_width"),
                "detect_height": data.get("detect_height"),
                "frame_count": data.get("frame_count"),
                "at_camera_threshold": data.get("at_camera_threshold"),
                "source": data.get("source"),
                "per_frame": window(&per_frame, offset, limit),
                "total_frames": total,
                "offset": offset, "limit": limit,
            })
        }
        "shot" => {
            let shots = data
                .get("shots")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = shots.as_array().map(Vec::len).unwrap_or(0);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "shots": window(&shots, offset, limit),
                "total_shots": total,
                "thresholds": data.get("thresholds"),
                "depends_on": data.get("depends_on"),
                "offset": offset, "limit": limit,
            })
        }
        "composition" => {
            let regions = data
                .get("regions")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = regions.as_array().map(Vec::len).unwrap_or(0);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "regions": window(&regions, offset, limit),
                "total_regions": total,
                "verification": data.get("verification"),
                "depends_on": data.get("depends_on"),
                "offset": offset, "limit": limit,
            })
        }
        "frame_quality" => {
            let per_frame = data
                .get("per_frame")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            let total = per_frame.as_array().map(Vec::len).unwrap_or(0);
            serde_json::json!({
                "asset_id": sidecar.get("asset_id"),
                "frame_rate_sampled": data.get("frame_rate_sampled"),
                "detect_width": data.get("detect_width"),
                "detect_height": data.get("detect_height"),
                "frame_count": data.get("frame_count"),
                "blur_sharp_threshold": data.get("blur_sharp_threshold"),
                "summary": data.get("summary"),
                "thumbnail_candidates": data.get("thumbnail_candidates"),
                "per_frame": window(&per_frame, offset, limit),
                "total_frames": total,
                "offset": offset, "limit": limit,
            })
        }
        "generated_description" => serde_json::json!({
            "asset_id": sidecar.get("asset_id"),
            "job_id": data.get("job_id"),
            "provider": data.get("provider"),
            "model": data.get("model"),
            "prompt": data.get("prompt"),
            "prompt_hash": data.get("prompt_hash"),
            "artifact_kind": data.get("artifact_kind"),
            "workflow_purpose": data.get("workflow_purpose"),
            "visual_summary": data.get("visual_summary"),
            "intended_use": data.get("intended_use"),
            "created_at": data.get("created_at"),
            "completed_at": data.get("completed_at"),
            "requires_disclosure": data.get("requires_disclosure"),
            "uses_likeness": data.get("uses_likeness"),
            "provenance": data.get("provenance"),
        }),
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

fn summary(project_root: &Path, asset_id: &str) -> Result<String, String> {
    let asset = AssetId::new(asset_id.to_string());
    let mut summary = serde_json::Map::new();
    for (channel, indexer) in [
        ("transcript", "whisper"),
        ("scenes", "scenedetect"),
        ("audio_levels", "audio-energy"),
        ("beats", "beats"),
        ("topics", "topic"),
        ("editorial_moments", "editorial-moments"),
        ("color", "color-analysis"),
        ("clip", "clip"),
        ("face", "face"),
        ("gaze", "gaze"),
        ("shot", "shot"),
        ("composition", "composition"),
        ("frame_quality", "frame-quality"),
        ("generated_description", "generated-description"),
    ] {
        match read_sidecar(project_root, indexer, &asset) {
            Ok(v) => {
                let projected = project_channel(channel, &v, 0, 0);
                let mut entry = serde_json::Map::new();
                for key in [
                    "language",
                    "speakers",
                    "total_segments",
                    "total_shots",
                    "duration_s",
                    "loudness_integrated_lufs",
                    "summary",
                    "total_frames",
                    "frame_count",
                    "total_moments",
                    "total_regions",
                    "speaker_to_face",
                    "has_embeddings",
                    "visual_summary",
                    "requires_disclosure",
                    "uses_likeness",
                ] {
                    if let Some(v) = projected.get(key) {
                        entry.insert(key.into(), v.clone());
                    }
                }
                if let Some(v) = projected.get("topics") {
                    entry.insert(
                        "topics_count".into(),
                        serde_json::json!(v.as_array().map(Vec::len).unwrap_or(0)),
                    );
                }
                if let Some(v) = projected.get("faces") {
                    entry.insert(
                        "face_count".into(),
                        serde_json::json!(v.as_array().map(Vec::len).unwrap_or(0)),
                    );
                }
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
    let body =
        serde_json::to_string(&serde_json::Value::Object(summary)).map_err(|e| e.to_string())?;
    Ok(cap_size(&body))
}

pub const DESCRIPTION: &str = "\
Read one channel of the footage index for an asset. Channels: \
'transcript' (whisper words+segments), 'scenes' (shot boundaries), \
'audio_levels' (LUFS + silences), 'beats' (tempo + beat times), 'topics' (topic segmentation), \
'editorial_moments' (typed edit beats), 'color' (per-frame color/exposure analysis), \
'clip' (CLIP embedding metadata), 'face', 'gaze', 'shot', 'composition', \
'frame_quality', 'generated_description' (prompt/provenance context for generated media), \
'summary' (one-line overview of all channels). Windowed channels accept \
offset+limit (default 0+50). Result is capped at 8KB; page via offset \
when truncated.";

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sidecar(root: &Path, indexer: &str, asset: &str, body: serde_json::Value) {
        let path = root
            .join("index")
            .join(indexer)
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }

    #[test]
    fn generated_description_channel_projects_generation_context() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "generated-description",
            "raw/generated/mock/gen-1.mp4",
            serde_json::json!({
                "indexer": "generated-description",
                "asset_id": "raw/generated/mock/gen-1.mp4",
                "data": {
                    "job_id": "gen-1",
                    "provider": "mock",
                    "model": "offline-placeholder",
                    "prompt": "quiet street at dusk",
                    "prompt_hash": "abc123",
                    "artifact_kind": "video",
                    "workflow_purpose": "broll",
                    "visual_summary": "quiet street at dusk",
                    "intended_use": "broll",
                    "requires_disclosure": true,
                    "uses_likeness": false,
                    "provenance": "generated_media_registry"
                }
            }),
        );

        let body = run(
            ReadIndexArgs {
                asset_id: "raw/generated/mock/gen-1.mp4".into(),
                channel: "generated_description".into(),
                offset: None,
                limit: None,
            },
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(body["job_id"], "gen-1");
        assert_eq!(body["visual_summary"], "quiet street at dusk");
        assert_eq!(body["requires_disclosure"], true);
        assert_eq!(body["provenance"], "generated_media_registry");
    }
}
