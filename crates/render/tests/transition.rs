//! Integration test for Step 14.5: a project whose OTIO carries a
//! TrackChild::Transition between two adjacent clips lands as an
//! `xfade=` token in the rendered argv. The render itself is gated
//! on ffmpeg being available — the argv assertion runs unconditionally.

#![allow(clippy::unwrap_used)]

use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind, Transition,
};
use montage_proto::project::files;
use montage_proto::transitions::{
    TransitionComposition, TransitionEasing, TransitionPrimitive, TransitionPrimitiveOp,
};
use montage_render::{build_timeline_render_spec, collect_timeline_plan, ffmpeg_path};
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
    clip_a.media_reference = MediaReference::External(external_ref_with_available(asset_a, 4.0));
    clip_a.source_range = Some(TimeRange::new(
        RationalTime::new(1.0 * 24.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));

    let mut clip_b = Clip::empty("clip-b".to_string());
    clip_b.media_reference = MediaReference::External(external_ref_with_available(asset_b, 4.0));
    clip_b.source_range = Some(TimeRange::new(
        RationalTime::new(1.0 * 24.0, 24.0),
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

fn external_ref_with_available(asset: &str, duration_s: f64) -> ExternalReference {
    let mut ext = ExternalReference::new(asset);
    ext.available_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(duration_s * 24.0, 24.0),
    ));
    ext
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
        clip.media_reference = MediaReference::External(external_ref_with_available(asset, 4.0));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(1.0 * 24.0, 24.0),
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
    assert!(
        cmd.contains("-ss 1 -t 2.5") && cmd.contains("-ss 0.5 -t 2.5"),
        "expected centered transition handles in argv, got: {cmd}",
    );
    // Centered transitions use source handles and preserve timeline duration.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 4.0).abs() < 1e-6,
        "expected total duration 4.0s with centered transition handles, got {dur}",
    );
}

#[test]
fn project_with_asymmetric_transition_uses_otio_offset_handles() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Transition(t) = &mut track.children[1] else {
        panic!("expected transition");
    };
    // OTIO semantics: in_offset consumes incoming pre-roll before the cut;
    // out_offset consumes outgoing post-roll after the cut.
    t.in_offset = RationalTime::new(0.25 * 24.0, 24.0);
    t.out_offset = RationalTime::new(0.75 * 24.0, 24.0);
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("-ss 1 -t 2.75") && cmd.contains("-ss 0.75 -t 2.25"),
        "expected outgoing post-handle and incoming pre-handle in argv, got: {cmd}",
    );
}

#[test]
fn project_with_transition_fails_when_incoming_handle_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Clip(clip_b) = &mut track.children[2] else {
        panic!("expected clip-b");
    };
    clip_b.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("incoming handle"),
        "expected clear missing-handle error, got: {err}",
    );
}

#[test]
fn project_with_duplicate_transition_nodes_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    track.children.insert(
        2,
        TrackChild::Transition(Transition::symmetric("SMPTE_Dissolve", 0.5, 24.0)),
    );
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("invalid transition placement"),
        "expected placement error, got: {err}",
    );
}

#[test]
fn project_with_trailing_transition_node_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition(dir.path());
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    track
        .children
        .push(TrackChild::Transition(Transition::symmetric(
            "SMPTE_Dissolve",
            0.5,
            24.0,
        )));
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("no incoming clip"),
        "expected trailing transition error, got: {err}",
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
    // Centered transitions use source handles and preserve timeline duration.
    let dur = spec.total_duration_s.unwrap();
    assert!(
        (dur - 6.0).abs() < 1e-6,
        "expected total duration 6.0s with centered transition handles, got {dur}",
    );
}

#[test]
fn project_with_montage_transition_id_maps_to_xfade() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.slide_left");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=slideleft"),
        "expected montage id to resolve to xfade slideleft, got: {cmd}",
    );
}

#[test]
fn project_with_transition_composition_carries_recipe_into_render_plan() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.slide_left");
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Transition(t) = &mut track.children[1] else {
        panic!("expected transition");
    };
    let composition = TransitionComposition {
        version: 1,
        primitives: vec![
            TransitionPrimitive {
                start: 0.0,
                end: 1.0,
                easing: TransitionEasing::EaseOutExpo,
                op: TransitionPrimitiveOp::Push {
                    direction: "left".into(),
                    distance: 0.9,
                },
            },
            TransitionPrimitive {
                start: 0.1,
                end: 0.7,
                easing: TransitionEasing::EaseOut,
                op: TransitionPrimitiveOp::Blur {
                    amount: montage_proto::transitions::ParamCurve::Const(0.65),
                    direction: Some("left".into()),
                },
            },
        ],
    };
    t.metadata.insert(
        "montage_transition".into(),
        serde_json::json!({
            "id": "montage.slide_left",
            "family": "slide",
            "intent": "beat_hit_motion_cover",
            "energy": 0.85,
            "direction": "left",
            "composition": composition,
        }),
    );
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let (_, transitions) = collect_timeline_plan(dir.path()).unwrap();
    let planned = transitions[0].composition.as_ref().unwrap();
    assert_eq!(planned.primitives.len(), 2);
    assert!(matches!(
        &planned.primitives[0].op,
        TransitionPrimitiveOp::Push {
            direction,
            distance
        } if direction == "left" && (*distance - 0.9).abs() < 1e-9
    ));

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=slideleft"),
        "phase-one export should still use stable xfade fallback, got: {cmd}",
    );
}

