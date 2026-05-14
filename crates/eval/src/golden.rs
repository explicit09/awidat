//! Golden edit runner.
//!
//! Golden cases are small JSON fixtures that describe a synthetic
//! project, an editorial objective, the EDL an agent/editor should
//! produce, and structural expectations after applying it. They are
//! intentionally media-free so `--golden` can run in CI.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use awidat_core::edl::{AnchorContext, apply, parse};
use awidat_proto::project::Project;
use serde::Deserialize;
use std::time::Instant;

use crate::fixtures::{
    ClipSpec, clip_count, clip_range_by_uuid, write_asset, write_project_with_clips,
};
use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

/// Return all checked-in golden edit scenarios.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    vec![Box::new(GoldenCaseScenario {
        fixture_name: "trim_dead_air.json",
    })]
}

struct GoldenCaseScenario {
    fixture_name: &'static str,
}

#[async_trait]
impl Scenario for GoldenCaseScenario {
    fn id(&self) -> &'static str {
        "golden::trim_dead_air_cut"
    }

    fn description(&self) -> &'static str {
        "Apply a JSON-defined golden EDL and compare the resulting tiny timeline to expected structure."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let case = load_case(self.fixture_name)?;
        let result = run_case(&case).map(|msg| ScenarioOutcome {
            id: case.id.clone(),
            status: ScenarioStatus::Pass,
            elapsed: started.elapsed(),
            message: msg,
        });
        Ok(match result {
            Ok(outcome) => outcome,
            Err(err) => ScenarioOutcome {
                id: case.id,
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!("{err:#}"),
            },
        })
    }
}

fn load_case(name: &str) -> Result<GoldenCase> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("golden")
        .join(name);
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn run_case(case: &GoldenCase) -> Result<String> {
    case.validate()?;
    let dir = tempfile::tempdir()?;
    let clips: Vec<ClipSpec> = case
        .project
        .clips
        .iter()
        .map(|clip| {
            ClipSpec::video(
                &clip.name,
                &clip.uuid,
                &clip.asset,
                clip.start_s,
                clip.duration_s,
            )
            .with_transcript(&clip.transcript)
        })
        .collect();
    write_project_with_clips(dir.path(), &case.project.name, &clips)?;
    for asset in &case.project.assets {
        write_asset(dir.path(), asset)?;
    }

    for forbidden in &case.expect.forbidden_ops {
        if case.edl.contains(&format!("*** {forbidden}")) {
            bail!("forbidden op {forbidden:?} appeared in EDL");
        }
    }

    let envelope = parse(&case.edl).context("parse golden EDL")?;
    let project = Project::read(dir.path())?;
    let ctx = AnchorContext::with_project_root(dir.path());
    let (timeline, outcome) =
        apply(&project.timeline, &envelope, &ctx).context("apply golden EDL")?;

    if let Some(expected) = case.expect.clip_count
        && clip_count(&timeline) != expected
    {
        bail!("expected {expected} clips, got {}", clip_count(&timeline));
    }

    let tolerance = case.expect.tolerance_s.unwrap_or(0.01);
    for expected in &case.expect.clip_ranges {
        let Some((start_s, duration_s)) = clip_range_by_uuid(&timeline, &expected.uuid) else {
            bail!("expected clip uuid {:?} not found", expected.uuid);
        };
        if (start_s - expected.start_s).abs() > tolerance {
            bail!(
                "clip {} start_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.start_s,
                start_s
            );
        }
        if (duration_s - expected.duration_s).abs() > tolerance {
            bail!(
                "clip {} duration_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.duration_s,
                duration_s
            );
        }
    }

    for required in &case.expect.applied_contains {
        if !outcome
            .applied
            .iter()
            .any(|applied| applied.description.contains(required))
        {
            bail!("applied-op log did not contain {required:?}");
        }
    }

    Ok(format!(
        "{}: {} op(s) matched expected structure",
        case.objective,
        outcome.applied.len()
    ))
}

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    objective: String,
    project: GoldenProject,
    edl: String,
    expect: GoldenExpect,
}

impl GoldenCase {
    fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("golden case id is empty");
        }
        if self.objective.trim().is_empty() {
            bail!("golden case objective is empty");
        }
        if self.project.clips.is_empty() {
            bail!("golden case has no input clips");
        }
        if self.edl.trim().is_empty() {
            bail!("golden case EDL is empty");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct GoldenProject {
    name: String,
    #[serde(default)]
    assets: Vec<String>,
    clips: Vec<GoldenClip>,
}

#[derive(Debug, Deserialize)]
struct GoldenClip {
    name: String,
    uuid: String,
    asset: String,
    start_s: f64,
    duration_s: f64,
    transcript: String,
}

#[derive(Debug, Deserialize)]
struct GoldenExpect {
    #[serde(default)]
    tolerance_s: Option<f64>,
    #[serde(default)]
    clip_count: Option<usize>,
    #[serde(default)]
    clip_ranges: Vec<ExpectedClipRange>,
    #[serde(default)]
    applied_contains: Vec<String>,
    #[serde(default)]
    forbidden_ops: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedClipRange {
    uuid: String,
    start_s: f64,
    duration_s: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_golden_case_runs() {
        let case = load_case("trim_dead_air.json").unwrap();
        run_case(&case).unwrap();
    }
}
