use std::fs;

use serde_json::json;

use super::artifacts::{
    ArtifactBundleInput, build_artifact_bundle_manifest, required_artifacts_present,
};
use super::case::{AcceptanceTranscriptExpect, SourceRangeExpectation, TranscriptSegment};
use super::external_fixture_scenarios_from_path;
use super::judges::{build_transcript_evidence, evaluate_source_range_expectations};
use super::media::{parse_black_segments, write_transcript_sidecar};
use super::runner::create_run_root;
use super::scorecard::{AcceptanceGateResult, scorecard_status};
use super::timeline::FinalKeptSourceRange;

#[test]
fn parses_blackdetect_segments_from_ffmpeg_stderr() {
    let stderr = "[blackdetect @ 0x123] black_start:1.25 black_end:2.00 black_duration:0.75";

    let segments = parse_black_segments(stderr);

    assert_eq!(segments.len(), 1);
    assert!((segments[0].start_s - 1.25).abs() < 1e-9);
    assert!((segments[0].end_s - 2.0).abs() < 1e-9);
    assert!((segments[0].duration_s - 0.75).abs() < 1e-9);
}

#[test]
fn create_run_root_returns_absolute_path() {
    let run_root = create_run_root("acceptance::path-regression").unwrap();

    assert!(
        run_root.is_absolute(),
        "render inputs must not become cwd-relative: {}",
        run_root.display()
    );
}

#[test]
fn create_run_root_avoids_same_second_collisions() {
    let first = create_run_root("acceptance::collision-regression").unwrap();
    let second = create_run_root("acceptance::collision-regression").unwrap();

    assert_ne!(
        first, second,
        "parallel acceptance processes must not reuse the same scenario run root"
    );
}

#[test]
fn write_transcript_sidecar_materializes_segments_and_words() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = AcceptanceTranscriptExpect {
        segments: vec![TranscriptSegment {
            text: "get those papers signed".into(),
            start_s: 1.5,
            end_s: 3.5,
        }],
        must_preserve: Vec::new(),
        must_remove: Vec::new(),
    };

    let path = write_transcript_sidecar(dir.path(), "raw/source.mp4", &transcript).unwrap();
    let sidecar: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();

    assert_eq!(sidecar["asset_id"], "raw/source.mp4");
    assert_eq!(
        sidecar["data"]["segments"][0]["text"],
        "get those papers signed"
    );
    assert_eq!(sidecar["data"]["words"][0]["text"], "get");
    assert_eq!(sidecar["data"]["words"][3]["text"], "signed");
}

#[test]
fn discovers_external_acceptance_fixture_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("real-a.json"),
        r#"{
          "id": "acceptance::real_a",
          "objective": "real fixture A",
          "project": {
            "name": "real-a",
            "source_asset": "raw/source.mp4",
            "source_duration_s": 1.0,
            "source_path": "/tmp/a.mp4",
            "source_start_s": 0.0
          },
          "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 1.0\n*** End EDL\n",
          "expect": {
            "duration_min_s": 0.5,
            "duration_max_s": 1.5,
            "max_long_silence_s": 0.8,
            "silence_threshold_db": -40.0,
            "max_black_segment_s": 0.2,
            "black_min_duration_s": 0.2,
            "min_clip_count": 1
          }
        }"#,
    )
    .unwrap();
    fs::write(dir.path().join("ignore.txt"), "not json").unwrap();

    let scenarios = external_fixture_scenarios_from_path(dir.path()).unwrap();
    let ids = scenarios
        .iter()
        .map(|scenario| scenario.id())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["acceptance::real_a"]);
}

#[test]
fn external_fixture_discovery_ignores_templates_and_samples() {
    let dir = tempfile::tempdir().unwrap();
    let runnable = r#"{
      "id": "acceptance::real_runnable",
      "objective": "real runnable fixture",
      "project": {
        "name": "real-runnable",
        "source_asset": "raw/source.mp4",
        "source_duration_s": 1.0,
        "source_path": "/tmp/runnable.mp4",
        "source_start_s": 0.0
      },
      "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 1.0\n*** End EDL\n",
      "expect": {
        "duration_min_s": 0.5,
        "duration_max_s": 1.5,
        "max_long_silence_s": 0.8,
        "silence_threshold_db": -40.0,
        "max_black_segment_s": 0.2,
        "black_min_duration_s": 0.2,
        "min_clip_count": 1
      }
    }"#;
    fs::write(dir.path().join("real-runnable.json"), runnable).unwrap();
    fs::write(
        dir.path().join("scenario.template.json"),
        "{ not runnable }",
    )
    .unwrap();
    fs::write(dir.path().join("scenario.sample.json"), "{ not runnable }").unwrap();

    let scenarios = external_fixture_scenarios_from_path(dir.path()).unwrap();
    let ids = scenarios
        .iter()
        .map(|scenario| scenario.id())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["acceptance::real_runnable"]);
}

#[test]
fn external_fixture_discovery_ignores_review_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let runnable = r#"{
      "id": "acceptance::real_runnable",
      "objective": "real runnable fixture",
      "project": {
        "name": "real-runnable",
        "source_asset": "raw/source.mp4",
        "source_duration_s": 1.0,
        "source_path": "/tmp/runnable.mp4",
        "source_start_s": 0.0
      },
      "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 1.0\n*** End EDL\n",
      "expect": {
        "duration_min_s": 0.5,
        "duration_max_s": 1.5,
        "max_long_silence_s": 0.8,
        "silence_threshold_db": -40.0,
        "max_black_segment_s": 0.2,
        "black_min_duration_s": 0.2,
        "min_clip_count": 1
      }
    }"#;
    fs::write(dir.path().join("real-runnable.json"), runnable).unwrap();
    fs::write(
        dir.path().join("real-runnable.review.json"),
        r#"{"reviewed_at":"2026-05-19T00:00:00Z"}"#,
    )
    .unwrap();

    let scenarios = external_fixture_scenarios_from_path(dir.path()).unwrap();
    let ids = scenarios
        .iter()
        .map(|scenario| scenario.id())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["acceptance::real_runnable"]);
}

