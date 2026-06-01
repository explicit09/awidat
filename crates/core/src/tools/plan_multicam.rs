//! `plan_multicam` tool — propose a flattened podcast program track.

use std::collections::HashMap;

use async_trait::async_trait;
use awidat_index::{read_sidecar, walk_indexer};
use awidat_proto::index::AssetId;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

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

        // Per-camera timeline offsets from applied `awidat.sync_group` effects.
        // Cameras recorded on separate devices are placed at different timeline
        // offsets; without this correction every per-camera sidecar lookup
        // would read the wrong source time. Falls back to an empty map (shared
        // timebase) when the project can't be read or no sync groups exist.
        let offsets = Project::read(&ctx.project_root)
            .map(|p| sync_offsets(&p.timeline))
            .unwrap_or_default();
        let am_offset = offset_of(&offsets, &audio_master);

        // "synced mode": at least one camera carries an applied sync offset.
        // In a correct N-camera sync, exactly one camera (the reference)
        // deliberately has no sync_group; the rest are offset against it.
        let synced_mode = !offsets.is_empty();
        let cameras_without_offset = cameras.iter().filter(|c| !offsets.contains_key(*c)).count();
        // The lone un-offset camera in synced mode is the reference (its
        // source time IS the program timebase, offset 0). If more than one
        // camera lacks an offset, we can't tell reference from skipped, so
        // those stay "uncorrected" and the partial-sync warning fires.
        let reference_is_unambiguous = synced_mode && cameras_without_offset == 1;

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
            let chosen_offset = offsets.get(&choice.asset);
            let sync_group_id = chosen_offset.and_then(|s| s.sync_group_id.clone());
            // Offset-corrected when the chosen camera has an applied offset,
            // OR it's the unambiguous reference (offset 0 by design). Carry
            // the offset-adjusted source IN/OUT so Apply Multicam Plan reads
            // the right source frames; timeline span stays start_s..end_s.
            let cam_offset = offset_of(&offsets, &choice.asset);
            // Clamp the source window to non-negative: a late-starting camera
            // (positive offset) chosen before its source exists would yield a
            // negative source_start, which Apply Multicam Plan copies into the
            // clip and the render path then rejects. Bound it to source 0 so
            // the plan always applies and renders.
            let source_start_s = (seg.start_s + am_offset - cam_offset).max(0.0);
            let source_end_s = (seg.end_s + am_offset - cam_offset).max(source_start_s);
            let offset_corrected = chosen_offset.is_some()
                || (reference_is_unambiguous && !offsets.contains_key(&choice.asset));
            decisions.push(serde_json::json!({
                "start_s": seg.start_s,
                "end_s": seg.end_s,
                "source_start_s": source_start_s,
                "source_end_s": source_end_s,
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
        if !synced_mode && cameras.len() > 1 {
            warnings.push(
                "no applied sync groups found — assuming all cameras share a timebase. \
If the cameras were recorded on separate devices, run analyze_sync and apply the \
Set Sync Group fragments first, then re-run plan_multicam for offset-corrected angles."
                    .to_string(),
            );
        } else if synced_mode && cameras_without_offset > 1 {
            // Partial sync: some cameras synced, but more than one lacks an
            // offset. Beyond the single reference, the rest are being scored
            // with the unsafe shared-timebase assumption this warning guards.
            warnings.push(format!(
                "{} of {} cameras have no applied sync group — only one can be the reference. \
The others are scored on the shared-timebase assumption and may pick wrong angles or source \
frames. Run analyze_sync for every non-reference camera and apply the Set Sync Group fragments \
before relying on this plan.",
                cameras_without_offset,
                cameras.len()
            ));
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
    Ok(parse_segments(&sidecar))
}

/// Parse `/data/segments` into [`Segment`]s. Pure over the sidecar JSON so
/// the speaker-id handling is unit-testable without disk.
fn parse_segments(sidecar: &serde_json::Value) -> Vec<Segment> {
    let Some(segments) = sidecar.pointer("/data/segments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    segments
        .iter()
        .filter_map(|seg| {
            let start_s = seg.get("start").or_else(|| seg.get("start_s"))?.as_f64()?;
            let end_s = seg.get("end").or_else(|| seg.get("end_s"))?.as_f64()?;
            (end_s > start_s).then_some(Segment {
                start_s,
                end_s,
                // Whisper sidecars label segments with `speaker_id`; accept the
                // legacy `speaker` key too. Without this the diarization bonus
                // never fires and the director falls back to shot/quality only.
                speaker: seg
                    .get("speaker_id")
                    .or_else(|| seg.get("speaker"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
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
            // `am_offset` rebases to the audio master's timebase; `offset_of`
            // subtracts the camera's own placement offset.
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

const DESCRIPTION: &str = "\
Create a reviewable N-camera podcast direction plan. The tool uses \
diarized transcript segments plus face speaker mapping, shot type, and \
frame quality sidecars when present. It returns flattened Program Video \
decisions with source_asset and reason metadata; it does not create OTIO \
multicam stacks.";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, Effect, ExternalReference, MediaReference, StackChild, Timeline, Track, TrackChild,
        TrackKind,
    };

    fn face_sidecar(face_id: &str, speaker: &str, frame_t_s: f64) -> serde_json::Value {
        serde_json::json!({
            "data": {
                "speaker_to_face": { speaker: face_id },
                "per_frame": [
                    { "t_s": frame_t_s, "faces": [ { "face_id": face_id } ] }
                ]
            }
        })
    }

    fn shot_sidecar(shot_type: &str) -> serde_json::Value {
        serde_json::json!({
            "data": { "shots": [ { "start_s": 0.0, "end_s": 1000.0, "type": shot_type } ] }
        })
    }

    #[test]
    fn picks_speaker_owning_camera() {
        let cameras = vec!["raw/cam-a.mp4".to_string(), "raw/cam-b.mp4".to_string()];
        let mut face = HashMap::new();
        face.insert(
            "raw/cam-b.mp4".to_string(),
            face_sidecar("face_1", "A", 10.0),
        );
        let mut shot = HashMap::new();
        shot.insert("raw/cam-a.mp4".to_string(), shot_sidecar("close"));
        shot.insert("raw/cam-b.mp4".to_string(), shot_sidecar("wide"));
        let quality = HashMap::new();
        let offsets = HashMap::new();

        // Speaker A is visible on cam-b even though cam-a is the tighter shot;
        // the +0.55 speaker bonus must win.
        let choice = choose_camera(
            &cameras,
            &face,
            &shot,
            &quality,
            &offsets,
            0.0,
            Some("A"),
            10.0,
        );
        assert_eq!(choice.asset, "raw/cam-b.mp4");
    }

    #[test]
    fn offset_corrected_lookup_picks_shifted_camera() {
        let cameras = vec!["raw/cam-a.mp4".to_string(), "raw/cam-b.mp4".to_string()];
        // cam-b started 2s late → placed at timeline offset +2. Speaker A's
        // face is at cam-b SOURCE time 8.0, i.e. program time 10.0.
        let mut face = HashMap::new();
        face.insert(
            "raw/cam-b.mp4".to_string(),
            face_sidecar("face_1", "A", 8.0),
        );
        let mut shot = HashMap::new();
        shot.insert("raw/cam-a.mp4".to_string(), shot_sidecar("close")); // +0.20
        shot.insert("raw/cam-b.mp4".to_string(), shot_sidecar("wide")); // +0.05
        let quality = HashMap::new();

        // Without offsets: cam-b is looked up at t=10 → no face → cam-a (close)
        // wins on shot bonus alone.
        let no_offset = choose_camera(
            &cameras,
            &face,
            &shot,
            &quality,
            &HashMap::new(),
            0.0,
            Some("A"),
            10.0,
        );
        assert_eq!(
            no_offset.asset, "raw/cam-a.mp4",
            "uncorrected lookup misses the shifted camera"
        );

        // With the +2 offset applied: cam-b is looked up at source t=8 → face
        // found → +0.55 speaker bonus flips the choice to cam-b.
        let mut offsets = HashMap::new();
        offsets.insert(
            "raw/cam-b.mp4".to_string(),
            SyncInfo {
                offset_s: 2.0,
                sync_group_id: Some("sync-ab".to_string()),
            },
        );
        let corrected = choose_camera(
            &cameras,
            &face,
            &shot,
            &quality,
            &offsets,
            0.0,
            Some("A"),
            10.0,
        );
        assert_eq!(
            corrected.asset, "raw/cam-b.mp4",
            "offset-corrected lookup finds the shifted speaker face"
        );
    }

    #[test]
    fn sync_offsets_reads_applied_effect() {
        let mut clip = Clip::empty("cam-b".to_string());
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/cam-b.mp4".to_string()));
        let mut effect = Effect::new(SYNC_GROUP_EFFECT_NAME);
        effect
            .metadata
            .insert("offset_s".into(), serde_json::json!(2.5));
        effect
            .metadata
            .insert("sync_group_id".into(), serde_json::json!("sync-ab"));
        clip.effects.push(effect);

        let mut track = Track::empty("Camera B".to_string(), TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = Timeline::empty("p");
        timeline.tracks.children.push(StackChild::Track(track));

        let offsets = sync_offsets(&timeline);
        let info = offsets.get("raw/cam-b.mp4").expect("offset present");
        assert!((info.offset_s - 2.5).abs() < 1e-9);
        assert_eq!(info.sync_group_id.as_deref(), Some("sync-ab"));
        // A camera with no sync_group effect reports the shared-timebase default.
        assert_eq!(offset_of(&offsets, "raw/cam-a.mp4"), 0.0);
    }

    #[test]
    fn parse_segments_reads_speaker_id_and_legacy_speaker() {
        // Real whisper sidecars label segments with `speaker_id`.
        let sidecar = serde_json::json!({
            "data": { "segments": [
                { "start_s": 0.0, "end_s": 1.0, "speaker_id": "A" },
                { "start_s": 1.0, "end_s": 2.0, "speaker": "B" },
                { "start_s": 2.0, "end_s": 3.0 },
            ] }
        });
        let segs = parse_segments(&sidecar);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].speaker.as_deref(), Some("A"), "reads speaker_id");
        assert_eq!(
            segs[1].speaker.as_deref(),
            Some("B"),
            "reads legacy speaker"
        );
        assert_eq!(segs[2].speaker, None);
    }
}
