//! Shared deterministic transcript cleanup heuristics.

/// Basic filler tokens used for conservative filler detection.
pub const BASIC_FILLER_TOKENS: &[&str] = &["um", "uh", "uhh", "umm", "ah", "ahh", "er", "err"];

/// Discourse-marker tokens used by more aggressive cleanup modes.
pub const DISCOURSE_MARKER_TOKENS: &[&str] = &[
    "like",
    "so",
    "just",
    "but",
    "yeah",
    "basically",
    "you know",
    "i mean",
];

const MAX_RESTART_LOOKBACK_S: f64 = 12.0;
const MAX_SNIPPET_CHARS: usize = 320;

/// A transcript segment in source-media seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    /// Segment start in source-media seconds.
    pub start_s: f64,
    /// Segment end in source-media seconds.
    pub end_s: f64,
    /// Segment text.
    pub text: String,
}

/// A transcript word in source-media seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptWord {
    /// Word text as produced by the transcript sidecar.
    pub text: String,
    /// Word start in source-media seconds.
    pub start_s: f64,
    /// Word end in source-media seconds.
    pub end_s: f64,
}

/// Configuration for segment-level transcript cleanup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CleanupConfig {
    /// Minimum ratio of filler/discourse-marker tokens required.
    pub min_filler_ratio: f64,
    /// Minimum absolute number of filler/discourse-marker tokens required.
    pub min_filler_tokens: usize,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            min_filler_ratio: 0.35,
            min_filler_tokens: 2,
        }
    }
}

/// A source range selected for transcript cleanup.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupRange {
    /// Range start in source-media seconds.
    pub start_s: f64,
    /// Range end in source-media seconds.
    pub end_s: f64,
    /// Number of filler/discourse-marker tokens in the segment.
    pub filler_token_count: usize,
    /// Number of normalized tokens in the segment.
    pub total_token_count: usize,
}

/// A false-start range selected by restart-marker detection.
#[derive(Debug, Clone, PartialEq)]
pub struct FalseStartRange {
    /// Source range start in source-media seconds.
    pub start_s: f64,
    /// Source range end in source-media seconds.
    pub end_s: f64,
    /// The restart marker text that triggered the range.
    pub marker: String,
    /// Text inside the selected false-start range.
    pub snippet: String,
}

/// A source range where the transcript is likely production talk,
/// coaching, or a botched-take reset instead of publishable dialogue.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionAsideRange {
    /// Source range start in source-media seconds.
    pub start_s: f64,
    /// Source range end in source-media seconds.
    pub end_s: f64,
    /// The phrase family that triggered the range.
    pub marker: String,
    /// Text inside the selected production-aside range.
    pub snippet: String,
}

/// Return transcript segment ranges that are dense enough in filler
/// or discourse-marker tokens to be removed by deterministic cleanup.
pub fn filler_dense_ranges(
    segments: &[TranscriptSegment],
    config: CleanupConfig,
) -> Vec<CleanupRange> {
    segments
        .iter()
        .filter_map(|segment| cleanup_range(segment, config))
        .collect()
}

/// Return the default filler vocabulary.
pub fn default_filler_tokens(include_discourse_markers: bool) -> Vec<String> {
    let mut tokens = BASIC_FILLER_TOKENS
        .iter()
        .map(|token| (*token).to_string())
        .collect::<Vec<_>>();
    if include_discourse_markers {
        tokens.extend(
            DISCOURSE_MARKER_TOKENS
                .iter()
                .map(|token| (*token).to_string()),
        );
    }
    tokens
}

/// Return false-start ranges found in a visible source span.
pub fn false_start_ranges(
    words: &[TranscriptWord],
    clip_source_start_s: f64,
    clip_source_end_s: f64,
) -> Vec<FalseStartRange> {
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if word.end_s <= clip_source_start_s || word.start_s >= clip_source_end_s {
            index += 1;
            continue;
        }
        let Some(marker) = restart_marker_at(words, index) else {
            index += 1;
            continue;
        };
        let preceding_start_s = words
            .iter()
            .take(index)
            .enumerate()
            .rev()
            .find(|(previous_index, previous)| {
                previous.start_s >= clip_source_start_s
                    && restart_marker_at(words, *previous_index).is_some()
            })
            .map(|(_, previous)| previous.end_s)
            .unwrap_or(clip_source_start_s);

        let visible_start_s = preceding_start_s
            .max(word.start_s - MAX_RESTART_LOOKBACK_S)
            .max(clip_source_start_s);
        let visible_end_s = word.start_s.min(clip_source_end_s);
        if visible_end_s > visible_start_s {
            let snippet = words
                .iter()
                .filter(|candidate| {
                    candidate.start_s >= visible_start_s && candidate.end_s <= visible_end_s
                })
                .map(|candidate| candidate.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            ranges.push(FalseStartRange {
                start_s: visible_start_s,
                end_s: visible_end_s,
                marker,
                snippet: compact_snippet(&snippet),
            });
        }
        index += 1;
    }
    ranges
}

