use montage_eval::{RepairPolicyKind, Scenario};

const VALID_SCENARIO: &str = r#"
id: podcast_dead_air_basic_001
category: podcast
tool: auto-cutter
source: corpus/podcast/two_speaker_dead_air_12min.mp4
task: Remove dead air longer than 1.0s and preserve meaningful speech.
hard_gates:
  playable: true
  aspect_ratio: "16:9"
  max_remaining_silence_seconds: 1.0
  min_speech_retention: 0.97
  max_caption_wer: 0.08
  no_black_frames: true
  no_invalid_timeline_overlaps: true
  no_mid_word_cuts: true
soft_gates:
  declared_playbook: auto-cutter
  min_style_score: 0.80
repair:
  policy: while_improving
  safety_ceiling: 10
guards:
  allow_scenario_edits: false
  allow_threshold_edits: false
  max_files_changed: 6
  max_lines_changed: 400
"#;

#[test]
fn loads_the_spec_scenario_contract() {
    let scenario = Scenario::from_yaml_str(VALID_SCENARIO)
        .unwrap_or_else(|error| panic!("scenario should load: {error}"));

    assert_eq!(scenario.id, "podcast_dead_air_basic_001");
    assert_eq!(scenario.repair.policy, RepairPolicyKind::WhileImproving);
    assert_eq!(scenario.repair.safety_ceiling, 10);
    assert!(!scenario.guards.allow_scenario_edits);
    assert_eq!(scenario.hard_gates.aspect_ratio.as_deref(), Some("16:9"));
}

#[test]
fn rejects_a_zero_safety_ceiling() {
    let input = VALID_SCENARIO.replace("safety_ceiling: 10", "safety_ceiling: 0");
    let error = match Scenario::from_yaml_str(input) {
        Ok(_) => panic!("zero attempts must fail closed"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("safety_ceiling"));
}

#[test]
fn rejects_mutable_scenario_guards() {
    let input = VALID_SCENARIO.replace("allow_scenario_edits: false", "allow_scenario_edits: true");
    let error = match Scenario::from_yaml_str(input) {
        Ok(_) => panic!("scenario edits must stay forbidden"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("allow_scenario_edits"));
}

#[test]
fn rejects_out_of_range_hard_gate_thresholds() {
    let input = VALID_SCENARIO.replace("min_speech_retention: 0.97", "min_speech_retention: 1.2");
    let error = match Scenario::from_yaml_str(input) {
        Ok(_) => panic!("retention above one must fail closed"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("min_speech_retention"));
}

#[test]
fn rejects_empty_scenario_identifiers() {
    let input = VALID_SCENARIO.replace("id: podcast_dead_air_basic_001", "id: \"\"");
    let error = match Scenario::from_yaml_str(input) {
        Ok(_) => panic!("empty scenario id must fail closed"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("id"));
}
