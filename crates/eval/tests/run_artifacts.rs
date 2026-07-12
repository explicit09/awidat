use montage_eval::{RunArtifacts, Scenario};

const SCENARIO: &str = r#"
id: podcast_dead_air_basic_001
category: podcast
tool: auto-cutter
source: corpus/podcast/source.mp4
task: Remove dead air.
hard_gates:
  playable: true
soft_gates:
  declared_playbook: auto-cutter
  min_style_score: 0.8
repair:
  policy: while_improving
  safety_ceiling: 10
guards:
  allow_scenario_edits: false
  allow_threshold_edits: false
  max_files_changed: 6
  max_lines_changed: 400
"#;

fn scenario() -> Scenario {
    Scenario::from_yaml_str(SCENARIO)
        .unwrap_or_else(|error| panic!("test scenario should load: {error}"))
}

#[test]
fn creates_the_spec_run_layout() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let run = RunArtifacts::create(temp.path(), "run-001", &scenario())
        .unwrap_or_else(|error| panic!("run should be created: {error}"));

    assert!(run.root().ends_with("run-001/podcast_dead_air_basic_001"));
    assert!(run.task_path().is_file());
    assert!(run.input_manifest_path().is_file());
}

#[test]
fn attempt_directories_are_immutable() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let run = RunArtifacts::create(temp.path(), "run-001", &scenario())
        .unwrap_or_else(|error| panic!("run should be created: {error}"));

    let attempt = run
        .create_attempt(1)
        .unwrap_or_else(|error| panic!("attempt should be created: {error}"));
    assert!(attempt.root().ends_with("attempt_1"));
    assert!(attempt.evidence_dir().is_dir());
    assert!(run.create_attempt(1).is_err());
}

#[test]
fn rejects_unsafe_run_identifiers() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
    let error = match RunArtifacts::create(temp.path(), "../escape", &scenario()) {
        Ok(_) => panic!("run id must not escape the runs root"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("run id"));
}
