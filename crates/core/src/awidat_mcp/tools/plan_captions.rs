//! `plan_captions` — format-agnostic caption planner. Reads the whisper
//! transcript sidecar, segments via the readability model for the requested
//! format, applies the (format, mood) style, and returns CaptionRecommendations
//! plus a reviewable `*** Insert Caption` EDL fragment. Read-only; apply with
//! apply_edl after inspection. Never burns captions into the picture.

use awidat_index::{SidecarError, read_sidecar};
use awidat_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::caption::edl::build_caption_edl_lines;
use crate::caption::planner::{LowerSafeZoneStrategy, plan};
use crate::caption::readability::{CaptionFormatProfile, lint, segment, words_from_transcript};
use crate::caption::styles::{CaptionFormat, CaptionMood, resolve_preset, resolve_style};

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
    /// Optional named style preset; overrides (format, mood) when set.
    /// Values: clean_white | word_pop | boxed | emphasis.
    #[serde(default)]
    pub preset: Option<String>,
    /// Zero-based cue indices (into the returned recommendations) the agent has
    /// judged to be hook/keyword/payoff lines. Those cues render with the poppier
    /// `emphasis` preset; the rest keep the default. Empty = no emphasis (default).
    /// Restraint is the agent's call — reserve this for the 1-2 lines that carry.
    #[serde(default)]
    pub emphasis_line_indices: Vec<usize>,
}

pub fn run(args: PlanCaptionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.asset_id.trim().is_empty() {
        return Err("plan_captions: asset_id must be non-empty.".into());
    }
    let format = parse_format(&args.format)?;
    if format == CaptionFormat::ShortForm {
        return Err("plan_captions: short_form is not supported here; use plan_scene_aware_short_form for scene-aware vertical short-form captioning.".into());
    }
    let mood = parse_mood(&args.mood)?;
    let profile = match format {
        CaptionFormat::ShortForm => CaptionFormatProfile::short_form(),
        CaptionFormat::LongForm => CaptionFormatProfile::long_form(),
        CaptionFormat::Accessibility => CaptionFormatProfile::accessibility(),
    };

    let asset = AssetId::new(args.asset_id.clone());
    let transcript = match read_sidecar(&ctx.project_root, "whisper", &asset) {
        Ok(sidecar) => sidecar
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        Err(SidecarError::NotFound { .. }) => {
            return Err("plan_captions: no transcript index for this asset. Run indexing (whisper) first; captions need word timings.".into());
        }
        Err(err) => {
            return Err(format!(
                "plan_captions: failed to read whisper sidecar: {err}"
            ));
        }
    };

    let words = words_from_transcript(&transcript);
    if words.is_empty() {
        return Err(
            "plan_captions: transcript index is empty for this asset; nothing to caption.".into(),
        );
    }

    let cues = segment(&words, &profile);
    let lint_proposals = lint(&cues, &profile);
    let mut recs = plan(&cues, &LowerSafeZoneStrategy);
    // The agent flags hook/keyword/payoff cues by index; those render with the
    // poppier `emphasis` preset. Out-of-range indices are ignored.
    for &idx in &args.emphasis_line_indices {
        if let Some(rec) = recs.get_mut(idx) {
            rec.is_emphasized = true;
        }
    }
    let emphasis_spec = resolve_preset("emphasis");
    let spec = match args.preset.as_deref().and_then(resolve_preset) {
        Some(s) => s,
        None => resolve_style(format, mood),
    };
    // Opportunistic placement: a busy bottom region (e.g. a burned-in lower-third)
    // raises captions clear of it; absent composition data, the standard band is
    // the floor. (short_form is rejected earlier in run().)
    let safe_area = if composition_bottom_busy(&ctx.project_root, &asset) {
        "lower_third"
    } else {
        "standard"
    };

    let mut lines = vec!["*** Begin EDL".to_string()];
    lines.extend(build_caption_edl_lines(
        &recs,
        &spec,
        emphasis_spec.as_ref(),
        safe_area,
    ));
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
    serde_json::to_string_pretty(&body)
        .map_err(|e| format!("plan_captions: serialization failed: {e}"))
}

