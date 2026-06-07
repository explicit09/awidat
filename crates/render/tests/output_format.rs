//! Integration tests for `set_output_format`: the timeline's
//! `output_format` aspect ratio (written into
//! `Timeline.metadata.montage.extra["output_format"]` by the
//! `set_output_format` EDL op) must drive the render conform canvas, so
//! a 9:16 project renders vertical instead of being letterboxed into
//! the default 16:9 1920x1080 frame.

#![allow(clippy::unwrap_used)]

use montage_proto::montage_meta::MontageTimelineMetadata;
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind,
};
use montage_proto::project::files;
use montage_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

/// Write a two-cut project whose timeline carries an `output_format`
/// aspect ratio in its montage metadata `extra` map. Two segments force
/// a `concat` filter graph (past the single-segment stream-copy fast
/// path), which is where the conform canvas is chosen — mirroring a
/// scene-aware short-form EDL that cuts the source into scenes.
fn write_project_with_output_format(dir: &Path, aspect_ratio: &str) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let clip = |name: &str, start: f64, dur: f64| {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_a));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(start * 24.0, 24.0),
            RationalTime::new(dur * 24.0, 24.0),
        ));
        clip
    };

    let mut track = Track::empty("V1", TrackKind::Video);
    track
        .children
        .push(TrackChild::Clip(clip("clip-a", 0.0, 4.0)));
    track
        .children
        .push(TrackChild::Clip(clip("clip-b", 6.0, 4.0)));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;

    let mut meta = MontageTimelineMetadata::default();
    meta.extra.insert(
        "output_format".to_string(),
        serde_json::json!({
            "aspect_ratio": aspect_ratio,
            "platform": "vertical_short",
            "safe_area": "mobile",
        }),
    );
    tl.metadata.montage = Some(meta);

    fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();
}

#[test]
fn vertical_output_format_yields_portrait_conform_canvas() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_output_format(dir.path(), "9:16");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("scale=1080:1920:force_original_aspect_ratio=decrease,pad=1080:1920"),
        "expected a 1080x1920 vertical conform canvas, got: {cmd}",
    );
    assert!(
        !cmd.contains("scale=1920:1080"),
        "9:16 output must not letterbox into the 1920x1080 default canvas, got: {cmd}",
    );
}

#[test]
fn default_output_format_stays_landscape() {
    let dir = tempfile::tempdir().unwrap();
    // No output_format metadata -> legacy 16:9 1920x1080 canvas.
    write_project_with_output_format(dir.path(), "16:9");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080"),
        "expected the default 1920x1080 landscape conform canvas, got: {cmd}",
    );
}
