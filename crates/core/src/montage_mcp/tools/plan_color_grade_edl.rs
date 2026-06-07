//! EDL emission + registry parameter mapping for `plan_color_grade`.
//!
//! Split out of `plan_color_grade.rs` so the tool module stays focused on
//! the async run flow (frame sampling, orchestration, artifact writing)
//! while this sibling owns the pure, I/O-light concerns: shaper
//! resolution, building registry parameter maps, and rendering EDL op
//! text. Everything here is synchronous and easily unit-tested.

use serde_json::{Map, Value};

use crate::color_analysis::CorrectionRecommendation;
use montage_effects::COLOR_PIPELINE;

/// Resolved look plan ready for EDL emission and validation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LookPlan {
    /// Decoded-pixel input space.
    pub clip_input_space: String,
    /// Space the look LUT was authored in.
    pub lut_input_space: String,
    /// Project-relative `.cube` look path.
    pub look_lut: String,
    /// Blend strength, clamped to `0.0..=1.0`.
    pub look_strength: f64,
    /// Optional project-relative shaper `.csp` path (Log sources only).
    pub shaper_lut: Option<String>,
}

/// The full set of camera-Log input spaces that require a Log→display
/// shaper before a Rec.709-authored look LUT is applied.
pub(crate) const CAMERA_LOG_SPACES: &[&str] = &[
    "arri_logc3",
    "arri_logc4",
    "slog3_sgamut3",
    "vlog_vgamut",
    "redlog_filmgen5",
    "canon_log2",
    "canon_log3",
    "bmd_film_gen5",
];

/// The subset of [`CAMERA_LOG_SPACES`] for which a bundled
/// `<space>_to_rec709_g24.csp` shaper ships with the montage skills bundle.
pub(crate) const BUNDLED_SHAPER_SPACES: &[&str] = &[
    "arri_logc3",
    "arri_logc4",
    "slog3_sgamut3",
    "vlog_vgamut",
    "bmd_film_gen5",
];

/// True when `space` is a camera-Log encoding (any of [`CAMERA_LOG_SPACES`]).
pub(crate) fn is_camera_log_space(space: &str) -> bool {
    CAMERA_LOG_SPACES.contains(&space)
}

/// Map a camera-Log input space to its bundled shaper stem
/// (`<space>_to_rec709_g24`) when a bundled `.csp` exists for it. Returns
/// `None` for non-Log spaces AND for Log spaces without a bundled shaper.
pub(crate) fn shaper_stem_for_space(space: &str) -> Option<String> {
    if BUNDLED_SHAPER_SPACES.contains(&space) {
        Some(format!("{space}_to_rec709_g24"))
    } else {
        None
    }
}

/// Build the registry parameter map for the recommended correction.
pub(crate) fn correction_params(rec: &CorrectionRecommendation) -> Map<String, Value> {
    let mut params = Map::new();
    insert_num(&mut params, "exposure_ev", rec.exposure_ev);
    insert_num(&mut params, "contrast", rec.contrast);
    insert_num(&mut params, "saturation", rec.saturation);
    insert_num(&mut params, "temperature", rec.temperature);
    insert_num(&mut params, "tint", rec.tint);
    insert_num(&mut params, "shadows", rec.shadows);
    insert_num(&mut params, "highlights", rec.highlights);
    params
}

/// Build the registry parameter map for an `montage.color_pipeline` look.
pub(crate) fn look_params(plan: &LookPlan) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert(
        "clip_input_space".into(),
        Value::String(plan.clip_input_space.clone()),
    );
    params.insert(
        "lut_input_space".into(),
        Value::String(plan.lut_input_space.clone()),
    );
    params.insert("look_lut".into(), Value::String(plan.look_lut.clone()));
    insert_num(&mut params, "look_strength", plan.look_strength);
    if let Some(shaper) = &plan.shaper_lut {
        params.insert("shaper_lut".into(), Value::String(shaper.clone()));
    }
    params
}

fn insert_num(params: &mut Map<String, Value>, key: &str, value: f64) {
    if let Some(n) = serde_json::Number::from_f64(value) {
        params.insert(key.to_string(), Value::Number(n));
    }
}

/// Render a `Set Color Correction` EDL op block (no envelope markers).
pub(crate) fn correction_block(clip: &str, rec: &CorrectionRecommendation) -> String {
    let mut out = String::new();
    out.push_str("*** Set Color Correction\n");
    out.push_str(&format!("@@ anchor: clip_uuid={clip}\n"));
    out.push_str(&format!("+ exposure_ev: {}\n", fmt_num(rec.exposure_ev)));
    out.push_str(&format!("+ contrast: {}\n", fmt_num(rec.contrast)));
    out.push_str(&format!("+ saturation: {}\n", fmt_num(rec.saturation)));
    out.push_str(&format!("+ temperature: {}\n", fmt_num(rec.temperature)));
    out.push_str(&format!("+ tint: {}\n", fmt_num(rec.tint)));
    out.push_str(&format!("+ shadows: {}\n", fmt_num(rec.shadows)));
    out.push_str(&format!("+ highlights: {}\n", fmt_num(rec.highlights)));
    out
}

/// Render a `Set Effect` `montage.color_pipeline` EDL op block.
pub(crate) fn look_block(clip: &str, plan: &LookPlan) -> Result<String, String> {
    let params = look_params(plan);
    let params_json = serde_json::to_string(&Value::Object(params))
        .map_err(|e| format!("plan_color_grade: failed to encode look params: {e}"))?;
    let mut out = String::new();
    out.push_str("*** Set Effect\n");
    out.push_str(&format!("@@ anchor: clip_uuid={clip}\n"));
    out.push_str(&format!("+ effect: {COLOR_PIPELINE}\n"));
    out.push_str(&format!("+ params_json: {params_json}\n"));
    out.push_str("+ rationale: creative look applied after correction at reduced strength\n");
    Ok(out)
}

