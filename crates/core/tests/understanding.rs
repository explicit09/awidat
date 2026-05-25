//! Understanding fusion and clip candidate tests.

use std::path::Path;

use awidat_core::clip_candidates::build_clip_candidate_package;
use awidat_core::understanding::build_understanding_for_asset;
use awidat_index::sidecar_path;
use awidat_proto::index::AssetId;
use awidat_proto::professional::UnderstandingEvidenceKind;

#[test]
fn understanding_fuses_scene_topic_audio_transcript_and_moment() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture_sidecars(dir.path(), "raw/a.mov");

    let asset = build_understanding_for_asset(dir.path(), "raw/a.mov");

    assert_eq!(asset.asset_id, "raw/a.mov");
    assert!(asset.missing_evidence.is_empty());
    assert_eq!(asset.scenes.len(), 1);
    assert_eq!(asset.moments.len(), 1);
    let moment = &asset.moments[0];
    assert_eq!(moment.category, "hook");
    assert_eq!(moment.speaker_id.as_deref(), Some("SPEAKER_00"));
    assert_eq!(moment.topics, vec!["AI editing"]);
    assert_eq!(moment.energy.as_deref(), Some("high"));
    assert!(
        moment
            .transcript_excerpt
            .as_deref()
            .unwrap()
            .contains("edit")
    );
    assert!(moment.scene_id.is_some());
    assert!(
        moment
            .evidence
            .iter()
            .any(|evidence| evidence.kind == UnderstandingEvidenceKind::EditorialMoment)
    );
}

#[test]
fn understanding_reports_missing_evidence_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    write_sidecar(
        dir.path(),
        "editorial-moments",
        "raw/a.mov",
        serde_json::json!({
            "data": {
                "moments": [
                    {"moment_id": "m1", "kind": "story", "start_s": 2.0, "end_s": 8.0, "score": 0.7}
                ]
            }
        }),
    );

    let asset = build_understanding_for_asset(dir.path(), "raw/a.mov");

    assert_eq!(asset.moments.len(), 1);
    assert!(asset.scenes.is_empty());
    assert!(
        asset
            .missing_evidence
            .contains(&UnderstandingEvidenceKind::Transcript)
    );
    assert!(
        asset
            .missing_evidence
            .contains(&UnderstandingEvidenceKind::Scene)
    );
}

#[test]
fn clip_candidates_are_deterministic_and_reviewable() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture_sidecars(dir.path(), "raw/a.mov");
    let first = build_understanding_for_asset(dir.path(), "raw/a.mov");
    let second = build_understanding_for_asset(dir.path(), "raw/a.mov");

    let first_candidates = build_clip_candidate_package(&[first]);
    let second_candidates = build_clip_candidate_package(&[second]);

    let candidate = &first_candidates.assets[0].candidates[0];
    assert_eq!(candidate.id, second_candidates.assets[0].candidates[0].id);
    assert!(candidate.score > 0.7);
    assert!(candidate.explanation.contains("hook"));
    assert_eq!(candidate.assembly.aspect_ratio, "9:16");
    assert_eq!(candidate.assembly.caption_style, "kinetic_emphasis");
    assert_eq!(candidate.evidence_ids.len(), 1);
}

fn write_fixture_sidecars(project_root: &Path, asset_id: &str) {
    write_sidecar(
        project_root,
        "whisper",
        asset_id,
        serde_json::json!({
            "data": {
                "segments": [
                    {"text": "This changes how teams edit podcasts.", "start_s": 10.0, "end_s": 20.0, "speaker_id": "SPEAKER_00"}
                ],
                "words": [
                    {"text": "This", "start_s": 10.0, "end_s": 10.2, "speaker_id": "SPEAKER_00"}
                ]
            }
        }),
    );
    write_sidecar(
        project_root,
        "scenedetect",
        asset_id,
        serde_json::json!({
            "data": {"shots": [{"start_s": 0.0, "end_s": 30.0, "label": "interview"}]}
        }),
    );
    write_sidecar(
        project_root,
        "topic",
        asset_id,
        serde_json::json!({
            "data": {"topics": [{"label": "AI editing", "start_s": 5.0, "end_s": 25.0}]}
        }),
    );
    write_sidecar(
        project_root,
        "audio-energy",
        asset_id,
        serde_json::json!({
            "data": {"windows": [{"start_s": 10.0, "end_s": 20.0, "rms": 0.9}]}
        }),
    );
    write_sidecar(
        project_root,
        "editorial-moments",
        asset_id,
        serde_json::json!({
            "data": {
                "moments": [
                    {"moment_id": "hook-1", "kind": "hook", "start_s": 10.0, "end_s": 20.0, "score": 0.9, "note": "strong opening", "speaker": "SPEAKER_00"}
                ]
            }
        }),
    );
}

fn write_sidecar(project_root: &Path, indexer: &str, asset_id: &str, value: serde_json::Value) {
    let path = sidecar_path(project_root, indexer, &AssetId::new(asset_id.to_string())).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}
