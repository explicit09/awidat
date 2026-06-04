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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionWeight {
    Normal,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionCasing {
    AsIs,
    Upper,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptionBackground {
    None,
    Box { color: String, opacity: u8 },
}

/// EDL-carryable caption style knobs. Outline/shadow are render invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyleSpec {
    pub font_size: u32,
    pub weight: CaptionWeight,
    pub casing: CaptionCasing,
    pub primary_color: String,
    pub highlight_color: Option<String>,
    pub reveal: RevealMode,
    pub background: CaptionBackground,
}

impl CaptionStyleSpec {
    fn with_floor(mut self) -> Self {
        if self.font_size < MIN_LEGIBLE_FONT_SIZE {
            self.font_size = MIN_LEGIBLE_FONT_SIZE;
        }
        if !(self.primary_color.starts_with('#') && self.primary_color.len() == 7) {
            self.primary_color = "#FFFFFF".into();
        }
        self
    }
}

pub fn preset_names() -> &'static [&'static str] {
    &["clean_white", "word_pop", "boxed"]
}

pub fn resolve_preset(name: &str) -> Option<CaptionStyleSpec> {
    let spec = match name {
        "clean_white" => CaptionStyleSpec {
            font_size: 44,
            weight: CaptionWeight::Normal,
            casing: CaptionCasing::AsIs,
            primary_color: "#FFFFFF".into(),
            highlight_color: None,
            reveal: RevealMode::WholeCue,
            background: CaptionBackground::None,
        },
        "word_pop" => CaptionStyleSpec {
            font_size: 64,
            weight: CaptionWeight::Bold,
            casing: CaptionCasing::Upper,
            primary_color: "#FFFFFF".into(),
            highlight_color: Some("#FFE000".into()),
            reveal: RevealMode::ActiveWordPop,
            background: CaptionBackground::None,
        },
        "boxed" => CaptionStyleSpec {
            font_size: 48,
            weight: CaptionWeight::Normal,
            casing: CaptionCasing::AsIs,
            primary_color: "#FFFFFF".into(),
            highlight_color: None,
            reveal: RevealMode::WholeCue,
            background: CaptionBackground::Box {
                color: "#000000".into(),
                opacity: 153,
            },
        },
        _ => return None,
    };
    Some(spec.with_floor())
}

/// Resolve the style for a (format, mood), enforcing the legibility floor.
/// Delegates to named presets; format is informational.
pub fn resolve_style(format: CaptionFormat, mood: CaptionMood) -> CaptionStyleSpec {
    match (format, mood) {
        (_, CaptionMood::ActivePop) => {
            resolve_preset("word_pop").expect("word_pop preset must exist")
        }
        _ => resolve_preset("clean_white").expect("clean_white preset must exist"),
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
                    spec.primary_color.starts_with('#') && spec.primary_color.len() == 7,
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
        assert_eq!(pop.reveal, RevealMode::ActiveWordPop);
        assert!(pop.font_size >= calm.font_size);
    }

    #[test]
    fn presets_resolve_with_expected_shapes() {
        let clean = resolve_preset("clean_white").expect("clean_white");
        assert!(matches!(
            clean.reveal,
            crate::caption::readability::RevealMode::WholeCue
        ));
        assert!(clean.highlight_color.is_none());
        assert!(matches!(clean.background, CaptionBackground::None));

        let pop = resolve_preset("word_pop").expect("word_pop");
        assert!(matches!(
            pop.reveal,
            crate::caption::readability::RevealMode::ActiveWordPop
        ));
        assert_eq!(pop.highlight_color.as_deref(), Some("#FFE000"));
        assert!(matches!(pop.weight, CaptionWeight::Bold));
        assert!(matches!(pop.casing, CaptionCasing::Upper));

        let boxed = resolve_preset("boxed").expect("boxed");
        assert!(matches!(boxed.background, CaptionBackground::Box { .. }));

        assert!(resolve_preset("nope").is_none());
    }

    #[test]
    fn every_preset_meets_legibility_floor_and_round_trips() {
        for name in preset_names() {
            let spec = resolve_preset(name).unwrap();
            assert!(spec.font_size >= MIN_LEGIBLE_FONT_SIZE);
            assert!(spec.primary_color.starts_with('#') && spec.primary_color.len() == 7);
            let json = serde_json::to_string(&spec).unwrap();
            let back: CaptionStyleSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(spec, back);
        }
    }
}
