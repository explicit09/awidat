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
    let mut current_chars: usize = 0;

    for word in words {
        let token_chars = word.text.trim().chars().count();
        let space = usize::from(!current.is_empty());
        let new_chars = current_chars + space + token_chars;
        let new_dur = current
            .first()
            .map_or(0.0, |first| (word.end_s - first.start_s).max(1e-6));
        let fits = new_chars <= budget && new_dur <= profile.max_cue_s;

        if !current.is_empty() && !fits {
            cues.push(flush(&current, profile));
            current = vec![word.clone()];
            current_chars = token_chars;
        } else {
            current.push(word.clone());
            current_chars = new_chars;
            if ends_sense_unit(&word.text) {
                cues.push(flush(&current, profile));
                current = Vec::new();
                current_chars = 0;
            }
        }
    }
    if !current.is_empty() {
        cues.push(flush(&current, profile));
    }
    finalize_timing(&mut cues, profile);
    cues
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
/// Greedy left-fill: the first line is packed before spilling to the next (no bottom/top rebalancing pass).
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
            // Callers must keep total chars <= budget; an over-long single word
            // (longer than one line) overflows here rather than being dropped.
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
            // char_count() counts rendered chars (no inter-line space); this is
            // the readable-minimum estimate, off by <=1 per wrapped line vs the
            // grouping budget — negligible for the max 2-line profiles.
            let chars = cues[i].char_count();
            let min_dur = (chars as f64 / profile.max_cps).max(profile.min_cue_s);
            let min_end = cues[i].start_s + min_dur;
            if cues[i].end_s < min_end {
                cues[i].end_s = min_end;
            }
        }
    }
}

/// A non-destructive readability recommendation for an existing cue. The model
/// never rewrites a timeline; it proposes, with a human rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadabilityProposal {
    Split { at_s: f64, rationale: String },
    Extend { to_s: f64, rationale: String },
    Reflow { rationale: String },
}

impl ReadabilityProposal {
    pub fn rationale(&self) -> &str {
        match self {
            Self::Split { rationale, .. } | Self::Extend { rationale, .. } | Self::Reflow { rationale } => rationale,
        }
    }
}

/// Inspect existing cues and emit non-destructive proposals where a cue violates
/// the profile. Timing proposals are mutually exclusive: a too-short cue is
/// Extended (a longer hold also lowers CPS), and only a long-enough but text-dense
/// cue is Split. A layout (Reflow) proposal is independent of timing. Never mutates.
pub fn lint(cues: &[Cue], profile: &CaptionFormatProfile) -> Vec<ReadabilityProposal> {
    let mut out = Vec::new();
    for c in cues {
        let dur = c.end_s - c.start_s;
        // Timing fix (at most one): extend a too-short cue, else split a too-dense one.
        if dur + 1e-9 < profile.min_cue_s {
            let to = c.start_s + profile.min_cue_s;
            out.push(ReadabilityProposal::Extend {
                to_s: to,
                rationale: format!(
                    "Extend to {to:.1}s — cue is {dur:.2}s, under the {:.1}s minimum.",
                    profile.min_cue_s
                ),
            });
        } else if c.cps() > profile.max_cps + 1e-9 {
            let at = split_point(c);
            out.push(ReadabilityProposal::Split {
                at_s: at,
                rationale: format!(
                    "Split at {at:.1}s — {} chars/{:.1}s = {:.0} CPS exceeded {:.0} CPS reading ceiling.",
                    c.char_count(),
                    dur,
                    c.cps(),
                    profile.max_cps
                ),
            });
        }
        // Layout fix (independent of timing).
        let too_many_lines = c.lines.len() > profile.max_lines;
        let line_too_long = c
            .lines
            .iter()
            .any(|l| l.chars().count() > profile.max_chars_per_line);
        if too_many_lines || line_too_long {
            let detail = match (too_many_lines, line_too_long) {
                (true, true) => format!(
                    "more than {} line(s) and a line over {} chars",
                    profile.max_lines, profile.max_chars_per_line
                ),
                (true, false) => format!("more than {} line(s)", profile.max_lines),
                (false, true) => format!("a line over {} chars", profile.max_chars_per_line),
                (false, false) => unreachable!(),
            };
            out.push(ReadabilityProposal::Reflow {
                rationale: format!("Reflow — cue has {detail}."),
            });
        }
    }
    out
}

