//! Format-agnostic caption EDL emission. Produces `*** Insert Caption` blocks
//! consumable by `apply_edl`, applying the style spec's font/color/reveal.

use crate::caption::readability::RevealMode;
use crate::caption::styles::CaptionStyleSpec;
use crate::caption::types::CaptionRecommendation;

/// Emit `*** Insert Caption` EDL lines for each recommendation, applying the
/// style spec. `safe_area` is the profile string (e.g. "mobile" / "standard").
/// Per-word timings are emitted when the spec's reveal mode is word-by-word or
/// active_word_pop.
pub fn build_caption_edl_lines(
    recs: &[CaptionRecommendation],
    spec: &CaptionStyleSpec,
    safe_area: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    for caption in recs {
        lines.push("*** Insert Caption".to_string());
        lines.push(format!("+ start_s: {}", fmt_seconds(caption.start_s)));
        lines.push(format!("+ end_s: {}", fmt_seconds(caption.end_s)));
        // The line-based EDL cannot carry a real newline and the parser does not
        // JSON-decode `\n`, so a multi-line cue's break is flattened to a space
        // here and left to the renderer to word-wrap. (Hard two-line breaks await
        // the parser text-decode fix.) Short-form cues are single-line already.
        let text = caption.text.replace('\n', " ");
        lines.push(format!("+ text: {}", json_string(&text)));
        lines.push(format!("+ position: {}", edl_position(caption.placement)));
        lines.push(format!("+ font_size: {}", spec.font_size));
        lines.push(format!("+ color: {}", spec.primary_color));
        let style_json = serde_json::to_string(spec).unwrap_or_else(|_| "{}".into());
        lines.push(format!("+ style_json: {style_json}"));
        lines.push(format!("+ safe_area: {safe_area}"));
        if matches!(
            spec.reveal,
            RevealMode::WordByWord | RevealMode::ActiveWordPop
        ) && !caption.word_timings.is_empty()
        {
            lines.push(format!(
                "+ word_timings_json: {}",
                serde_json::to_string(&caption.word_timings).unwrap_or_else(|_| "[]".into())
            ));
        }
    }
    lines
}

/// The EDL caption `position` field is a vertical band; the parser accepts only
/// `top|center|bottom`. Map any placement (incl. the horizontal Left/Right hints)
/// to a valid vertical value so the emitted EDL always re-parses.
fn edl_position(placement: crate::caption::types::CaptionPlacement) -> &'static str {
    use crate::caption::types::CaptionPlacement;
    match placement {
        CaptionPlacement::Upper => "top",
        CaptionPlacement::Bottom | CaptionPlacement::Left | CaptionPlacement::Right => "bottom",
    }
}

fn fmt_seconds(value: f64) -> String {
    // Trim to 3 decimals without trailing zeros.
    let s = format!("{value:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::styles::{CaptionBackground, CaptionCasing, CaptionWeight, resolve_preset};
    use crate::caption::types::{
        CaptionPlacement, CaptionRecommendation, CaptionStyle, CaptionWordTiming,
    };

    fn rec() -> CaptionRecommendation {
        CaptionRecommendation {
            start_s: 1.0,
            end_s: 2.0,
            text: "hello".into(),
            word_timings: vec![CaptionWordTiming {
                text: "hello".into(),
                start_s: 1.0,
                end_s: 2.0,
            }],
            placement: CaptionPlacement::Bottom,
            style: CaptionStyle::Plain,
            transcript_reason: "t".into(),
            visual_reason: "v".into(),
            safety_reason: "s".into(),
            confidence: 0.7,
        }
    }

    #[test]
    fn multiline_cue_text_is_flattened_to_a_single_line() {
        let mut r = rec();
        r.text = "the rise of solar\nentrepreneurs.".into();
        let spec = resolve_preset("clean_white").unwrap();
        let blob = build_caption_edl_lines(&[r], &spec, "standard").join("\n");
        assert!(
            blob.contains("the rise of solar entrepreneurs."),
            "newline should flatten to a space: {blob}"
        );
        assert!(
            !blob.contains("solar\\nentrepreneurs"),
            "must not emit a literal backslash-n"
        );
    }

    #[test]
    fn whole_cue_spec_omits_word_timings() {
        let spec = resolve_preset("clean_white").unwrap();
        let lines = build_caption_edl_lines(&[rec()], &spec, "standard");
        let blob = lines.join("\n");
        assert!(blob.contains("*** Insert Caption"));
        assert!(blob.contains("+ font_size: 44"));
        assert!(blob.contains("+ position: bottom"));
        assert!(
            !blob.contains("word_timings_json"),
            "whole-cue must not emit per-word timings"
        );
    }

    #[test]
    fn word_by_word_spec_emits_word_timings() {
        use crate::caption::readability::RevealMode;
        use crate::caption::styles::{CaptionMotion, CaptionStyleSpec, MIN_LEGIBLE_FONT_SIZE};
        let spec = CaptionStyleSpec {
            font_size: MIN_LEGIBLE_FONT_SIZE.max(56),
            weight: CaptionWeight::Normal,
            casing: CaptionCasing::AsIs,
            primary_color: "#FFFFFF".into(),
            highlight_color: None,
            reveal: RevealMode::WordByWord,
            background: CaptionBackground::None,
            motion: CaptionMotion::default(),
        };
        let lines = build_caption_edl_lines(&[rec()], &spec, "standard");
        let blob = lines.join("\n");
        assert!(blob.contains("word_timings_json"));
        assert!(
            blob.contains("\"hello\""),
            "word timings JSON should contain the word text"
        );
    }

    #[test]
    fn active_word_pop_spec_emits_word_timings() {
        let spec = resolve_preset("word_pop").unwrap();
        let lines = build_caption_edl_lines(&[rec()], &spec, "standard");
        let blob = lines.join("\n");
        assert!(blob.contains("word_timings_json"));
    }

    #[test]
    fn emits_style_json_with_spec_fields() {
        use crate::caption::styles::resolve_preset;
        let spec = resolve_preset("word_pop").unwrap();
        let blob = build_caption_edl_lines(&[rec()], &spec, "mobile").join("\n");
        assert!(
            blob.contains("+ style_json:"),
            "must emit style_json: {blob}"
        );
        assert!(blob.contains("\"reveal\":\"active_word_pop\""));
        assert!(blob.contains("\"highlight_color\":\"#FFE000\""));
    }

    #[test]
    fn position_maps_to_a_parser_valid_vertical_band() {
        let spec = resolve_preset("clean_white").unwrap();
        for (placement, expected) in [
            (CaptionPlacement::Bottom, "bottom"),
            (CaptionPlacement::Upper, "top"),
            (CaptionPlacement::Left, "bottom"),
            (CaptionPlacement::Right, "bottom"),
        ] {
            let mut r = rec();
            r.placement = placement;
            let blob = build_caption_edl_lines(&[r], &spec, "standard").join("\n");
            assert!(
                blob.contains(&format!("+ position: {expected}")),
                "{placement:?} should map to {expected}"
            );
        }
    }
}
