//! `montage plan-transcript-trim-edl` — generate a transcript-anchored trim EDL.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use montage_core::transcript_cleanup::{
    CleanupConfig, TranscriptSegment as CleanupTranscriptSegment, TranscriptWord,
    false_start_ranges, filler_dense_ranges,
};
use montage_proto::project::Project;
use serde_json::Value;

use crate::plan_clip::{SelectedClip, select_clip};
use crate::plan_ranges::{
    SourceRange, build_kept_ranges_edl, kept_ranges_after_removing, push_non_empty_range,
};

/// Arguments for `montage plan-transcript-trim-edl`.
pub struct PlanTranscriptTrimEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Transcript phrase whose segment should become the first kept content.
    pub keep_from_phrase: String,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
}

/// Arguments for `montage plan-transcript-setup-edl`.
pub struct PlanTranscriptSetupEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
}

/// Arguments for `montage plan-transcript-remove-edl`.
pub struct PlanTranscriptRemoveEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Transcript phrases whose containing spans should be removed.
    pub remove_phrases: Vec<String>,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
}

/// Arguments for `montage plan-transcript-cleanup-edl`.
pub struct PlanTranscriptCleanupEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
    /// Minimum filler/discourse-marker ratio for removing a transcript segment.
    pub min_filler_ratio: f64,
    /// Minimum number of filler/discourse markers in a removable segment.
    pub min_filler_tokens: usize,
}

/// Arguments for `montage plan-false-start-edl`.
pub struct PlanFalseStartEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
}

pub fn run(args: PlanTranscriptTrimEdlArgs) -> Result<()> {
    validate_args(&args)?;
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let segments = read_transcript_segments(&args.project_root, &clip.asset)?;
    let anchor = find_phrase_anchor_segment(&segments, &args.keep_from_phrase)
        .with_context(|| format!("no transcript span contains {:?}", args.keep_from_phrase))?;
    let edl = build_trim_edl(&clip, anchor.start_s.max(clip.source_start_s));
    print!("{edl}");
    Ok(())
}

pub fn run_false_start(args: PlanFalseStartEdlArgs) -> Result<()> {
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let words = read_transcript_words(&args.project_root, &clip.asset)?;
    let removed_ranges = false_start_removed_ranges(&clip, &words);
    let kept_ranges = kept_ranges_after_removing(&clip, removed_ranges);
    let edl = build_kept_ranges_edl(&clip, &kept_ranges, "after-false-start-cleanup");
    print!("{edl}");
    Ok(())
}

pub fn run_setup(args: PlanTranscriptSetupEdlArgs) -> Result<()> {
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let segments = read_transcript_segments(&args.project_root, &clip.asset)?;
    let anchor = find_actionable_setup_boundary(&segments).context(
        "no actionable transcript segment found; pass plan-transcript-trim-edl --keep-from-phrase for an explicit anchor",
    )?;
    let edl = build_trim_edl(&clip, anchor.start_s.max(clip.source_start_s));
    print!("{edl}");
    Ok(())
}

pub fn run_remove(args: PlanTranscriptRemoveEdlArgs) -> Result<()> {
    validate_remove_args(&args)?;
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let segments = read_transcript_segments(&args.project_root, &clip.asset)?;
    let removed_ranges = transcript_removed_ranges(&clip, &segments, &args.remove_phrases)?;
    let kept_ranges = kept_ranges_after_removing(&clip, removed_ranges);
    if kept_ranges.is_empty() {
        bail!("transcript removal would remove the entire selected clip");
    }
    let edl = build_kept_ranges_edl(&clip, &kept_ranges, "after-transcript-removal");
    print!("{edl}");
    Ok(())
}

pub fn run_cleanup(args: PlanTranscriptCleanupEdlArgs) -> Result<()> {
    validate_cleanup_args(&args)?;
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let segments = read_transcript_segments(&args.project_root, &clip.asset)?;
    let removed_ranges = cleanup_removed_ranges(
        &clip,
        &segments,
        args.min_filler_ratio,
        args.min_filler_tokens,
    );
    let kept_ranges = kept_ranges_after_removing(&clip, removed_ranges);
    let edl = build_kept_ranges_edl(&clip, &kept_ranges, "after-transcript-cleanup");
    print!("{edl}");
    Ok(())
}

