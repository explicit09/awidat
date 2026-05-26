//! `plan_multicam` tool — propose a flattened podcast program track.

use std::collections::HashMap;

use async_trait::async_trait;
use awidat_index::{read_sidecar, walk_indexer};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `plan_multicam` tool.
pub struct PlanMulticamTool;

#[derive(Debug, Deserialize)]
struct PlanMulticamArgs {
    /// Candidate camera assets. Defaults to assets with shot/face sidecars.
    #[serde(default)]
    asset_ids: Option<Vec<String>>,
    /// Asset containing the diarized transcript. Defaults to first whisper sidecar.
    #[serde(default)]
    audio_master: Option<String>,
    /// Minimum hold duration in seconds. Defaults to 3.0.
    #[serde(default)]
    min_hold_s: Option<f64>,
}

#[async_trait]
impl ToolHandler for PlanMulticamTool {
    fn name(&self) -> &'static str {
        "plan_multicam"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_multicam".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_ids": { "type": "array", "items": { "type": "string" }, "description": "Camera assets to consider. Defaults to assets with shot/face sidecars." },
                    "audio_master": { "type": "string", "description": "Asset with diarized transcript. Defaults to first whisper sidecar." },
                    "min_hold_s": { "type": "number", "minimum": 1.0, "description": "Minimum hold duration between angle changes. Default 3s." }
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
        let args: PlanMulticamArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_multicam: invalid args ({e}). All fields are optional."
            ))
        })?;
        let mut cameras = args
            .asset_ids
            .unwrap_or_else(|| indexed_assets(&ctx.project_root));
        cameras.sort();
        cameras.dedup();
        if cameras.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_multicam: no camera assets found. Run shot/face indexing or pass asset_ids."
                    .into(),
            ));
        }

        let audio_master = args
            .audio_master
            .or_else(|| first_whisper_asset(&ctx.project_root))
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "plan_multicam: no whisper transcript found. Run whisper indexing or pass audio_master."
                        .into(),
                )
            })?;
        let min_hold_s = args.min_hold_s.unwrap_or(3.0).max(1.0);
        let transcript = load_segments(&ctx.project_root, &audio_master)?;
        let face_by_asset = load_index_map(&ctx.project_root, "face");
        let shot_by_asset = load_index_map(&ctx.project_root, "shot");
        let quality_by_asset = load_index_map(&ctx.project_root, "frame-quality");
        let topics = load_topics(&ctx.project_root, &audio_master);

        let mut decisions = Vec::new();
        let mut last_asset: Option<String> = None;
        let mut last_cut_s = f64::NEG_INFINITY;
        for seg in transcript {
            let topic_reset = topics.iter().any(|t| (t - seg.start_s).abs() < 1.0);
            let mut choice = choose_camera(
                &cameras,
                &face_by_asset,
                &shot_by_asset,
                &quality_by_asset,
                seg.speaker.as_deref(),
                (seg.start_s + seg.end_s) / 2.0,
            );
            if !topic_reset
                && seg.start_s - last_cut_s < min_hold_s
                && let Some(prev) = last_asset.clone()
            {
                choice.asset = prev;
                choice.reason = format!(
                    "held previous angle to satisfy {min_hold_s:.1}s minimum hold; {}",
                    choice.reason
                );
            }
            if topic_reset
                && let Some(wide) = cameras
                    .iter()
                    .find(|a| shot_type_at(&shot_by_asset, a, seg.start_s).contains("wide"))
            {
                choice.asset = wide.clone();
                choice.reason = "wide reset at topic change".into();
            }
            if last_asset.as_deref() != Some(choice.asset.as_str()) {
                last_cut_s = seg.start_s;
            }
            last_asset = Some(choice.asset.clone());
            decisions.push(serde_json::json!({
                "start_s": seg.start_s,
                "end_s": seg.end_s,
                "source_asset": choice.asset,
                "speaker": seg.speaker,
                "reason": choice.reason,
                "metadata": {
                    "traceable_source": true,
                    "min_hold_s": min_hold_s,
                    "flattened_program_track": "Program Video"
                }
            }));
        }

        let apply_plan = serde_json::json!({
            "program_track": "Program Video",
            "decisions": decisions,
        });
        let apply_edl = format!(
            "*** Begin EDL\n*** Apply Multicam Plan\n+ plan_json: {}\n*** End EDL\n",
            apply_plan
        );
        let body = serde_json::json!({
            "audio_master": audio_master,
            "camera_count": cameras.len(),
            "cameras": cameras,
            "program_track": "Program Video",
            "decisions": apply_plan["decisions"].clone(),
            "apply_edl": apply_edl,
            "review_flow": "Review the decisions, then apply the included Apply Multicam Plan EDL fragment to atomically replace the flattened Program Video track while preserving source_asset and reason metadata for vedit audit.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

#[derive(Debug)]
struct Segment {
    start_s: f64,
    end_s: f64,
    speaker: Option<String>,
}

#[derive(Debug)]
struct CameraChoice {
    asset: String,
    reason: String,
}

fn indexed_assets(project_root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for indexer in ["face", "shot"] {
        if let Ok(iter) = walk_indexer(project_root, indexer) {
            out.extend(iter.map(|(asset, _)| asset));
        }
    }
    out
}

fn first_whisper_asset(project_root: &std::path::Path) -> Option<String> {
    walk_indexer(project_root, "whisper")
        .ok()
        .and_then(|mut it| it.next().map(|(asset, _)| asset))
}

fn load_segments(
    project_root: &std::path::Path,
    asset_id: &str,
) -> Result<Vec<Segment>, FunctionCallError> {
    let sidecar = read_sidecar(project_root, "whisper", &AssetId::new(asset_id.to_string()))
        .map_err(|e| FunctionCallError::RespondToModel(format!("plan_multicam: {e}")))?;
    let Some(segments) = sidecar.pointer("/data/segments").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    Ok(segments
        .iter()
        .filter_map(|seg| {
            let start_s = seg.get("start").or_else(|| seg.get("start_s"))?.as_f64()?;
            let end_s = seg.get("end").or_else(|| seg.get("end_s"))?.as_f64()?;
            (end_s > start_s).then_some(Segment {
                start_s,
                end_s,
                speaker: seg
                    .get("speaker")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect())
}

fn load_index_map(
    project_root: &std::path::Path,
    indexer: &str,
) -> HashMap<String, serde_json::Value> {
    walk_indexer(project_root, indexer)
        .map(|it| it.collect())
        .unwrap_or_default()
}

fn load_topics(project_root: &std::path::Path, asset_id: &str) -> Vec<f64> {
    let Ok(sidecar) = read_sidecar(project_root, "topic", &AssetId::new(asset_id.to_string()))
    else {
        return Vec::new();
    };
    sidecar
        .pointer("/data/topics")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("start_s").and_then(|v| v.as_f64()))
        .collect()
}

fn choose_camera(
    cameras: &[String],
    face_by_asset: &HashMap<String, serde_json::Value>,
    shot_by_asset: &HashMap<String, serde_json::Value>,
    quality_by_asset: &HashMap<String, serde_json::Value>,
    speaker: Option<&str>,
    t_s: f64,
) -> CameraChoice {
    let mut ranked = cameras
        .iter()
        .map(|asset| {
            let mut score = quality_score_at(quality_by_asset, asset, t_s);
            let shot = shot_type_at(shot_by_asset, asset, t_s);
            if shot.contains("close") || shot.contains("medium") {
                score += 0.20;
            }
            if shot.contains("wide") {
                score += 0.05;
            }
            if let Some(speaker) = speaker
                && speaker_on_asset(face_by_asset, asset, speaker, t_s)
            {
                score += 0.55;
            }
            (asset.clone(), score, shot)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (asset, score, shot) = ranked.into_iter().next().unwrap_or_else(|| {
        (
            cameras.first().cloned().unwrap_or_default(),
            0.0,
            "unknown".into(),
        )
    });
    CameraChoice {
        asset,
        reason: format!(
            "speaker/quality/shot score {:.2}; shot_type={}",
            score, shot
        ),
    }
}

fn speaker_on_asset(
    face_by_asset: &HashMap<String, serde_json::Value>,
    asset: &str,
    speaker: &str,
    t_s: f64,
) -> bool {
    let Some(sidecar) = face_by_asset.get(asset) else {
        return false;
    };
    let Some(face_id) = sidecar
        .pointer("/data/speaker_to_face")
        .and_then(|m| m.get(speaker))
        .and_then(|v| v.as_str())
    else {
        return false;
    };
    sidecar
        .pointer("/data/per_frame")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|f| {
            f.get("t_s")
                .and_then(|v| v.as_f64())
                .is_some_and(|t| (t - t_s).abs() <= 0.75)
        })
        .any(|f| {
            f.get("faces")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .any(|face| face.get("face_id").and_then(|v| v.as_str()) == Some(face_id))
        })
}

fn shot_type_at(map: &HashMap<String, serde_json::Value>, asset: &str, t_s: f64) -> String {
    map.get(asset)
        .and_then(|v| v.pointer("/data/shots"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|shot| {
            let start = shot.get("start_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let end = shot.get("end_s").and_then(|v| v.as_f64()).unwrap_or(start);
            t_s >= start && t_s < end
        })
        .and_then(|shot| shot.get("type").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string()
}

fn quality_score_at(map: &HashMap<String, serde_json::Value>, asset: &str, t_s: f64) -> f64 {
    let Some(sidecar) = map.get(asset) else {
        return 0.5;
    };
    sidecar
        .pointer("/data/per_frame")
        .or_else(|| sidecar.pointer("/data/frames"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let t = f.get("t_s").and_then(|v| v.as_f64())?;
            if (t - t_s).abs() > 0.75 {
                return None;
            }
            f.get("sharpness")
                .or_else(|| f.get("quality"))
                .or_else(|| f.get("score"))
                .and_then(|v| v.as_f64())
        })
        .fold(None, |acc: Option<f64>, v| Some(acc.unwrap_or(v).max(v)))
        .unwrap_or(0.5)
}

const DESCRIPTION: &str = "\
Create a reviewable N-camera podcast direction plan. The tool uses \
diarized transcript segments plus face speaker mapping, shot type, and \
frame quality sidecars when present. It returns flattened Program Video \
decisions with source_asset and reason metadata; it does not create OTIO \
multicam stacks.";
