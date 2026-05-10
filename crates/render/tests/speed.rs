//! Integration test for Step 15.4: a project whose OTIO carries a
//! Clip with an `awidat.speed` Effect lands setpts/atempo filters
//! in the rendered argv and the spec's total_duration_s reflects
//! the rescaled value.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_speed_effect(dir: &Path, factor: f64) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    // 4s of source media.
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(4.0 * 24.0, 24.0),
    ));
    let mut speed_effect = Effect::new("awidat.speed");
    speed_effect
        .metadata
        .insert("factor".to_string(), serde_json::json!(factor));
    clip_a.effects.push(speed_effect);

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip_a));

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
fn project_with_2x_speed_emits_setpts_and_atempo() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_speed_effect(dir.path(), 2.0);
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("[0:v:0]setpts=0.5*PTS[sv0]"),
        "expected setpts in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("[0:a:0]atempo=2[sa0]"),
        "expected atempo in argv, got: {cmd}",
    );
    // 4s source @ 2× → 2s effective.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 2.0).abs() < 1e-6,
        "expected total duration 2.0s after 2× speed, got {dur}",
    );
}

#[test]
fn project_with_0_5x_speed_doubles_total_duration() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_speed_effect(dir.path(), 0.5);
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("setpts=2*PTS"),
        "expected setpts=2*PTS for 0.5× speed, got: {cmd}",
    );
    // 4s source @ 0.5× → 8s effective.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 8.0).abs() < 1e-6,
        "expected total duration 8.0s after 0.5× speed, got {dur}",
    );
}
