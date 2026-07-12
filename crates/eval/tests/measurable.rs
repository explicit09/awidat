use montage_eval::{
    DetectorEvidence, HardGates, SidecarEvidence, SidecarPaths, Span, evaluate_measurable,
    parse_blackdetect, parse_freezedetect, parse_silencedetect,
};

const SILENCE_LOG: &str = r#"
[silencedetect @ 0x1] silence_start: 12
[silencedetect @ 0x1] silence_end: 14.4 | silence_duration: 2.4
"#;
const BLACK_LOG: &str = "[blackdetect @ 0x2] black_start:30 black_end:31.2 black_duration:1.2";
const FREEZE_LOG: &str = r#"
[freezedetect @ 0x3] freeze_start: 42
[freezedetect @ 0x3] freeze_duration: 3
[freezedetect @ 0x3] freeze_end: 45
"#;

#[test]
fn reads_existing_sidecars_without_rederiving_signals() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/measurable");
    let evidence = SidecarEvidence::load(&SidecarPaths {
        audio_energy: root.join("audio-energy.json"),
        frame_quality: root.join("frame-quality.json"),
        composition: root.join("composition.json"),
    })
    .unwrap_or_else(|error| panic!("sidecars should load: {error}"));

    assert!((evidence.audio.integrated_lufs - (-14.2)).abs() < f64::EPSILON);
    assert!((evidence.audio.true_peak_dbfs - (-1.4)).abs() < f64::EPSILON);
    assert!(evidence.composition.verification.passed);
    assert_eq!(evidence.composition.verification.checked_regions, 12);
    assert!(evidence.frame_quality.thumbnail_candidates[0].thumbnail_score > 0.8);
}

#[test]
fn missing_required_measurement_fails_closed() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/measurable");
    let audio = temp.path().join("audio.json");
    std::fs::write(&audio, br#"{"true_peak_dbfs": -1.4}"#)
        .unwrap_or_else(|error| panic!("fixture should write: {error}"));

    let result = SidecarEvidence::load(&SidecarPaths {
        audio_energy: audio,
        frame_quality: root.join("frame-quality.json"),
        composition: root.join("composition.json"),
    });
    let error = match result {
        Ok(_) => panic!("missing LUFS must fail closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("loudness_integrated_lufs"));
}

#[test]
fn parses_silence_black_and_freeze_windows() {
    assert_eq!(
        parse_silencedetect(SILENCE_LOG)
            .unwrap_or_else(|error| panic!("silence log should parse: {error}")),
        vec![Span::new(12.0, 14.4)]
    );
    assert_eq!(
        parse_blackdetect(BLACK_LOG)
            .unwrap_or_else(|error| panic!("black log should parse: {error}")),
        vec![Span::new(30.0, 31.2)]
    );
    assert_eq!(
        parse_freezedetect(FREEZE_LOG)
            .unwrap_or_else(|error| panic!("freeze log should parse: {error}")),
        vec![Span::new(42.0, 45.0)]
    );
}

#[test]
fn unmatched_detector_boundaries_fail_closed() {
    let error = match parse_silencedetect("silence_start: 12") {
        Ok(_) => panic!("unmatched start must not become partial evidence"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unmatched"));
}

#[test]
fn configured_detector_and_composition_failures_block_tier_two() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/measurable");
    let evidence = SidecarEvidence::load(&SidecarPaths {
        audio_energy: root.join("audio-energy.json"),
        frame_quality: root.join("frame-quality.json"),
        composition: root.join("composition.json"),
    })
    .unwrap_or_else(|error| panic!("sidecars should load: {error}"));
    let detectors = DetectorEvidence {
        silences: vec![Span::new(12.0, 14.4)],
        black_frames: vec![Span::new(30.0, 31.2)],
        freezes: Vec::new(),
    };
    let requirements = HardGates {
        playable: true,
        aspect_ratio: Some("16:9".into()),
        max_remaining_silence_seconds: Some(1.0),
        min_speech_retention: None,
        max_caption_wer: None,
        no_black_frames: true,
        no_freeze_frames: true,
        no_invalid_timeline_overlaps: true,
        no_mid_word_cuts: true,
    };

    let report = evaluate_measurable(&requirements, &evidence, &detectors);
    assert!(!report.passed);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "SILENCE_TOO_LONG" && !check.passed)
    );
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.code == "BLACK_FRAMES" && !check.passed)
    );
}