fn composition_bottom_busy(project_root: &std::path::Path, asset: &AssetId) -> bool {
    use crate::scene_aware_short_form::{caption_placement_from_str, composition_zones};
    let composition = match read_sidecar(project_root, "composition", asset) {
        Ok(s) => s.get("data").cloned().unwrap_or(serde_json::Value::Null),
        Err(_) => return false,
    };
    let busy = composition_zones(
        &composition,
        0.0,
        f64::MAX,
        &["busy_regions", "unsafe_text_zones", "protected_regions"],
    );
    match caption_placement_from_str("bottom") {
        Some(bottom) => busy.contains(&bottom),
        None => false,
    }
}

fn parse_format(s: &str) -> Result<CaptionFormat, String> {
    match s.trim() {
        "short_form" => Ok(CaptionFormat::ShortForm),
        "long_form" => Ok(CaptionFormat::LongForm),
        "accessibility" => Ok(CaptionFormat::Accessibility),
        other => Err(format!(
            "plan_captions: unknown format {other:?}; use short_form|long_form|accessibility."
        )),
    }
}

fn parse_mood(s: &str) -> Result<CaptionMood, String> {
    match s.trim() {
        "minimal_cinematic" => Ok(CaptionMood::MinimalCinematic),
        "active_pop" => Ok(CaptionMood::ActivePop),
        other => Err(format!(
            "plan_captions: unknown mood {other:?}; use minimal_cinematic|active_pop."
        )),
    }
}

