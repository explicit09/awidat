//! Compact transcript packs for speech-led planning.

use std::path::Path;

use awidat_index::walk_indexer;
use awidat_proto::otio::{MediaReference, Stack, StackChild, Timeline, Track, TrackChild};
use serde::Serialize;

/// Maximum words emitted per asset when no caller override is supplied.
pub const DEFAULT_MAX_WORDS_PER_ASSET: usize = 120;
/// Hard cap to keep tool output bounded.
pub const MAX_WORDS_PER_ASSET: usize = 500;

/// Transcript-pack selection options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptPackOptions {
    /// Optional project-relative asset id to include.
    pub asset_id: Option<String>,
    /// Maximum word-timing entries emitted per asset.
    pub max_words_per_asset: Option<usize>,
}

/// Compact transcript evidence grouped by asset.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptPack {
    /// Included asset packs.
    pub assets: Vec<TranscriptPackAsset>,
    /// Count of assets found before truncating/filtering output.
    pub total_matching_assets: usize,
    /// Count of parsed word timings before per-asset truncation.
    pub total_word_count: usize,
    /// Human-readable caveats about missing or truncated data.
    pub warnings: Vec<String>,
}

/// Transcript evidence for one asset.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptPackAsset {
    /// Project asset id.
    pub asset_id: String,
    /// Parsed transcript segments.
    pub segments: Vec<TranscriptPackSegment>,
    /// Parsed word-level timings, capped by the request.
    pub words: Vec<TranscriptPackWord>,
    /// Number of word timings present before capping.
    pub word_count: usize,
    /// Whether word timings were capped.
    pub truncated: bool,
    /// Compact plain text assembled from segments first, then words.
    pub text: String,
}

/// One transcript segment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptPackSegment {
    /// Segment start in source-media seconds.
    pub start_s: Option<f64>,
    /// Segment end in source-media seconds.
    pub end_s: Option<f64>,
    /// Optional speaker label.
    pub speaker: Option<String>,
    /// Segment text.
    pub text: String,
}

/// One transcript word timing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptPackWord {
    /// Word text.
    pub text: String,
    /// Word start in source-media seconds.
    pub start_s: f64,
    /// Word end in source-media seconds.
    pub end_s: f64,
    /// Optional speaker label.
    pub speaker: Option<String>,
}

/// Build compact transcript packs from whisper sidecars.
pub fn build_transcript_pack(
    project_root: &Path,
    options: TranscriptPackOptions,
) -> Result<TranscriptPack, awidat_index::SidecarError> {
    build_transcript_pack_with_filter(project_root, options, None)
}

/// Build compact transcript packs limited to source ranges visible on the timeline.
pub fn build_timeline_transcript_pack(
    project_root: &Path,
    timeline: &Timeline,
    options: TranscriptPackOptions,
) -> Result<TranscriptPack, awidat_index::SidecarError> {
    let ranges = timeline_visible_source_ranges(timeline);
    build_transcript_pack_with_filter(project_root, options, Some(&ranges))
}

