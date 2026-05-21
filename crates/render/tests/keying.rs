//! Integration tests for graph-native chroma/luma key effects.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn write_project_with_effect(dir: &Path, effect: Effect) {
    let asset_a = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();

    let mut clip_a = Clip::empty("clip-a".to_string());
    clip_a.media_reference = MediaReference::External(ExternalReference::new(asset_a));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(4.0 * 24.0, 24.0),
    ));
    clip_a.effects.push(effect);

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
fn project_with_chroma_key_emits_chromakey_filter() {
    let dir = tempfile::tempdir().unwrap();
    let mut effect = Effect::new("awidat.chroma_key");
    effect
        .metadata
        .insert("key_color".to_string(), serde_json::json!("0x00ff00"));
    effect
        .metadata
        .insert("similarity".to_string(), serde_json::json!(0.18));
    effect
        .metadata
        .insert("blend".to_string(), serde_json::json!(0.04));
    write_project_with_effect(dir.path(), effect);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("chromakey=0x00ff00:0.18:0.04[ckv0]"),
        "expected chromakey filter, got: {cmd}",
    );
}

#[test]
fn project_with_luma_key_emits_alpha_filter() {
    let dir = tempfile::tempdir().unwrap();
    let mut effect = Effect::new("awidat.luma_key");
    effect
        .metadata
        .insert("threshold".to_string(), serde_json::json!(0.45));
    effect
        .metadata
        .insert("softness".to_string(), serde_json::json!(0.12));
    write_project_with_effect(dir.path(), effect);

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("format=rgba,lumakey=threshold=0.45:softness=0.12[lkv0]"),
        "expected alpha-bearing luma key filter, got: {cmd}",
    );
}
