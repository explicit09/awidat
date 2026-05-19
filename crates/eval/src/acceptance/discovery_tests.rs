use std::fs;

use serde_json::json;

use super::discover::{
    DiscoveredSilence, DiscoveryCandidate, DiscoveryError, DiscoveryOptions,
    DiscoveryOptionsReport, DiscoveryReport, build_fixture_draft, collect_video_paths,
    parse_silencedetect_output, write_fixture_drafts,
};
use super::discover_transcript::discover_transcript_candidates;

#[test]
fn discovery_collects_video_paths_with_depth_limit() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("clip.mp4"), b"video").unwrap();
    fs::write(dir.path().join("notes.txt"), b"not video").unwrap();
    fs::create_dir_all(dir.path().join("nested").join("too-deep")).unwrap();
    fs::write(dir.path().join("nested").join("short.mov"), b"video").unwrap();
    fs::write(
        dir.path()
            .join("nested")
            .join("too-deep")
            .join("ignored.mp4"),
        b"video",
    )
    .unwrap();

    let paths = collect_video_paths(dir.path(), 1).unwrap();
    let names = paths
        .iter()
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["clip.mp4", "short.mov"]);
}

#[test]
fn discovery_prioritizes_shallower_video_paths() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("z-top.mp4"), b"video").unwrap();
    fs::create_dir_all(dir.path().join("a-library")).unwrap();
    fs::write(dir.path().join("a-library").join("a-nested.mp4"), b"video").unwrap();

    let paths = collect_video_paths(dir.path(), 1).unwrap();
    let names = paths
        .iter()
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["z-top.mp4", "a-nested.mp4"]);
}

#[test]
fn discovery_parses_silencedetect_ranges() {
    let stderr = "\
[silencedetect @ 0x1] silence_start: 1.213333
[silencedetect @ 0x1] silence_end: 2.509 | silence_duration: 1.295667
[silencedetect @ 0x1] silence_start: 4.67225
[silencedetect @ 0x1] silence_end: 5.548958 | silence_duration: 0.876708";

    let ranges = parse_silencedetect_output(stderr);

    assert_eq!(ranges.len(), 2);
    assert!((ranges[0].start_s - 1.213333).abs() < 1e-9);
    assert!((ranges[0].duration_s - 1.295667).abs() < 1e-9);
    assert!((ranges[1].end_s - 5.548958).abs() < 1e-9);
}

#[test]
fn discovery_options_default_to_small_opt_in_scan() {
    let options = DiscoveryOptions::default();

    assert_eq!(options.max_depth, 2);
    assert_eq!(options.max_files, 50);
    assert_eq!(options.max_transcript_depth, 6);
    assert_eq!(options.max_transcript_files, 100);
    assert!(options.sample_duration_s > 0.0);
}

#[test]
fn discovery_builds_dead_air_fixture_draft_from_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("clip.mp4");
    fs::write(&source_path, b"media").unwrap();
    let candidate = DiscoveryCandidate {
        path: source_path.to_string_lossy().into_owned(),
        duration_s: Some(12.0),
        width: Some(1080),
        height: Some(1920),
        has_video: true,
        has_audio: true,
        long_silences: vec![DiscoveredSilence {
            start_s: 1.2,
            end_s: 2.6,
            duration_s: 1.4,
        }],
        score: 29.0,
        suggested_behavior: "dead_air_cleanup".into(),
        fixture_id_hint: "acceptance::candidate_clip".into(),
        fixture_draft: None,
    };
    let options = DiscoveryOptions::default();

    let Some(draft) = build_fixture_draft(&candidate, &options) else {
        panic!("expected a dead-air fixture draft");
    };

    assert_eq!(draft["id"], "acceptance::candidate_clip");
    assert_eq!(
        draft["project"]["source_path"],
        source_path.to_string_lossy().as_ref()
    );
    assert_eq!(
        draft["edl_generator"]["command"][2],
        "exec \"${AWIDAT_ACCEPTANCE_CLI:-awidat}\" plan-dead-air-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\" --min-duration-s 0.8 --silence-threshold-db -40"
    );
    assert_eq!(draft["expect"]["removed_source_ranges"][0]["start_s"], 1.2);
    assert_eq!(draft["expect"]["removed_source_ranges"][0]["end_s"], 2.6);
}

