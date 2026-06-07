//! Transcript Alignment v2 normalization helpers.

use montage_proto::otio::{Clip, MediaReference, Stack, StackChild, Timeline, Track, TrackChild};
use montage_proto::professional::{
    AlignedTranscriptPhrase, AlignedTranscriptWord, SourceRange, TranscriptAlignmentPackage,
    TranscriptCorrection, TranscriptCorrectionTarget, TranscriptEditAction, TranscriptEditRecord,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Phrase grouping controls for transcript normalization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhraseGroupingOptions {
    /// Maximum words in one phrase before forcing a split.
    pub max_words: usize,
    /// Silence gap in seconds that starts a new phrase.
    pub silence_gap_s: f64,
}

/// A transcript source range projected onto a concrete timeline clip range.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptTimelineSpan {
    /// Clip name that carries this projected source range.
    pub clip_id: String,
    /// Media asset id / target URL matched by the transcript package.
    pub asset_id: String,
    /// Matched range in the original media asset.
    pub source_range: SourceRange,
    /// Corresponding range in timeline seconds.
    pub timeline_range: SourceRange,
}

impl Default for PhraseGroupingOptions {
    fn default() -> Self {
        Self {
            max_words: 12,
            silence_gap_s: 0.75,
        }
    }
}

/// Transcript alignment normalization error.
#[derive(Debug, Error)]
pub enum TranscriptAlignmentError {
    /// The sidecar does not contain a usable `data.words` array.
    #[error("transcript alignment: sidecar has no usable data.words array")]
    MissingWords,
    /// A word has invalid timing.
    #[error("transcript alignment: word #{index} has invalid timing")]
    InvalidWordTiming {
        /// Word index.
        index: usize,
    },
    /// Correction target was not found.
    #[error("transcript alignment: correction target not found")]
    CorrectionTargetNotFound,
    /// Edit record was not found.
    #[error("transcript alignment: edit {edit_id} not found")]
    EditNotFound {
        /// Missing edit id.
        edit_id: String,
    },
    /// Correction record was not found.
    #[error("transcript alignment: correction {correction_id} not found")]
    CorrectionNotFound {
        /// Missing correction id.
        correction_id: String,
    },
}

