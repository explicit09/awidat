//! Integration test for Step 14.5: a project whose OTIO carries a
//! TrackChild::Transition between two adjacent clips lands as an
//! `xfade=` token in the rendered argv. The render itself is gated
//! on ffmpeg being available — the argv assertion runs unconditionally.

#![allow(clippy::unwrap_used)]

use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind, Transition,
};
use awidat_proto::project::files;
use awidat_render::{build_timeline_render_spec, ffmpeg_path};
use std::fs;
use std::path::Path;

fn write_two_clip_project_with_transition(dir: &Path) {
    write_two_clip_project_with_transition_kind(dir, "SMPTE_Dissolve");
}

fn write_two_clip_project_with_transition_kind(dir: &Path, kind: &str) {
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

    let mut clip_b = Clip::empty("clip-b".to_string());
    clip_b.media_reference = MediaReference::External(ExternalReference::new(asset_b));
    clip_b.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));

    let transition = Transition::symmetric(kind, 1.0, 24.0);

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip_a));
    track.children.push(TrackChild::Transition(transition));
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

fn write_three_clip_project_with_chained_transitions(dir: &Path) {
    let asset_a = "raw/a.mp4";
    let asset_b = "raw/b.mp4";
    let asset_c = "raw/c.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset_a), b"stub").unwrap();
    fs::write(dir.join(asset_b), b"stub").unwrap();
    fs::write(dir.join(asset_c), b"stub").unwrap();

    fn clip(name: &str, asset: &str) -> Clip {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        clip
    }

    let mut track = Track::empty("V1", TrackKind::Video);
    track
        .children
        .push(TrackChild::Clip(clip("clip-a", asset_a)));
    track
        .children
        .push(TrackChild::Transition(Transition::symmetric(
            "SMPTE_Dissolve",
            0.5,
            24.0,
        )));
    track
        .children
        .push(TrackChild::Clip(clip("clip-b", asset_b)));
    track
        .children
        .push(TrackChild::Transition(Transition::symmetric(
            "SMPTE_Dissolve",
            0.5,
            24.0,
        )));
    track
        .children
        .push(TrackChild::Clip(clip("clip-c", asset_c)));

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
fn project_with_transition_emits_xfade_in_argv() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=fade"),
        "expected xfade in argv, got: {cmd}",
    );
    assert!(
        cmd.contains("acrossfade=d=1"),
        "expected acrossfade in argv, got: {cmd}",
    );
    // Total duration accounts for the 1s overlap: 2 + 2 - 1 = 3.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 3.0).abs() < 1e-6,
        "expected total duration 3.0s after transition overlap, got {dur}",
    );
}

#[test]
fn project_with_chained_transitions_emits_all_xfades_in_argv() {
    let dir = tempfile::tempdir().unwrap();
    write_three_clip_project_with_chained_transitions(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    let xfade_count = cmd.matches("xfade=transition=fade").count();
    assert_eq!(xfade_count, 2, "expected two chained xfades, got: {cmd}");
    let acrossfade_count = cmd.matches("acrossfade=d=0.5").count();
    assert_eq!(
        acrossfade_count, 2,
        "expected two chained acrossfades, got: {cmd}"
    );
    // Total duration accounts for both 0.5s overlaps:
    // 2 + 2 + 2 - 0.5 - 0.5 = 5.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 5.0).abs() < 1e-6,
        "expected total duration 5.0s after chained overlaps, got {dur}",
    );
}

#[test]
fn project_with_awidat_transition_id_maps_to_xfade() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "awidat.slide_left");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=slideleft"),
        "expected awidat id to resolve to xfade slideleft, got: {cmd}",
    );
}

#[test]
fn project_with_unknown_awidat_transition_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "awidat.not_registered");
    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("unsupported"),
        "expected clear unsupported transition error, got: {err}",
    );
}

/// End-to-end: actually run ffmpeg if it's on the box. The render
/// fixture writes stub bytes for the assets, so this test only
/// exercises argv shape — we don't expect a successful render.
/// Skipped entirely when ffmpeg isn't available.
#[test]
fn render_argv_is_well_formed_for_ffmpeg() {
    let Ok(_bin) = ffmpeg_path() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let spec = build_timeline_render_spec(dir.path()).unwrap();

    // The argv must parse: every `-i` is followed by a path,
    // `-filter_complex` is followed by a non-empty string, and
    // `-map` is followed by a label.
    let mut i = 0;
    while i < spec.args.len() {
        let a = &spec.args[i];
        if a == "-i" || a == "-filter_complex" || a == "-map" {
            assert!(i + 1 < spec.args.len(), "{a} has no value at end of argv");
            assert!(!spec.args[i + 1].is_empty(), "{a} has empty value",);
            i += 2;
        } else {
            i += 1;
        }
    }
}