#[test]
fn discovery_writes_fixture_drafts_without_overwriting_existing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let report = DiscoveryReport {
        source_root: "/source".into(),
        generated_at: "2026-05-19T00:00:00Z".into(),
        options: DiscoveryOptionsReport::from(&DiscoveryOptions::default()),
        scanned_files: 1,
        candidates: vec![DiscoveryCandidate {
            path: "/source/clip.mp4".into(),
            duration_s: Some(12.0),
            width: Some(1920),
            height: Some(1080),
            has_video: true,
            has_audio: true,
            long_silences: Vec::new(),
            score: 1.0,
            suggested_behavior: "dead_air_cleanup".into(),
            fixture_id_hint: "acceptance::candidate_clip".into(),
            fixture_draft: Some(json!({
                "id": "acceptance::candidate_clip",
                "project": {
                    "source_path": "/source/clip.mp4"
                }
            })),
        }],
        transcript_candidates: Vec::new(),
        errors: Vec::<DiscoveryError>::new(),
    };

    let first = write_fixture_drafts(&report, dir.path())?;

    assert_eq!(first.written.len(), 1);
    assert_eq!(first.skipped_existing.len(), 0);
    let draft_path = dir.path().join("candidate_clip.json");
    assert_eq!(first.written[0], draft_path);
    let draft: serde_json::Value = serde_json::from_slice(&fs::read(&draft_path)?)?;
    assert_eq!(draft["id"], "acceptance::candidate_clip");

    fs::write(&draft_path, b"{\"id\":\"manual-edit\"}\n")?;
    let second = write_fixture_drafts(&report, dir.path())?;

    assert_eq!(second.written.len(), 0);
    assert_eq!(second.skipped_existing, vec![draft_path.clone()]);
    let preserved: serde_json::Value = serde_json::from_slice(&fs::read(&draft_path)?)?;
    assert_eq!(preserved["id"], "manual-edit");
    Ok(())
}

#[test]
fn discovery_writes_fixture_draft_manifest_for_review() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let source_path = dir.path().join("source").join("clip.mp4");
    fs::create_dir_all(source_path.parent().unwrap())?;
    fs::write(&source_path, b"media")?;
    let report = DiscoveryReport {
        source_root: dir.path().to_string_lossy().into_owned(),
        generated_at: "2026-05-19T00:00:00Z".into(),
        options: DiscoveryOptionsReport::from(&DiscoveryOptions::default()),
        scanned_files: 2,
        candidates: vec![DiscoveryCandidate {
            path: source_path.to_string_lossy().into_owned(),
            duration_s: Some(12.0),
            width: Some(1920),
            height: Some(1080),
            has_video: true,
            has_audio: true,
            long_silences: vec![DiscoveredSilence {
                start_s: 1.0,
                end_s: 2.0,
                duration_s: 1.0,
            }],
            score: 42.0,
            suggested_behavior: "dead_air_cleanup".into(),
            fixture_id_hint: "acceptance::candidate_clip".into(),
            fixture_draft: Some(json!({
                "id": "acceptance::candidate_clip",
                "project": {
                    "source_path": source_path.to_string_lossy()
                }
            })),
        }],
        transcript_candidates: Vec::new(),
        errors: Vec::<DiscoveryError>::new(),
    };

    let written = write_fixture_drafts(&report, dir.path())?;

    assert_eq!(
        written.manifest_path,
        dir.path().join("fixture_drafts_manifest.json")
    );
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(&written.manifest_path)?)?;
    assert_eq!(
        manifest["source_root"],
        dir.path().to_string_lossy().as_ref()
    );
    assert_eq!(manifest["draft_count"], 1);
    assert_eq!(manifest["written_count"], 1);
    assert_eq!(manifest["skipped_existing_count"], 0);
    assert_eq!(manifest["drafts"][0]["id"], "acceptance::candidate_clip");
    assert_eq!(manifest["drafts"][0]["rank"], 1);
    assert_eq!(manifest["drafts"][0]["fixture_path"], "candidate_clip.json");
    assert_eq!(
        manifest["drafts"][0]["source_path"],
        source_path.to_string_lossy().as_ref()
    );
    assert_eq!(manifest["drafts"][0]["scenario_type"], "dead_air_cleanup");
    assert_eq!(manifest["drafts"][0]["score"], 42.0);
    assert_eq!(manifest["drafts"][0]["long_silence_count"], 1);
    assert_eq!(
        manifest["drafts"][0]["ranking_signals"]["source_path_confidence"],
        "absolute_existing"
    );
    assert_eq!(
        manifest["drafts"][0]["ranking_signals"]["expected_usefulness"],
        "high"
    );
    assert_eq!(
        manifest["drafts"][0]["ranking_signals"]["transcript_cleanup_evidence"]["available"],
        false
    );
    assert_eq!(
        manifest["drafts"][0]["ranking_signals"]["long_silence_total_s"],
        1.0
    );
    assert_eq!(
        manifest["drafts"][0]["ranking_signals"]["silence_density"],
        0.083
    );
    assert!(
        manifest["drafts"][0]["ranking_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("1 long silence")
    );
    Ok(())
}

