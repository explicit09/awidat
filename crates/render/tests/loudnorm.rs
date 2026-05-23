//! Integration test for the two-pass master loudnorm pipeline.
//!
//! Verifies that when a timeline carries a `master_loudnorm` setting,
//! the render orchestrator emits TWO ffmpeg invocations:
//!   - **Pass 1 (measure):** runs `loudnorm=...:print_format=json` against
//!     a null sink (`-f null -`) so the JSON measurement lands on stderr.
//!   - **Pass 2 (apply):** runs the normal render with `measured_I=...`,
//!     `measured_LRA=...`, `measured_TP=...`, `measured_thresh=...`,
//!     and `offset=...` substituted into the master loudnorm filter.
//!
//! The mocked `MeasuredLoudnorm` stands in for the JSON we'd pull off
//! ffmpeg's stderr — we do not run ffmpeg here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::files;
use awidat_render::{
    MeasuredLoudnorm, build_master_loudnorm_apply_spec, build_master_loudnorm_measure_spec,
    build_timeline_render_spec,
};
use std::fs;
use std::path::Path;

fn write_project_with_master_loudnorm(dir: &Path) {
    let asset = "raw/a.mp4";
    fs::create_dir_all(dir.join("raw")).unwrap();
    fs::write(dir.join(asset), b"stub").unwrap();

    let mut clip = Clip::empty("clip-a".to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));

    let mut tl = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;

    let mut meta = AwidatTimelineMetadata::default();
    meta.extra.insert(
        "master_loudnorm".into(),
        serde_json::json!({
            "target_lufs": -16.0,
            "target_lra": 11.0,
            "target_tp": -1.5,
        }),
    );
    tl.metadata.awidat = Some(meta);

    fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).unwrap(),
    )
    .unwrap();
}

#[test]
fn master_loudnorm_emits_two_pass_specs() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_master_loudnorm(dir.path());

    // ---- Pass 1: measure ----
    let measure = build_master_loudnorm_measure_spec(dir.path()).unwrap();
    assert_eq!(
        measure
            .metadata
            .get("master_loudnorm_pass")
            .map(String::as_str),
        Some("measure")
    );
    assert_eq!(
        measure
            .metadata
            .get("master_loudnorm_output_mode")
            .map(String::as_str),
        Some("null_muxer")
    );
    let measure_cmd = measure.args.join(" ");
    assert!(
        measure_cmd.contains("print_format=json"),
        "measure pass must request JSON output: {measure_cmd}"
    );
    assert!(
        measure_cmd.contains("loudnorm=I=-16:LRA=11:TP=-1.5"),
        "measure pass must include target loudnorm filter: {measure_cmd}"
    );
    // Null sink: `-f null -` (so no encode, only measurement).
    let mut found_null_sink = false;
    for (i, w) in measure.args.iter().enumerate() {
        if w == "-f" && measure.args.get(i + 1).map(String::as_str) == Some("null") {
            found_null_sink = true;
            break;
        }
    }
    assert!(
        found_null_sink,
        "measure pass must use null muxer for measurement, got: {measure_cmd}"
    );
    // Should not produce the final encoded output file.
    assert!(
        !measure_cmd.contains("libx264"),
        "measure pass must skip the video encoder: {measure_cmd}"
    );

    // ---- Pass 2: apply ----
    // Mock the JSON ffmpeg would have printed on stderr.
    let measured = MeasuredLoudnorm {
        measured_i: -19.7,
        measured_lra: 7.3,
        measured_tp: -3.1,
        measured_thresh: -29.9,
        target_offset: 0.4,
    };
    let apply = build_master_loudnorm_apply_spec(dir.path(), measured).unwrap();
    assert_eq!(
        apply
            .metadata
            .get("master_loudnorm_pass")
            .map(String::as_str),
        Some("apply")
    );
    assert_eq!(
        apply
            .metadata
            .get("master_loudnorm_output_mode")
            .map(String::as_str),
        Some("encoded_output")
    );
    let apply_cmd = apply.args.join(" ");
    assert!(
        apply_cmd.contains("measured_I=-19.7"),
        "apply pass must substitute measured_I: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("measured_LRA=7.3"),
        "apply pass must substitute measured_LRA: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("measured_TP=-3.1"),
        "apply pass must substitute measured_TP: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("measured_thresh=-29.9"),
        "apply pass must substitute measured_thresh: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("offset=0.4"),
        "apply pass must substitute target_offset: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("linear=true"),
        "apply pass must request linear normalization: {apply_cmd}"
    );
    assert!(
        apply_cmd.contains("print_format=summary"),
        "apply pass must request summary output: {apply_cmd}"
    );
    // Apply pass keeps the normal x264 encode chain.
    assert!(
        apply_cmd.contains("libx264"),
        "apply pass keeps the normal video encoder: {apply_cmd}"
    );
}

#[test]
fn normal_timeline_spec_marks_master_loudnorm_unapplied() {
    let dir = tempfile::tempdir().unwrap();
    write_project_with_master_loudnorm(dir.path());

    let spec = build_timeline_render_spec(dir.path()).unwrap();

    assert_eq!(
        spec.metadata
            .get("master_loudnorm_enabled")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        spec.metadata
            .get("master_loudnorm_pass")
            .map(String::as_str),
        Some("unapplied")
    );
    assert_eq!(
        spec.metadata
            .get("master_loudnorm_output_mode")
            .map(String::as_str),
        Some("timeline_single_pass")
    );
}

#[test]
fn parse_loudnorm_measure_json_extracts_values() {
    // Stderr from `loudnorm=...:print_format=json -f null -` ends with a
    // JSON block containing the measured values.
    let stderr = r#"
[Parsed_loudnorm_0 @ 0x600003a00000]
{
    "input_i" : "-19.74",
    "input_tp" : "-3.10",
    "input_lra" : "7.30",
    "input_thresh" : "-29.92",
    "output_i" : "-15.99",
    "output_tp" : "-1.50",
    "output_lra" : "5.20",
    "output_thresh" : "-26.10",
    "normalization_type" : "dynamic",
    "target_offset" : "0.40"
}
"#;
    let m = awidat_render::parse_loudnorm_measure_json(stderr).unwrap();
    assert!(
        (m.measured_i - -19.74).abs() < 1e-6,
        "measured_i={}",
        m.measured_i
    );
    assert!(
        (m.measured_lra - 7.30).abs() < 1e-6,
        "measured_lra={}",
        m.measured_lra
    );
    assert!(
        (m.measured_tp - -3.10).abs() < 1e-6,
        "measured_tp={}",
        m.measured_tp
    );
    assert!(
        (m.measured_thresh - -29.92).abs() < 1e-6,
        "measured_thresh={}",
        m.measured_thresh
    );
    assert!(
        (m.target_offset - 0.40).abs() < 1e-6,
        "target_offset={}",
        m.target_offset
    );
}
