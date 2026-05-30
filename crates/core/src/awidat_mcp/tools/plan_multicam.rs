//! `plan_multicam` — read-only planner for a flattened N-camera podcast
//! program track. Ported from `crates/core/src/tools/plan_multicam.rs`
//! to the in-process MCP server.

use std::collections::HashMap;

use awidat_index::{read_sidecar, walk_indexer};
use awidat_proto::index::AssetId;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `plan_multicam`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanMulticamArgs {
    /// Candidate camera assets. Defaults to assets with shot/face sidecars.
    #[serde(default)]
    pub asset_ids: Option<Vec<String>>,
    /// Asset containing the diarized transcript. Defaults to first whisper
    /// sidecar.
    #[serde(default)]
    pub audio_master: Option<String>,
    /// Minimum hold duration in seconds between angle changes. Defaults to
    /// 3.0.
    #[serde(default)]
    pub min_hold_s: Option<f64>,
}

/// Run `plan_multicam`.
pub fn run(args: PlanMulticamArgs, ctx: McpToolCtx) -> Result<String, String> {
    let mut cameras = args
        .asset_ids
        .unwrap_or_else(|| indexed_assets(&ctx.project_root));
    cameras.sort();
    cameras.dedup();
    if cameras.is_empty() {
        return Err(
            "plan_multicam: no camera assets found. Run shot/face indexing or pass asset_ids."
                .into(),
        );
    }

    let audio_master = args
        .audio_master
        .or_else(|| first_whisper_asset(&ctx.project_root))
        .ok_or_else(|| {
            "plan_multicam: no whisper transcript found. Run whisper indexing or pass audio_master."
                .to_string()
        })?;
    let min_hold_s = args.min_hold_s.unwrap_or(3.0).max(1.0);
    let transcript = load_segments(&ctx.project_root, &audio_master)?;
    let face_by_asset = load_index_map(&ctx.project_root, "face");
    let shot_by_asset = load_index_map(&ctx.project_root, "shot");
    let quality_by_asset = load_index_map(&ctx.project_root, "frame-quality");
    let topics = load_topics(&ctx.project_root, &audio_master);

    // Per-camera timeline offsets from applied `awidat.sync_group` effects, so
    // separate-device cameras are scored at the correct source time. Falls back
    // to a shared timebase when the project can't be read or has no sync groups.
    let offsets = Project::read(&ctx.project_root)
        .map(|p| sync_offsets(&p.timeline))
        .unwrap_or_default();
    let am_offset = offset_of(&offsets, &audio_master);

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
            &offsets,
            am_offset,
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
            && let Some(wide) = cameras.iter().find(|a| {
                let t = seg.start_s + am_offset - offset_of(&offsets, a);
                shot_type_at(&shot_by_asset, a, t).contains("wide")
            })
        {
            choice.asset = wide.clone();
            choice.reason = "wide reset at topic change".into();
        }
        if last_asset.as_deref() != Some(choice.asset.as_str()) {
            last_cut_s = seg.start_s;
        }
        last_asset = Some(choice.asset.clone());
        let sync_group_id = offsets
            .get(&choice.asset)
            .and_then(|s| s.sync_group_id.clone());
        let offset_corrected = offsets.contains_key(&choice.asset);
        decisions.push(serde_json::json!({
            "start_s": seg.start_s,
            "end_s": seg.end_s,
            "source_asset": choice.asset,
            "sync_group_id": sync_group_id,
            "speaker": seg.speaker,
            "reason": choice.reason,
            "metadata": {
                "traceable_source": true,
                "min_hold_s": min_hold_s,
                "offset_corrected": offset_corrected,
                "flattened_program_track": "Program Video"
            }
        }));
    }

    let mut warnings = Vec::new();
    if offsets.is_empty() && cameras.len() > 1 {
        warnings.push(
            "no applied sync groups found — assuming all cameras share a timebase. \
If the cameras were recorded on separate devices, run analyze_sync and apply the \
Set Sync Group fragments first, then re-run plan_multicam for offset-corrected angles."
                .to_string(),
        );
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
        "warnings": warnings,
        "apply_edl": apply_edl,
        "review_flow": "Review the decisions, then apply the included Apply Multicam Plan EDL fragment to atomically replace the flattened Program Video track while preserving source_asset, sync_group_id, and reason metadata for vedit audit.",
    });
    Ok(body.to_string())
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

