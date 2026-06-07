//! Subtitle interchange contract tests.

use montage_proto::subtitle::{
    SubtitleCue, SubtitleFormat, SubtitleTrack, TranscriptSegment, format_srt, format_vtt,
    parse_srt, parse_vtt,
};

#[test]
fn srt_and_vtt_round_trip_editable_subtitle_cues() -> Result<(), Box<dyn std::error::Error>> {
    let srt = "\
1
00:00:01,000 --> 00:00:02,250
First line

2
00:00:03,500 --> 00:00:05,000
Second line

";
    let cues = parse_srt(srt)?;

    assert_eq!(cues.len(), 2);
    assert_eq!(cues[0].start_s, 1.0);
    assert_eq!(cues[0].end_s, 2.25);
    assert_eq!(cues[0].text, "First line");
    assert!(format_srt(&cues).contains("00:00:03,500 --> 00:00:05,000"));

    let vtt = format_vtt(&cues);
    assert!(vtt.starts_with("WEBVTT\n\n"));
    assert!(vtt.contains("00:00:01.000 --> 00:00:02.250"));
    assert_eq!(parse_vtt(&vtt)?, cues);
    Ok(())
}

#[test]
fn subtitle_track_validation_rejects_collisions() -> Result<(), Box<dyn std::error::Error>> {
    let track = SubtitleTrack {
        id: "captions-en".into(),
        label: "English captions".into(),
        language: Some("en".into()),
        source_format: Some(SubtitleFormat::Srt),
        cues: vec![
            SubtitleCue::new(0.0, 1.2, "one")?,
            SubtitleCue::new(1.0, 1.6, "overlap")?,
        ],
    };

    let err = match track.validate() {
        Ok(()) => return Err("expected subtitle collision error".into()),
        Err(err) => err,
    };
    assert!(err.to_string().contains("overlaps previous cue"));
    Ok(())
}

#[test]
fn transcript_segments_convert_to_subtitle_track() -> Result<(), Box<dyn std::error::Error>> {
    let segments = vec![
        TranscriptSegment {
            start_s: 2.0,
            end_s: 2.4,
            text: "Hello".into(),
        },
        TranscriptSegment {
            start_s: 2.4,
            end_s: 2.9,
            text: "world".into(),
        },
    ];

    let track = SubtitleTrack::from_transcript_segments(
        "captions-en",
        "English captions",
        Some("en"),
        segments,
    )?;

    assert_eq!(track.cues.len(), 2);
    assert_eq!(track.cues[1].text, "world");
    assert_eq!(track.validate(), Ok(()));
    Ok(())
}
