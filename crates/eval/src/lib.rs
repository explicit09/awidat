//! Editorial benchmark / regression harness.
//!
//! Per the cross-harness survey: aider, swe-agent, cline, roo-code, continue,
//! and goose all ship eval suites — they're the closed-loop signal that tells
//! you a model/tool change made things better or worse. We didn't have one.
//! This crate is the V1: a scenario-driven runner with three V1 scenario
//! types — **structural**, **tool-eval**, and **agent-eval** (gated on
//! ANTHROPIC_API_KEY).
//!
//! V1 ships only structural + tool-eval scenarios because:
//! 1. They run in CI without burning API budget.
//! 2. They cover the regression surface that breaks most often (tool
//!    schemas, EDL anchor matching, render dispatch).
//! 3. Editorial-quality eval needs golden cuts, which we don't have yet —
//!    that's a Week 8 + #149 follow-up.
//!
//! Shape ported from `harnesses/aider/benchmark/benchmark.py` (the
//! scenario runner pattern), with the validated cross-harness convention
//! of: scenarios are declarative, runner is generic, report is one-line
//! per scenario + a summary footer. We do not need pandas / typer / git
//! integration that aider's full version carries — those are repo-level
//! polyglot-benchmark concerns, not editorial concerns.

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::time::{Duration, Instant};

pub mod scenarios;
pub mod stress;

/// Outcome of one scenario run.
#[derive(Debug, Clone)]
pub struct ScenarioOutcome {
    /// Stable id (matches `Scenario::id`).
    pub id: String,
    /// Pass / fail / skipped.
    pub status: ScenarioStatus,
    /// Wall-clock duration. Surfaced in the report so a slow scenario
    /// that's also passing still gets visibility.
    pub elapsed: Duration,
    /// One-line message — the failure cause, or a checkmark phrase
    /// for passes.
    pub message: String,
}

/// Pass/fail/skip per scenario. Skipped is for scenarios whose
/// preconditions weren't met (e.g. agent-eval with no API key) — they
/// shouldn't fail the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioStatus {
    /// All assertions passed.
    Pass,
    /// At least one assertion failed.
    Fail,
    /// Preconditions not met — counted toward neither pass nor fail.
    Skipped,
}

/// One scenario. Implementors live in `scenarios/`. Each scenario is
/// self-contained: it builds its own fixture (usually via
/// `awidat_test_support::fixture::project`), runs whatever it needs to,
/// and returns a verdict.
#[async_trait]
pub trait Scenario: Send + Sync {
    /// Stable id used in reports and CLI filtering.
    fn id(&self) -> &'static str;

    /// One-line description shown in the listing.
    fn description(&self) -> &'static str;

    /// True iff this scenario needs an Anthropic API key. Used by the
    /// runner to skip agent-eval scenarios when `ANTHROPIC_API_KEY`
    /// isn't set, instead of failing them.
    fn needs_api_key(&self) -> bool {
        false
    }

    /// Run the scenario and report. The implementor should return
    /// `Ok(ScenarioOutcome { status, ... })`; reserve `Err` for
    /// fixture-bringup failures that can't be expressed as a Fail
    /// (e.g. tempdir creation refused).
    async fn run(&self) -> Result<ScenarioOutcome>;
}

