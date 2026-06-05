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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntranceMotion {
    None,
    PopIn,
    SlideUp,
    FadeIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWordMotion {
    None,
    Bounce,
    ScalePop,
    Shake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitMotion {
    None,
    PopOut,
    FadeOut,
    SlideDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuousMotion {
    None,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionMotion {
    pub entrance: EntranceMotion,
    pub active_word: ActiveWordMotion,
    pub exit: ExitMotion,
    pub continuous: ContinuousMotion,
}

impl Default for CaptionMotion {
    fn default() -> Self {
        Self {
            entrance: EntranceMotion::None,
            active_word: ActiveWordMotion::None,
            exit: ExitMotion::None,
            continuous: ContinuousMotion::None,
        }
    }
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
    #[serde(default)]
    pub motion: CaptionMotion,
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
    &["clean_white", "word_pop", "boxed", "emphasis"]
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
            motion: CaptionMotion::default(),
        },
        "word_pop" => CaptionStyleSpec {
            font_size: 84,
            weight: CaptionWeight::Bold,
            casing: CaptionCasing::Upper,
            primary_color: "#FFFFFF".into(),
            highlight_color: Some("#FFE000".into()),
            reveal: RevealMode::ActiveWordPop,
            background: CaptionBackground::None,
            motion: CaptionMotion {
                entrance: EntranceMotion::PopIn,
                active_word: ActiveWordMotion::Bounce,
                exit: ExitMotion::None,
                continuous: ContinuousMotion::None,
            },
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
            motion: CaptionMotion::default(),
        },
        "emphasis" => CaptionStyleSpec {
            font_size: 72,
            weight: CaptionWeight::Bold,
            casing: CaptionCasing::Upper,
            primary_color: "#FFFFFF".into(),
            highlight_color: Some("#FFE000".into()),
            reveal: RevealMode::ActiveWordPop,
            background: CaptionBackground::Box {
                color: "#000000".into(),
                opacity: 178,
            },
            motion: CaptionMotion {
                entrance: EntranceMotion::PopIn,
                active_word: ActiveWordMotion::Bounce,
                exit: ExitMotion::None,
                continuous: ContinuousMotion::None,
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

    #[test]
    fn caption_motion_defaults_to_none_and_round_trips() {
        let m = CaptionMotion::default();
        assert!(matches!(m.entrance, EntranceMotion::None));
        assert!(matches!(m.active_word, ActiveWordMotion::None));
        assert!(matches!(m.exit, ExitMotion::None));
        assert!(matches!(m.continuous, ContinuousMotion::None));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<CaptionMotion>(&json).unwrap(), m);
    }

    #[test]
    fn emphasis_preset_is_poppy_boxed_and_animated() {
        let e = resolve_preset("emphasis").expect("emphasis");
        assert!(matches!(e.background, CaptionBackground::Box { .. }));
        assert!(matches!(e.motion.entrance, EntranceMotion::PopIn));
        assert!(matches!(e.motion.active_word, ActiveWordMotion::Bounce));
        assert!(e.font_size >= 64);
        assert!(preset_names().contains(&"emphasis"));
    }

    #[test]
    fn preset_motions_match_corpus_defaults() {
        let clean = resolve_preset("clean_white").unwrap();
        assert_eq!(clean.motion, CaptionMotion::default(), "cinematic = minimal/none");
        let boxed = resolve_preset("boxed").unwrap();
        assert_eq!(boxed.motion, CaptionMotion::default());
        let pop = resolve_preset("word_pop").unwrap();
        assert!(matches!(pop.motion.entrance, EntranceMotion::PopIn));
        assert!(matches!(pop.motion.active_word, ActiveWordMotion::Bounce));
    }

    #[test]
    fn style_json_without_motion_deserializes_to_default() {
        let legacy = r##"{"font_size":44,"weight":"normal","casing":"as_is","primary_color":"#FFFFFF","highlight_color":null,"reveal":"whole_cue","background":{"kind":"none"}}"##;
        let spec: CaptionStyleSpec = serde_json::from_str(legacy).unwrap();
        assert_eq!(spec.motion, CaptionMotion::default());
    }
}