#[test]
fn discovery_reports_transcript_candidates_from_whisper_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_path = dir
        .path()
        .join("awidat-run")
        .join("index")
        .join("whisper")
        .join("raw")
        .join("clip.mp4.json");
    fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&json!({
            "asset_id": "raw/clip.mp4",
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 1.0, "text": "So this wait actually keep going"},
                    {"start_s": 2.0, "end_s": 3.0, "text": "um uh like um"},
                    {"start_s": 3.0, "end_s": 5.0, "text": "important conclusion stays"}
                ],
                "words": [
                    {"start_s": 0.0, "end_s": 0.3, "text": "So"},
                    {"start_s": 0.3, "end_s": 0.7, "text": "this"},
                    {"start_s": 0.7, "end_s": 1.0, "text": "wait,"},
                    {"start_s": 1.0, "end_s": 1.2, "text": "actually"},
                    {"start_s": 1.2, "end_s": 1.6, "text": "keep"},
                    {"start_s": 1.6, "end_s": 2.0, "text": "going"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let candidates = discover_transcript_candidates(dir.path(), 4, 20).unwrap();
    let behaviors = candidates
        .iter()
        .map(|candidate| candidate.suggested_behavior.as_str())
        .collect::<Vec<_>>();

    assert!(behaviors.contains(&"false_start_cleanup"));
    assert!(behaviors.contains(&"transcript_filler_cleanup"));
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.snippet.contains("So this"))
    );
}

#[test]
fn discovery_resolves_transcript_candidate_source_path() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path().join("awidat-run");
    let source_path = project_root.join("raw").join("clip.mp4");
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    fs::write(&source_path, b"media").unwrap();
    let sidecar_path = project_root
        .join("index")
        .join("whisper")
        .join("raw")
        .join("clip.mp4.json");
    fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&json!({
            "asset_id": "raw/clip.mp4",
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 1.0, "text": "So this wait actually keep going"}
                ],
                "words": [
                    {"start_s": 0.0, "end_s": 0.3, "text": "So"},
                    {"start_s": 0.3, "end_s": 0.7, "text": "this"},
                    {"start_s": 0.7, "end_s": 1.0, "text": "wait,"},
                    {"start_s": 1.0, "end_s": 1.2, "text": "actually"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let candidates = discover_transcript_candidates(dir.path(), 4, 20).unwrap();
    let expected_source_path = source_path
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    assert_eq!(
        candidates[0].source_path.as_deref(),
        Some(expected_source_path.as_str())
    );
}

#[test]
fn discovery_does_not_promote_bare_actually_phrase_as_false_start() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_path = dir
        .path()
        .join("awidat-run")
        .join("index")
        .join("whisper")
        .join("raw")
        .join("clip.mp4.json");
    fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&json!({
            "asset_id": "raw/clip.mp4",
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 2.0, "text": "working on something that I actually enjoy"}
                ],
                "words": [
                    {"start_s": 0.0, "end_s": 0.3, "text": "working"},
                    {"start_s": 0.3, "end_s": 0.6, "text": "on"},
                    {"start_s": 0.6, "end_s": 0.9, "text": "something"},
                    {"start_s": 0.9, "end_s": 1.1, "text": "that"},
                    {"start_s": 1.1, "end_s": 1.2, "text": "I"},
                    {"start_s": 1.2, "end_s": 1.5, "text": "actually"},
                    {"start_s": 1.5, "end_s": 2.0, "text": "enjoy"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let candidates = discover_transcript_candidates(dir.path(), 4, 20).unwrap();

    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.suggested_behavior != "false_start_cleanup"),
        "bare mid-phrase actually should not become a cleanup fixture candidate: {candidates:#?}"
    );
}