fn validate_args(args: &PlanTranscriptTrimEdlArgs) -> Result<()> {
    if args.keep_from_phrase.trim().is_empty() {
        bail!("--keep-from-phrase must not be empty");
    }
    Ok(())
}

fn validate_cleanup_args(args: &PlanTranscriptCleanupEdlArgs) -> Result<()> {
    if !args.min_filler_ratio.is_finite()
        || args.min_filler_ratio <= 0.0
        || args.min_filler_ratio > 1.0
    {
        bail!("--min-filler-ratio must be finite and in the range (0, 1]");
    }
    if args.min_filler_tokens == 0 {
        bail!("--min-filler-tokens must be > 0");
    }
    Ok(())
}

fn validate_remove_args(args: &PlanTranscriptRemoveEdlArgs) -> Result<()> {
    if args.remove_phrases.is_empty() {
        bail!("at least one --remove-phrase is required");
    }
    if args
        .remove_phrases
        .iter()
        .any(|phrase| phrase.trim().is_empty())
    {
        bail!("--remove-phrase values must not be empty");
    }
    Ok(())
}

fn read_transcript_segments(project_root: &Path, asset: &str) -> Result<Vec<TranscriptSegment>> {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read transcript sidecar {}", path.display()))?;
    let sidecar: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let segments = sidecar
        .pointer("/data/segments")
        .and_then(|segments| segments.as_array())
        .context("transcript sidecar missing /data/segments array")?
        .iter()
        .filter_map(transcript_segment_from_json)
        .filter(|segment| !segment.text.trim().is_empty() && segment.end_s > segment.start_s)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        bail!(
            "transcript sidecar {} has no usable segments",
            path.display()
        );
    }
    Ok(segments)
}

fn read_transcript_words(project_root: &Path, asset: &str) -> Result<Vec<TranscriptWord>> {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read transcript sidecar {}", path.display()))?;
    let sidecar: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let words = sidecar
        .pointer("/data/words")
        .and_then(|words| words.as_array())
        .context("transcript sidecar missing /data/words array")?
        .iter()
        .filter_map(transcript_word_from_json)
        .filter(|word| !word.text.trim().is_empty() && word.end_s > word.start_s)
        .collect::<Vec<_>>();
    if words.is_empty() {
        bail!("transcript sidecar {} has no usable words", path.display());
    }
    Ok(words)
}

fn transcript_segment_from_json(value: &Value) -> Option<TranscriptSegment> {
    let text = value.get("text")?.as_str()?.to_string();
    let start_s = value
        .get("start_s")
        .or_else(|| value.get("start"))?
        .as_f64()?;
    let end_s = value.get("end_s").or_else(|| value.get("end"))?.as_f64()?;
    Some(TranscriptSegment {
        text,
        start_s,
        end_s,
    })
}

fn transcript_word_from_json(value: &Value) -> Option<TranscriptWord> {
    let text = value.get("text")?.as_str()?.to_string();
    let start_s = value
        .get("start_s")
        .or_else(|| value.get("start"))?
        .as_f64()?;
    let end_s = value.get("end_s").or_else(|| value.get("end"))?.as_f64()?;
    Some(TranscriptWord {
        text,
        start_s,
        end_s,
    })
}

fn find_phrase_anchor_segment<'a>(
    segments: &'a [TranscriptSegment],
    phrase: &str,
) -> Option<&'a TranscriptSegment> {
    find_phrase_segment_span(segments, phrase).map(|(start, _end)| start)
}

fn find_phrase_segment_span<'a>(
    segments: &'a [TranscriptSegment],
    phrase: &str,
) -> Option<(&'a TranscriptSegment, &'a TranscriptSegment)> {
    let (start_index, end_index) = find_phrase_segment_indices(segments, phrase)?;
    Some((segments.get(start_index)?, segments.get(end_index)?))
}