/// Best split time for a too-dense cue: the inter-word boundary nearest the cue's
/// time midpoint when word timings are available, else the plain midpoint.
fn split_point(c: &Cue) -> f64 {
    let mid = c.start_s + (c.end_s - c.start_s) / 2.0;
    if c.word_timings.len() < 2 {
        return mid;
    }
    c.word_timings
        .iter()
        .skip(1)
        .map(|w| w.start_s)
        .min_by(|a, b| {
            (a - mid)
                .abs()
                .partial_cmp(&(b - mid).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(mid)
}

/// Flatten a Whisper-style transcript JSON into segmentation input words.
///
/// Walks `/segments[*]/words[*]`, accepting `text`|`word` for the token and
/// `start_s`|`start` / `end_s`|`end` for timing. Skips words with empty text,
/// non-finite timings, or `end_s <= start_s`. Falls back to the segment-level
/// text+timing when a segment has no usable words. Timings are rounded to 3
/// decimals. This is the single source of truth for both the short-form planner
/// and the general `plan_captions` tool.
pub fn words_from_transcript(transcript: &serde_json::Value) -> Vec<InputWord> {
    let mut out = Vec::new();
    let Some(segments) = transcript.pointer("/segments").and_then(|v| v.as_array()) else {
        return out;
    };
    for seg in segments {
        let seg_start = wft_num(seg, "start_s", wft_num(seg, "start", 0.0));
        let seg_end = wft_num(seg, "end_s", wft_num(seg, "end", seg_start));
        let mut pushed_any = false;
        if let Some(ws) = seg.get("words").and_then(|v| v.as_array()) {
            for w in ws {
                let text = w
                    .get("text")
                    .or_else(|| w.get("word"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let start_s = wft_num(w, "start_s", wft_num(w, "start", seg_start));
                let end_s = wft_num(w, "end_s", wft_num(w, "end", seg_end));
                if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
                    continue;
                }
                out.push(InputWord { text, start_s: wft_round3(start_s), end_s: wft_round3(end_s) });
                pushed_any = true;
            }
        }
        if !pushed_any {
            let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if !text.is_empty() && seg_start.is_finite() && seg_end.is_finite() && seg_end > seg_start {
                out.push(InputWord { text, start_s: wft_round3(seg_start), end_s: wft_round3(seg_end) });
            }
        }
    }
    out
}

fn wft_num(v: &serde_json::Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(default)
}

fn wft_round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
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

    #[test]
    fn segment_returns_empty_for_no_words() {
        assert_eq!(segment(&[], &CaptionFormatProfile::long_form()), Vec::<Cue>::new());
    }

    #[test]
    fn segment_keeps_an_overlong_word_rather_than_dropping_it() {
        // A single word longer than the line budget must survive (overflow), not vanish.
        let profile = CaptionFormatProfile::short_form(); // budget 15
        let w = words(&[("supercalifragilistic", 0.0, 1.0)]);
        let cues = segment(&w, &profile);
        let joined: String = cues.iter().flat_map(|c| c.lines.iter().cloned()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("supercalifragilistic"), "overlong word was dropped: {joined:?}");
    }

    fn cue(start_s: f64, end_s: f64, text: &str) -> Cue {
        Cue { start_s, end_s, lines: vec![text.into()], word_timings: vec![] }
    }

    #[test]
    fn lint_flags_cps_overrun_with_split_proposal() {
        // 40 chars in 1.0s = 40 CPS.
        let cues = vec![cue(0.0, 1.0, "an extraordinarily dense caption line!!")];
        let proposals = lint(&cues, &CaptionFormatProfile::long_form());
        assert!(proposals.iter().any(|p| matches!(p, ReadabilityProposal::Split { .. })));
        let p = proposals.iter().find(|p| matches!(p, ReadabilityProposal::Split { .. })).unwrap();
        assert!(p.rationale().contains("CPS"), "rationale must explain the CPS overrun: {}", p.rationale());
    }

    #[test]
    fn lint_flags_sub_minimum_duration_with_extend_proposal() {
        let cues = vec![cue(0.0, 0.2, "hi")]; // 0.2s < 0.5s minimum
        let proposals = lint(&cues, &CaptionFormatProfile::long_form());
        assert!(proposals.iter().any(|p| matches!(p, ReadabilityProposal::Extend { .. })));
    }

    #[test]
    fn lint_is_silent_on_clean_cues() {
        let cues = vec![cue(0.0, 2.0, "a calm, readable line")];
        let proposals = lint(&cues, &CaptionFormatProfile::long_form());
        assert!(proposals.is_empty(), "clean cue should not be flagged: {proposals:?}");
    }

    #[test]
    fn lint_flags_overlong_line_with_reflow_proposal() {
        // 64 chars on one line exceeds long_form()'s 42-char limit.
        let c = cue(0.0, 5.0, "this is a very long single line that clearly exceeds the limit!!");
        let proposals = lint(&[c], &CaptionFormatProfile::long_form());
        assert!(proposals.iter().any(|p| matches!(p, ReadabilityProposal::Reflow { .. })));
    }

    #[test]
    fn words_from_transcript_skips_bad_words_and_falls_back_to_segment_text() {
        let t = serde_json::json!({
            "segments": [
                { "start_s": 0.0, "end_s": 1.0, "text": "hi there", "words": [
                    {"text": "hi", "start_s": 0.0, "end_s": 0.5},
                    {"text": "bad", "start_s": 0.8, "end_s": 0.7},   // end<=start: skipped
                    {"text": "there", "start_s": 0.5, "end_s": 1.0}
                ]},
                { "start_s": 1.0, "end_s": 2.0, "text": "no words here", "words": [] }
            ]
        });
        let words = words_from_transcript(&t);
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["hi", "there", "no words here"]);
    }
}