#[test]
fn source_range_expectations_detect_removed_and_preserved_spans() {
    let kept = vec![
        FinalKeptSourceRange {
            clip_name: "intro".into(),
            asset: "raw/source.mp4".into(),
            timeline_start_s: 0.0,
            timeline_end_s: 1.2,
            source_start_s: 0.0,
            source_end_s: 1.2,
        },
        FinalKeptSourceRange {
            clip_name: "answer".into(),
            asset: "raw/source.mp4".into(),
            timeline_start_s: 1.2,
            timeline_end_s: 3.7,
            source_start_s: 2.6,
            source_end_s: 5.1,
        },
    ];
    let removed = vec![SourceRangeExpectation {
        start_s: 1.3,
        end_s: 2.5,
        reason: Some("dead air".into()),
        tolerance_s: Some(0.15),
    }];
    let preserved = vec![SourceRangeExpectation {
        start_s: 2.8,
        end_s: 4.8,
        reason: Some("important answer".into()),
        tolerance_s: Some(0.15),
    }];

    let report = evaluate_source_range_expectations(&kept, &removed, &preserved);

    assert!(report.removed_passed);
    assert!(report.preserved_passed);
    assert_eq!(report.removed[0].status, "pass");
    assert_eq!(report.preserved[0].status, "pass");
}

#[test]
fn transcript_evidence_uses_only_text_inside_final_kept_source_ranges() {
    let transcript = AcceptanceTranscriptExpect {
        segments: vec![
            TranscriptSegment {
                text: "false start let me restart".into(),
                start_s: 1.3,
                end_s: 2.3,
            },
            TranscriptSegment {
                text: "the final argument stays intact".into(),
                start_s: 2.8,
                end_s: 4.2,
            },
        ],
        must_preserve: vec!["final argument".into()],
        must_remove: vec!["false start".into()],
    };
    let kept = vec![FinalKeptSourceRange {
        clip_name: "answer".into(),
        asset: "raw/source.mp4".into(),
        timeline_start_s: 0.0,
        timeline_end_s: 1.6,
        source_start_s: 2.6,
        source_end_s: 4.2,
    }];

    let evidence = build_transcript_evidence(&transcript, &kept);

    assert!(evidence.preserve_passed);
    assert!(evidence.remove_passed);
    assert!(evidence.final_kept_text.contains("final argument"));
    assert!(!evidence.final_kept_text.contains("false start"));
}

#[test]
fn scorecard_status_warns_when_only_soft_gates_fail() {
    let gates = vec![
        AcceptanceGateResult {
            name: "render_ok".into(),
            severity: "hard".into(),
            status: "pass".into(),
            message: "rendered".into(),
            observed: json!({}),
        },
        AcceptanceGateResult {
            name: "semantic_quality".into(),
            severity: "soft".into(),
            status: "fail".into(),
            message: "judge below threshold".into(),
            observed: json!({}),
        },
    ];

    assert_eq!(scorecard_status(&gates), "warn");
}

#[test]
fn artifact_bundle_manifest_reports_required_files() {
    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("render.mp4");
    let final_edl_path = dir.path().join("final_edl.edl");
    let edit_manifest_path = dir.path().join("edit_manifest.json");
    let ffprobe_json_path = dir.path().join("ffprobe.json");
    let silence_path = dir.path().join("silence.json");
    let blackdetect_path = dir.path().join("blackdetect.json");
    let scorecard_path = dir.path().join("scorecard.json");
    let generator_stdout_path = dir.path().join("edl_generator_stdout.txt");
    let generator_stderr_path = dir.path().join("edl_generator_stderr.txt");
    for path in [
        &output_path,
        &final_edl_path,
        &edit_manifest_path,
        &ffprobe_json_path,
        &silence_path,
        &blackdetect_path,
        &scorecard_path,
        &generator_stdout_path,
        &generator_stderr_path,
    ] {
        fs::write(path, b"artifact").unwrap();
    }

    let manifest = build_artifact_bundle_manifest(ArtifactBundleInput {
        run_root: dir.path(),
        output_path: &output_path,
        final_edl_path: &final_edl_path,
        edit_manifest_path: &edit_manifest_path,
        ffprobe_json_path: &ffprobe_json_path,
        silence_path: &silence_path,
        blackdetect_path: &blackdetect_path,
        transcript_path: None,
        generator_stdout_path: Some(&generator_stdout_path),
        generator_stderr_path: Some(&generator_stderr_path),
        scorecard_path: &scorecard_path,
    });

    assert!(manifest.complete);
    assert_eq!(manifest.entries.len(), 9);
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.name == "final_edl")
    );
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.name == "edl_generator_stdout")
    );
    assert!(
        manifest
            .entries
            .iter()
            .all(|entry| entry.exists && entry.size_bytes.unwrap_or_default() > 0)
    );
}

#[test]
fn required_artifacts_present_allows_empty_optional_generator_stderr() {
    let entries = vec![
        super::artifacts::ArtifactBundleEntry {
            name: "edl_generator_stdout".into(),
            path: "/tmp/stdout".into(),
            required: true,
            exists: true,
            size_bytes: Some(10),
        },
        super::artifacts::ArtifactBundleEntry {
            name: "edl_generator_stderr".into(),
            path: "/tmp/stderr".into(),
            required: false,
            exists: true,
            size_bytes: Some(0),
        },
    ];

    assert!(required_artifacts_present(&entries));
}
