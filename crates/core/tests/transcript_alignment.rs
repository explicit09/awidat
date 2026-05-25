//! Transcript Alignment v2 foundation tests.

use awidat_core::transcript_alignment::{
    PhraseGroupingOptions, apply_transcript_correction, map_transcript_range_to_timeline,
    normalize_whisper_alignment, revert_transcript_edit,
};
use awidat_proto::otio::{
    Clip, ExternalReference, Gap, MediaReference, RationalTime, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind,
};
use awidat_proto::professional::{TranscriptCorrection, TranscriptCorrectionTarget};

fn sidecar(words: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "asset_id": "raw/interview.mov",
        "asset_sha256": "abc123",
        "data": {
            "words": words
        }
    })
}

#[test]
fn transcript_alignment_ids_are_stable() {
    let input = sidecar(serde_json::json!([
        {"text": "Hello", "start_s": 1.0, "end_s": 1.2, "speaker_id": "SPEAKER_00"},
        {"text": "world.", "start_s": 1.25, "end_s": 1.6, "speaker_id": "SPEAKER_00"}
    ]));

    let first = normalize_whisper_alignment(
        "raw/interview.mov",
        &input,
        PhraseGroupingOptions::default(),
    )
    .expect("first normalization");
    let second = normalize_whisper_alignment(
        "raw/interview.mov",
        &input,
        PhraseGroupingOptions::default(),
    )
    .expect("second normalization");

    assert_eq!(first.words[0].id, second.words[0].id);
    assert_eq!(first.words[1].id, second.words[1].id);
    assert_eq!(first.phrases[0].id, second.phrases[0].id);
}

#[test]
fn repeated_words_at_different_ranges_do_not_collide() {
    let input = sidecar(serde_json::json!([
        {"text": "test", "start_s": 1.0, "end_s": 1.2},
        {"text": "test", "start_s": 2.0, "end_s": 2.2}
    ]));

    let alignment = normalize_whisper_alignment(
        "raw/interview.mov",
        &input,
        PhraseGroupingOptions::default(),
    )
    .expect("normalization");

    assert_eq!(alignment.words.len(), 2);
    assert_ne!(alignment.words[0].id, alignment.words[1].id);
}

#[test]
fn phrase_grouping_splits_on_speaker_silence_punctuation_and_max_words() {
    let input = sidecar(serde_json::json!([
        {"text": "One", "start_s": 0.0, "end_s": 0.2, "speaker_id": "A"},
        {"text": "two", "start_s": 0.25, "end_s": 0.45, "speaker_id": "A"},
        {"text": "three", "start_s": 0.5, "end_s": 0.7, "speaker_id": "A"},
        {"text": "done.", "start_s": 0.75, "end_s": 1.0, "speaker_id": "A"},
        {"text": "gap", "start_s": 2.0, "end_s": 2.2, "speaker_id": "A"},
        {"text": "speaker", "start_s": 2.25, "end_s": 2.5, "speaker_id": "B"}
    ]));

    let alignment = normalize_whisper_alignment(
        "raw/interview.mov",
        &input,
        PhraseGroupingOptions {
            max_words: 3,
            silence_gap_s: 0.5,
        },
    )
    .expect("normalization");

    let phrase_texts = alignment
        .phrases
        .iter()
        .map(|phrase| phrase.text.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        phrase_texts,
        vec!["One two three", "done.", "gap", "speaker"]
    );
}

