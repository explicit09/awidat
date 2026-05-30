//! Integration test for Step 16.3: a project with a Titles track
//! containing one synthesized title clip lands a `drawtext=` filter
//! in the rendered argv.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_title(dir: &Path) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(5.0 * 24.0, 24.0),
    ));

    // Build the title clip: empty media reference, awidat.title effect.
    let mut title_clip = Clip::empty("title-0.000-3.000".to_string());
    title_clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(3.0 * 24.0, 24.0),
    ));
    let mut title_effect = Effect::new("awidat.title");
    title_effect
        .metadata
        .insert("text".to_string(), serde_json::json!("Welcome"));
    title_effect
        .metadata
        .insert("start_s".to_string(), serde_json::json!(0.0));
    title_effect
        .metadata
        .insert("end_s".to_string(), serde_json::json!(3.0));
    title_effect
        .metadata
        .insert("position".to_string(), serde_json::json!("top"));
    title_effect
        .metadata
        .insert("font_size".to_string(), serde_json::json!(64));
    title_effect
        .metadata
        .insert("color".to_string(), serde_json::json!("#FFFFFF"));
    title_effect
        .metadata
        .insert("font_weight".to_string(), serde_json::json!("normal"));
    title_effect
        .metadata
        .insert("animation".to_string(), serde_json::json!("none"));
    title_clip.effects.push(title_effect);

    let mut v1 = Track::empty("V1", TrackKind::Video);
    v1.children.push(TrackChild::Clip(clip_a));

    let mut titles_track = Track::empty("Titles", TrackKind::Video);
    titles_track
        .metadata
        .insert("awidat_track_role".to_string(), serde_json::json!("titles"));
    titles_track.children.push(TrackChild::Clip(title_clip));

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
fn project_with_title_emits_drawtext_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_title(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("drawtext=text='Welcome'"),
        "expected drawtext in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("enable='between(t\\,0\\,3)'"),
        "expected enable window in argv, got: {cmd}",
    );
    // The mapped video output is [titled_v] now.
    assert!(
        cmd.contains("-map [titled_v]"),
        "expected video map to [titled_v], got: {cmd}",
    );
    // Total duration accounts only for media segments — titles are
    // overlays, not contributors.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 5.0).abs() < 1e-6,
        "expected total duration 5.0s (title overlay doesn't extend timeline), got {dur}",
    );
}

#[test]
fn project_with_no_titles_uses_copy_path() {
    // Sanity: when there's no Titles track, the renderer can avoid a
    // filter graph and copy the single input directly.
    let dir = tempfile::tempdir().unwrap();
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join(asset_a), b"stub").unwrap();
    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut v1 = Track::empty("V1", TrackKind::Video);
    v1.children.push(TrackChild::Clip(clip_a));
    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(v1));
    tl.tracks = stack;
    fs::write(
        dir.path().join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("-map 0"),
        "expected input map when no titles, got: {cmd}",
    );
    assert!(
        cmd.contains("-c copy"),
        "expected copy path when no titles, got: {cmd}",
    );
    assert!(!cmd.contains("[titled_v]"));
}
