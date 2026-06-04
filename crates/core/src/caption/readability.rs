//! Caption reading-speed and segmentation model. Pure: no I/O, no scene data.

use serde::{Deserialize, Serialize};

/// Hard reading-speed ceiling in characters per second (≈160–180 wpm).
pub const MAX_CPS: f64 = 17.0;

/// One transcript word with timing, the input to segmentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputWord {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

/// How a cue is revealed; controls whether per-word timings are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealMode {
    WholeCue,
    WordByWord,
}

/// Per-format readability constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionFormatProfile {
    pub max_chars_per_line: usize,
    pub max_lines: usize,
    pub max_cps: f64,
    pub min_cue_s: f64,
    pub max_cue_s: f64,
    pub reveal: RevealMode,
}

impl CaptionFormatProfile {
    pub fn short_form() -> Self {
        Self {
            max_chars_per_line: 15,
            max_lines: 1,
            max_cps: MAX_CPS,
            min_cue_s: 0.5,
            max_cue_s: 7.0,
            reveal: RevealMode::WordByWord,
        }
    }
    pub fn long_form() -> Self {
        Self {
            max_chars_per_line: 42,
            max_lines: 2,
            max_cps: MAX_CPS,
            min_cue_s: 0.5,
            max_cue_s: 7.0,
            reveal: RevealMode::WholeCue,
        }
    }
    pub fn accessibility() -> Self {
        Self {
            max_chars_per_line: 42,
            max_lines: 2,
            max_cps: MAX_CPS,
            min_cue_s: 0.7,
            max_cue_s: 7.0,
            reveal: RevealMode::WholeCue,
        }
    }
}

/// A finished caption cue: timing, wrapped lines, and word timings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start_s: f64,
    pub end_s: f64,
    pub lines: Vec<String>,
    pub word_timings: Vec<InputWord>,
}

impl Cue {
    pub fn char_count(&self) -> usize {
        self.lines.iter().map(|l| l.chars().count()).sum()
    }
    pub fn cps(&self) -> f64 {
        let dur = (self.end_s - self.start_s).max(1e-6);
        self.char_count() as f64 / dur
    }
}

/// Group words into cues by char budget and sense units, then fix timing.
///
/// Grouping ignores CPS on purpose: over-dense speech cannot be made readable by
/// splitting without overlapping cues or shifting starts off the audio. Instead
/// `finalize_timing` keeps starts synced, makes cues zero-gap and non-overlapping,
/// and extends only the trailing cue toward the readable minimum; `lint()` then
/// surfaces any residual CPS overrun as a proposal.
pub fn segment(words: &[InputWord], profile: &CaptionFormatProfile) -> Vec<Cue> {
    let budget = profile.max_chars_per_line * profile.max_lines;
    let mut cues = Vec::new();
    let mut current: Vec<InputWord> = Vec::new();

    for word in words {
        let mut candidate = current.clone();
        candidate.push(word.clone());
        if !current.is_empty() && !fits_budget(&candidate, profile, budget) {
            cues.push(flush(&current, profile));
            current = vec![word.clone()];
        } else {
            current = candidate;
            if ends_sense_unit(&word.text) {
                cues.push(flush(&current, profile));
                current = Vec::new();
            }
        }
    }
    if !current.is_empty() {
        cues.push(flush(&current, profile));
    }
    finalize_timing(&mut cues, profile);
    cues
}

fn fits_budget(words: &[InputWord], profile: &CaptionFormatProfile, budget: usize) -> bool {
    cue_chars(words) <= budget && cue_dur(words) <= profile.max_cue_s
}