/// Assemble the full EDL envelope from optional correction + look blocks.
pub(crate) fn assemble_edl(blocks: &[String]) -> String {
    let mut out = String::from("*** Begin EDL\n");
    for block in blocks {
        out.push_str(block);
    }
    out.push_str("*** End EDL\n");
    out
}

/// Format an `f64` for EDL emission: trim trailing zeros, keep it parseable.
fn fmt_num(value: f64) -> String {
    let mut s = format!("{value:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::op::{Anchor, EdlOp};
    use crate::edl::parser::parse;
    use montage_effects::{COLOR_CORRECTION, normalize_params};

    fn sample_rec() -> CorrectionRecommendation {
        CorrectionRecommendation {
            exposure_ev: 0.35,
            contrast: 1.1,
            saturation: 1.0,
            temperature: -0.2,
            tint: 0.05,
            shadows: 0.1,
            highlights: -0.05,
        }
    }

    #[test]
    fn correction_block_parses_to_single_set_color_correction() {
        let block = correction_block("raw/clip-1.mov", &sample_rec());
        let edl = assemble_edl(&[block]);
        let env = parse(&edl).unwrap();
        assert_eq!(env.len(), 1);
        match &env.ops[0] {
            EdlOp::SetColorCorrection {
                anchor,
                exposure_ev,
                contrast,
                saturation,
                ..
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "raw/clip-1.mov"));
                assert_eq!(*exposure_ev, Some(0.35));
                assert_eq!(*contrast, Some(1.1));
                assert_eq!(*saturation, Some(1.0));
            }
            other => panic!("want SetColorCorrection, got {other:?}"),
        }
    }

    #[test]
    fn look_block_rec709_has_no_shaper_key() {
        let plan = LookPlan {
            clip_input_space: "rec709_g24".into(),
            lut_input_space: "rec709_g24".into(),
            look_lut: "luts/show.cube".into(),
            look_strength: 0.5,
            shaper_lut: None,
        };
        let block = look_block("clip-x", &plan).unwrap();
        let env = parse(&assemble_edl(&[block])).unwrap();
        assert_eq!(env.len(), 1);
        match &env.ops[0] {
            EdlOp::SetEffect { effect, params, .. } => {
                assert_eq!(effect, COLOR_PIPELINE);
                assert_eq!(
                    params.get("look_lut").and_then(|v| v.as_str()),
                    Some("luts/show.cube")
                );
                assert!(params.get("shaper_lut").is_none(), "no shaper for rec709");
            }
            other => panic!("want SetEffect, got {other:?}"),
        }
    }

    #[test]
    fn look_block_arri_logc4_includes_shaper() {
        let plan = LookPlan {
            clip_input_space: "arri_logc4".into(),
            lut_input_space: "rec709_g24".into(),
            look_lut: "luts/show.cube".into(),
            look_strength: 0.4,
            shaper_lut: Some("skills/color-corrector/shapers/arri_logc4_to_rec709_g24.csp".into()),
        };
        let block = look_block("clip-x", &plan).unwrap();
        let env = parse(&assemble_edl(&[block])).unwrap();
        match &env.ops[0] {
            EdlOp::SetEffect { params, .. } => {
                assert_eq!(
                    params.get("shaper_lut").and_then(|v| v.as_str()),
                    Some("skills/color-corrector/shapers/arri_logc4_to_rec709_g24.csp")
                );
            }
            other => panic!("want SetEffect, got {other:?}"),
        }
    }

    #[test]
    fn correction_params_round_trip_through_registry() {
        let params = correction_params(&sample_rec());
        assert!(normalize_params(COLOR_CORRECTION, &params).is_ok());
    }

    #[test]
    fn look_params_round_trip_through_registry() {
        let plan = LookPlan {
            clip_input_space: "arri_logc4".into(),
            lut_input_space: "rec709_g24".into(),
            look_lut: "luts/show.cube".into(),
            look_strength: 0.5,
            shaper_lut: Some("skills/color-corrector/shapers/arri_logc4_to_rec709_g24.csp".into()),
        };
        let params = look_params(&plan);
        assert!(normalize_params(COLOR_PIPELINE, &params).is_ok());
    }

    #[test]
    fn shaper_stem_only_for_bundled_log_spaces() {
        assert_eq!(
            shaper_stem_for_space("arri_logc4").as_deref(),
            Some("arri_logc4_to_rec709_g24")
        );
        assert!(shaper_stem_for_space("rec709_g24").is_none());
        assert!(shaper_stem_for_space("scene_linear").is_none());
        // Camera-Log spaces WITHOUT a bundled shaper return None too.
        assert!(shaper_stem_for_space("canon_log3").is_none());
        assert!(shaper_stem_for_space("redlog_filmgen5").is_none());
    }

    #[test]
    fn camera_log_set_includes_unbundled_spaces() {
        // The full camera-Log set covers spaces that lack a bundled shaper.
        for space in ["canon_log2", "canon_log3", "redlog_filmgen5"] {
            assert!(is_camera_log_space(space), "{space} should be camera-Log");
            assert!(
                shaper_stem_for_space(space).is_none(),
                "{space} has no bundled shaper"
            );
        }
        // Bundled Log spaces are also camera-Log.
        for space in BUNDLED_SHAPER_SPACES {
            assert!(is_camera_log_space(space));
        }
        // Non-Log spaces are not camera-Log.
        assert!(!is_camera_log_space("rec709_g24"));
        assert!(!is_camera_log_space("scene_linear"));
    }
}
