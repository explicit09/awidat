//! `find_speaker_oncam` tool — when is a given speaker's face on screen?
//!
//! Reads the face sidecar's `speaker_to_face` mapping (filled in when
//! whisper diarization ran first) and walks `per_frame` to find every
//! contiguous range where that speaker's mapped face_id was detected.
//! Returns time ranges, not per-frame data — the agent decides what to
//! do with them (cut to a reaction shot, hide a B-roll, find moments
//! of direct address).

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `find_speaker_oncam` tool.
pub struct FindSpeakerOncamTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Speaker label to look up (must match the face sidecar's
    /// `speaker_to_face` keys, typically "A"/"B"/"C" from whisper
    /// diarization).
    speaker: String,
    /// Restrict to one asset id; otherwise scan all face sidecars.
    #[serde(default)]
    asset_id: Option<String>,
    /// Minimum range duration in seconds. Shorter on-screen blips
    /// (single-second detections that drop the next second) are
    /// usually noise — default 1.0s suppresses them.
    #[serde(default)]
    min_duration_s: Option<f64>,
    /// Max ranges to return. Default 50, hard cap 200.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindSpeakerOncamTool {
    fn name(&self) -> &'static str {
        "find_speaker_oncam"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_speaker_oncam".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "speaker": {
                        "type": "string",
                        "description": "Speaker label from whisper diarization (e.g. 'A', 'B'). Must exist in the face sidecar's speaker_to_face map."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's face sidecar."
                    },
                    "min_duration_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Drop ranges shorter than this. Default 1.0s."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max ranges to return. Default 50."
                    }
                },
                "required": ["speaker"]
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
                "find_speaker_oncam: invalid args ({e}). 'speaker' is required."
            ))
        })?;
        let min_duration_s = args.min_duration_s.unwrap_or(1.0);
        let limit = args.limit.unwrap_or(50).min(200);

        let walker = walk_indexer(&ctx.project_root, "face")
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut more = false;
        let mut speaker_seen_anywhere = false;
        for (asset_id, sidecar) in walker {
            if let Some(filter) = &args.asset_id
                && filter != &asset_id
            {
                continue;
            }
            let Some(data) = sidecar.get("data") else { continue };

            // Look up the face_id this speaker maps to. Empty mapping
            // means diarization wasn't available when face-mcp ran —
            // we surface that as a hint, not silent zero results.
            let target_face_id = match data
                .pointer("/speaker_to_face")
                .and_then(|m| m.as_object())
                .and_then(|m| m.get(&args.speaker))
                .and_then(|v| v.as_str())
            {
                Some(fid) => fid.to_string(),
                None => continue,
            };
            speaker_seen_anywhere = true;

            // Walk per_frame, accumulating contiguous runs where the
            // target face_id appears in the frame's `faces` list.
            let frames = data
                .pointer("/per_frame")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            // Detect the sample period from the sidecar; fall back to
            // 1 fps if missing.
            let period_s = data
                .get("frame_rate_sampled")
                .and_then(|v| v.as_f64())
                .map(|f| if f > 0.0 { 1.0 / f } else { 1.0 })
                .unwrap_or(1.0);

            let mut run_start: Option<f64> = None;
            let mut last_t: f64 = 0.0;
            for frame in &frames {
                let t = frame.get("t_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let faces = frame
                    .get("faces")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let on_screen = faces.iter().any(|f| {
                    f.get("face_id").and_then(|v| v.as_str()) == Some(target_face_id.as_str())
                });

                if on_screen {
                    if run_start.is_none() {
                        run_start = Some(t);
                    }
                    last_t = t;
                } else if let Some(start) = run_start.take() {
                    // Run just ended; close it at last_t + one sample.
                    let end = last_t + period_s;
                    if end - start >= min_duration_s {
                        results.push(serde_json::json!({
                            "asset_id": asset_id,
                            "start_s": start,
                            "end_s": end,
                            "face_id": target_face_id,
                        }));
                        if results.len() >= limit {
                            more = true;
                            break;
                        }
                    }
                }
            }
            // Flush trailing run.
            if let Some(start) = run_start {
                let end = last_t + period_s;
                if end - start >= min_duration_s {
                    results.push(serde_json::json!({
                        "asset_id": asset_id,
                        "start_s": start,
                        "end_s": end,
                        "face_id": target_face_id,
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

        let body = if !speaker_seen_anywhere {
            serde_json::json!({
                "results": [],
                "more_available": false,
                "hint": format!(
                    "speaker '{}' not found in any face sidecar's speaker_to_face mapping. \
                     The mapping is only populated when whisper-mcp ran with diarization \
                     before face-mcp. Run `awidat index --indexer whisper` first \
                     (with HF_TOKEN set), then re-run `awidat index --indexer face`.",
                    args.speaker
                ),
            })
        } else {
            serde_json::json!({
                "results": results,
                "more_available": more,
            })
        };
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Find time ranges where a given speaker's face is on screen. Reads the \
face indexer's per_frame data and the speaker→face_id mapping that \
gets populated when whisper diarization ran before face-mcp.\
\n\n\
Use this for editorial decisions about reaction shots, B-roll \
overlay timing, and direct-address sequences. The agent should \
think of it as 'when can I cut TO this person'.\
\n\n\
Examples:\
\n  find_speaker_oncam(speaker='B') → every range where speaker B's \
face is visible\
\n  find_speaker_oncam(speaker='A', min_duration_s=3) → only the \
sustained on-camera moments\
\n\n\
If speaker→face mapping isn't populated, the tool returns a hint \
explaining why (usually whisper diarization didn't run).\
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
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "find_speaker_oncam".into(),
            args,
        }
    }

    fn write_face_sidecar(
        root: &std::path::Path,
        asset: &str,
        speaker_to_face: serde_json::Value,
        per_frame: Vec<serde_json::Value>,
    ) {
        let p = root.join("index/face").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "face",
                "asset_id": asset,
                "data": {
                    "frame_rate_sampled": 1.0,
                    "speaker_to_face": speaker_to_face,
                    "per_frame": per_frame,
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn finds_contiguous_ranges_for_speaker() {
        // Speaker A is mapped to f_0. Frames 0,1,2 have f_0; frame 3
        // has f_1; frames 4,5 have f_0 again. Expect two ranges:
        // [0, 3] and [4, 6].
        let dir = tempfile::tempdir().unwrap();
        write_face_sidecar(
            dir.path(),
            "raw/ep.mp4",
            serde_json::json!({"A": "f_0", "B": "f_1"}),
            vec![
                serde_json::json!({"t_s": 0.0, "faces": [{"face_id": "f_0"}]}),
                serde_json::json!({"t_s": 1.0, "faces": [{"face_id": "f_0"}]}),
                serde_json::json!({"t_s": 2.0, "faces": [{"face_id": "f_0"}]}),
                serde_json::json!({"t_s": 3.0, "faces": [{"face_id": "f_1"}]}),
                serde_json::json!({"t_s": 4.0, "faces": [{"face_id": "f_0"}]}),
                serde_json::json!({"t_s": 5.0, "faces": [{"face_id": "f_0"}]}),
            ],
        );
        let out = FindSpeakerOncamTool
            .handle(
                invoke(serde_json::json!({"speaker": "A", "min_duration_s": 0.5})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["start_s"], 0.0);
        assert_eq!(results[0]["end_s"], 3.0);
        assert_eq!(results[1]["start_s"], 4.0);
        assert_eq!(results[1]["end_s"], 6.0);
    }

    #[tokio::test]
    async fn missing_speaker_returns_hint() {
        let dir = tempfile::tempdir().unwrap();
        write_face_sidecar(
            dir.path(),
            "raw/ep.mp4",
            serde_json::json!({}),
            vec![],
        );
        let out = FindSpeakerOncamTool
            .handle(
                invoke(serde_json::json!({"speaker": "A"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
        assert!(
            v["hint"]
                .as_str()
                .unwrap()
                .contains("not found in any face sidecar")
        );
    }

    #[tokio::test]
    async fn min_duration_filters_short_runs() {
        // Single-frame on-screen blip at t=2; should be filtered when
        // min_duration_s=1.5 (the run is exactly one sample = 1.0s).
        let dir = tempfile::tempdir().unwrap();
        write_face_sidecar(
            dir.path(),
            "raw/ep.mp4",
            serde_json::json!({"A": "f_0"}),
            vec![
                serde_json::json!({"t_s": 0.0, "faces": []}),
                serde_json::json!({"t_s": 1.0, "faces": []}),
                serde_json::json!({"t_s": 2.0, "faces": [{"face_id": "f_0"}]}),
                serde_json::json!({"t_s": 3.0, "faces": []}),
            ],
        );
        let out = FindSpeakerOncamTool
            .handle(
                invoke(serde_json::json!({"speaker": "A", "min_duration_s": 1.5})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
    }
}
