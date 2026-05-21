//! Integration test for the speed-ramp transition execution path.
//!
//! A 2-clip timeline with the `awidat.ramp_in_beat` transition carries
//! a `TimeRemap` primitive in its composition recipe. The render-prepare
//! step must emit a real `setpts` retime filter for the transition
//! window — not just the paired-flash xfade fallback. The TimeRemap is
//! what makes the transition feel like a speed ramp; without it the
//! exported clip plays at realtime even though the recipe claimed a
//! speed change.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind, Transition,
};
use awidat_proto::project::files;
use awidat_proto::transitions::builtin_transition_composition;
use awidat_render::build_timeline_render_spec;
use std::fs;
use std::path::Path;

fn external_ref_with_available(asset: &str, duration_s: f64) -> ExternalReference {
    let mut ext = ExternalReference::new(asset);
    ext.available_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(duration_s * 24.0, 24.0),
    ));
    ext
}

fn write_two_clip_project_with_speed_ramp(dir: &Path, kind: &str) {
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

    // Attach the registered speed-ramp composition to the OTIO metadata
    // so the renderer can lower the TimeRemap primitive without having
    // to auto-derive from `kind`. This matches the convention used by
    // other composition-aware tests in `crates/render/tests/transition.rs`.
    let composition = builtin_transition_composition(kind)
        .unwrap_or_else(|| panic!("registered composition for {kind}"));
    let mut transition = Transition::symmetric(kind, 0.5, 24.0);
    transition.metadata.insert(
        "awidat_transition".into(),
        serde_json::json!({
            "id": kind,
            "family": "speed_ramp",
            "composition": composition,
        }),
    );

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

fn filter_complex(spec_args: &[String]) -> String {
    spec_args
        .windows(2)
        .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
        .expect("argv contains -filter_complex")
}

#[test]
fn ramp_in_beat_emits_setpts_retime_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_speed_ramp(dir.path(), "awidat.ramp_in_beat");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let filter = filter_complex(&spec.args);

    // Paired-flash xfade fallback still emitted for the visual layer.
    assert!(
        filter.contains("xfade=transition=fadewhite"),
        "expected fadewhite xfade fallback in filter graph, got: {filter}",
    );

    // The TimeRemap primitive must lower to a real setpts retime
    // expression. Per-clip `setpts=PTS-STARTPTS` resets do not count —
    // we need a speed-curve-driven retime in the transition window.
    let has_retime_expr = filter
        .split(';')
        .any(|stage| stage.contains("setpts=") && !stage.contains("setpts=PTS-STARTPTS"));
    assert!(
        has_retime_expr,
        "expected setpts retime expression (not just PTS-STARTPTS reset) for speed-ramp, got: {filter}",
    );

    // The retimed labels must feed into the xfade so the speed curve
    // actually affects the rendered output, not the un-retimed staged
    // labels. Find the retimed output labels and confirm xfade reads
    // from them.
    let retime_output_labels: Vec<String> = filter
        .split(';')
        .filter(|s| s.contains("setpts=") && !s.contains("setpts=PTS-STARTPTS"))
        .filter_map(|stage| {
            // Stage shape: `[in]setpts='...'[out]`. Take the trailing
            // [..] as the output label.
            stage.rfind('[').map(|idx| stage[idx..].to_string())
        })
        .collect();
    assert!(
        !retime_output_labels.is_empty(),
        "expected at least one retime stage output label, got: {filter}",
    );
    let xfade_stage = filter
        .split(';')
        .find(|s| s.contains("xfade=transition="))
        .expect("xfade stage present");
    let consumes_retime = retime_output_labels
        .iter()
        .any(|label| xfade_stage.contains(label));
    assert!(
        consumes_retime,
        "expected xfade to read from a setpts retime label {retime_output_labels:?}, got: {xfade_stage}",
    );
}

#[test]
fn ramp_out_chapter_emits_setpts_retime_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_two_clip_project_with_speed_ramp(dir.path(), "awidat.ramp_out_chapter");
    let spec = build_timeline_render_spec(dir.path()).unwrap();
    let filter = filter_complex(&spec.args);

    let has_retime_expr = filter.contains("setpts='")
        || filter.contains("setpts=expr=")
        || filter
            .split(';')
            .any(|stage| stage.contains("setpts=") && !stage.contains("setpts=PTS-STARTPTS"));
    assert!(
        has_retime_expr,
        "expected setpts retime expression for ramp_out_chapter, got: {filter}",
    );
}
