//! Shared caption planning types. Moved out of `scene_aware_short_form` so the
//! general caption path and the short-form path use one definition.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPlacement {
    Bottom,
    Upper,
    Left,
    Right,
}

impl CaptionPlacement {
    pub fn edl_value(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Upper => "top",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionRecommendation {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub word_timings: Vec<CaptionWordTiming>,
    pub placement: CaptionPlacement,
    pub style: CaptionStyle,
    pub transcript_reason: String,
    pub visual_reason: String,
    pub safety_reason: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionWordTiming {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionStyle {
    Plain,
    Boxed,
    Minimal,
}