fn build_transcript_pack_with_filter(
    project_root: &Path,
    options: TranscriptPackOptions,
    visible_ranges: Option<&[VisibleSourceRange]>,
) -> Result<TranscriptPack, awidat_index::SidecarError> {
    let max_words = options
        .max_words_per_asset
        .unwrap_or(DEFAULT_MAX_WORDS_PER_ASSET)
        .clamp(1, MAX_WORDS_PER_ASSET);
    let mut assets = Vec::new();
    let mut total_word_count = 0usize;
    let walker = walk_indexer(project_root, "whisper")?;
    for (asset_id, sidecar) in walker {
        if options
            .asset_id
            .as_ref()
            .is_some_and(|filter| filter != &asset_id)
        {
            continue;
        }
        let mut segments = parse_segments(&sidecar);
        let mut words = parse_words(&sidecar);
        if let Some(ranges) = visible_ranges {
            let asset_ranges = ranges
                .iter()
                .filter(|range| range.asset_id == asset_id)
                .collect::<Vec<_>>();
            if asset_ranges.is_empty() {
                continue;
            }
            segments.retain(|segment| {
                timed_range(segment.start_s, segment.end_s)
                    .is_some_and(|(start_s, end_s)| overlaps_any(start_s, end_s, &asset_ranges))
            });
            words.retain(|word| overlaps_any(word.start_s, word.end_s, &asset_ranges));
        }
        let word_count = words.len();
        total_word_count += word_count;
        let truncated = word_count > max_words;
        words.truncate(max_words);
        let text = pack_text(&segments, &words);
        assets.push(TranscriptPackAsset {
            asset_id,
            segments,
            words,
            word_count,
            truncated,
            text,
        });
    }
    assets.sort_by(|left, right| left.asset_id.cmp(&right.asset_id));
    let mut warnings = Vec::new();
    if assets.is_empty() {
        warnings.push("no matching whisper transcript sidecars found".into());
    }
    warnings.extend(
        assets
            .iter()
            .filter(|asset| asset.words.is_empty())
            .map(|asset| format!("{} has no word-level timings", asset.asset_id)),
    );
    warnings.extend(assets.iter().filter(|asset| asset.truncated).map(|asset| {
        format!(
            "{} word timings truncated from {} to {}",
            asset.asset_id,
            asset.word_count,
            asset.words.len()
        )
    }));
    Ok(TranscriptPack {
        total_matching_assets: assets.len(),
        total_word_count,
        assets,
        warnings,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct VisibleSourceRange {
    asset_id: String,
    start_s: f64,
    end_s: f64,
}

fn timeline_visible_source_ranges(timeline: &Timeline) -> Vec<VisibleSourceRange> {
    let mut ranges = Vec::new();
    collect_stack_visible_ranges(&timeline.tracks, &mut ranges);
    ranges
}

fn collect_stack_visible_ranges(stack: &Stack, ranges: &mut Vec<VisibleSourceRange>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_visible_ranges(track, ranges),
            StackChild::Stack(stack) => collect_stack_visible_ranges(stack, ranges),
            StackChild::Clip(clip) => collect_clip_visible_range(clip, ranges),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_visible_ranges(track: &Track, ranges: &mut Vec<VisibleSourceRange>) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_clip_visible_range(clip, ranges),
            TrackChild::Stack(stack) => collect_stack_visible_ranges(stack, ranges),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_clip_visible_range(
    clip: &awidat_proto::otio::Clip,
    ranges: &mut Vec<VisibleSourceRange>,
) {
    let MediaReference::External(reference) = &clip.media_reference else {
        return;
    };
    let Some(source_range) = clip.source_range.as_ref() else {
        return;
    };
    let start_s = source_range.start_time.to_seconds();
    let duration_s = source_range.duration.to_seconds();
    if !start_s.is_finite() || !duration_s.is_finite() || duration_s <= 0.0 {
        return;
    }
    ranges.push(VisibleSourceRange {
        asset_id: reference.target_url.clone(),
        start_s,
        end_s: start_s + duration_s,
    });
}

fn overlaps_any(start_s: f64, end_s: f64, ranges: &[&VisibleSourceRange]) -> bool {
    ranges
        .iter()
        .any(|range| end_s > range.start_s && start_s < range.end_s)
}

fn timed_range(start_s: Option<f64>, end_s: Option<f64>) -> Option<(f64, f64)> {
    let start_s = start_s?;
    let end_s = end_s?;
    (end_s >= start_s).then_some((start_s, end_s))
}

fn parse_segments(sidecar: &serde_json::Value) -> Vec<TranscriptPackSegment> {
    sidecar
        .pointer("/data/segments")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|segment| {
            let text = segment.get("text").and_then(|value| value.as_str())?;
            let text = text.trim();
            (!text.is_empty()).then(|| TranscriptPackSegment {
                start_s: numeric_field(segment, &["start_s", "start"]),
                end_s: numeric_field(segment, &["end_s", "end"]),
                speaker: speaker_field(segment),
                text: text.to_string(),
            })
        })
        .collect()
}

fn parse_words(sidecar: &serde_json::Value) -> Vec<TranscriptPackWord> {
    let mut words = sidecar
        .pointer("/data/words")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|word| {
            let text = word.get("text").and_then(|value| value.as_str())?.trim();
            let start_s = numeric_field(word, &["start_s", "start"])?;
            let end_s = numeric_field(word, &["end_s", "end"])?;
            (!text.is_empty() && end_s >= start_s).then(|| TranscriptPackWord {
                text: text.to_string(),
                start_s,
                end_s,
                speaker: speaker_field(word),
            })
        })
        .collect::<Vec<_>>();
    words.sort_by(|left, right| {
        left.start_s
            .total_cmp(&right.start_s)
            .then_with(|| left.end_s.total_cmp(&right.end_s))
            .then_with(|| left.text.cmp(&right.text))
    });
    words
}

fn numeric_field(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_f64()))
        .filter(|number| number.is_finite())
}