/// Return production/coaching ranges that overlap a visible source span.
pub fn production_aside_ranges(
    segments: &[TranscriptSegment],
    clip_source_start_s: f64,
    clip_source_end_s: f64,
) -> Vec<ProductionAsideRange> {
    let mut ranges = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let Some(marker) = production_marker(&segment.text) else {
            continue;
        };
        let mut start_index = index;
        while start_index > 0 {
            let previous = &segments[start_index - 1];
            if segment.start_s - previous.end_s > 20.0 || !production_lead_in(&previous.text) {
                break;
            }
            start_index -= 1;
        }

        let mut end_index = index;
        while end_index + 1 < segments.len() {
            let current = &segments[end_index];
            let next = &segments[end_index + 1];
            if next.start_s - current.end_s > 6.0
                || !(production_marker(&next.text).is_some()
                    || production_lead_in(&next.text)
                    || production_continuation(&next.text))
            {
                break;
            }
            end_index += 1;
        }

        let start_s = segments[start_index].start_s.max(clip_source_start_s);
        let end_s = segments[end_index].end_s.min(clip_source_end_s);
        if end_s <= start_s {
            continue;
        }
        let snippet = segments[start_index..=end_index]
            .iter()
            .map(|candidate| candidate.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        ranges.push(ProductionAsideRange {
            start_s,
            end_s,
            marker,
            snippet: compact_snippet(&snippet),
        });
    }
    merge_production_asides(ranges)
}

/// Normalize a transcript token for filler/restart matching.
pub fn normalize_transcript_token(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| matches!(c, '.' | ',' | '?' | '!' | ':' | ';'))
        .to_lowercase()
}

fn production_marker(text: &str) -> Option<String> {
    let normalized = normalize_phrase(text);
    let markers = [
        ("you can just say", "coaching"),
        ("we don't have to", "coaching"),
        ("we dont have to", "coaching"),
        ("go into a long", "coaching"),
        ("when do you when do you decide", "coaching"),
        ("hold it for me", "retake_direction"),
        ("one more time", "retake_direction"),
        ("try again", "retake_direction"),
        ("start over", "retake_direction"),
        ("that was a practice", "rehearsal"),
        ("i'm just practicing", "rehearsal"),
        ("im just practicing", "rehearsal"),
        ("am i supposed", "setup_talk"),
        ("look at that camera", "setup_talk"),
        ("is it on me", "setup_talk"),
        ("what's the other one", "setup_talk"),
        ("whats the other one", "setup_talk"),
    ];
    for (needle, marker) in markers {
        if normalized.contains(needle) {
            return Some(marker.to_string());
        }
    }
    if normalize_phrase(text)
        .split_whitespace()
        .any(|token| token == "cut")
    {
        return Some("retake_direction".to_string());
    }
    None
}

fn production_lead_in(text: &str) -> bool {
    let normalized = normalize_phrase(text);
    if normalized.contains("doing good") || normalized.contains("don't mind me") {
        return true;
    }
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    matches!(
        tokens.as_slice(),
        ["youre"]
            | ["you're"]
            | ["i"]
            | ["think"]
            | ["mhmm"]
            | ["yeah"]
            | ["okay"]
            | ["mhmm", "i"]
            | ["i", "think"]
    )
}

fn production_continuation(text: &str) -> bool {
    let normalized = normalize_phrase(text);
    normalized.contains("i'm building")
        || normalized.contains("im building")
        || normalized.contains("i'm working")
        || normalized.contains("im working")
        || normalized.contains("ceo of")
        || normalized.contains("this and this")
        || normalized.contains("then maybe")
}

fn normalize_phrase(text: &str) -> String {
    text.to_lowercase()
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '\''))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn merge_production_asides(mut ranges: Vec<ProductionAsideRange>) -> Vec<ProductionAsideRange> {
    ranges.sort_by(|left, right| left.start_s.total_cmp(&right.start_s));
    let mut merged: Vec<ProductionAsideRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start_s <= last.end_s + 0.001
        {
            last.end_s = last.end_s.max(range.end_s);
            if !last.marker.contains(&range.marker) {
                last.marker = format!("{},{}", last.marker, range.marker);
            }
            if !range.snippet.is_empty() && !last.snippet.contains(&range.snippet) {
                last.snippet = compact_snippet(&format!("{} {}", last.snippet, range.snippet));
            }
            continue;
        }
        merged.push(range);
    }
    merged
}

fn compact_snippet(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= MAX_SNIPPET_CHARS {
        return trimmed.to_string();
    }
    let mut end = MAX_SNIPPET_CHARS;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", trimmed[..end].trim_end())
}

fn restart_marker_at(words: &[TranscriptWord], index: usize) -> Option<String> {
    let word = words.get(index)?;
    let normalized = normalize_transcript_token(&word.text);
    if matches!(normalized.as_str(), "wait" | "actually") {
        return Some(word.text.trim().to_string());
    }
    let next = words.get(index + 1)?;
    if normalized == "let" && normalize_transcript_token(&next.text) == "me" {
        return Some("let me".to_string());
    }
    None
}