/// Run a list of scenarios sequentially, capture per-scenario timing,
/// and return the full list. Sequential is intentional — some
/// scenarios touch shared resources (ffmpeg, MCP processes); parallel
/// runs would need a coordinator we don't have for V1.
pub async fn run_all(scenarios: &[Box<dyn Scenario>]) -> Vec<ScenarioOutcome> {
    let mut out = Vec::with_capacity(scenarios.len());
    let has_api_key = std::env::var("ANTHROPIC_API_KEY")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    for s in scenarios {
        let id = s.id().to_string();
        if s.needs_api_key() && !has_api_key {
            out.push(ScenarioOutcome {
                id,
                status: ScenarioStatus::Skipped,
                elapsed: Duration::ZERO,
                message: "ANTHROPIC_API_KEY not set; skipping agent-eval".into(),
            });
            continue;
        }
        let started = Instant::now();
        let outcome = match s.run().await {
            Ok(o) => o,
            Err(e) => ScenarioOutcome {
                id: id.clone(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!("scenario bringup failed: {e:#}"),
            },
        };
        out.push(outcome);
    }
    out
}

/// Format a scenario list as a one-line-per-row report with a summary.
/// Returns the formatted text + an exit-code hint (0 if all pass-or-
/// skip, 1 if any fail).
pub fn format_report(outcomes: &[ScenarioOutcome]) -> (String, i32) {
    use std::fmt::Write as _;
    let mut s = String::new();
    let pass = outcomes
        .iter()
        .filter(|o| o.status == ScenarioStatus::Pass)
        .count();
    let fail = outcomes
        .iter()
        .filter(|o| o.status == ScenarioStatus::Fail)
        .count();
    let skip = outcomes
        .iter()
        .filter(|o| o.status == ScenarioStatus::Skipped)
        .count();

    for o in outcomes {
        let tag = match o.status {
            ScenarioStatus::Pass => "PASS",
            ScenarioStatus::Fail => "FAIL",
            ScenarioStatus::Skipped => "SKIP",
        };
        let _ = writeln!(
            s,
            "[{tag}] {id:<32} {ms:>6}ms  {msg}",
            id = o.id,
            ms = o.elapsed.as_millis(),
            msg = o.message,
        );
    }
    let _ = writeln!(
        s,
        "\n{pass} passed, {fail} failed, {skip} skipped — total {total}",
        total = outcomes.len()
    );
    let exit = if fail > 0 { 1 } else { 0 };
    (s, exit)
}

/// Resolve a tempdir-rooted project relative to its tempdir guard. A
/// helper to keep scenarios short.
pub fn project_path(dir: &tempfile::TempDir) -> &Path {
    dir.path()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysPass;
    #[async_trait]
    impl Scenario for AlwaysPass {
        fn id(&self) -> &'static str {
            "always-pass"
        }
        fn description(&self) -> &'static str {
            "trivial pass"
        }
        async fn run(&self) -> Result<ScenarioOutcome> {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed: Duration::from_millis(1),
                message: "ok".into(),
            })
        }
    }

    struct AlwaysFail;
    #[async_trait]
    impl Scenario for AlwaysFail {
        fn id(&self) -> &'static str {
            "always-fail"
        }
        fn description(&self) -> &'static str {
            "trivial fail"
        }
        async fn run(&self) -> Result<ScenarioOutcome> {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed: Duration::from_millis(1),
                message: "expected fail".into(),
            })
        }
    }

    #[tokio::test]
    async fn runner_aggregates_outcomes() {
        let scenarios: Vec<Box<dyn Scenario>> = vec![Box::new(AlwaysPass), Box::new(AlwaysFail)];
        let outcomes = run_all(&scenarios).await;
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].status, ScenarioStatus::Pass);
        assert_eq!(outcomes[1].status, ScenarioStatus::Fail);
    }

    #[test]
    fn report_exit_code_reflects_failures() {
        let outcomes = vec![
            ScenarioOutcome {
                id: "a".into(),
                status: ScenarioStatus::Pass,
                elapsed: Duration::from_millis(1),
                message: "ok".into(),
            },
            ScenarioOutcome {
                id: "b".into(),
                status: ScenarioStatus::Fail,
                elapsed: Duration::from_millis(1),
                message: "broke".into(),
            },
        ];
        let (text, exit) = format_report(&outcomes);
        assert_eq!(exit, 1);
        assert!(text.contains("PASS"));
        assert!(text.contains("FAIL"));
        assert!(text.contains("1 passed, 1 failed"));
    }

    #[test]
    fn all_pass_exits_zero() {
        let outcomes = vec![ScenarioOutcome {
            id: "a".into(),
            status: ScenarioStatus::Pass,
            elapsed: Duration::ZERO,
            message: "ok".into(),
        }];
        let (_, exit) = format_report(&outcomes);
        assert_eq!(exit, 0);
    }

    /// Verifies the no-API-key skip path. We can only test the skip
    /// branch deterministically; if a developer happens to have
    /// `ANTHROPIC_API_KEY` set, this test self-skips rather than
    /// panicking — a real runner-level skip test belongs in CI where
    /// the env is controlled.
    #[tokio::test]
    async fn agent_eval_without_key_is_skipped_not_failed() {
        let has_key = std::env::var("ANTHROPIC_API_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if has_key {
            // Test self-skip: we can't safely run a scenario body that
            // expects no key when the env actually has one. Coverage
            // for the has-key path lives in agent-eval scenarios.
            return;
        }
        struct AgentScenario;
        #[async_trait]
        impl Scenario for AgentScenario {
            fn id(&self) -> &'static str {
                "agent-x"
            }
            fn description(&self) -> &'static str {
                "needs API key"
            }
            fn needs_api_key(&self) -> bool {
                true
            }
            async fn run(&self) -> Result<ScenarioOutcome> {
                unreachable!("runner must skip when key absent")
            }
        }
        let scenarios: Vec<Box<dyn Scenario>> = vec![Box::new(AgentScenario)];
        let outcomes = run_all(&scenarios).await;
        assert_eq!(outcomes[0].status, ScenarioStatus::Skipped);
    }
}
