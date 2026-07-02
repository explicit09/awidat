//! `montage plan-dead-air-edl` — generate a deterministic silence-cleanup EDL.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use montage_core::continuity::{WhisperWord, load_whisper_words};
use montage_proto::project::Project;
use montage_render::SilenceRange;
use tokio_util::sync::CancellationToken;

use crate::plan_clip::{SelectedClip, resolve_asset_path, select_clips};
use crate::plan_ranges::{
    SourceRange, append_kept_ranges_edl_ops_with_position_offset, kept_ranges_after_removing,
    merge_ranges, push_non_empty_range,
};

const TRANSCRIPT_WORD_GUARD_PADDING_S: f64 = 0.080;

/// Arguments for `montage plan-dead-air-edl`.
pub struct PlanDeadAirEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
    /// Minimum silence duration to remove, in seconds.
    pub min_duration_s: f64,
    /// Silence threshold in dBFS.
    pub silence_threshold_db: f64,
    /// Seconds to preserve at each side of each detected silence.
    pub keep_padding_s: f64,
    /// Maximum allowed gap between transcript words before cutting dead air.
    pub max_transcript_gap_s: Option<f64>,
}

pub fn run(args: PlanDeadAirEdlArgs) -> Result<()> {
    validate_args(&args)?;
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clips = select_clips(&project.timeline.tracks, args.asset.as_deref())?;
    let mut silences_by_asset = BTreeMap::new();
    let mut transcript_words_by_asset = BTreeMap::new();
    let mut inserted_before_by_track: BTreeMap<String, usize> = BTreeMap::new();
    let mut edl = String::from("*** Begin EDL\n");
    for clip in clips {
        let silences = match silences_by_asset.get(&clip.asset) {
            Some(silences) => silences,
            None => {
                let asset_path = resolve_asset_path(&args.project_root, &clip.asset);
                let silences =
                    detect_silences(&asset_path, args.silence_threshold_db, args.min_duration_s)?;
                silences_by_asset.insert(clip.asset.clone(), silences);
                silences_by_asset
                    .get(&clip.asset)
                    .context("cached silence detection result missing")?
            }
        };
        let transcript_words = transcript_words_by_asset
            .entry(clip.asset.clone())
            .or_insert_with(|| {
                load_whisper_words(&args.project_root, &clip.asset).unwrap_or_default()
            });
        let removed_ranges = dead_air_cuts(
            &clip,
            silences,
            args.keep_padding_s,
            transcript_words,
            max_transcript_gap_s(&args),
        );
        let kept_ranges = kept_ranges_after_removing(&clip, removed_ranges);
        let position_offset = inserted_before_by_track
            .get(&clip.track_name)
            .copied()
            .unwrap_or(0);
        let inserted_count = append_kept_ranges_edl_ops_with_position_offset(
            &mut edl,
            &clip,
            &kept_ranges,
            "after-dead-air",
            position_offset,
        );
        *inserted_before_by_track
            .entry(clip.track_name.clone())
            .or_insert(0) += inserted_count;
    }
    edl.push_str("*** End EDL\n");
    print!("{edl}");
    Ok(())
}

fn validate_args(args: &PlanDeadAirEdlArgs) -> Result<()> {
    if !args.min_duration_s.is_finite() || args.min_duration_s <= 0.0 {
        bail!("--min-duration-s must be finite and > 0");
    }
    if !args.silence_threshold_db.is_finite() || args.silence_threshold_db >= 0.0 {
        bail!("--silence-threshold-db must be finite and < 0");
    }
    if !args.keep_padding_s.is_finite() || args.keep_padding_s < 0.0 {
        bail!("--keep-padding-s must be finite and >= 0");
    }
    if let Some(max_transcript_gap_s) = args.max_transcript_gap_s
        && (!max_transcript_gap_s.is_finite() || max_transcript_gap_s <= 0.0)
    {
        bail!("--max-transcript-gap-s must be finite and > 0");
    }
    Ok(())
}

