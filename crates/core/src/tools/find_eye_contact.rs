//! `find_eye_contact` tool — when is someone looking at the camera?
//!
//! Reads the gaze sidecar's `per_frame[].faces[].at_camera` flag and
//! returns contiguous ranges where at least one face on screen has
//! `at_camera == true`. The signal is the editorial moment of direct
//! address: the host turning to camera for a question, the guest's
//! eye-contact while delivering a punchline.
//!
//! Filtering by speaker is supported when the face sidecar's
//! `speaker_to_face` mapping is present — we project the speaker's
//! mapped face_id onto the gaze sidecar's per-face entries (which
//! lack face_ids — gaze runs without clustering — so the projection
//! is positional: same frame, same box overlap > 0.7).

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `find_eye_contact` tool.
pub struct FindEyeContactTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Restrict to one asset id; otherwise scan all gaze sidecars.
    #[serde(default)]
    asset_id: Option<String>,
    /// Minimum range duration in seconds. Below this, single-frame
    /// at-camera blips are dropped. Default 1.0s.
    #[serde(default)]
    min_duration_s: Option<f64>,
    /// Optional speaker label to project through the face sidecar's
    /// speaker_to_face mapping. Only ranges where this speaker's face
    /// is the at-camera face are returned. If unset, any face's
    /// at-camera state contributes.
    #[serde(default)]
    speaker: Option<String>,
    /// Max ranges to return. Default 50, hard cap 200.
    #[serde(default)]
    limit: Option<usize>,
}