/// Normalize a Whisper-like sidecar into Montage-owned stable alignment records.
pub fn normalize_whisper_alignment(
    asset_id: &str,
    sidecar: &Value,
    options: PhraseGroupingOptions,
) -> Result<TranscriptAlignmentPackage, TranscriptAlignmentError> {
    let words_value = sidecar
        .pointer("/data/words")
        .and_then(Value::as_array)
        .ok_or(TranscriptAlignmentError::MissingWords)?;
    let source_sha256 = sidecar
        .get("asset_sha256")
        .and_then(Value::as_str)
        .map(str::to_string);

    let asset_hash = short_hash(asset_id);
    let mut words = Vec::new();
    for (index, value) in words_value.iter().enumerate() {
        let text = value
            .get("text")
            .or_else(|| value.get("word"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let start_s = value
            .get("start_s")
            .or_else(|| value.get("start"))
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let end_s = value
            .get("end_s")
            .or_else(|| value.get("end"))
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let range = SourceRange { start_s, end_s };
        if !range.is_valid() {
            return Err(TranscriptAlignmentError::InvalidWordTiming { index });
        }
        let occurrence = words.len() as u32;
        words.push(AlignedTranscriptWord {
            id: word_id(&asset_hash, occurrence, &range, text),
            original_text: text.to_string(),
            corrected_text: None,
            range,
            occurrence,
            speaker_id: value
                .get("speaker_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            confidence: value.get("confidence").and_then(Value::as_f64),
        });
    }

    let phrases = group_phrases(&asset_hash, &words, options);
    Ok(TranscriptAlignmentPackage {
        asset_id: asset_id.to_string(),
        source_sha256,
        words,
        phrases,
        corrections: Vec::new(),
        edit_log: Vec::new(),
    })
}

/// Apply a transcript correction while preserving original alignment identity.
pub fn apply_transcript_correction(
    package: &mut TranscriptAlignmentPackage,
    correction: TranscriptCorrection,
) -> Result<TranscriptEditRecord, TranscriptAlignmentError> {
    match &correction.target {
        TranscriptCorrectionTarget::Word { word_id } => {
            let word = package
                .words
                .iter_mut()
                .find(|word| &word.id == word_id)
                .ok_or(TranscriptAlignmentError::CorrectionTargetNotFound)?;
            word.corrected_text = Some(correction.corrected_text.clone());
        }
        TranscriptCorrectionTarget::Phrase { phrase_id } => {
            if !package.phrases.iter().any(|phrase| &phrase.id == phrase_id) {
                return Err(TranscriptAlignmentError::CorrectionTargetNotFound);
            }
        }
    }

    let edit = TranscriptEditRecord {
        id: format!("edit:{}", short_hash(&format!("{}:apply", correction.id))),
        correction_id: correction.id.clone(),
        action: TranscriptEditAction::ApplyCorrection,
        reverted: false,
        recorded_at: correction.corrected_at.clone(),
    };
    package.corrections.push(correction);
    package.edit_log.push(edit.clone());
    Ok(edit)
}

/// Revert a previously applied transcript correction.
pub fn revert_transcript_edit(
    package: &mut TranscriptAlignmentPackage,
    edit_id: &str,
) -> Result<(), TranscriptAlignmentError> {
    let edit_index = package
        .edit_log
        .iter()
        .position(|edit| edit.id == edit_id)
        .ok_or_else(|| TranscriptAlignmentError::EditNotFound {
            edit_id: edit_id.to_string(),
        })?;
    let correction_id = package.edit_log[edit_index].correction_id.clone();
    let correction = package
        .corrections
        .iter()
        .find(|correction| correction.id == correction_id)
        .ok_or_else(|| TranscriptAlignmentError::CorrectionNotFound {
            correction_id: correction_id.clone(),
        })?;

    match &correction.target {
        TranscriptCorrectionTarget::Word { word_id } => {
            let word = package
                .words
                .iter_mut()
                .find(|word| &word.id == word_id)
                .ok_or(TranscriptAlignmentError::CorrectionTargetNotFound)?;
            word.corrected_text = None;
        }
        TranscriptCorrectionTarget::Phrase { .. } => {}
    }
    package.edit_log[edit_index].reverted = true;
    package.edit_log.push(TranscriptEditRecord {
        id: format!("edit:{}", short_hash(&format!("{correction_id}:revert"))),
        correction_id,
        action: TranscriptEditAction::RevertCorrection,
        reverted: false,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    });
    Ok(())
}

/// Project transcript source seconds onto all matching visible timeline spans.
pub fn map_transcript_range_to_timeline(
    timeline: &Timeline,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
) -> Vec<TranscriptTimelineSpan> {
    if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
        return Vec::new();
    }
    let mut spans = Vec::new();
    collect_stack_timeline_spans(&timeline.tracks, asset_id, start_s, end_s, 0.0, &mut spans);
    spans
}

fn collect_stack_timeline_spans(
    stack: &Stack,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
    timeline_start_s: f64,
    spans: &mut Vec<TranscriptTimelineSpan>,
) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_timeline_spans(
                track,
                asset_id,
                start_s,
                end_s,
                timeline_start_s,
                spans,
            ),
            StackChild::Stack(stack) => collect_stack_timeline_spans(
                stack,
                asset_id,
                start_s,
                end_s,
                timeline_start_s,
                spans,
            ),
            StackChild::Clip(clip) => {
                collect_clip_timeline_span(clip, asset_id, start_s, end_s, timeline_start_s, spans)
            }
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_timeline_spans(
    track: &Track,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
    timeline_start_s: f64,
    spans: &mut Vec<TranscriptTimelineSpan>,
) {
    let mut cursor_s = timeline_start_s;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                collect_clip_timeline_span(clip, asset_id, start_s, end_s, cursor_s, spans);
                cursor_s += clip_duration_s(clip);
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Stack(stack) => {
                collect_stack_timeline_spans(stack, asset_id, start_s, end_s, cursor_s, spans);
                cursor_s += stack_duration_s(stack);
            }
            TrackChild::Transition(_) => {}
        }
    }
}