fn load_segments(project_root: &std::path::Path, asset_id: &str) -> Result<Vec<Segment>, String> {
    let sidecar = read_sidecar(project_root, "whisper", &AssetId::new(asset_id.to_string()))
        .map_err(|e| format!("plan_multicam: {e}"))?;
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

/// Per-camera timeline sync info read from applied `awidat.sync_group` effects.
#[derive(Debug, Clone, Default)]
struct SyncInfo {
    /// Timeline offset (seconds) relative to the reference. Positive means the
    /// camera was placed later, so its source time at program time `t` is
    /// `t - offset_s`.
    offset_s: f64,
    /// Sync group this camera belongs to, propagated onto multicam decisions.
    sync_group_id: Option<String>,
}

const SYNC_GROUP_EFFECT_NAME: &str = "awidat.sync_group";

/// Collect per-asset sync offsets from applied `awidat.sync_group` effects.
/// Maps source asset id (clip `target_url`) → [`SyncInfo`]. The first effect
/// seen per asset wins (one sync group per camera in v1).
fn sync_offsets(timeline: &Timeline) -> HashMap<String, SyncInfo> {
    let mut out: HashMap<String, SyncInfo> = HashMap::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for tc in &track.children {
            let TrackChild::Clip(clip) = tc else {
                continue;
            };
            let MediaReference::External(ext) = &clip.media_reference else {
                continue;
            };
            let Some(effect) = clip
                .effects
                .iter()
                .find(|e| e.effect_name == SYNC_GROUP_EFFECT_NAME)
            else {
                continue;
            };
            let Some(offset_s) = effect.metadata.get("offset_s").and_then(|v| v.as_f64()) else {
                continue;
            };
            let sync_group_id = effect
                .metadata
                .get("sync_group_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            out.entry(ext.target_url.clone()).or_insert(SyncInfo {
                offset_s,
                sync_group_id,
            });
        }
    }
    out
}

/// Timeline offset for `asset`, or 0.0 (shared timebase) when no sync group
/// has been applied to it.
fn offset_of(offsets: &HashMap<String, SyncInfo>, asset: &str) -> f64 {
    offsets.get(asset).map(|s| s.offset_s).unwrap_or(0.0)
}

fn choose_camera(
    cameras: &[String],
    face_by_asset: &HashMap<String, serde_json::Value>,
    shot_by_asset: &HashMap<String, serde_json::Value>,
    quality_by_asset: &HashMap<String, serde_json::Value>,
    offsets: &HashMap<String, SyncInfo>,
    am_offset: f64,
    speaker: Option<&str>,
    t_s: f64,
) -> CameraChoice {
    let mut ranked = cameras
        .iter()
        .map(|asset| {
            // Convert the program-timeline time into this camera's source time.
            let t = t_s + am_offset - offset_of(offsets, asset);
            let mut score = quality_score_at(quality_by_asset, asset, t);
            let shot = shot_type_at(shot_by_asset, asset, t);
            if shot.contains("close") || shot.contains("medium") {
                score += 0.20;
            }
            if shot.contains("wide") {
                score += 0.05;
            }
            if let Some(speaker) = speaker
                && speaker_on_asset(face_by_asset, asset, speaker, t)
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

pub const DESCRIPTION: &str = "\
Create a reviewable N-camera podcast direction plan. The tool uses \
diarized transcript segments plus face speaker mapping, shot type, and \
frame quality sidecars when present. It returns flattened Program Video \
decisions with source_asset and reason metadata; it does not create OTIO \
multicam stacks.";
