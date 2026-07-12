//! Typed scenario contracts for deterministic eval runs.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// One tool- or flow-level eval scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Stable scenario identifier.
    pub id: String,
    /// Progression category, such as `podcast`.
    pub category: String,
    /// Tool or flow under evaluation.
    pub tool: String,
    /// Source media path, resolved by the campaign runner.
    pub source: PathBuf,
    /// Agent-visible editing task.
    pub task: String,
    /// Deterministic and faithfulness gates that must pass.
    pub hard_gates: HardGates,
    /// Named playbook and style threshold for the later tier-4 judge.
    pub soft_gates: SoftGates,
    /// Repair-loop policy.
    pub repair: RepairPolicy,
    /// Driver-enforced worker limits.
    pub guards: Guards,
}

/// Hard-gate thresholds from the approved scenario contract.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardGates {
    /// Require ffprobe to recognize a playable output.
    #[serde(default)]
    pub playable: bool,
    /// Required display aspect ratio, for example `16:9`.
    pub aspect_ratio: Option<String>,
    /// Longest allowed remaining silence.
    pub max_remaining_silence_seconds: Option<f64>,
    /// Minimum fraction of source speech retained.
    pub min_speech_retention: Option<f64>,
    /// Maximum caption word-error rate.
    pub max_caption_wer: Option<f64>,
    /// Reject detected black frames.
    #[serde(default)]
    pub no_black_frames: bool,
    /// Reject detected frozen video spans.
    #[serde(default)]
    pub no_freeze_frames: bool,
    /// Reject illegal timeline overlaps.
    #[serde(default)]
    pub no_invalid_timeline_overlaps: bool,
    /// Reject cut boundaries inside words.
    #[serde(default)]
    pub no_mid_word_cuts: bool,
}

/// Tier-4 standard declared before editing starts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftGates {
    /// Bundled editorial playbook the worker must follow.
    pub declared_playbook: String,
    /// Minimum normalized style score.
    pub min_style_score: f64,
}

/// Repair-loop settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairPolicy {
    /// Permitted loop behavior.
    pub policy: RepairPolicyKind,
    /// Fire-alarm attempt ceiling.
    pub safety_ceiling: u32,
}

/// Supported repair policy.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairPolicyKind {
    /// Continue only while defects strictly improve without regressions.
    WhileImproving,
}

/// Worker diff and contract-mutation limits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    /// Must remain false; scenarios are immutable during a run.
    pub allow_scenario_edits: bool,
    /// Must remain false; thresholds are immutable during a run.
    pub allow_threshold_edits: bool,
    /// Maximum files a worker may change.
    pub max_files_changed: u32,
    /// Maximum changed lines across the worker diff.
    pub max_lines_changed: u32,
}

/// Scenario load or contract error.
#[derive(Debug, Error)]
pub enum ScenarioError {
    /// Reading a YAML file failed.
    #[error("reading scenario {path}: {source}")]
    Read {
        /// Scenario path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// YAML did not match the typed contract.
    #[error("invalid scenario YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    /// Parsed values violate a driver invariant.
    #[error("invalid scenario contract: {0}")]
    Contract(String),
}

impl Scenario {
    /// Load and validate a scenario YAML file.
    pub fn from_yaml_file(path: impl AsRef<Path>) -> Result<Self, ScenarioError> {
        let path = path.as_ref();
        let input = std::fs::read_to_string(path).map_err(|source| ScenarioError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_yaml_str(input)
    }

    /// Parse and validate scenario YAML text.
    pub fn from_yaml_str(input: impl AsRef<str>) -> Result<Self, ScenarioError> {
        let scenario: Self = serde_yaml::from_str(input.as_ref())?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<(), ScenarioError> {
        for (name, value) in [
            ("id", self.id.as_str()),
            ("category", self.category.as_str()),
            ("tool", self.tool.as_str()),
            ("task", self.task.as_str()),
            (
                "declared_playbook",
                self.soft_gates.declared_playbook.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ScenarioError::Contract(format!("{name} must not be empty")));
            }
        }
        if self.source.as_os_str().is_empty() {
            return Err(ScenarioError::Contract("source must not be empty".into()));
        }
        if self.repair.safety_ceiling == 0 {
            return Err(ScenarioError::Contract(
                "safety_ceiling must be greater than zero".into(),
            ));
        }
        if self.guards.allow_scenario_edits {
            return Err(ScenarioError::Contract(
                "allow_scenario_edits must be false".into(),
            ));
        }
        if self.guards.allow_threshold_edits {
            return Err(ScenarioError::Contract(
                "allow_threshold_edits must be false".into(),
            ));
        }
        if self.guards.max_files_changed == 0 || self.guards.max_lines_changed == 0 {
            return Err(ScenarioError::Contract(
                "diff budgets must be greater than zero".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.soft_gates.min_style_score) {
            return Err(ScenarioError::Contract(
                "min_style_score must be between 0 and 1".into(),
            ));
        }
        if let Some(maximum) = self.hard_gates.max_remaining_silence_seconds
            && (!maximum.is_finite() || maximum < 0.0)
        {
            return Err(ScenarioError::Contract(
                "max_remaining_silence_seconds must be finite and non-negative".into(),
            ));
        }
        validate_normalized("min_speech_retention", self.hard_gates.min_speech_retention)?;
        validate_normalized("max_caption_wer", self.hard_gates.max_caption_wer)?;
        Ok(())
    }
}

fn validate_normalized(name: &str, value: Option<f64>) -> Result<(), ScenarioError> {
    if let Some(value) = value
        && (!value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(ScenarioError::Contract(format!(
            "{name} must be finite and between 0 and 1"
        )));
    }
    Ok(())
}
