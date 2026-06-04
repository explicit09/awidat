//! Caption style registry keyed by (format, mood). Returns the EDL-carryable
//! style knobs (font size, color, reveal). The render layer always draws an
//! outline + shadow, so the in-code legibility floor here is min font size +
//! a valid high-contrast color.

use serde::{Deserialize, Serialize};

use crate::caption::readability::RevealMode;

/// Smallest font size we will ever emit; below this captions fail on mobile.
pub const MIN_LEGIBLE_FONT_SIZE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionFormat {
    ShortForm,
    LongForm,
    Accessibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionMood {
    MinimalCinematic,
    ActivePop,
}

/// EDL-carryable caption style knobs. Outline/shadow are render invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyleSpec {
    pub font_size: u32,
    pub color: String,
    pub reveal: RevealMode,
}

/// Resolve the style for a (format, mood), enforcing the legibility floor.
pub fn resolve_style(format: CaptionFormat, mood: CaptionMood) -> CaptionStyleSpec {
    let (mut font_size, color, reveal) = match (format, mood) {
        (CaptionFormat::ShortForm, CaptionMood::MinimalCinematic) => {
            (52, "#FFFFFF", RevealMode::WholeCue)
        }
        (CaptionFormat::ShortForm, CaptionMood::ActivePop) => {
            (64, "#FFFFFF", RevealMode::WordByWord)
        }
        (CaptionFormat::LongForm, CaptionMood::MinimalCinematic) => {
            (44, "#FFFFFF", RevealMode::WholeCue)
        }
        (CaptionFormat::LongForm, CaptionMood::ActivePop) => {
            (56, "#FFFFFF", RevealMode::WordByWord)
        }
        (CaptionFormat::Accessibility, _) => (44, "#FFFFFF", RevealMode::WholeCue),
    };
    if font_size < MIN_LEGIBLE_FONT_SIZE {
        font_size = MIN_LEGIBLE_FONT_SIZE;
    }
    let color = if color.starts_with('#') && color.len() == 7 {
        color.to_string()
    } else {
        "#FFFFFF".to_string()
    };
    CaptionStyleSpec {
        font_size,
        color,
        reveal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::readability::RevealMode;

    #[test]
    fn every_mood_meets_the_legibility_floor() {
        for format in [
            CaptionFormat::ShortForm,
            CaptionFormat::LongForm,
            CaptionFormat::Accessibility,
        ] {
            for mood in [CaptionMood::MinimalCinematic, CaptionMood::ActivePop] {
                let spec = resolve_style(format, mood);
                assert!(
                    spec.font_size >= MIN_LEGIBLE_FONT_SIZE,
                    "{format:?}/{mood:?} font too small"
                );
                assert!(
                    spec.color.starts_with('#') && spec.color.len() == 7,
                    "{format:?}/{mood:?} bad color"
                );
            }
        }
    }

    #[test]
    fn moods_are_distinct() {
        let calm = resolve_style(CaptionFormat::LongForm, CaptionMood::MinimalCinematic);
        let pop = resolve_style(CaptionFormat::LongForm, CaptionMood::ActivePop);
        assert_eq!(calm.reveal, RevealMode::WholeCue);
        assert_eq!(pop.reveal, RevealMode::WordByWord);
        assert!(pop.font_size >= calm.font_size);
    }
}
