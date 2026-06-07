//! Integration tests for clip-level freeze-frame lowering.

#![allow(clippy::unwrap_used)]

use montage_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use montage_proto::project::files;
use montage_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_freeze_effect(dir: &Path) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(4.0 * 24.0, 24.0),
    ));

    let mut freeze = Effect::new("montage.freeze");
    freeze
        .metadata
        .insert("freeze_at_source_s".to_string(), serde_json::json!(1.2));
    freeze
        .metadata
        .insert("duration_s".to_string(), serde_json::json!(0.8));
    freeze
        .metadata
        .insert("freeze_position".to_string(), serde_json::json!("at"));
    freeze
        .metadata
        .insert("audio_behavior".to_string(), serde_json::json!("silence"));
    clip_a.effects.push(freeze);

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
fn project_with_freeze_emits_hold_frame_and_silence_concat() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_freeze_effect(dir.path());

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains(
            "trim=1.2:1.241667,setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration=0.8"
        ),
        "expected held video frame via tpad, got: {cmd}",
    );
    assert!(
        cmd.contains("anullsrc=channel_layout=stereo:sample_rate=48000,atrim=0:0.8"),
        "expected generated silent audio for freeze duration, got: {cmd}",
    );
    assert!(
        cmd.contains("concat=n=3:v=1:a=0[fv0]"),
        "expected video freeze splice concat, got: {cmd}",
    );
    assert!(
        cmd.contains("concat=n=3:v=0:a=1[fa0]"),
        "expected audio freeze splice concat, got: {cmd}",
    );

    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 4.8).abs() < 1e-6,
        "expected total duration 4.8s after inserted freeze, got {dur}",
    );
}