fn speaker_field(value: &serde_json::Value) -> Option<String> {
    value
        .get("speaker")
        .or_else(|| value.get("speaker_id"))
        .and_then(|speaker| speaker.as_str())
        .map(ToString::to_string)
}

fn pack_text(segments: &[TranscriptPackSegment], words: &[TranscriptPackWord]) -> String {
    if !words.is_empty() {
        return words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
    }
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
        Timeline, Track, TrackChild, TrackKind,
    };

    fn write_sidecar(root: &Path, asset_id: &str, body: serde_json::Value) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
    }

    fn timeline_with_clip(asset_id: &str, source_start_s: f64, duration_s: f64) -> Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = Timeline::empty("test");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        timeline
    }

    #[test]
    fn transcript_pack_caps_words_and_preserves_segments() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            serde_json::json!({
                "data": {
                    "segments": [
                        {"start_s": 0.0, "end_s": 1.0, "speaker": "S1", "text": "hello world"}
                    ],
                    "words": [
                        {"start_s": 0.0, "end_s": 0.2, "speaker": "S1", "text": "hello"},
                        {"start_s": 0.2, "end_s": 0.5, "speaker": "S1", "text": "world"},
                        {"start_s": 0.5, "end_s": 0.8, "speaker": "S1", "text": "again"}
                    ]
                }
            }),
        );

        let pack = build_transcript_pack(
            dir.path(),
            TranscriptPackOptions {
                asset_id: Some("raw/a.mp4".into()),
                max_words_per_asset: Some(2),
            },
        )
        .unwrap();

        assert_eq!(pack.total_matching_assets, 1);
        assert_eq!(pack.total_word_count, 3);
        assert_eq!(pack.assets[0].segments[0].text, "hello world");
        assert_eq!(pack.assets[0].words.len(), 2);
        assert!(pack.assets[0].truncated);
        assert!(
            pack.warnings
                .iter()
                .any(|warning| warning.contains("truncated from 3 to 2"))
        );
    }

    #[test]
    fn transcript_pack_warns_when_words_are_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            serde_json::json!({
                "data": {
                    "segments": [
                        {"start": 1.0, "end": 2.0, "speaker_id": "S2", "text": "segment only"}
                    ]
                }
            }),
        );

        let pack = build_transcript_pack(dir.path(), TranscriptPackOptions::default()).unwrap();

        assert_eq!(pack.assets[0].text, "segment only");
        assert!(pack.assets[0].words.is_empty());
        assert!(
            pack.warnings
                .iter()
                .any(|warning| warning.contains("no word-level timings"))
        );
    }

    #[test]
    fn timeline_transcript_pack_filters_to_visible_clip_source_ranges() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            serde_json::json!({
                "data": {
                    "segments": [
                        {"start_s": 0.0, "end_s": 5.0, "text": "before visible after"}
                    ],
                    "words": [
                        {"start_s": 0.0, "end_s": 0.5, "text": "before"},
                        {"start_s": 1.2, "end_s": 1.6, "text": "visible"},
                        {"start_s": 4.0, "end_s": 4.5, "text": "after"}
                    ]
                }
            }),
        );
        let timeline = timeline_with_clip("raw/a.mp4", 1.0, 1.0);

        let pack = build_timeline_transcript_pack(
            dir.path(),
            &timeline,
            TranscriptPackOptions {
                asset_id: None,
                max_words_per_asset: Some(10),
            },
        )
        .unwrap();

        assert_eq!(pack.total_matching_assets, 1);
        assert_eq!(pack.total_word_count, 1);
        assert_eq!(pack.assets[0].words.len(), 1);
        assert_eq!(pack.assets[0].words[0].text, "visible");
        assert_eq!(pack.assets[0].text, "visible");
    }
}
