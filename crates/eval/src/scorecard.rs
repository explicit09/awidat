//! Machine-readable verdicts that drive the deterministic repair loop.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

/// Complete verdict for one scenario attempt.
#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    /// Stable scenario identifier.
    pub scenario_id: String,
    /// One-based attempt number.
    pub attempt: u32,
    /// Rolled-up pass or fail state.
    pub status: ScorecardStatus,
    /// Normalized aggregate score.
    pub score: f64,
    /// Per-tier outcomes.
    pub tiers: Tiers,
    /// Structured defects blocking progression.
    pub blocking_failures: Vec<Defect>,
    /// Terminal reason, when the loop stops.
    pub stop_reason: Option<StopReason>,
    /// Deterministic driver action.
    pub next_action: NextAction,
}

/// Scorecard pass/fail state.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScorecardStatus {
    /// All required gates passed.
    Pass,
    /// At least one required gate failed.
    Fail,
}

/// Verification tier outcomes.
#[derive(Debug, Clone, Serialize)]
pub struct Tiers {
    /// Tier-1 outcome.
    pub mechanical: TierStatus,
    /// Tier-2 outcome.
    pub measurable: TierStatus,
    /// Tier-3 outcome.
    pub faithfulness: TierStatus,
    /// Tier-4 normalized score.
    pub style: f64,
}

/// Hard-gate tier outcome.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TierStatus {
    /// Tier requirements passed.
    Pass,
    /// Tier requirements failed.
    Fail,
    /// Tier was not requested or is not available yet.
    Skipped,
}

/// One structured verifier defect.
#[derive(Debug, Clone, Serialize)]
pub struct Defect {
    /// Stable failure taxonomy code.
    pub code: String,
    /// Severity used by the improvement comparator.
    pub severity: DefectSeverity,
    /// Machine-readable measurements and spans.
    pub evidence: Value,
    /// Focused instruction passed to the fix worker.
    pub repair_instruction: String,
}

/// Defect severity.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefectSeverity {
    /// Prevents the artifact from progressing.
    Blocker,
    /// Material quality or faithfulness defect.
    Major,
    /// Non-blocking defect retained for scoring.
    Minor,
}

/// Explicit terminal loop condition.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StopReason {
    /// All required gates passed.
    Pass,
    /// The same defect survived another repair.
    SameFailureRepeated,
    /// The defect set stopped strictly improving.
    ScoreNotImproving,
    /// Fire-alarm attempt ceiling reached.
    MaxAttempts,
    /// Deterministic evidence cannot resolve the case.
    HumanReviewRequired,
}

/// Driver action after reading this scorecard.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    /// Advance to the next queued item.
    Advance,
    /// Invoke the schema-constrained fix worker.
    Repair,
    /// Stop and retain the best version.
    Stop,
}

/// Scorecard validation or persistence failure.
#[derive(Debug, Error)]
pub enum ScorecardError {
    /// Scorecard values violate the machine contract.
    #[error("invalid scorecard: {0}")]
    Contract(String),
    /// JSON encoding failed.
    #[error("serializing scorecard: {0}")]
    Json(#[from] serde_json::Error),
    /// Atomic persistence failed.
    #[error("{action} {}: {source}", path.display())]
    Io {
        /// Operation being attempted.
        action: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
}

impl Scorecard {
    /// Validate and atomically write a pretty JSON scorecard.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), ScorecardError> {
        self.validate()?;
        let path = path.as_ref();
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&temporary, bytes).map_err(|source| ScorecardError::Io {
            action: "writing temporary scorecard",
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(|source| ScorecardError::Io {
            action: "renaming scorecard",
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    fn validate(&self) -> Result<(), ScorecardError> {
        if self.scenario_id.is_empty() {
            return Err(ScorecardError::Contract(
                "scenario_id must not be empty".into(),
            ));
        }
        if self.attempt == 0 {
            return Err(ScorecardError::Contract(
                "attempt must be greater than zero".into(),
            ));
        }
        for (name, score) in [("score", self.score), ("style score", self.tiers.style)] {
            if !score.is_finite() || !(0.0..=1.0).contains(&score) {
                return Err(ScorecardError::Contract(format!(
                    "{name} must be finite and between 0 and 1"
                )));
            }
        }
        Ok(())
    }
}
