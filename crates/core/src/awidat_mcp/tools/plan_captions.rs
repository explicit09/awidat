//! `plan_captions` — format-agnostic caption planner. Reads the whisper
//! transcript sidecar, segments via the readability model for the requested
//! format, applies the (format, mood) style, and returns CaptionRecommendations
//! plus a reviewable `*** Insert Caption` EDL fragment. Read-only; apply with
//! apply_edl after inspection. Never burns captions into the picture.

use awidat_index::{read_sidecar, SidecarError};
use awidat_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::caption::edl::build_caption_edl_lines;
use crate::caption::planner::{plan, LowerSafeZoneStrategy};
use crate::caption::readability::{lint, segment, CaptionFormatProfile, InputWord};
use crate::caption::styles::{resolve_style, CaptionFormat, CaptionMood};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanCaptionsArgs {
    /// Project-relative source asset id, e.g. raw/episode.mp4.
    pub asset_id: String,
    /// Timeline clip uuid/name used in EDL anchors.
    pub clip_id: String,
    /// Caption format: "short_form" | "long_form" | "accessibility".
    pub format: String,
    /// Mood register: "minimal_cinematic" | "active_pop".
    pub mood: String,
}

pub fn run(args: PlanCaptionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.asset_id.trim().is_empty() {
        return Err("plan_captions: asset_id must be non-empty.".into());
    }
    let format = parse_format(&args.format)?;
    let mood = parse_mood(&args.mood)?;
    let profile = match format {
        CaptionFormat::ShortForm => CaptionFormatProfile::short_form(),
        CaptionFormat::LongForm => CaptionFormatProfile::long_form(),
        CaptionFormat::Accessibility => CaptionFormatProfile::accessibility(),
    };

    let asset = AssetId::new(args.asset_id.clone());
    let transcript = match read_sidecar(&ctx.project_root, "whisper", &asset) {
        Ok(sidecar) => sidecar.get("data").cloned().unwrap_or(serde_json::Value::Null),
        Err(SidecarError::NotFound { .. }) => {
            return Err("plan_captions: no transcript index for this asset. Run indexing (whisper) first; captions need word timings.".into());
        }
        Err(err) => return Err(format!("plan_captions: failed to read whisper sidecar: {err}")),
    };

    let words = words_from_transcript(&transcript);
    if words.is_empty() {
        return Err("plan_captions: transcript index is empty for this asset; nothing to caption.".into());
    }

    let cues = segment(&words, &profile);
    let lint_proposals = lint(&cues, &profile);
    let recs = plan(&cues, &LowerSafeZoneStrategy);
    let spec = resolve_style(format, mood);
    let safe_area = if format == CaptionFormat::ShortForm { "mobile" } else { "standard" };

    let mut lines = vec!["*** Begin EDL".to_string()];
    lines.extend(build_caption_edl_lines(&recs, &spec, safe_area));
    lines.push("*** End EDL".to_string());
    let edl_fragment = lines.join("\n") + "\n";

    let body = serde_json::json!({
        "asset_id": args.asset_id,
        "clip_id": args.clip_id,
        "format": args.format,
        "mood": args.mood,
        "style": spec,
        "caption_plan": recs,
        "readability_lint": lint_proposals,
        "edl_fragment": edl_fragment,
        "verification_plan": [
            "Confirm no cue exceeds the 17 CPS reading ceiling.",
            "Confirm captions sit in the lower safe zone and clear of faces.",
            "Inspect the timeline diff, then render and check the artifact frame."
        ],
    });
    serde_json::to_string_pretty(&body).map_err(|e| format!("plan_captions: serialization failed: {e}"))
}

fn parse_format(s: &str) -> Result<CaptionFormat, String> {
    match s.trim() {
        "short_form" => Ok(CaptionFormat::ShortForm),
        "long_form" => Ok(CaptionFormat::LongForm),
        "accessibility" => Ok(CaptionFormat::Accessibility),
        other => Err(format!("plan_captions: unknown format {other:?}; use short_form|long_form|accessibility.")),
    }
}

