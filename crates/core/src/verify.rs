//! Verification stack — cheap → expensive, gated milestones.
//!
//! Per `PLAN.md` §9. Three tiers:
//!
//!   - **Tier 1** — every edit, mandatory. Lives inside
//!     `apply_edl` (parse + anchor resolution + asset existence
//!     + frame-range bounds + OTIO round-trip). Runs synchronously
//!     in <100ms. Not in this module.
//!   - **Tier 2** — per feature, mandatory. Triggered when the
//!     agent flips a plan item to `completed`. This module
//!     hosts those checks.
//!   - **Tier 3** — milestones, before "done". Requires a human
//!     in the TUI viewer. Not in this module.
//!
//! The tier-2 surface here is intentionally thin for v1. The plan
//! lists transcript-vs-cut detection and A/V-sync analysis as
//! deliverables; both need real DSP code that's worth real
//! benchmarking. Until then we ship the *framework* + a single
//! cheap check (does the timeline still render without error?)
//! so future verifiers have a place to slot in.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Outcome of running tier-2 verification on a plan item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerifyResult {
    /// All checks passed. The agent can leave the plan item
    /// `completed` and move on.
    Pass {
        /// Per-check verdicts so the agent can see what was run.
        checks: Vec<CheckVerdict>,
    },
    /// At least one check failed. The agent should roll the plan
    /// item back to `pending` (or annotate it `failed`) and
    /// address the diagnostic before re-asserting completion.
    Fail {
        /// Per-check verdicts; at least one is failing.
        checks: Vec<CheckVerdict>,
        /// Aggregated short reason for the rollup. The agent
        /// should surface this to the user verbatim.
        reason: String,
    },
}

/// One verifier's verdict on one plan item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckVerdict {
    /// Stable id (e.g. `timeline_renders`, `transcript_cut`,
    /// `av_sync`). Lets the agent and downstream tooling key
    /// off-results.
    pub id: String,
    /// True if this check passed.
    pub pass: bool,
    /// Diagnostic. On pass: usually empty. On fail: actionable
    /// — what the agent should try next.
    pub message: String,
}

/// Run tier-2 verification on a project. Returns the rolled-up
/// verdict.
///
/// `plan_item` is the agent-visible step text (e.g. "Tighten the
/// intro segment from 0:00 to 0:30") — used only for diagnostic
/// messages, not for picking which checks to run. We always run
/// the full tier-2 suite; the cost difference is negligible
/// for a handful of cheap checks.
pub fn tier_2(project_root: &Path, _plan_item: &str) -> VerifyResult {
    let mut checks = Vec::new();
    checks.push(check_timeline_renders(project_root));
    // Future v1.1 checks (PLAN.md §9.2 — need real-video
    // benchmark data before they're usable):
    //   checks.push(check_transcript_cut_alignment(project_root));
    //   checks.push(check_av_sync_within_40ms(project_root));
    let any_fail = checks.iter().any(|c| !c.pass);
    if any_fail {
        let reason = checks
            .iter()
            .filter(|c| !c.pass)
            .map(|c| format!("[{}] {}", c.id, c.message))
            .collect::<Vec<_>>()
            .join("; ");
        VerifyResult::Fail { checks, reason }
    } else {
        VerifyResult::Pass { checks }
    }
}

/// Cheapest tier-2 check: does the timeline still parse + serialize
/// without error? Catches the case where the agent committed an
/// `apply_edl` op that left the timeline in a malformed state
/// (wrong rate, negative duration, etc.) that tier-1 didn't
/// catch.
///
/// Costs <50ms on a typical project. Always runs.
fn check_timeline_renders(project_root: &Path) -> CheckVerdict {
    let timeline_path = project_root.join("project.otio.json");
    if !timeline_path.exists() {
        return CheckVerdict {
            id: "timeline_renders".into(),
            pass: false,
            message: format!(
                "expected timeline at {} — project may not be initialized",
                timeline_path.display()
            ),
        };
    }
    let mut warnings = Vec::new();
    match awidat_proto::project::read_otio_timeline(&timeline_path, &mut warnings) {
        Ok(_) => CheckVerdict {
            id: "timeline_renders".into(),
            pass: true,
            message: if warnings.is_empty() {
                String::new()
            } else {
                format!("timeline parses with {} schema warning(s)", warnings.len())
            },
        },
        Err(e) => CheckVerdict {
            id: "timeline_renders".into(),
            pass: false,
            message: format!(
                "timeline failed to parse: {e}. The most recent apply_edl probably \
                 wrote a malformed OTIO. Run `awidat validate <project>` for a \
                 detailed diagnostic, then revert the offending edit."
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_timeline_fails_tier_2() {
        let dir = tempfile::tempdir().unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Fail { reason, .. } => {
                assert!(reason.contains("timeline_renders"));
            }
            VerifyResult::Pass { .. } => panic!("empty dir should fail tier-2"),
        }
    }

    #[test]
    fn valid_project_passes_tier_2() {
        let dir = tempfile::tempdir().unwrap();
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Pass { checks } => {
                assert_eq!(checks.len(), 1);
                assert_eq!(checks[0].id, "timeline_renders");
                assert!(checks[0].pass);
            }
            VerifyResult::Fail { reason, .. } => panic!("fresh init should pass; got {reason}"),
        }
    }

    #[test]
    fn malformed_timeline_fails_with_actionable_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        awidat_proto::project::Project::init(dir.path()).unwrap();
        // Corrupt the timeline.
        std::fs::write(
            dir.path().join("project.otio.json"),
            b"{ this is not valid JSON",
        )
        .unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Fail { reason, checks } => {
                assert_eq!(checks.len(), 1);
                let c = &checks[0];
                assert_eq!(c.id, "timeline_renders");
                assert!(!c.pass);
                assert!(c.message.contains("apply_edl"));
                assert!(reason.contains("timeline_renders"));
            }
            VerifyResult::Pass { .. } => panic!("malformed JSON should fail"),
        }
    }
}
