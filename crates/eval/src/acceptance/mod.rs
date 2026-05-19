//! Rendered-output behavioral acceptance scenarios.
//!
//! The acceptance tier is intentionally opt-in. It creates real media,
//! drives the same timeline render path used by the CLI/desktop export,
//! and scores the resulting MP4 with deterministic probes before any
//! semantic judging is introduced.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use awidat_render::{ffmpeg_path, ffprobe_path};

use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

mod artifacts;
pub mod batch;
mod case;
pub mod discover;
mod discover_draft;
mod discover_report;
mod discover_transcript;
pub mod failure_package;
pub mod fixture_manifest;
mod judges;
mod media;
pub mod promotion;
mod runner;
mod scorecard;
mod timeline;

use case::{AcceptanceFixture, load_case, load_case_from_path};
use runner::run_case;

#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod driver_tests;
#[cfg(test)]
mod failure_package_tests;
#[cfg(test)]
mod fixture_manifest_tests;
#[cfg(test)]
mod promotion_tests;
#[cfg(test)]
mod tests;

/// Return all checked-in rendered-output acceptance scenarios.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    let mut scenarios: Vec<Box<dyn Scenario>> = vec![Box::new(AcceptanceScenario {
        scenario_id: "acceptance::synthetic_dead_air_cleanup_render",
        fixture: AcceptanceFixture::CheckedIn("synthetic_dead_air_cleanup.json"),
    })];
    if let Ok(path) = std::env::var("AWIDAT_REAL_ACCEPTANCE_FIXTURE")
        && !path.trim().is_empty()
    {
        match external_fixture_scenarios_from_path(Path::new(&path)) {
            Ok(mut external) => scenarios.append(&mut external),
            Err(err) => scenarios.push(Box::new(AcceptanceDiscoveryFailure {
                message: format!("{err:#}"),
            })),
        }
    }
    scenarios
}

struct AcceptanceScenario {
    scenario_id: &'static str,
    fixture: AcceptanceFixture,
}

struct AcceptanceDiscoveryFailure {
    message: String,
}

#[async_trait]
impl Scenario for AcceptanceScenario {
    fn id(&self) -> &'static str {
        self.scenario_id
    }

    fn description(&self) -> &'static str {
        "Render a scenario MP4 and score deterministic media/timeline hard gates."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        if ffmpeg_path().is_err() || ffprobe_path().is_err() {
            return Ok(ScenarioOutcome {
                id: self.scenario_id.to_string(),
                status: ScenarioStatus::Skipped,
                elapsed: started.elapsed(),
                message: "ffmpeg/ffprobe not found; skipping rendered-output acceptance".into(),
            });
        }

        let case = load_case(&self.fixture)?;
        let run = run_case(&case).await;
        Ok(match run {
            Ok(run) => {
                let status = if run.scorecard.status == "pass" {
                    ScenarioStatus::Pass
                } else {
                    ScenarioStatus::Fail
                };
                let failed = run
                    .scorecard
                    .gates
                    .iter()
                    .filter(|gate| gate.status == "fail")
                    .map(|gate| gate.name.as_str())
                    .collect::<Vec<_>>();
                let message = if failed.is_empty() {
                    format!(
                        "score {:.1}/100; scorecard {}",
                        run.scorecard.score,
                        run.scorecard_path.display()
                    )
                } else {
                    format!(
                        "score {:.1}/100; failed gates [{}]; scorecard {}",
                        run.scorecard.score,
                        failed.join(", "),
                        run.scorecard_path.display()
                    )
                };
                ScenarioOutcome {
                    id: case.id,
                    status,
                    elapsed: started.elapsed(),
                    message,
                }
            }
            Err(err) => ScenarioOutcome {
                id: case.id,
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!("{err:#}"),
            },
        })
    }
}

#[async_trait]
impl Scenario for AcceptanceDiscoveryFailure {
    fn id(&self) -> &'static str {
        "acceptance::external_fixture_discovery"
    }

    fn description(&self) -> &'static str {
        "Report an invalid AWIDAT_REAL_ACCEPTANCE_FIXTURE setting."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        Ok(ScenarioOutcome {
            id: self.id().to_string(),
            status: ScenarioStatus::Fail,
            elapsed: std::time::Duration::ZERO,
            message: self.message.clone(),
        })
    }
}

pub(crate) fn external_fixture_scenarios_from_path(path: &Path) -> Result<Vec<Box<dyn Scenario>>> {
    let paths = external_fixture_paths_from_path(path)?;
    let mut scenarios: Vec<Box<dyn Scenario>> = Vec::new();
    for path in paths {
        let case = load_case_from_path(&path)?;
        let scenario_id = Box::leak(case.id.into_boxed_str());
        scenarios.push(Box::new(AcceptanceScenario {
            scenario_id,
            fixture: AcceptanceFixture::External(path),
        }));
    }
    Ok(scenarios)
}

pub(crate) fn external_fixture_paths_from_path(path: &Path) -> Result<Vec<PathBuf>> {
    let paths = if path.is_file() {
        vec![path.to_path_buf()]
    } else if path.is_dir() {
        let mut files = Vec::new();
        for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry?;
            let path = entry.path();
            if is_runnable_fixture_path(&path) {
                files.push(path);
            }
        }
        files.sort();
        files
    } else {
        bail!(
            "AWIDAT_REAL_ACCEPTANCE_FIXTURE path does not exist: {}",
            path.display()
        );
    };
    Ok(paths)
}

fn is_runnable_fixture_path(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
        return false;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    !file_name.ends_with(".template.json")
        && !file_name.ends_with(".sample.json")
        && !file_name.ends_with(".review.json")
}