fn parse_mood(s: &str) -> Result<CaptionMood, String> {
    match s.trim() {
        "minimal_cinematic" => Ok(CaptionMood::MinimalCinematic),
        "active_pop" => Ok(CaptionMood::ActivePop),
        other => Err(format!("plan_captions: unknown mood {other:?}; use minimal_cinematic|active_pop.")),
    }
}

fn words_from_transcript(transcript: &serde_json::Value) -> Vec<InputWord> {
    let mut out = Vec::new();
    let Some(segments) = transcript.pointer("/segments").and_then(|v| v.as_array()) else {
        return out;
    };
    for seg in segments {
        let seg_start = num(seg, "start_s", num(seg, "start", 0.0));
        let seg_end = num(seg, "end_s", num(seg, "end", seg_start));
        match seg.get("words").and_then(|v| v.as_array()) {
            Some(ws) if !ws.is_empty() => {
                for w in ws {
                    let text = w.get("text").or_else(|| w.get("word")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    out.push(InputWord {
                        text,
                        start_s: num(w, "start_s", num(w, "start", seg_start)),
                        end_s: num(w, "end_s", num(w, "end", seg_end)),
                    });
                }
            }
            _ => {
                let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if !text.is_empty() {
                    out.push(InputWord { text, start_s: seg_start, end_s: seg_end });
                }
            }
        }
    }
    out
}

fn num(v: &serde_json::Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(default)
}

// consumed by the MCP router in the next task
#[allow(dead_code)]
pub const DESCRIPTION: &str = "\
Build a read-only, format-aware caption plan for one clip from its transcript \
index. Segments transcript words to a reading-speed ceiling (<=17 CPS) and the \
per-format characters-per-line / line-count targets (short_form, long_form, or \
accessibility), applies a (format, mood) style, and returns caption \
recommendations, a readability lint, and a reviewable Insert Caption EDL \
fragment. Apply separately with apply_edl after inspection. Never burns captions \
into the picture.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awidat_mcp::context::McpToolCtx;

    fn write_whisper(root: &std::path::Path, asset: &str, data: serde_json::Value) {
        let path = root.join("index").join("whisper").join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
            "indexer": "whisper", "asset_id": asset, "data": data,
        })).unwrap()).unwrap();
    }

    #[test]
    fn long_form_plan_emits_bottom_captions_under_cps_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mp4";
        write_whisper(dir.path(), asset, serde_json::json!({
            "segments": [{
                "start_s": 0.0, "end_s": 4.0,
                "text": "absolutely incredible breakthrough today",
                "words": [
                    {"text": "absolutely", "start_s": 0.0, "end_s": 1.0},
                    {"text": "incredible", "start_s": 1.0, "end_s": 2.0},
                    {"text": "breakthrough", "start_s": 2.0, "end_s": 3.0},
                    {"text": "today", "start_s": 3.0, "end_s": 4.0}
                ]
            }]
        }));
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let out = run(PlanCaptionsArgs {
            asset_id: asset.into(), clip_id: "clip-1".into(),
            format: "long_form".into(), mood: "minimal_cinematic".into(),
        }, ctx).unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(!body["caption_plan"].as_array().unwrap().is_empty(), "should produce captions");
        assert!(body["edl_fragment"].as_str().unwrap().contains("*** Insert Caption"));
        assert_eq!(body["format"], "long_form");
    }

    #[test]
    fn missing_transcript_is_a_clear_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let err = run(PlanCaptionsArgs {
            asset_id: "raw/none.mp4".into(), clip_id: "c".into(),
            format: "long_form".into(), mood: "minimal_cinematic".into(),
        }, ctx).unwrap_err();
        assert!(err.to_lowercase().contains("transcript") || err.to_lowercase().contains("index"));
    }

    #[test]
    fn unknown_format_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let err = run(PlanCaptionsArgs {
            asset_id: "raw/x.mp4".into(), clip_id: "c".into(),
            format: "square".into(), mood: "minimal_cinematic".into(),
        }, ctx).unwrap_err();
        assert!(err.contains("format"));
    }
}
