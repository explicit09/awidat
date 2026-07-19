//! montage-eval CLI. CI invokes `montage-eval --ci --product --golden
//! --json` (see .github/workflows/evals.yml); today the golden gate suite
//! is implemented and other lanes report themselves skipped.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

#[derive(Serialize)]
struct CliReport {
    golden: Option<Vec<montage_eval::suite::SuiteResult>>,
    lanes: Vec<LaneReport>,
}

#[derive(Serialize)]
struct LaneReport {
    lane: &'static str,
    status: LaneStatus,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum LaneStatus {
    Skipped,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let json = has("--json");
    let value_of = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|idx| args.get(idx + 1))
            .cloned()
    };

    if let Some(scenario_path) = value_of("--ab") {
        return run_ab(&args, &scenario_path);
    }

    let mut lanes = Vec::new();
    for lane in ["--product", "--stress", "--live"] {
        if has(lane) {
            lanes.push(LaneReport {
                lane: lane.trim_start_matches("--"),
                status: LaneStatus::Skipped,
                reason: "lane runner is not implemented yet",
            });
        }
    }

    let fixtures = format!("{}/fixtures", env!("CARGO_MANIFEST_DIR"));
    let golden = if has("--golden") {
        match montage_eval::suite::run_golden(&fixtures) {
            Ok(results) => Some(results),
            Err(e) => {
                eprintln!("montage-eval: golden suite error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    let regressed = golden.as_ref().is_some_and(|rs| rs.iter().any(|r| !r.ok));

    let report = CliReport { golden, lanes };
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("montage-eval: serializing report: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(results) = &report.golden {
        for r in results {
            let mark = if r.ok { "ok" } else { "REGRESSED" };
            println!("{mark:9} {} — {}", r.case, r.detail);
        }
    }

    if !json {
        for lane in &report.lanes {
            println!("skipped   {} — {}", lane.lane, lane.reason);
        }
    }

    if regressed || (has("--fail-on-skip") && !report.lanes.is_empty()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `montage-eval --ab <scenario.yaml> --ab-manifest <manifest.toml>
/// [--ab-codex-exec <path>] [--ab-runs-root <dir>]`
///
/// Runs one scenario through both configs in the manifest against a real
/// `codex-exec` binary (`SubprocessCodexExecRunner`) — this is the only
/// code path in this crate that spawns a real process; everything under
/// `cargo test -p montage-eval` uses `FakeCodexExecRunner` instead, per
/// the constraint that building/testing this crate must never invoke a
/// live model. This lane has no gate scoring wired to a real project yet:
/// it reports the paired telemetry comparison and writes artifacts, but
/// `AttemptOutput` (the OTIO/cuts evidence tier-1/pacing gates need) must
/// be supplied by the caller once a real project-output convention exists
/// — see `ab_driver::ScoredAttempt::measurable` doc comment for the
/// broader gap.
#[allow(clippy::print_stderr)]
fn run_ab(args: &[String], scenario_path: &str) -> ExitCode {
    use montage_eval::RunArtifacts;
    use montage_eval::Scenario;
    use montage_eval::ab_driver::{
        AbDriver, AbManifest, AttemptOutput, SubprocessCodexExecRunner, write_attempt_artifacts,
        write_comparison,
    };

    let value_of = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|idx| args.get(idx + 1))
            .cloned()
    };

    let Some(manifest_path) = value_of("--ab-manifest") else {
        eprintln!("montage-eval --ab requires --ab-manifest <manifest.toml>");
        return ExitCode::FAILURE;
    };
    let codex_exec_binary = value_of("--ab-codex-exec").unwrap_or_else(|| "codex-exec".into());
    let runs_root = value_of("--ab-runs-root").unwrap_or_else(|| "ab-runs".into());

    let scenario = match Scenario::from_yaml_file(scenario_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("montage-eval: loading scenario {scenario_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let manifest = match AbManifest::from_toml_file(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("montage-eval: loading A/B manifest {manifest_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let runner = SubprocessCodexExecRunner;
    let driver = AbDriver {
        runner: &runner,
        codex_exec_binary: PathBuf::from(codex_exec_binary),
        profile: None,
        archetype: None,
    };

    // No project-output convention is wired yet, so every attempt scores
    // against an empty AttemptOutput (telemetry-only comparison). See the
    // function doc comment.
    let (a_attempts, b_attempts, comparison) =
        match driver.run_scenario(&manifest, &scenario, |_config_name, _trial| AttemptOutput {
            otio_path: None,
            cut_times: None,
            duration_secs: None,
        }) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("montage-eval: A/B run failed: {e}");
                return ExitCode::FAILURE;
            }
        };

    // `RunArtifacts::create_attempt` numbers attempts within one run id
    // with no config dimension, so config A and config B — which each
    // start numbering trials at 1 — need separate run ids to avoid
    // colliding on the same `attempt_N` directory.
    let base_run_id = format!("ab-{}", scenario.id);
    for (config_name, attempts) in [
        (&manifest.config_a.name, &a_attempts),
        (&manifest.config_b.name, &b_attempts),
    ] {
        let run_id = format!("{base_run_id}-{config_name}");
        let run = match RunArtifacts::create(&runs_root, &run_id, &scenario) {
            Ok(run) => run,
            Err(e) => {
                eprintln!("montage-eval: creating run artifacts for {config_name}: {e}");
                return ExitCode::FAILURE;
            }
        };
        for attempt in attempts.iter() {
            if let Err(e) = write_attempt_artifacts(&run, attempt) {
                eprintln!("montage-eval: writing attempt artifacts for {config_name}: {e}");
                return ExitCode::FAILURE;
            }
        }
        if let Err(e) = write_comparison(&run, &comparison) {
            eprintln!("montage-eval: writing comparison under {config_name}: {e}");
            return ExitCode::FAILURE;
        }
    }

    match serde_json::to_string_pretty(&comparison) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("montage-eval: serializing comparison: {e}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
