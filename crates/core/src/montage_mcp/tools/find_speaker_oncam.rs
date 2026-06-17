//! `find_speaker_oncam` — when is a given speaker's face on screen?
//!
//! Ported from `crates/core/src/tools/find_speaker_oncam.rs` to the
//! in-process MCP server. Reads the face sidecar's `speaker_to_face`
//! mapping and walks `per_frame` to find every contiguous range where
//! that speaker's mapped face_id was detected.

use montage_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `find_speaker_oncam`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindSpeakerOncamArgs {
    /// Speaker label to look up (must match the face sidecar's
    /// `speaker_to_face` keys, typically "A"/"B"/"C" from whisper
    /// diarization).
    pub speaker: String,
    /// Restrict to one asset id; otherwise scan all face sidecars.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Minimum range duration in seconds. Shorter on-screen blips
    /// (single-second detections that drop the next second) are
    /// usually noise — default 1.0s suppresses them.
    #[serde(default)]
    pub min_duration_s: Option<f64>,
    /// Max ranges to return. Default 50, hard cap 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Run `find_speaker_oncam` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; sidecar-walk
/// errors return `Err(String)`.
pub fn run(args: FindSpeakerOncamArgs, ctx: McpToolCtx) -> Result<String, String> {
    let min_duration_s = args.min_duration_s.unwrap_or(1.0);
    let limit = args.limit.unwrap_or(50).min(200);

    let walker = walk_indexer(&ctx.project_root, "face").map_err(|e| {
        format!(
            "find_speaker_oncam: face sidecars not readable ({e}). \
             Run `montage index --indexer face <project>` and retry."
        )
    })?;

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut more = false;
    let mut speaker_seen_anywhere = false;
    for (asset_id, sidecar) in walker {
        if let Some(filter) = &args.asset_id
            && filter != &asset_id
        {
            continue;
        }
        let Some(data) = sidecar.get("data") else {
            continue;
        };

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
                 before face-mcp. Add your Hugging Face key in Settings -> Advanced -> \
                 Provider Keys, run `montage index --indexer whisper`, then re-run \
                 `montage index --indexer face`.",
                args.speaker
            ),
        })
    } else {
        serde_json::json!({
            "results": results,
            "more_available": more,
        })
    };
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
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
