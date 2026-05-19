use std::fs;

use serde_json::json;

use super::promotion::{PromotionOptions, promote_fixture};

#[test]
fn promotion_copies_fixture_and_refuses_overwrite_without_force()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let draft_path = dir.path().join("candidate.json");
    fs::write(
        &draft_path,
        serde_json::to_vec_pretty(&json!({
            "id": "acceptance::candidate_clip",
            "objective": "Generate and apply a dead-air cleanup EDL.",
            "scenario_type": "dead_air_cleanup",
            "project": {
                "name": "candidate-clip",
                "source_asset": "raw/source.mp4",
                "source_path": "/mnt/video-corpus/clip.mp4",
                "source_duration_s": 10.0
            },
            "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 10.0\n*** End EDL\n",
            "expect": {
                "duration_min_s": 9.0,
                "duration_max_s": 10.5,
                "max_long_silence_s": 0.8,
                "silence_threshold_db": -40.0,
                "max_black_segment_s": 0.2,
                "black_min_duration_s": 0.2,
                "min_clip_count": 1
            }
        }))?,
    )?;
    let manifest_path = dir.path().join("fixture_drafts_manifest.json");
    fs::write(&manifest_path, "{}\n")?;
    let promoted_dir = dir.path().join("promoted");

    let report = promote_fixture(
        &draft_path,
        &promoted_dir,
        PromotionOptions {
            reviewer_note: "Reviewed scorecard and render around flagged ranges.",
            promotion_reason: "Good durable dead-air cleanup candidate.",
            discovery_manifest_path: Some(&manifest_path),
            force: false,
        },
    )?;

    assert!(report.promoted_fixture_path.is_file());
    assert!(report.review_metadata_path.is_file());
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&report.review_metadata_path)?)?;
    assert_eq!(metadata["scenario_type"], "dead_air_cleanup");
    assert_eq!(
        metadata["reviewer_note"],
        "Reviewed scorecard and render around flagged ranges."
    );
    assert_eq!(
        metadata["promotion_reason"],
        "Good durable dead-air cleanup candidate."
    );
    assert_eq!(
        metadata["original_discovery_manifest_path"],
        manifest_path.to_string_lossy().as_ref()
    );

    let error = promote_fixture(
        &draft_path,
        &promoted_dir,
        PromotionOptions {
            reviewer_note: "second review",
            promotion_reason: "should not overwrite",
            discovery_manifest_path: None,
            force: false,
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("already exists"));
    Ok(())
}
