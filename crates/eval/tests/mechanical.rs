use std::collections::BTreeMap;

use montage_eval::{HardGates, inspect_ffprobe, inspect_manifest, inspect_otio};
use montage_render::{
    RenderBackendKind, RenderExecutionManifest, RenderExecutionManifestInput, RenderReplayPlan,
    output_artifact, write_render_manifest,
};

fn gates() -> HardGates {
    HardGates {
        playable: true,
        aspect_ratio: Some("16:9".into()),
        max_remaining_silence_seconds: None,
        min_speech_retention: None,
        max_caption_wer: None,
        no_black_frames: true,
        no_freeze_frames: true,
        no_invalid_timeline_overlaps: true,
        no_mid_word_cuts: true,
    }
}

#[test]
fn requires_playable_audio_and_video_streams() {
    let valid = inspect_ffprobe(
        include_str!("../fixtures/mechanical/ffprobe-valid.json"),
        &gates(),
    )
    .unwrap_or_else(|error| panic!("valid fixture should parse: {error}"));
    assert!(valid.passed);

    let missing = inspect_ffprobe(
        include_str!("../fixtures/mechanical/ffprobe-missing-audio.json"),
        &gates(),
    )
    .unwrap_or_else(|error| panic!("valid fixture should parse: {error}"));
    assert!(!missing.passed);
    assert!(
        missing
            .checks
            .iter()
            .any(|check| check.code == "AUDIO_STREAM_MISSING" && !check.passed)
    );
}

#[test]
fn rejects_an_output_with_the_wrong_aspect_ratio() {
    let input = include_str!("../fixtures/mechanical/ffprobe-valid.json").replace("1920", "1080");
    let report = inspect_ffprobe(input, &gates())
        .unwrap_or_else(|error| panic!("fixture should parse: {error}"));

    assert!(!report.passed);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "ASPECT_RATIO" && !check.passed)
    );
}

#[test]
fn rejects_manifest_with_missing_declared_output() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let output_path = temp.path().join("missing.mp4");
    let manifest_path = temp.path().join("render.manifest.json");
    let manifest = RenderExecutionManifest::planned(RenderExecutionManifestInput {
        created_at: "2026-07-10T00:00:00Z".into(),
        montage_version: "0.1.0".into(),
        project_root: temp.path().to_string_lossy().into_owned(),
        project_hash: None,
        timeline_hash: None,
        backend: RenderBackendKind::TimelineFfmpegReencode,
        replay: RenderReplayPlan::FfmpegArgv {
            argv: vec!["ffmpeg".into()],
            cwd: None,
        },
        inputs: Vec::new(),
        outputs: vec![output_artifact(&output_path, true)],
        sidecars: Vec::new(),
        limitations: Vec::new(),
        verification: None,
        metadata: BTreeMap::new(),
    });
    write_render_manifest(&manifest_path, &manifest)
        .unwrap_or_else(|error| panic!("manifest should write: {error}"));

    let report = inspect_manifest(&manifest_path, &output_path)
        .unwrap_or_else(|error| panic!("manifest should parse: {error}"));
    assert!(!report.passed);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "MANIFEST_OUTPUT_MISSING" && !check.passed)
    );
}

#[test]
fn malformed_otio_is_a_blocking_mechanical_defect() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let path = temp.path().join("output.otio");
    std::fs::write(&path, b"not json")
        .unwrap_or_else(|error| panic!("fixture should write: {error}"));

    let report = inspect_otio(&path);
    assert!(!report.passed);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "OTIO_INVALID" && !check.passed)
    );
}
