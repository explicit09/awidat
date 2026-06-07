//! Integration test for Step 15.4: a project whose OTIO carries a
//! Clip with an `montage.speed` Effect lands setpts/atempo filters
//! in the rendered argv and the spec's total_duration_s reflects
//! the rescaled value.

#![allow(clippy::unwrap_used)]

use montage_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use montage_proto::project::files;
use montage_render::build_timeline_render_spec;
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

fn write_project_with_speed_effect(dir: &Path, factor: f64) {
    let mut metadata = Map::new();
    metadata.insert("factor".to_string(), serde_json::json!(factor));
    write_project_with_speed_metadata(dir, metadata);
}

fn write_project_with_speed_metadata(dir: &Path, metadata: Map<String, Value>) {
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
    let mut speed_effect = Effect::new("montage.speed");
    speed_effect.metadata = metadata;
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

#[test]
fn project_with_reverse_speed_emits_reverse_filters() {
    let dir = tempfile::tempdir().unwrap();
    let mut metadata = Map::new();
    metadata.insert("factor".into(), serde_json::json!(2.0));
    metadata.insert("reverse".into(), serde_json::json!(true));
    write_project_with_speed_metadata(dir.path(), metadata);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("[0:v:0]reverse,setpts=0.5*PTS[sv0]"),
        "expected reverse before setpts in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("[0:a:0]areverse,atempo=2[sa0]"),
        "expected areverse before atempo in argv, got: {cmd}",
    );
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 2.0).abs() < 1e-6,
        "expected total duration 2.0s after reversed 2x speed, got {dur}",
    );
}

#[test]
fn project_with_speed_can_disable_pitch_preservation() {
    let dir = tempfile::tempdir().unwrap();
    let mut metadata = Map::new();
    metadata.insert("factor".into(), serde_json::json!(1.5));
    metadata.insert("maintain_pitch".into(), serde_json::json!(false));
    write_project_with_speed_metadata(dir.path(), metadata);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("[0:a:0]asetrate=sample_rate*1.5,aresample=sample_rate[sa0]"),
        "expected sample-rate retime when maintain_pitch=false, got: {cmd}",
    );
    assert!(
        !cmd.contains("[0:a:0]atempo=1.5[sa0]"),
        "maintain_pitch=false should not use atempo: {cmd}",
    );
}

#[test]
fn project_with_blend_frame_blending_emits_interpolation_filters() {
    let dir = tempfile::tempdir().unwrap();
    let mut metadata = Map::new();
    metadata.insert("factor".into(), serde_json::json!(0.5));
    metadata.insert("frame_blending".into(), serde_json::json!("blend"));
    write_project_with_speed_metadata(dir.path(), metadata);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("setpts=2*PTS,tblend=all_mode=average,framerate=fps=30000/1001[sv0]"),
        "expected blend interpolation after setpts, got: {cmd}",
    );
}

#[test]
fn project_with_flow_frame_blending_emits_minterpolate() {
    let dir = tempfile::tempdir().unwrap();
    let mut metadata = Map::new();
    metadata.insert("factor".into(), serde_json::json!(0.5));
    metadata.insert("frame_blending".into(), serde_json::json!("flow"));
    write_project_with_speed_metadata(dir.path(), metadata);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains(
            "setpts=2*PTS,minterpolate=mi_mode=mci:mc_mode=aobmc:vsbmc=1:fps=30000/1001[sv0]"
        ),
        "expected flow interpolation after setpts, got: {cmd}",
    );
}

#[test]
fn project_with_default_frame_blending_emits_nearest_only() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_speed_effect(dir.path(), 0.5);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("[0:v:0]setpts=2*PTS[sv0]"),
        "expected nearest setpts-only retime, got: {cmd}",
    );
    assert!(
        !cmd.contains("tblend="),
        "nearest mode should not blend: {cmd}"
    );
    assert!(
        !cmd.contains("minterpolate="),
        "nearest mode should not use optical flow: {cmd}",
    );
}