fn max_transcript_gap_s(args: &PlanDeadAirEdlArgs) -> f64 {
    args.max_transcript_gap_s.unwrap_or(args.min_duration_s)
}

fn detect_silences(
    asset_path: &Path,
    threshold_db: f64,
    min_duration_s: f64,
) -> Result<Vec<SilenceRange>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime
        .block_on(montage_render::generate_silences(
            asset_path,
            threshold_db,
            min_duration_s,
            CancellationToken::new(),
        ))
        .with_context(|| format!("failed to detect silences in {}", asset_path.display()))
}

fn dead_air_cuts(
    clip: &SelectedClip,
    silences: &[SilenceRange],
    keep_padding_s: f64,
    transcript_words: &[WhisperWord],
    max_transcript_gap_s: f64,
) -> Vec<SourceRange> {
    let mut cuts = Vec::new();
    for silence in silences {
        let start_s = (silence.start_s + keep_padding_s).max(clip.source_start_s);
        let end_s = (silence.end_s - keep_padding_s).min(clip.source_end_s);
        append_transcript_safe_silence_cuts(&mut cuts, start_s, end_s, transcript_words);
    }
    cuts.extend(transcript_gap_cuts(
        clip,
        transcript_words,
        keep_padding_s,
        max_transcript_gap_s,
    ));
    merge_ranges(cuts)
}

fn append_transcript_safe_silence_cuts(
    cuts: &mut Vec<SourceRange>,
    start_s: f64,
    end_s: f64,
    transcript_words: &[WhisperWord],
) {
    let protected = transcript_word_protected_ranges(start_s, end_s, transcript_words);
    let mut cursor_s = start_s;
    for range in protected {
        push_non_empty_range(cuts, cursor_s, range.start_s);
        cursor_s = cursor_s.max(range.end_s);
    }
    push_non_empty_range(cuts, cursor_s, end_s);
}

fn transcript_word_protected_ranges(
    start_s: f64,
    end_s: f64,
    transcript_words: &[WhisperWord],
) -> Vec<SourceRange> {
    let protected = transcript_words
        .iter()
        .filter(|word| {
            !word.text.trim().is_empty()
                && word.start_s.is_finite()
                && word.end_s.is_finite()
                && word.end_s > start_s
                && word.start_s < end_s
                && word.end_s > word.start_s
        })
        .map(|word| SourceRange {
            start_s: (word.start_s - TRANSCRIPT_WORD_GUARD_PADDING_S).max(start_s),
            end_s: (word.end_s + TRANSCRIPT_WORD_GUARD_PADDING_S).min(end_s),
        })
        .collect::<Vec<_>>();
    merge_ranges(protected)
}

fn transcript_gap_cuts(
    clip: &SelectedClip,
    transcript_words: &[WhisperWord],
    keep_padding_s: f64,
    max_transcript_gap_s: f64,
) -> Vec<SourceRange> {
    let words = transcript_words_in_clip(clip, transcript_words);
    let mut cuts = Vec::new();
    for pair in words.windows(2) {
        let [left, right] = pair else {
            continue;
        };
        let gap_s = right.start_s - left.end_s;
        if gap_s <= max_transcript_gap_s {
            continue;
        }
        let start_s = left.end_s + TRANSCRIPT_WORD_GUARD_PADDING_S + keep_padding_s;
        let end_s = right.start_s - TRANSCRIPT_WORD_GUARD_PADDING_S - keep_padding_s;
        push_non_empty_range(&mut cuts, start_s, end_s);
    }
    cuts
}

fn transcript_words_in_clip(
    clip: &SelectedClip,
    transcript_words: &[WhisperWord],
) -> Vec<WhisperWord> {
    let mut words = transcript_words
        .iter()
        .filter(|word| {
            !word.text.trim().is_empty()
                && word.start_s.is_finite()
                && word.end_s.is_finite()
                && word.end_s > word.start_s
                && word.end_s > clip.source_start_s
                && word.start_s < clip.source_end_s
        })
        .cloned()
        .collect::<Vec<_>>();
    words.sort_by(|left, right| left.start_s.total_cmp(&right.start_s));
    words
}
