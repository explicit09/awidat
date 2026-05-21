//! Integration test for the ASS/libass caption burn-in path.
//!
//! A project with a Titles track whose caption clip carries
//! per-word timings (WhisperX-style) renders via libass instead of
//! drawtext: the ffmpeg cmd contains `subtitles=<path>` pointing at
//! a real `.ass` file that holds karaoke `\k` tags and one
//! Dialogue line per word so libass owns the reveal timing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_word_timed_caption(dir: &Path) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(5.0 * 24.0, 24.0),
    ));

    let mut caption_clip = Clip::empty("caption-1.000-2.500".to_string());
    caption_clip.source_range = Some(TimeRange::new(
        RationalTime::new(1.0 * 24.0, 24.0),
        RationalTime::new(1.5 * 24.0, 24.0),
    ));
    let mut caption_effect = Effect::new("awidat.title");
    caption_effect
        .metadata
        .insert("text".to_string(), serde_json::json!("hello world"));
    caption_effect
        .metadata
        .insert("start_s".to_string(), serde_json::json!(1.0));
    caption_effect
        .metadata
        .insert("end_s".to_string(), serde_json::json!(2.5));
    caption_effect
        .metadata
        .insert("position".to_string(), serde_json::json!("bottom"));
    caption_effect
        .metadata
        .insert("font_size".to_string(), serde_json::json!(64));
    caption_effect
        .metadata
        .insert("color".to_string(), serde_json::json!("#FFFFFF"));
    caption_effect
        .metadata
        .insert("font_weight".to_string(), serde_json::json!("bold"));
    caption_effect
        .metadata
        .insert("animation".to_string(), serde_json::json!("none"));
    caption_effect
        .metadata
        .insert("role".to_string(), serde_json::json!("caption"));
    caption_effect.metadata.insert(
        "word_timings".to_string(),
        serde_json::json!([
            {"text": "hello", "start_s": 1.0, "end_s": 1.6},
            {"text": "world", "start_s": 1.6, "end_s": 2.4},
        ]),
    );
    caption_clip.effects.push(caption_effect);

    let mut v1 = Track::empty("V1", TrackKind::Video);
    v1.children.push(TrackChild::Clip(clip_a));

    let mut titles_track = Track::empty("Titles", TrackKind::Video);
    titles_track
        .metadata
        .insert("awidat_track_role".to_string(), serde_json::json!("titles"));
    titles_track.children.push(TrackChild::Clip(caption_clip));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(v1));
    stack.children.push(StackChild::Track(titles_track));
    tl.tracks = stack;

    fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();
}

#[test]
fn word_timed_caption_emits_libass_subtitles_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_word_timed_caption(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    // (a) The filter graph should reference an ASS subtitles file
    // instead of (or in addition to) any drawtext for this caption.
    assert!(
        cmd.contains("subtitles="),
        "expected subtitles= filter for word-timed caption, got: {cmd}",
    );

    // (b) Locate the .ass file path emitted into the cmd and verify
    // its contents include karaoke `\k` tags plus the two words.
    let ass_path = cmd
        .split("subtitles=")
        .nth(1)
        .map(|s| {
            // `subtitles=` value runs up to the next filter-arg
            // separator (`:`), filter-chain separator (`,`), label
            // start (`[`), graph separator (`;`), or end of arg.
            let end = s
                .find([':', ',', '[', ';', ' '])
                .unwrap_or(s.len());
            s[..end].to_string()
        })
        .expect("subtitles= argument should carry a path");
    let ass_path = ass_path.trim_matches('\'');

    assert!(
        Path::new(ass_path).exists(),
        "ASS file should be written to disk before ffmpeg runs: {ass_path}",
    );
    let ass = fs::read_to_string(ass_path).unwrap();
    assert!(
        ass.contains("[Script Info]") && ass.contains("[Events]"),
        "ASS file should be a valid ASS subtitle container, got: {ass}",
    );
    assert!(
        ass.contains("\\k"),
        "ASS file should contain karaoke `\\k` tags for word-level timing, got: {ass}",
    );
    assert!(
        ass.contains("hello") && ass.contains("world"),
        "ASS file should contain both transcript words, got: {ass}",
    );
}