#[test]
fn project_with_agent_composite_lowers_recipe_to_xfade() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.composite");
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Transition(t) = &mut track.children[1] else {
        panic!("expected transition");
    };
    t.metadata.insert(
        "montage_transition".into(),
        serde_json::json!({
            "id": "montage.composite",
            "family": "custom",
            "intent": "beat_hit_motion_cover",
            "energy": 0.85,
            "direction": "left",
            "composition": {
                "version": 1,
                "primitives": [
                    {"op": "blur", "amount": 0.65, "direction": "left", "start": 0.1, "end": 0.7},
                    {"op": "push", "direction": "left", "distance": 0.9, "start": 0.0, "end": 1.0, "easing": "ease_out_expo"},
                    {"op": "flash", "color": "#ffffff", "peak": 0.25, "start": 0.35, "end": 0.55}
                ]
            }
        }),
    );
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let (_, transitions) = collect_timeline_plan(dir.path()).unwrap();
    assert_eq!(transitions[0].kind, "montage.composite");
    assert!(transitions[0].composition.is_some());

    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=slideleft"),
        "expected push primitive to lower to slideleft xfade, got: {cmd}",
    );
}

#[test]
fn project_with_composite_missing_recipe_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.composite");
    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("requires metadata.montage_transition.composition"),
        "expected missing composite recipe error, got: {err}",
    );
}

#[test]
fn project_with_unlowerable_composite_recipe_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.composite");
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Transition(t) = &mut track.children[1] else {
        panic!("expected transition");
    };
    t.metadata.insert(
        "montage_transition".into(),
        serde_json::json!({
            "id": "montage.composite",
            "family": "custom",
            "composition": {
                "version": 1,
                "primitives": [
                    {"op": "shake", "amount": 0.5, "decay": 0.7}
                ]
            }
        }),
    );
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("no phase-one FFmpeg lowering"),
        "expected unlowerable composite recipe error, got: {err}",
    );
}

#[test]
fn project_with_invalid_transition_composition_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.slide_left");
    let otio_path = dir.path().join(files::OTIO);
    let mut tl: Timeline = serde_json::from_str(&fs::read_to_string(&otio_path).unwrap()).unwrap();
    let StackChild::Track(track) = &mut tl.tracks.children[0] else {
        panic!("expected track");
    };
    let TrackChild::Transition(t) = &mut track.children[1] else {
        panic!("expected transition");
    };
    t.metadata.insert(
        "montage_transition".into(),
        serde_json::json!({
            "id": "montage.slide_left",
            "composition": {
                "version": 1,
                "primitives": [
                    {"op": "push", "direction": "diagonal", "distance": 0.9}
                ]
            }
        }),
    );
    fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();

    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("invalid transition metadata")
            && err.to_string().contains("direction"),
        "expected invalid composition error, got: {err}",
    );
}

#[test]
fn project_with_imported_cross_dissolve_downgrades_to_supported_xfade() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "Cross Dissolve");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=fade"),
        "expected imported dissolve to downgrade to xfade fade, got: {cmd}",
    );
    assert!(
        cmd.contains("acrossfade=d=1"),
        "expected imported dissolve to keep crossfade audio, got: {cmd}",
    );
}

#[test]
fn project_with_imported_dip_to_black_downgrades_to_fade_black() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "Dip To Black");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let cmd = spec.args.join(" ");
    assert!(
        cmd.contains("xfade=transition=fadeblack"),
        "expected imported black dip to downgrade to fadeblack, got: {cmd}",
    );
}

#[test]
fn project_with_unknown_montage_transition_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "montage.not_registered");
    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("unsupported"),
        "expected clear unsupported transition error, got: {err}",
    );
}

#[test]
fn project_with_unknown_imported_transition_fails_before_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_transition_kind(dir.path(), "ThirdPartyFancySpin");
    let err = build_timeline_render_spec(dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported transition kind \"ThirdPartyFancySpin\""),
        "expected clear unsupported imported-transition error, got: {err}",
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