/// Box-IOU helper. Both inputs are `[top, right, bottom, left]` per
/// the face-mcp / gaze-mcp convention.
fn iou(a: &[i64; 4], b: &[i64; 4]) -> f64 {
    let (at, ar, ab, al) = (a[0], a[1], a[2], a[3]);
    let (bt, br, bb, bl) = (b[0], b[1], b[2], b[3]);
    let it = at.max(bt);
    let ir = ar.min(br);
    let ib = ab.min(bb);
    let il = al.max(bl);
    if ir <= il || ib <= it {
        return 0.0;
    }
    let inter = ((ir - il) * (ib - it)) as f64;
    let area_a = ((ar - al) * (ab - at)) as f64;
    let area_b = ((br - bl) * (bb - bt)) as f64;
    let union = area_a + area_b - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn parse_box(v: &serde_json::Value) -> Option<[i64; 4]> {
    let arr = v.as_array()?;
    if arr.len() != 4 {
        return None;
    }
    Some([
        arr[0].as_i64()?,
        arr[1].as_i64()?,
        arr[2].as_i64()?,
        arr[3].as_i64()?,
    ])
}

#[async_trait]
impl ToolHandler for FindEyeContactTool {
    fn name(&self) -> &'static str {
        "find_eye_contact"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_eye_contact".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's gaze sidecar."
                    },
                    "min_duration_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Drop ranges shorter than this. Default 1.0s."
                    },
                    "speaker": {
                        "type": "string",
                        "description": "Filter to one speaker via the face sidecar's speaker_to_face mapping. Requires both face and gaze indexers, with whisper diarization having run before face."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max ranges to return. Default 50."
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
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_eye_contact: invalid args ({e}). All fields optional."
            ))
        })?;
        let min_duration_s = args.min_duration_s.unwrap_or(1.0);
        let limit = args.limit.unwrap_or(50).min(200);

        // For the optional speaker filter, pre-load the face sidecars
        // we'll need to project face_id → box-at-frame.
        let face_walker = walk_indexer(&ctx.project_root, "face").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_eye_contact: face sidecars not readable ({e}). \
                 Run `awidat index --indexer face <project>` and retry."
            ))
        })?;
        let face_by_asset: std::collections::HashMap<String, serde_json::Value> =
            face_walker.collect();

        let gaze_walker = walk_indexer(&ctx.project_root, "gaze").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_eye_contact: gaze sidecars not readable ({e}). \
                 Run `awidat index --indexer gaze <project>` and retry."
            ))
        })?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut more = false;
        for (asset_id, sidecar) in gaze_walker {
            if let Some(filter) = &args.asset_id
                && filter != &asset_id
            {
                continue;
            }
            let Some(data) = sidecar.get("data") else {
                continue;
            };
            let period_s = data
                .get("frame_rate_sampled")
                .and_then(|v| v.as_f64())
                .map(|f| if f > 0.0 { 1.0 / f } else { 1.0 })
                .unwrap_or(1.0);
            let frames = data
                .pointer("/per_frame")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // If a speaker filter is set, build a frame-indexed lookup
            // of the speaker's face_id → box for *this* asset's face
            // sidecar. None means we skip the speaker filter for this
            // asset (face data missing).
            let speaker_box_per_t: Option<std::collections::HashMap<i64, [i64; 4]>> =
                if let Some(speaker) = args.speaker.as_deref() {
                    face_by_asset.get(&asset_id).and_then(|face_doc| {
                        let face_data = face_doc.get("data")?;
                        let face_id = face_data
                            .pointer("/speaker_to_face")
                            .and_then(|m| m.as_object())
                            .and_then(|m| m.get(speaker))
                            .and_then(|v| v.as_str())?;
                        let face_frames =
                            face_data.pointer("/per_frame").and_then(|v| v.as_array())?;
                        let mut out = std::collections::HashMap::new();
                        for fr in face_frames {
                            let t = fr.get("t_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            // Quantize to milliseconds-as-i64 so the
                            // hashmap key is stable across float
                            // round-tripping between sidecars.
                            let key = (t * 1000.0).round() as i64;
                            let faces = fr.get("faces").and_then(|v| v.as_array());
                            let Some(faces) = faces else { continue };
                            for f in faces {
                                if f.get("face_id").and_then(|v| v.as_str()) == Some(face_id) {
                                    if let Some(b) = f.get("box").and_then(parse_box) {
                                        out.insert(key, b);
                                    }
                                    break;
                                }
                            }
                        }
                        Some(out)
                    })
                } else {
                    None
                };

            let mut run_start: Option<f64> = None;
            let mut last_t: f64 = 0.0;
            for frame in &frames {
                let t = frame.get("t_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let faces = frame
                    .get("faces")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Decide whether THIS frame counts as "eye contact."
                let mut at_camera = faces.iter().any(|f| {
                    f.get("at_camera")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                });
                if at_camera && let Some(map) = &speaker_box_per_t {
                    // Project the speaker's box onto the at-camera face;
                    // if no overlapping detection in face sidecar at this
                    // t_s, the speaker isn't the one looking at camera.
                    let key = (t * 1000.0).round() as i64;
                    if let Some(speaker_box) = map.get(&key) {
                        // Find the gaze face whose box best overlaps the
                        // speaker's face box.
                        let mut best_iou = 0.0;
                        let mut best_at_cam = false;
                        for f in &faces {
                            let Some(b) = f.get("box").and_then(parse_box) else {
                                continue;
                            };
                            let i = iou(&b, speaker_box);
                            if i > best_iou {
                                best_iou = i;
                                best_at_cam = f
                                    .get("at_camera")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                            }
                        }
                        // Require IoU > 0.3 to consider it the same face.
                        // Below that the gaze and face sidecars likely
                        // disagree on detection at this frame; skip.
                        at_camera = best_iou > 0.3 && best_at_cam;
                    } else {
                        at_camera = false;
                    }
                }

                if at_camera {
                    if run_start.is_none() {
                        run_start = Some(t);
                    }
                    last_t = t;
                } else if let Some(start) = run_start.take() {
                    let end = last_t + period_s;
                    if end - start >= min_duration_s {
                        results.push(serde_json::json!({
                            "asset_id": asset_id,
                            "start_s": start,
                            "end_s": end,
                        }));
                        if results.len() >= limit {
                            more = true;
                            break;
                        }
                    }
                }
            }
            if let Some(start) = run_start {
                let end = last_t + period_s;
                if end - start >= min_duration_s {
                    results.push(serde_json::json!({
                        "asset_id": asset_id,
                        "start_s": start,
                        "end_s": end,
                    }));
                    if results.len() >= limit {
                        more = true;
                    }
                }
            }
            if more {
                break;
            }
        }

        Ok(ToolOutput::text(
            serde_json::json!({
                "results": results,
                "more_available": more,
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Find time ranges where at least one face on screen is looking at the \
camera (gaze score within the at-camera threshold). Reads the gaze \
sidecar; optionally filters by speaker via the face sidecar's \
speaker→face mapping. Use this for moments of direct address — host \
breaking the fourth wall, guest delivering a punchline to camera, \
the closing pitch.\
\n\n\
Examples:\
\n  find_eye_contact()                  → every direct-address range\
\n  find_eye_contact(speaker='A')       → only when speaker A is the \
one looking at camera (requires diarization + face mapping)\
\n  find_eye_contact(min_duration_s=2)  → only sustained looks, not \
quick glances\
\n\n\
Requires: gaze indexer must have run. The speaker filter further \
requires face indexer + whisper diarization run before face-mcp.\
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
            name: "find_eye_contact".into(),
            args,
        }
    }

    fn write_gaze(root: &std::path::Path, asset: &str, per_frame: Vec<serde_json::Value>) {
        let p = root.join("index/gaze").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "gaze",
                "asset_id": asset,
                "data": {
                    "frame_rate_sampled": 1.0,
                    "per_frame": per_frame,
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn finds_at_camera_runs_without_speaker_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_gaze(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"t_s": 0.0, "faces": [{"at_camera": true, "box": [0,100,100,0]}]}),
                serde_json::json!({"t_s": 1.0, "faces": [{"at_camera": true, "box": [0,100,100,0]}]}),
                serde_json::json!({"t_s": 2.0, "faces": [{"at_camera": false, "box": [0,100,100,0]}]}),
                serde_json::json!({"t_s": 3.0, "faces": [{"at_camera": true, "box": [0,100,100,0]}]}),
                serde_json::json!({"t_s": 4.0, "faces": [{"at_camera": true, "box": [0,100,100,0]}]}),
            ],
        );
        let out = FindEyeContactTool
            .handle(
                invoke(serde_json::json!({"min_duration_s": 0.5})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let r = v["results"].as_array().unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["start_s"], 0.0);
        assert_eq!(r[0]["end_s"], 2.0);
        assert_eq!(r[1]["start_s"], 3.0);
        assert_eq!(r[1]["end_s"], 5.0);
    }

    #[tokio::test]
    async fn empty_when_no_gaze_index() {
        let dir = tempfile::tempdir().unwrap();
        let out = FindEyeContactTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn iou_helper_basic_cases() {
        // Identical box.
        let a = [0i64, 100, 100, 0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-9);
        // Non-overlapping.
        let b = [200i64, 300, 300, 200];
        assert_eq!(iou(&a, &b), 0.0);
        // Half-overlap.
        let c = [0i64, 100, 100, 50];
        assert!((iou(&a, &c) - 0.5).abs() < 1e-9);
    }
}
