use montage_eval::{
    Defect, DefectSeverity, NextAction, Scorecard, ScorecardStatus, TierStatus, Tiers,
};
use serde_json::json;

fn failing_scorecard() -> Scorecard {
    Scorecard {
        scenario_id: "podcast_dead_air_basic_001".into(),
        attempt: 2,
        status: ScorecardStatus::Fail,
        score: 0.74,
        tiers: Tiers {
            mechanical: TierStatus::Pass,
            measurable: TierStatus::Fail,
            faithfulness: TierStatus::Pass,
            style: 0.71,
        },
        blocking_failures: vec![Defect {
            code: "SILENCE_TOO_LONG".into(),
            severity: DefectSeverity::Blocker,
            evidence: json!({"segments": [{"start": 118.2, "end": 121.6}]}),
            repair_instruction: "Tighten these silences without cutting speech.".into(),
        }],
        stop_reason: None,
        next_action: NextAction::Repair,
    }
}

#[test]
fn writes_the_machine_readable_scorecard_contract() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let path = temp.path().join("scorecard.json");

    failing_scorecard()
        .write(&path)
        .unwrap_or_else(|error| panic!("scorecard should write: {error}"));

    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("scorecard should read: {error}"));
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("scorecard should be JSON: {error}"));
    assert_eq!(value["tiers"]["mechanical"], "pass");
    assert_eq!(value["blocking_failures"][0]["code"], "SILENCE_TOO_LONG");
    assert_eq!(value["next_action"], "repair");
}

#[test]
fn rejects_invalid_normalized_scores() {
    let mut scorecard = failing_scorecard();
    scorecard.score = 1.1;

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let error = match scorecard.write(temp.path().join("scorecard.json")) {
        Ok(()) => panic!("out-of-range score must fail closed"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("score"));
}
