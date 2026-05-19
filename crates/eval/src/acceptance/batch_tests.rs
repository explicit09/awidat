use std::path::Path;

use serde_json::json;

use super::batch::batch_entry_from_scorecard;
use super::scorecard::{AcceptanceArtifacts, AcceptanceGateResult, AcceptanceScorecard};

#[test]
fn batch_entry_reports_paths_status_score_and_top_failing_gates() {
    let scorecard = AcceptanceScorecard {
        scenario_id: "acceptance::draft_a".into(),
        objective: "Remove dead air from a discovered clip.".into(),
        status: "fail".into(),
        score: 81.25,
        generated_at: "2026-05-19T12:00:00Z".into(),
        run_root: "/tmp/run".into(),
        edit_driver: "external_cli_apply_edl".into(),
        render_driver: "external_cli".into(),
        output_path: "/tmp/run/project/renders/timeline.mp4".into(),
        artifacts: AcceptanceArtifacts {
            artifacts_root: "/tmp/run/artifacts".into(),
            final_edl: "/tmp/run/artifacts/final_edl.edl".into(),
            edit_manifest: "/tmp/run/artifacts/edit_manifest.json".into(),
            ffprobe_json: "/tmp/run/artifacts/ffprobe.json".into(),
            silence_json: "/tmp/run/artifacts/silence.json".into(),
            blackdetect_json: "/tmp/run/artifacts/blackdetect.json".into(),
            transcript_json: None,
            edl_generator_stdout: Some("/tmp/run/artifacts/edl_generator_stdout.txt".into()),
            edl_generator_stderr: Some("/tmp/run/artifacts/edl_generator_stderr.txt".into()),
            artifact_bundle_json: "/tmp/run/artifacts/artifact_bundle.json".into(),
            scorecard_json: "/tmp/run/artifacts/scorecard.json".into(),
        },
        ffprobe: json!({}),
        gates: vec![
            AcceptanceGateResult {
                name: "render_ok".into(),
                severity: "hard".into(),
                status: "pass".into(),
                message: "rendered".into(),
                observed: json!({}),
            },
            AcceptanceGateResult {
                name: "no_long_silence".into(),
                severity: "hard".into(),
                status: "fail".into(),
                message: "long silence remained".into(),
                observed: json!({"ranges": [{"start_s": 3.0, "end_s": 4.2}]}),
            },
        ],
    };

    let entry = batch_entry_from_scorecard(
        Path::new("/fixtures/draft_a.json"),
        Some("dead_air_cleanup"),
        &scorecard,
        Path::new("/tmp/run/artifacts/scorecard.json"),
    );

    assert_eq!(entry.fixture_id, "acceptance::draft_a");
    assert_eq!(entry.objective, "Remove dead air from a discovered clip.");
    assert_eq!(entry.scenario_type, "dead_air_cleanup");
    assert_eq!(entry.status, "fail");
    assert_eq!(entry.score, Some(81.25));
    assert_eq!(
        entry.scorecard_path.as_deref(),
        Some("/tmp/run/artifacts/scorecard.json")
    );
    assert_eq!(
        entry.rendered_output_path.as_deref(),
        Some("/tmp/run/project/renders/timeline.mp4")
    );
    assert_eq!(
        entry.artifact_bundle_path.as_deref(),
        Some("/tmp/run/artifacts/artifact_bundle.json")
    );
    assert_eq!(entry.top_failing_gates, vec!["no_long_silence"]);
}