fn cue_chars(words: &[InputWord]) -> usize {
    let text = words
        .iter()
        .map(|w| w.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    text.chars().count()
}

fn cue_dur(words: &[InputWord]) -> f64 {
    match (words.first(), words.last()) {
        (Some(f), Some(l)) => (l.end_s - f.start_s).max(1e-6),
        _ => 1e-6,
    }
}

fn ends_sense_unit(text: &str) -> bool {
    text.trim_end().ends_with(['.', '?', '!', ',', ';', ':'])
}

fn flush(words: &[InputWord], profile: &CaptionFormatProfile) -> Cue {
    let lines = wrap_lines(words, profile.max_chars_per_line, profile.max_lines);
    Cue {
        start_s: words.first().map(|w| w.start_s).unwrap_or(0.0),
        end_s: words.last().map(|w| w.end_s).unwrap_or(0.0),
        lines,
        word_timings: words.to_vec(),
    }
}

/// Greedily pack words into up to `max_lines` lines of `max_chars_per_line`.
/// For 2-line cues, keep the bottom line no longer than the top.
fn wrap_lines(words: &[InputWord], max_chars_per_line: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for w in words {
        let token = w.text.trim();
        if line.is_empty() {
            line.push_str(token);
        } else if line.chars().count() + 1 + token.chars().count() <= max_chars_per_line {
            line.push(' ');
            line.push_str(token);
        } else if lines.len() + 1 < max_lines {
            lines.push(std::mem::take(&mut line));
            line.push_str(token);
        } else {
            // No room left under the line budget: append anyway (segment() guards the budget).
            line.push(' ');
            line.push_str(token);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Fix cue timing without breaking audio sync:
/// - Non-last cue: end at the next cue's start (zero-gap), but never hold longer
///   than `max_cue_s` (so a long silence leaves a gap rather than a stuck caption),
///   and never overlap the next cue.
/// - Last cue: extend toward the readable minimum (`min_cue_s` and the CPS ceiling),
///   since it has no successor to overlap.
/// Starts are never moved, so captions stay synced to the spoken word. Residual
/// CPS overruns on interior cues are intentional and left for `lint()`.
fn finalize_timing(cues: &mut [Cue], profile: &CaptionFormatProfile) {
    let n = cues.len();
    for i in 0..n {
        if i + 1 < n {
            let next_start = cues[i + 1].start_s;
            let max_end = cues[i].start_s + profile.max_cue_s;
            let target = next_start.min(max_end);
            if cues[i].end_s < target {
                cues[i].end_s = target;
            }
            if cues[i].end_s > next_start {
                cues[i].end_s = next_start; // defensive: never overlap
            }
        } else {
            let chars = cues[i].char_count();
            let min_dur = (chars as f64 / profile.max_cps).max(profile.min_cue_s);
            let min_end = cues[i].start_s + min_dur;
            if cues[i].end_s < min_end {
                cues[i].end_s = min_end;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(pairs: &[(&str, f64, f64)]) -> Vec<InputWord> {
        pairs
            .iter()
            .map(|(t, s, e)| InputWord { text: (*t).into(), start_s: *s, end_s: *e })
            .collect()
    }

    #[test]
    fn segment_splits_by_char_budget_without_overlap() {
        let profile = CaptionFormatProfile::short_form(); // 1 line, 15 cpl -> budget 15
        let w = words(&[
            ("one", 0.0, 0.5),
            ("two", 0.5, 1.0),
            ("three", 1.0, 1.5),
            ("four", 1.5, 2.0),
            ("five", 2.0, 2.5),
            ("sixsix", 2.5, 3.0),
        ]);
        let cues = segment(&w, &profile);
        assert!(cues.len() >= 2, "should split across the char budget, got {}", cues.len());
        for cue in &cues {
            assert!(cue.lines.len() <= profile.max_lines);
            for line in &cue.lines {
                assert!(line.chars().count() <= profile.max_chars_per_line, "line too long: {line:?}");
            }
        }
        for pair in cues.windows(2) {
            assert!(pair[0].end_s <= pair[1].start_s + 1e-6, "cues must not overlap: {pair:?}");
        }
    }

    #[test]
    fn segment_is_zero_gap_on_continuous_speech() {
        let profile = CaptionFormatProfile::short_form();
        let w = words(&[
            ("the", 0.0, 0.3),
            ("quick", 0.3, 0.7),
            ("brown", 0.7, 1.1),
            ("fox", 1.1, 1.5),
            ("jumps", 1.5, 1.9),
        ]);
        let cues = segment(&w, &profile);
        assert!(cues.len() >= 2, "continuous speech over the budget should split");
        for pair in cues.windows(2) {
            assert!((pair[1].start_s - pair[0].end_s).abs() < 1e-6, "must be zero-gap (no gap, no overlap): {pair:?}");
        }
    }

    #[test]
    fn segment_extends_a_short_final_cue_toward_readable_minimum() {
        let profile = CaptionFormatProfile::long_form();
        let cues = segment(&words(&[("hi", 0.0, 0.1)]), &profile);
        assert_eq!(cues.len(), 1);
        assert!(
            cues[0].end_s - cues[0].start_s >= profile.min_cue_s - 1e-6,
            "the final short cue should extend toward the readable minimum: {:?}", cues[0]
        );
    }

    #[test]
    fn segment_does_not_overlap_or_desync_on_dense_fast_speech() {
        // 34 chars in 1.0s is physically faster than 17 CPS. segment() must NOT
        // overlap cues or shift starts to "fix" this — lint() surfaces the
        // residual instead.
        let profile = CaptionFormatProfile::long_form();
        let w = words(&[
            ("absolutely", 0.0, 0.4),
            ("incredible", 0.4, 0.7),
            ("breakthrough", 0.7, 1.0),
        ]);
        let cues = segment(&w, &profile);
        assert_eq!(cues[0].start_s, 0.0, "starts must stay synced to audio");
        for pair in cues.windows(2) {
            assert!(pair[0].end_s <= pair[1].start_s + 1e-6, "cues must not overlap: {pair:?}");
        }
    }
}
