use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use super::case::AcceptanceCase;

pub(crate) fn push_gate(
    gates: &mut Vec<AcceptanceGateResult>,
    name: &str,
    passed: bool,
    message: &str,
    observed: serde_json::Value,
) {
    gates.push(AcceptanceGateResult {
        name: name.into(),
        severity: "hard".into(),
        status: if passed { "pass".into() } else { "fail".into() },
        message: message.into(),
        observed,
    });
}

pub(crate) fn build_scorecard(input: ScorecardInput<'_>) -> AcceptanceScorecard {
    let passed = input
        .gates
        .iter()
        .filter(|gate| gate.status == "pass")
        .count();
    let total = input.gates.len();
    let score = if total == 0 {
        0.0
    } else {
        (passed as f64 / total as f64) * 100.0
    };
    let status = scorecard_status(&input.gates);
    AcceptanceScorecard {
        scenario_id: input.case.id.clone(),
        objective: input.case.objective.clone(),
        status: status.into(),
        score,
        generated_at: Utc::now().to_rfc3339(),
        run_root: input.run_root.to_string_lossy().into_owned(),
        edit_driver: input.edit_driver.to_string(),
        render_driver: input.render_driver.to_string(),
        output_path: input.output_path.to_string_lossy().into_owned(),
        artifacts: AcceptanceArtifacts {
            artifacts_root: input.artifacts_root.to_string_lossy().into_owned(),
            final_edl: input.final_edl_path.to_string_lossy().into_owned(),
            edit_manifest: input.edit_manifest_path.to_string_lossy().into_owned(),
            ffprobe_json: input.ffprobe_json_path.to_string_lossy().into_owned(),
            silence_json: input.silence_path.to_string_lossy().into_owned(),
            blackdetect_json: input.blackdetect_path.to_string_lossy().into_owned(),
            transcript_json: input
                .transcript_path
                .map(|path| path.to_string_lossy().into_owned()),
            edl_generator_stdout: input
                .generator_stdout_path
                .map(|path| path.to_string_lossy().into_owned()),
            edl_generator_stderr: input
                .generator_stderr_path
                .map(|path| path.to_string_lossy().into_owned()),
            artifact_bundle_json: input.artifact_bundle_path.to_string_lossy().into_owned(),
            scorecard_json: input
                .artifacts_root
                .join("scorecard.json")
                .to_string_lossy()
                .into_owned(),
        },
        ffprobe: input.ffprobe_json,
        gates: input.gates,
    }
}

pub(crate) fn scorecard_status(gates: &[AcceptanceGateResult]) -> &'static str {
    if gates
        .iter()
        .any(|gate| gate.severity == "hard" && gate.status == "fail")
    {
        return "fail";
    }
    if gates.iter().any(|gate| gate.status == "fail") {
        return "warn";
    }
    "pass"
}
#[derive(Debug, Serialize)]
pub(crate) struct AcceptanceScorecard {
    pub(crate) scenario_id: String,
    pub(crate) objective: String,
    pub(crate) status: String,
    pub(crate) score: f64,
    pub(crate) generated_at: String,
    pub(crate) run_root: String,
    pub(crate) edit_driver: String,
    pub(crate) render_driver: String,
    pub(crate) output_path: String,
    pub(crate) artifacts: AcceptanceArtifacts,
    pub(crate) ffprobe: serde_json::Value,
    pub(crate) gates: Vec<AcceptanceGateResult>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcceptanceArtifacts {
    pub(crate) artifacts_root: String,
    pub(crate) final_edl: String,
    pub(crate) edit_manifest: String,
    pub(crate) ffprobe_json: String,
    pub(crate) silence_json: String,
    pub(crate) blackdetect_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transcript_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) edl_generator_stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) edl_generator_stderr: Option<String>,
    pub(crate) artifact_bundle_json: String,
    pub(crate) scorecard_json: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcceptanceGateResult {
    pub(crate) name: String,
    pub(crate) severity: String,
    pub(crate) status: String,
    pub(crate) message: String,
    pub(crate) observed: serde_json::Value,
}
pub(crate) struct ScorecardInput<'a> {
    pub(crate) case: &'a AcceptanceCase,
    pub(crate) run_root: &'a Path,
    pub(crate) artifacts_root: &'a Path,
    pub(crate) output_path: &'a Path,
    pub(crate) final_edl_path: &'a Path,
    pub(crate) edit_manifest_path: &'a Path,
    pub(crate) ffprobe_json_path: &'a Path,
    pub(crate) silence_path: &'a Path,
    pub(crate) blackdetect_path: &'a Path,
    pub(crate) transcript_path: Option<&'a Path>,
    pub(crate) generator_stdout_path: Option<&'a Path>,
    pub(crate) generator_stderr_path: Option<&'a Path>,
    pub(crate) artifact_bundle_path: &'a Path,
    pub(crate) edit_driver: &'a str,
    pub(crate) render_driver: &'a str,
    pub(crate) ffprobe_json: serde_json::Value,
    pub(crate) gates: Vec<AcceptanceGateResult>,
}
