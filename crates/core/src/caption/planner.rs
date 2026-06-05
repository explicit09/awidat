//! Turns readability cues into CaptionRecommendations via an injected strategy.
//! Placement/style are the only short-form-specific concerns, so they live
//! behind `CaptionPlanStrategy`. The short-form strategy lives in
//! `scene_aware_short_form`; `LowerSafeZoneStrategy` here is the general default.

use crate::caption::readability::Cue;
use crate::caption::types::{
    CaptionPlacement, CaptionRecommendation, CaptionStyle, CaptionWordTiming,
};

/// Per-cue placement + style decision plus the reasons that justify it.
pub struct CuePlan {
    pub placement: CaptionPlacement,
    pub style: CaptionStyle,
    pub visual_reason: String,
    pub safety_reason: String,
    pub confidence: f64,
}

/// Strategy that decides placement/style for each cue. Implementors:
/// `ShotAwareStrategy` (short-form, in `scene_aware_short_form`) and
/// `LowerSafeZoneStrategy` (general default, below).
pub trait CaptionPlanStrategy {
    fn plan_cue(&self, cue: &Cue) -> CuePlan;
}

/// Build CaptionRecommendations from cues using the given strategy.
pub fn plan(cues: &[Cue], strategy: &dyn CaptionPlanStrategy) -> Vec<CaptionRecommendation> {
    cues.iter()
        .filter(|c| !c.lines.join(" ").trim().is_empty())
        .map(|c| {
            let decision = strategy.plan_cue(c);
            CaptionRecommendation {
                start_s: c.start_s,
                end_s: c.end_s,
                text: c.lines.join("\n"),
                word_timings: c
                    .word_timings
                    .iter()
                    .map(|w| CaptionWordTiming {
                        text: w.text.clone(),
                        start_s: w.start_s,
                        end_s: w.end_s,
                    })
                    .collect(),
                placement: decision.placement,
                style: decision.style,
                transcript_reason: "readability-segmented transcript cue with source timings"
                    .into(),
                visual_reason: decision.visual_reason,
                safety_reason: decision.safety_reason,
                confidence: decision.confidence,
            }
        })
        .collect()
}

/// General default: bottom safe zone, whole-cue, no scene analysis required.
pub struct LowerSafeZoneStrategy;

impl CaptionPlanStrategy for LowerSafeZoneStrategy {
    fn plan_cue(&self, _cue: &Cue) -> CuePlan {
        CuePlan {
            placement: CaptionPlacement::Bottom,
            style: CaptionStyle::Plain,
            visual_reason: "lower safe-zone default (no scene analysis for this format)".into(),
            safety_reason: "bottom band avoids faces and the action by convention".into(),
            confidence: 0.7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::readability::{Cue, InputWord};

    fn cue(text: &str) -> Cue {
        Cue {
            start_s: 1.0,
            end_s: 2.5,
            lines: vec![text.into()],
            word_timings: vec![InputWord {
                text: text.into(),
                start_s: 1.0,
                end_s: 2.5,
            }],
        }
    }

    #[test]
    fn lower_safe_zone_strategy_places_at_bottom() {
        let cues = vec![cue("hello world")];
        let recs = plan(&cues, &LowerSafeZoneStrategy);
        assert_eq!(recs.len(), 1);
        assert_eq!(
            recs[0].placement,
            crate::caption::types::CaptionPlacement::Bottom
        );
        assert_eq!(recs[0].text, "hello world");
        assert_eq!(recs[0].start_s, 1.0);
        assert_eq!(recs[0].word_timings.len(), 1);
    }

    #[test]
    fn plan_skips_blank_cues() {
        let cues = vec![Cue {
            start_s: 0.0,
            end_s: 1.0,
            lines: vec!["   ".into()],
            word_timings: vec![],
        }];
        let recs = plan(&cues, &LowerSafeZoneStrategy);
        assert!(
            recs.is_empty(),
            "blank cues should not produce recommendations"
        );
    }
}