fn cleanup_range(segment: &TranscriptSegment, config: CleanupConfig) -> Option<CleanupRange> {
    if segment.end_s <= segment.start_s {
        return None;
    }
    let tokens = tokenize(&segment.text);
    if tokens.is_empty() || actionable_score(&tokens) >= 3 {
        return None;
    }
    let filler_token_count = tokens
        .iter()
        .filter(|token| is_cleanup_filler_token(token))
        .count();
    if filler_token_count < config.min_filler_tokens {
        return None;
    }
    let total_token_count = tokens.len();
    let ratio = filler_token_count as f64 / total_token_count as f64;
    if ratio < config.min_filler_ratio {
        return None;
    }
    Some(CleanupRange {
        start_s: segment.start_s,
        end_s: segment.end_s,
        filler_token_count,
        total_token_count,
    })
}

fn actionable_score(tokens: &[String]) -> usize {
    if tokens.is_empty() {
        return 0;
    }
    let joined = tokens.join(" ");
    let mut score = 0;
    if joined.contains("biggest thing") {
        score += 3;
    }
    if joined.contains("founder") || joined.contains("founders") {
        score += 2;
    }
    if joined.contains("advice") || joined.contains("lesson") || joined.contains("tip") {
        score += 2;
    }
    if joined.contains("make sure") || joined.contains("you need") || joined.contains("you should")
    {
        score += 2;
    }
    if joined.contains("i can say") || joined.contains("what i can say") {
        score += 1;
    }
    score
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(normalize_transcript_token)
        .filter(|token| !token.is_empty())
        .collect()
}

fn is_cleanup_filler_token(token: &str) -> bool {
    BASIC_FILLER_TOKENS.contains(&token) || DISCOURSE_MARKER_TOKENS.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start_s: f64, end_s: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start_s,
            end_s,
            text: text.to_string(),
        }
    }

    fn word(text: &str, start_s: f64, end_s: f64) -> TranscriptWord {
        TranscriptWord {
            text: text.to_string(),
            start_s,
            end_s,
        }
    }

    #[test]
    fn production_aside_includes_coaching_lead_in() {
        let segments = vec![
            segment(1525.335, 1525.975, "problems."),
            segment(1527.495, 1529.175, "You're"),
            segment(1529.175, 1530.375, "doing good. Mhmm."),
            segment(1531.255, 1533.390, "Mhmm. I"),
            segment(1535.310, 1535.870, "think"),
            segment(1542.590, 1543.870, "think you can just say, like,"),
            segment(
                1544.595,
                1548.595,
                "I'm building I'm working. I'm the CEO of CoralX.",
            ),
            segment(
                1549.155,
                1554.995,
                "Then maybe we can ask you, when do you when do you decide to become a builder?",
            ),
            segment(1556.115, 1557.395, "We don't have to"),
            segment(
                1558.950,
                1561.350,
                "go into a long one on one in the beginning.",
            ),
            segment(1562.630, 1563.350, "Mhmm."),
            segment(1568.550, 1569.990, "I've actually"),
        ];

        let ranges = production_aside_ranges(&segments, 1463.375, 1531.640);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].start_s - 1527.495).abs() < 1e-9);
        assert!((ranges[0].end_s - 1531.640).abs() < 1e-9);
        assert!(ranges[0].snippet.contains("you can just say"));
    }

    #[test]
    fn production_aside_returns_visible_overlap_for_later_clip() {
        let segments = vec![
            segment(1542.590, 1543.870, "think you can just say, like,"),
            segment(
                1544.595,
                1548.595,
                "I'm building I'm working. I'm the CEO of CoralX.",
            ),
            segment(
                1549.155,
                1554.995,
                "Then maybe we can ask you, when do you when do you decide to become a builder?",
            ),
            segment(1556.115, 1557.395, "We don't have to"),
            segment(
                1558.950,
                1561.350,
                "go into a long one on one in the beginning.",
            ),
            segment(1562.630, 1563.350, "Mhmm."),
            segment(1568.550, 1569.990, "I've actually"),
        ];

        let ranges = production_aside_ranges(&segments, 1542.680, 1556.850);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].start_s - 1542.680).abs() < 1e-9);
        assert!((ranges[0].end_s - 1556.850).abs() < 1e-9);
    }

    #[test]
    fn false_start_lookback_is_bounded_for_late_markers() {
        let words = vec![
            word("First", 0.0, 0.2),
            word("publishable", 0.2, 0.5),
            word("discussion", 0.5, 0.8),
            word("goes", 90.0, 90.2),
            word("on", 90.2, 90.4),
            word("for", 90.4, 90.6),
            word("minutes", 90.6, 91.0),
            word("actually", 100.0, 100.3),
            word("restart", 100.3, 100.7),
        ];

        let ranges = false_start_ranges(&words, 0.0, 120.0);

        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].start_s - 88.0).abs() < 1e-9);
        assert!((ranges[0].end_s - 100.0).abs() < 1e-9);
        assert!(!ranges[0].snippet.contains("First publishable"));
    }
}
