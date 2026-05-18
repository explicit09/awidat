//! Integration tests for graph-native color effects: OTIO clip effects
//! must flow into the timeline render argv as FFmpeg video filters.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_color_effect(dir: &Path, include_lut: bool) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    if include_lut {
        fs::create_dir_all(dir.join("luts")).unwrap();
        fs::write(dir.join("luts/show-look.cube"), b"# stub").unwrap();
    }

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(4.0 * 24.0, 24.0),
    ));

    let mut color = Effect::new("awidat.color_correction");
    color
        .metadata
        .insert("exposure_ev".to_string(), serde_json::json!(0.5));
    color
        .metadata
        .insert("contrast".to_string(), serde_json::json!(1.2));
    color
        .metadata
        .insert("saturation".to_string(), serde_json::json!(0.9));
    color
        .metadata
        .insert("temperature".to_string(), serde_json::json!(0.3));
    clip_a.effects.push(color);

    if include_lut {
        let mut lut = Effect::new("awidat.lut");
        lut.metadata.insert(
            "lut_path".to_string(),
            serde_json::json!("luts/show-look.cube"),
        );
        lut.metadata.insert(
            "interpolation".to_string(),
            serde_json::json!("tetrahedral"),
        );
        clip_a.effects.push(lut);
    }

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

fn write_project_with_source_color_effect(dir: &Path) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut source_color = Effect::new("awidat.source_color");
    source_color
        .metadata
        .insert("transfer".to_string(), serde_json::json!("smpte2084"));
    clip_a.effects.push(source_color);

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
fn project_with_color_correction_emits_color_filters() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_color_effect(dir.path(), false);
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("[0:v:0]eq=brightness=0.07:contrast=1.2:saturation=0.9,colorbalance="),
        "expected color filters in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("[cv0][0:a:0]concat"),
        "expected concat to consume corrected video label, got: {cmd}",
    );
}

#[test]
fn project_with_source_color_emits_hdr_tonemap() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_source_color_effect(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("tonemap=tonemap=hable:desat=0"),
        "expected HDR tone map in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("zscale=transfer=bt709:matrix=bt709:primaries=bt709"),
        "expected SDR output transfer in argv, got: {cmd}",
    );
}

#[test]
fn project_with_lut_emits_lut3d() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_color_effect(dir.path(), true);
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("lut3d=file='"),
        "expected lut3d in argv, got: {cmd}"
    );
    assert!(
        cmd.contains(":interp=tetrahedral"),
        "expected LUT interpolation in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("luts/show-look.cube"),
        "expected LUT path in argv, got: {cmd}",
    );
}