#[test]
fn phrase_ids_before_an_unrelated_later_change_remain_stable() {
    let original = sidecar(serde_json::json!([
        {"text": "Keep", "start_s": 0.0, "end_s": 0.2},
        {"text": "this.", "start_s": 0.25, "end_s": 0.5},
        {"text": "Later", "start_s": 1.0, "end_s": 1.2}
    ]));
    let changed = sidecar(serde_json::json!([
        {"text": "Keep", "start_s": 0.0, "end_s": 0.2},
        {"text": "this.", "start_s": 0.25, "end_s": 0.5},
        {"text": "Changed", "start_s": 1.0, "end_s": 1.2}
    ]));

    let first = normalize_whisper_alignment(
        "raw/interview.mov",
        &original,
        PhraseGroupingOptions::default(),
    )
    .expect("first");
    let second = normalize_whisper_alignment(
        "raw/interview.mov",
        &changed,
        PhraseGroupingOptions::default(),
    )
    .expect("second");

    assert_eq!(first.phrases[0].id, second.phrases[0].id);
}

#[test]
fn transcript_correction_is_reversible() {
    let input = sidecar(serde_json::json!([
        {"text": "teh", "start_s": 1.0, "end_s": 1.2, "speaker_id": "SPEAKER_00"}
    ]));
    let mut alignment = normalize_whisper_alignment(
        "raw/interview.mov",
        &input,
        PhraseGroupingOptions::default(),
    )
    .expect("normalization");
    let word_id = alignment.words[0].id.clone();

    let edit = apply_transcript_correction(
        &mut alignment,
        TranscriptCorrection {
            id: "corr-1".into(),
            target: TranscriptCorrectionTarget::Word {
                word_id: word_id.clone(),
            },
            corrected_text: "the".into(),
            original_text: "teh".into(),
            source: "user".into(),
            corrected_at: "2026-05-25T00:00:00Z".into(),
        },
    )
    .expect("apply correction");

    assert_eq!(alignment.words[0].id, word_id);
    assert_eq!(alignment.words[0].original_text, "teh");
    assert_eq!(alignment.words[0].corrected_text.as_deref(), Some("the"));
    assert_eq!(alignment.words[0].speaker_id.as_deref(), Some("SPEAKER_00"));
    assert_eq!(alignment.words[0].range.start_s, 1.0);

    revert_transcript_edit(&mut alignment, &edit.id).expect("revert correction");

    assert_eq!(alignment.words[0].corrected_text, None);
    assert!(alignment.edit_log.iter().any(|record| record.reverted));
}

#[test]
fn transcript_source_range_maps_to_timeline() {
    let timeline = timeline_with_repeated_source();

    let spans = map_transcript_range_to_timeline(&timeline, "raw/interview.mov", 12.0, 13.0);

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].clip_id, "clip-a");
    assert_eq!(spans[0].timeline_range.start_s, 2.0);
    assert_eq!(spans[0].timeline_range.end_s, 3.0);
    assert_eq!(spans[0].source_range.start_s, 12.0);
    assert_eq!(spans[0].source_range.end_s, 13.0);
    assert_eq!(spans[1].clip_id, "clip-b");
    assert_eq!(spans[1].timeline_range.start_s, 8.0);
    assert_eq!(spans[1].timeline_range.end_s, 9.0);

    let outside = map_transcript_range_to_timeline(&timeline, "raw/interview.mov", 30.0, 31.0);
    assert!(outside.is_empty());
}

fn timeline_with_repeated_source() -> Timeline {
    let mut timeline = Timeline::empty("transcript-map");
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip(
        "clip-a",
        "raw/interview.mov",
        10.0,
        5.0,
    )));
    track
        .children
        .push(TrackChild::Gap(Gap::of_duration(3.0, 24.0)));
    track.children.push(TrackChild::Clip(clip(
        "clip-b",
        "raw/interview.mov",
        12.0,
        3.0,
    )));
    track
        .children
        .push(TrackChild::Clip(clip("clip-c", "raw/other.mov", 12.0, 3.0)));
    timeline.tracks.children.push(StackChild::Track(track));
    timeline
}

fn clip(name: &str, asset_id: &str, source_start_s: f64, duration_s: f64) -> Clip {
    let mut clip = Clip::empty(name.to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset_id.to_string()));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(source_start_s * 24.0, 24.0),
        RationalTime::new(duration_s * 24.0, 24.0),
    ));
    clip
}
