//! Integration test for Step 15.3: a project whose OTIO carries a
//! Clip with an `awidat.volume` Effect lands a `volume=` filter in
//! the rendered argv.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_volume_effect(dir: &Path) {
    let asset_a = "raw/a.mp4";
    let asset_b = "raw/b.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();
    fs::write(dir.join(asset_b), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    // Stamp the awidat.volume effect on clip-a.
    let mut volume_effect = Effect::new("awidat.volume");
    volume_effect
        .metadata
        .insert("value".to_string(), serde_json::json!(0.5));
    clip_a.effects.push(volume_effect);

    let mut clip_b = Clip::empty("clip-b".to_string());
    clip_b.media_reference = MediaReference::External(ExternalReference::new(asset_b));
    clip_b.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip_a));
    track.children.push(TrackChild::Clip(clip_b));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;

    fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();
}

#[test]
fn project_with_volume_effect_emits_volume_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_volume_effect(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("[0:a:0]volume=0.5[av0]"),
        "expected volume filter in argv, got: {cmd}",
    );
    // Concat for seg 0 must read the smoothed volume-filtered audio label.
    assert!(
        cmd.contains("[av0]afade=t=out:st=1.97:d=0.03[bfade0]"),
        "expected hard-cut smoothing after [av0] for seg 0: {cmd}",
    );
    assert!(
        cmd.contains("[cf0][bfade0]"),
        "expected concat to consume the conformed video + [bfade0] for seg 0: {cmd}",
    );
    // Total duration is unaffected by volume (15.4 wires speed which
    // does affect duration).
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 4.0).abs() < 1e-6,
        "expected total duration 4.0s (no speed effects), got {dur}",
    );
}