fn collect_clip_timeline_span(
    clip: &Clip,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
    timeline_start_s: f64,
    spans: &mut Vec<TranscriptTimelineSpan>,
) {
    let MediaReference::External(reference) = &clip.media_reference else {
        return;
    };
    if reference.target_url != asset_id {
        return;
    }
    let Some(source_range) = clip.source_range.as_ref() else {
        return;
    };
    let clip_source_start_s = source_range.start_time.to_seconds();
    let clip_source_end_s = clip_source_start_s + source_range.duration.to_seconds();
    if !clip_source_start_s.is_finite() || !clip_source_end_s.is_finite() {
        return;
    }

    let overlap_start_s = start_s.max(clip_source_start_s);
    let overlap_end_s = end_s.min(clip_source_end_s);
    if overlap_end_s <= overlap_start_s {
        return;
    }

    let timeline_overlap_start_s = timeline_start_s + (overlap_start_s - clip_source_start_s);
    let timeline_overlap_end_s = timeline_start_s + (overlap_end_s - clip_source_start_s);
    spans.push(TranscriptTimelineSpan {
        clip_id: clip.name.clone(),
        asset_id: reference.target_url.clone(),
        source_range: SourceRange {
            start_s: overlap_start_s,
            end_s: overlap_end_s,
        },
        timeline_range: SourceRange {
            start_s: timeline_overlap_start_s,
            end_s: timeline_overlap_end_s,
        },
    });
}

fn stack_duration_s(stack: &Stack) -> f64 {
    stack
        .children
        .iter()
        .map(|child| match child {
            StackChild::Track(track) => track_duration_s(track),
            StackChild::Stack(stack) => stack_duration_s(stack),
            StackChild::Clip(clip) => clip_duration_s(clip),
            StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        })
        .fold(0.0, f64::max)
}

fn track_duration_s(track: &Track) -> f64 {
    track
        .children
        .iter()
        .map(|child| match child {
            TrackChild::Clip(clip) => clip_duration_s(clip),
            TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
            TrackChild::Stack(stack) => stack_duration_s(stack),
            TrackChild::Transition(_) => 0.0,
        })
        .sum()
}

fn clip_duration_s(clip: &Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .filter(|duration_s| duration_s.is_finite() && *duration_s > 0.0)
        .unwrap_or(0.0)
}

fn group_phrases(
    asset_hash: &str,
    words: &[AlignedTranscriptWord],
    options: PhraseGroupingOptions,
) -> Vec<AlignedTranscriptPhrase> {
    let max_words = options.max_words.max(1);
    let mut phrases = Vec::new();
    let mut start = 0usize;
    while start < words.len() {
        let mut end = start + 1;
        while end < words.len() {
            let prev = &words[end - 1];
            let next = &words[end];
            let word_count = end - start;
            if word_count >= max_words
                || ends_phrase(&prev.original_text)
                || speaker_changed(prev, next)
                || next.range.start_s - prev.range.end_s >= options.silence_gap_s
            {
                break;
            }
            end += 1;
        }
        phrases.push(build_phrase(asset_hash, &words[start..end]));
        start = end;
    }
    phrases
}

fn build_phrase(asset_hash: &str, words: &[AlignedTranscriptWord]) -> AlignedTranscriptPhrase {
    let first = &words[0];
    let last = &words[words.len() - 1];
    let text = words
        .iter()
        .map(|word| word.original_text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let speaker_id = words
        .iter()
        .map(|word| word.speaker_id.as_deref())
        .reduce(|a, b| if a == b { a } else { None })
        .flatten()
        .map(str::to_string);
    AlignedTranscriptPhrase {
        id: phrase_id(asset_hash, first.occurrence, last.occurrence, &text),
        range: SourceRange {
            start_s: first.range.start_s,
            end_s: last.range.end_s,
        },
        word_ids: words.iter().map(|word| word.id.clone()).collect(),
        text,
        speaker_id,
    }
}

fn speaker_changed(prev: &AlignedTranscriptWord, next: &AlignedTranscriptWord) -> bool {
    prev.speaker_id != next.speaker_id
}

fn ends_phrase(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .is_some_and(|ch| matches!(ch, '.' | '?' | '!' | ';'))
}

fn word_id(asset_hash: &str, occurrence: u32, range: &SourceRange, text: &str) -> String {
    format!(
        "word:{asset_hash}:{occurrence}:{}:{}:{}",
        millis(range.start_s),
        millis(range.end_s),
        short_hash(&normalize_text(text))
    )
}

fn phrase_id(asset_hash: &str, first_occurrence: u32, last_occurrence: u32, text: &str) -> String {
    format!(
        "phrase:{asset_hash}:{first_occurrence}:{last_occurrence}:{}",
        short_hash(&normalize_text(text))
    )
}

fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}").chars().take(12).collect()
}