pub const DESCRIPTION: &str = "\
Build a read-only, format-aware caption plan for one clip from its transcript \
index. Supports long_form and accessibility formats only (use \
plan_scene_aware_short_form for vertical short-form). Segments transcript words \
to a <=17 CPS reading ceiling with per-format characters-per-line targets, \
applies a (format, mood) style, and returns caption recommendations, a \
readability lint, and a reviewable Insert Caption EDL fragment. Pass the \
optional `preset` field (values: clean_white | word_pop | boxed | emphasis) to \
override the (format, mood) style with a named preset. Pass \
`emphasis_line_indices` (zero-based cue indices) to render the 1-2 \
hook/keyword/payoff lines you judge as carrying the moment with the poppier \
`emphasis` look while the rest stay default; reserve it for those few lines, \
not every cue. Note: accessibility uses whole-cue reveal regardless of mood. \
Apply with apply_edl after inspection. Never burns captions into the picture.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awidat_mcp::context::McpToolCtx;

    fn write_whisper(root: &std::path::Path, asset: &str, data: serde_json::Value) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "whisper", "asset_id": asset, "data": data,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_composition(root: &std::path::Path, asset: &str, data: serde_json::Value) {
        let path = root
            .join("index")
            .join("composition")
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "composition", "asset_id": asset, "data": data,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn explicit_preset_drives_style_json() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper(
            dir.path(),
            asset,
            serde_json::json!({
                "words": [{"text":"hi","start_s":0.0,"end_s":1.0}], "segments":[{"text":"hi","start_s":0.0,"end_s":1.0}]
            }),
        );
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let out = run(
            PlanCaptionsArgs {
                asset_id: asset.into(),
                clip_id: "c".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: Some("word_pop".into()),
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("\"reveal\":\"active_word_pop\""),
            "explicit preset must drive style_json: {}",
            body["edl_fragment"]
        );
    }

    #[test]
    fn emphasis_line_indices_box_only_the_flagged_cue() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Two sentences -> at least two cues; long_form default is clean_white
        // (no box), emphasis preset is boxed.
        write_whisper(
            dir.path(),
            asset,
            serde_json::json!({
                "words": [
                    {"text":"First","start_s":0.0,"end_s":0.5},
                    {"text":"sentence","start_s":0.5,"end_s":1.0},
                    {"text":"here.","start_s":1.0,"end_s":1.6},
                    {"text":"Second","start_s":2.0,"end_s":2.5},
                    {"text":"sentence","start_s":2.5,"end_s":3.0},
                    {"text":"now.","start_s":3.0,"end_s":3.6}
                ],
                "segments": []
            }),
        );
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let out = run(
            PlanCaptionsArgs {
                asset_id: asset.into(),
                clip_id: "c".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![0],
            },
            ctx,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        let edl = body["edl_fragment"].as_str().unwrap();
        assert_eq!(
            edl.matches("\"kind\":\"box\"").count(),
            1,
            "only the flagged cue should be boxed (emphasis): {edl}"
        );
        // The plan also reports which cue is emphasized.
        let plan = body["caption_plan"].as_array().unwrap();
        assert!(plan[0]["is_emphasized"].as_bool().unwrap());
        assert!(
            plan.iter()
                .skip(1)
                .all(|c| !c["is_emphasized"].as_bool().unwrap())
        );
    }

    #[test]
    fn busy_bottom_region_raises_safe_area_to_lower_third() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper(
            dir.path(),
            asset,
            serde_json::json!({
                "words": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}],
                "segments": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}]
            }),
        );
        write_composition(
            dir.path(),
            asset,
            serde_json::json!({
                "regions": [{"start_s": 0.0, "end_s": 60.0, "busy_regions": ["bottom"]}]
            }),
        );
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let out = run(
            PlanCaptionsArgs {
                asset_id: asset.into(),
                clip_id: "c".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("+ safe_area: lower_third"),
            "busy bottom must raise captions: {}",
            body["edl_fragment"]
        );
    }

    #[test]
    fn no_composition_defaults_to_standard_safe_area() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper(
            dir.path(),
            asset,
            serde_json::json!({
                "words": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}],
                "segments": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}]
            }),
        );
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let out = run(
            PlanCaptionsArgs {
                asset_id: asset.into(),
                clip_id: "c".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("+ safe_area: standard"),
            "no composition sidecar must default to standard: {}",
            body["edl_fragment"]
        );
    }

    #[test]
    fn long_form_plan_emits_bottom_captions_under_cps_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mp4";
        write_whisper(
            dir.path(),
            asset,
            serde_json::json!({
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
            }),
        );
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let out = run(
            PlanCaptionsArgs {
                asset_id: asset.into(),
                clip_id: "clip-1".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            !body["caption_plan"].as_array().unwrap().is_empty(),
            "should produce captions"
        );
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("*** Insert Caption")
        );
        assert_eq!(body["format"], "long_form");
    }

    #[test]
    fn missing_transcript_is_a_clear_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let err = run(
            PlanCaptionsArgs {
                asset_id: "raw/none.mp4".into(),
                clip_id: "c".into(),
                format: "long_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap_err();
        assert!(err.to_lowercase().contains("transcript") || err.to_lowercase().contains("index"));
    }

    #[test]
    fn unknown_format_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let err = run(
            PlanCaptionsArgs {
                asset_id: "raw/x.mp4".into(),
                clip_id: "c".into(),
                format: "square".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap_err();
        assert!(err.contains("format"));
    }

    #[test]
    fn short_form_is_rejected_pointing_to_scene_aware() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx {
            project_root: dir.path().to_path_buf(),
        };
        let err = run(
            PlanCaptionsArgs {
                asset_id: "raw/x.mp4".into(),
                clip_id: "c".into(),
                format: "short_form".into(),
                mood: "minimal_cinematic".into(),
                preset: None,
                emphasis_line_indices: vec![],
            },
            ctx,
        )
        .unwrap_err();
        assert!(
            err.contains("plan_scene_aware_short_form"),
            "should point to the short-form tool: {err}"
        );
    }
}
