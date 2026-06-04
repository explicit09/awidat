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

/// Group words into cues that satisfy `profile`. Greedy: accumulate words while
/// the cue stays within the char-per-line budget, total line budget, and the CPS
/// ceiling; flush on a sense-unit boundary (trailing `.?!,;:`) or when the next
/// word would violate a limit. Continuous speech is zero-gap (a cue ends exactly
/// where the next begins).
pub fn segment(words: &[InputWord], profile: &CaptionFormatProfile) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut current: Vec<InputWord> = Vec::new();

    let budget = profile.max_chars_per_line * profile.max_lines;

    for word in words {
        let mut candidate = current.clone();
        candidate.push(word.clone());
        if !current.is_empty() && !candidate_fits(&candidate, profile, budget) {
            cues.push(flush(&current, profile));
            current = vec![word.clone()];
        } else {
            current = candidate;
            if ends_sense_unit(&word.text)
                && cue_chars(&current) as f64 / cue_dur(&current) <= profile.max_cps
            {
                cues.push(flush(&current, profile));
                current = Vec::new();
            }
        }
    }
    if !current.is_empty() {
        cues.push(flush(&current, profile));
    }
    stretch_and_zero_gap(&mut cues, profile.max_cps);
    cues
}

fn candidate_fits(words: &[InputWord], profile: &CaptionFormatProfile, budget: usize) -> bool {
    let chars = cue_chars(words);
    let dur = cue_dur(words);
    chars <= budget && dur <= profile.max_cue_s && (chars as f64 / dur) <= profile.max_cps
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

/// Extend each cue's `end_s` to satisfy `max_cps`, then close any gap between
/// consecutive cues (zero-gap invariant). The two passes are combined so that
/// stretching a cue already satisfies the zero-gap requirement for the pair.
fn stretch_and_zero_gap(cues: &mut [Cue], max_cps: f64) {
    // Pass 1: stretch each cue's end_s to the minimum required by max_cps.
    for cue in cues.iter_mut() {
        let chars = cue.char_count();
        if chars == 0 {
            continue;
        }
        let min_dur = chars as f64 / max_cps;
        let actual_dur = cue.end_s - cue.start_s;
        if actual_dur < min_dur {
            cue.end_s = cue.start_s + min_dur;
        }
    }
    // Pass 2: close gaps between consecutive cues.
    for i in 1..cues.len() {
        let prev_end = cues[i - 1].end_s;
        if cues[i].start_s > prev_end {
            cues[i - 1].end_s = cues[i].start_s;
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
    fn segment_splits_when_cps_would_exceed_ceiling() {
        // 30 characters spoken in 1.0s = 30 CPS, well over the 17 ceiling.
        let w = words(&[
            ("absolutely", 0.0, 0.4),
            ("incredible", 0.4, 0.7),
            ("breakthrough", 0.7, 1.0),
        ]);
        let cues = segment(&w, &CaptionFormatProfile::long_form());
        assert!(cues.len() >= 2, "over-fast speech must split into >=2 cues, got {}", cues.len());
        for cue in &cues {
            let chars: usize = cue.lines.iter().map(|l| l.chars().count()).sum();
            let dur = cue.end_s - cue.start_s;
            assert!(chars as f64 / dur <= 17.0 + 1e-6, "cue exceeds 17 CPS: {cue:?}");
        }
    }

    #[test]
    fn segment_respects_chars_per_line_and_line_count() {
        let profile = CaptionFormatProfile::short_form(); // 1 line, 15 cpl
        let w = words(&[
            ("one", 0.0, 0.5),
            ("two", 0.5, 1.0),
            ("three", 1.0, 1.5),
            ("four", 1.5, 2.0),
            ("five", 2.0, 2.5),
        ]);
        let cues = segment(&w, &profile);
        for cue in &cues {
            assert!(cue.lines.len() <= profile.max_lines);
            for line in &cue.lines {
                assert!(
                    line.chars().count() <= profile.max_chars_per_line,
                    "line too long: {line:?}"
                );
            }
        }
    }

    #[test]
    fn segment_is_zero_gap_on_continuous_speech() {
        let w = words(&[
            ("the", 0.0, 0.3),
            ("quick", 0.3, 0.7),
            ("brown", 0.7, 1.1),
            ("fox", 1.1, 1.5),
            ("jumps", 1.5, 1.9),
        ]);
        let cues = segment(&w, &CaptionFormatProfile::long_form());
        for pair in cues.windows(2) {
            assert!(
                (pair[1].start_s - pair[0].end_s).abs() < 1e-6,
                "gap between cues: {pair:?}"
            );
        }
    }
}
