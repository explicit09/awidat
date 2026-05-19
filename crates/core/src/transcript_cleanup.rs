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

        let visible_start_s = preceding_start_s.max(clip_source_start_s);
        let visible_end_s = word.start_s.min(clip_source_end_s);
        if visible_end_s > visible_start_s {
            ranges.push(FalseStartRange {
                start_s: visible_start_s,
                end_s: visible_end_s,
                marker,
                snippet: words
                    .iter()
                    .filter(|candidate| {
                        candidate.start_s >= visible_start_s && candidate.end_s <= visible_end_s
                    })
                    .map(|candidate| candidate.text.trim())
                    .collect::<Vec<_>>()
                    .join(" "),
            });
        }
        index += 1;
    }
    ranges
}

/// Normalize a transcript token for filler/restart matching.
pub fn normalize_transcript_token(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| matches!(c, '.' | ',' | '?' | '!' | ':' | ';'))
        .to_lowercase()
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