fn find_phrase_segment_indices(
    segments: &[TranscriptSegment],
    phrase: &str,
) -> Option<(usize, usize)> {
    let phrase_tokens = tokenize(phrase);
    if phrase_tokens.is_empty() {
        return None;
    }
    let transcript_tokens = segments
        .iter()
        .enumerate()
        .flat_map(|(segment_index, segment)| {
            tokenize(&segment.text)
                .into_iter()
                .map(move |text| TranscriptToken {
                    text,
                    segment_index,
                })
        })
        .collect::<Vec<_>>();
    transcript_tokens
        .windows(phrase_tokens.len())
        .find(|window| {
            window
                .iter()
                .zip(&phrase_tokens)
                .all(|(token, expected)| token.text == *expected)
        })
        .and_then(|window| {
            let first = window.first()?;
            let last = window.last()?;
            Some((first.segment_index, last.segment_index))
        })
}

fn find_actionable_setup_boundary(segments: &[TranscriptSegment]) -> Option<&TranscriptSegment> {
    segments
        .iter()
        .filter(|segment| segment.start_s.is_finite() && segment.end_s.is_finite())
        .find(|segment| actionable_score(segment) >= 3)
}

fn actionable_score(segment: &TranscriptSegment) -> usize {
    let text = tokenize(&segment.text);
    if text.is_empty() {
        return 0;
    }
    let joined = text.join(" ");
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
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn transcript_removed_ranges(
    clip: &SelectedClip,
    segments: &[TranscriptSegment],
    phrases: &[String],
) -> Result<Vec<SourceRange>> {
    let mut removed_ranges = Vec::new();
    for phrase in phrases {
        let (start, end) = find_phrase_segment_span(segments, phrase)
            .with_context(|| format!("no transcript span contains {phrase:?}"))?;
        push_non_empty_range(
            &mut removed_ranges,
            start.start_s.max(clip.source_start_s),
            end.end_s.min(clip.source_end_s),
        );
    }
    Ok(removed_ranges)
}

fn cleanup_removed_ranges(
    clip: &SelectedClip,
    segments: &[TranscriptSegment],
    min_filler_ratio: f64,
    min_filler_tokens: usize,
) -> Vec<SourceRange> {
    let cleanup_segments = segments
        .iter()
        .map(|segment| CleanupTranscriptSegment {
            start_s: segment.start_s,
            end_s: segment.end_s,
            text: segment.text.clone(),
        })
        .collect::<Vec<_>>();
    let config = CleanupConfig {
        min_filler_ratio,
        min_filler_tokens,
    };
    let mut removed_ranges = Vec::new();
    for range in filler_dense_ranges(&cleanup_segments, config) {
        push_non_empty_range(
            &mut removed_ranges,
            range.start_s.max(clip.source_start_s),
            range.end_s.min(clip.source_end_s),
        );
    }
    removed_ranges
}

fn false_start_removed_ranges(clip: &SelectedClip, words: &[TranscriptWord]) -> Vec<SourceRange> {
    let mut removed_ranges = Vec::new();
    for range in false_start_ranges(words, clip.source_start_s, clip.source_end_s) {
        push_non_empty_range(
            &mut removed_ranges,
            range.start_s.max(clip.source_start_s),
            range.end_s.min(clip.source_end_s),
        );
    }
    removed_ranges
}

fn build_trim_edl(clip: &SelectedClip, keep_start_s: f64) -> String {
    build_kept_ranges_edl(
        clip,
        &[SourceRange {
            start_s: keep_start_s,
            end_s: clip.source_end_s,
        }],
        "after-transcript-trim",
    )
}

#[derive(Debug)]
struct TranscriptSegment {
    text: String,
    start_s: f64,
    end_s: f64,
}

#[derive(Debug)]
struct TranscriptToken {
    text: String,
    segment_index: usize,
}
