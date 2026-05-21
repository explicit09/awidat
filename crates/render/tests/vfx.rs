//! Integration tests for focused VFX filter lowering.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, Effect, ExternalReference, Gap, MediaReference, RationalTime, Stack, StackChild,
    TimeRange, Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::build_timeline_render_spec;
use std::fs;

fn media_clip(name: &str, asset: &str, duration_s: f64) -> Clip {
    let mut clip = Clip::empty(name.to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(duration_s * 24.0, 24.0),
    ));
    clip
}

#[test]
fn project_with_region_blur_emits_crop_blur_overlay() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join("raw/a.mp4"), b"stub").unwrap();

    let mut clip = media_clip("clip-a", "raw/a.mp4", 4.0);
    let mut effect = Effect::new("awidat.region_blur");
    effect
        .metadata
        .insert("shape".to_string(), serde_json::json!("ellipse"));
    effect
        .metadata
        .insert("x".to_string(), serde_json::json!(0.1));
    effect
        .metadata
        .insert("y".to_string(), serde_json::json!(0.2));
    effect
        .metadata
        .insert("width".to_string(), serde_json::json!(0.3));
    effect
        .metadata
        .insert("height".to_string(), serde_json::json!(0.4));
    effect
        .metadata
        .insert("radius_px".to_string(), serde_json::json!(12.0));
    clip.effects.push(effect);

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));
    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;
    fs::write(
        dir.path().join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains("crop=w=iw*0.3:h=ih*0.4:x=iw*0.1:y=ih*0.2,boxblur=12"),
        "expected cropped region blur, got: {cmd}",
    );
    assert!(
        cmd.contains("geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)'"),
        "expected ellipse alpha mask, got: {cmd}",
    );
    assert!(
        cmd.contains("[rb_base0][rb_crop0]overlay=x=main_w*0.1:y=main_h*0.2[rbv0]"),
        "expected blurred crop overlay, got: {cmd}",
    );
}

#[test]
fn full_frame_overlay_with_blend_mode_uses_blend_filter() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join("raw/a.mp4"), b"stub").unwrap();
    fs::write(dir.path().join("raw/b.mp4"), b"stub").unwrap();

    let base = media_clip("base", "raw/a.mp4", 4.0);
    let mut overlay = media_clip("overlay", "raw/b.mp4", 2.0);
    let mut overlay_effect = Effect::new("awidat.video_overlay");
    overlay_effect
        .metadata
        .insert("blend_mode".to_string(), serde_json::json!("screen"));
    overlay.effects.push(overlay_effect);

    let mut base_track = Track::empty("V1", TrackKind::Video);
    base_track.children.push(TrackChild::Clip(base));
    let mut overlay_track = Track::empty("V2", TrackKind::Video);
    overlay_track
        .children
        .push(TrackChild::Gap(Gap::of_duration(1.0, 24.0)));
    overlay_track.children.push(TrackChild::Clip(overlay));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(base_track));
    stack.children.push(StackChild::Track(overlay_track));
    tl.tracks = stack;
    fs::write(
        dir.path().join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");

    assert!(
        cmd.contains(
            "blend=all_mode=screen:all_opacity=1:enable='between(t\\,1\\,3)'[media_overlay_v0]"
        ),
        "expected screen blend mode for full-frame overlay, got: {cmd}",
    );
}
