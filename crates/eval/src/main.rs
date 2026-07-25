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

    if let Some(project_root) = value_of("--taste-lower") {
        return run_taste_lower(&args, &project_root);
    }

    if let Some(ground_path) = value_of("--taste") {
        return run_taste(&args, &ground_path);
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
/// `montage-eval --taste-lower <project_root> --taste-raw <substr>
///   --taste-house <house> --taste-pair-id <id> --taste-raw-duration <s>`
///
/// Lower an edited project's timeline into a proposed decision list over
/// one raw asset (printed as JSON) — pipe into a file and score it with
/// `--taste ... --taste-proposed ...`. This is the bridge that makes the
/// agent's actual edits taste-scoreable.
#[allow(clippy::print_stderr)]
fn run_taste_lower(args: &[String], project_root: &str) -> ExitCode {
    use montage_eval::taste::decision_list_from_timeline;

    let value_of = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|idx| args.get(idx + 1))
            .cloned()
    };

    let (Some(raw_ref), Some(house), Some(pair_id), Some(duration)) = (
        value_of("--taste-raw"),
        value_of("--taste-house"),
        value_of("--taste-pair-id"),
        value_of("--taste-raw-duration"),
    ) else {
        eprintln!(
            "montage-eval --taste-lower requires --taste-raw <substr> \
             --taste-house <house> --taste-pair-id <id> --taste-raw-duration <seconds>"
        );
        return ExitCode::FAILURE;
    };
    let Ok(raw_duration_s) = duration.parse::<f64>() else {
        eprintln!("montage-eval: --taste-raw-duration '{duration}' is not a number");
        return ExitCode::FAILURE;
    };

    let otio_path = PathBuf::from(project_root).join(montage_proto::project::files::OTIO);
    let mut warnings = Vec::new();
    let timeline = match montage_proto::project::read_otio_timeline(&otio_path, &mut warnings) {
        Ok(timeline) => timeline,
        Err(e) => {
            eprintln!("montage-eval: reading {}: {e}", otio_path.display());
            return ExitCode::FAILURE;
        }
    };
    for warning in &warnings {
        eprintln!("montage-eval: schema warning: {warning:?}");
    }

    match decision_list_from_timeline(&timeline, &raw_ref, raw_duration_s, &house, &pair_id) {
        Ok(list) => match serde_json::to_string_pretty(&list) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("montage-eval: serializing decision list: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("montage-eval: lowering failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `montage-eval --taste <ground_truth.json>
///   (--taste-proposed <proposed.json> | --taste-baseline keep_all)`
///
/// Scores a proposed decision list against professional ground truth
/// (docs/taste-gate-plan-2026-07-25.md Phase B) and prints the
/// `TasteScore` JSON. `--taste-baseline keep_all` scores the
/// keep-everything floor instead of a proposed file.
#[allow(clippy::print_stderr)]
fn run_taste(args: &[String], ground_path: &str) -> ExitCode {
    use montage_eval::taste::{DecisionList, keep_everything_baseline, score};

    let value_of = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|idx| args.get(idx + 1))
            .cloned()
    };

    let ground = match DecisionList::from_json_file(ground_path) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("montage-eval: loading ground truth {ground_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let proposed = match (value_of("--taste-proposed"), value_of("--taste-baseline")) {
        (Some(path), None) => match DecisionList::from_json_file(&path) {
            Ok(list) => list,
            Err(e) => {
                eprintln!("montage-eval: loading proposed list {path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        (None, Some(name)) if name == "keep_all" => keep_everything_baseline(&ground),
        (None, Some(other)) => {
            eprintln!("montage-eval: unknown --taste-baseline '{other}' (expected keep_all)");
            return ExitCode::FAILURE;
        }
        _ => {
            eprintln!(
                "montage-eval --taste requires exactly one of \
                 --taste-proposed <file> or --taste-baseline keep_all"
            );
            return ExitCode::FAILURE;
        }
    };

    match score(&ground, &proposed) {
        Ok(taste_score) => match serde_json::to_string_pretty(&taste_score) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("montage-eval: serializing taste score: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("montage-eval: taste scoring failed: {e}");
            ExitCode::FAILURE
        }
    }
}

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
    // Canonicalize a path-like binary argument: the runner spawns with
    // `current_dir` set to the scenario's project root, so a relative path
    // like `./target/debug/codex-exec` would otherwise resolve inside the
    // project directory and fail with a misleading "No such file" error.
    // A bare command name (no separator) is left alone for PATH lookup.
    let codex_exec_binary = value_of("--ab-codex-exec").unwrap_or_else(|| "codex-exec".into());
    let codex_exec_binary = if codex_exec_binary.contains(std::path::MAIN_SEPARATOR) {
        match std::fs::canonicalize(&codex_exec_binary) {
            Ok(p) => p.display().to_string(),
            Err(e) => {
                eprintln!("montage-eval: resolving --ab-codex-exec {codex_exec_binary}: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        codex_exec_binary
    };
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

    // Snapshot the project's mutable state once, up front. Attempts edit
    // the project on disk (apply_edl rewrites project.otio.json, renders
    // land in renders/), so without a per-attempt restore every attempt
    // after the first starts from the previous attempt's edits and the
    // paired comparison is confounded — arm B would be "cleaning up" a
    // project arm A already cleaned. Large read-only inputs (raw/, index/,
    // proxies/) are excluded from the snapshot: agents consume them but
    // the sanctioned tool surface never rewrites them.
    let snapshot_dir = PathBuf::from(&runs_root).join(format!("pristine-{}", scenario.id));
    if let Err(e) = snapshot_project(&scenario.source, &snapshot_dir) {
        eprintln!(
            "montage-eval: snapshotting project {}: {e}",
            scenario.source.display()
        );
        return ExitCode::FAILURE;
    }

    let source = scenario.source.clone();
    let reset = move |config_name: &str, trial: u32| {
        if let Err(e) = restore_project(&snapshot_dir, &source) {
            // A failed restore invalidates every later attempt; abort hard
            // rather than silently comparing contaminated runs.
            panic!("restoring project before {config_name} trial {trial}: {e}");
        }
    };

    let runner = SubprocessCodexExecRunner;
    let driver = AbDriver {
        runner: &runner,
        codex_exec_binary: PathBuf::from(codex_exec_binary),
        profile: None,
        archetype: None,
        pre_attempt: Some(&reset),
    };

    // Score each attempt against the project state the agent left behind:
    // `output_for_trial` runs after the attempt and before the next
    // attempt's reset, so the OTIO on disk at that moment is this
    // attempt's product. Cut-time extraction for the pacing gates is not
    // wired yet, so those gates stay skipped.
    let project_otio = scenario.source.join("project.otio.json");
    let (a_attempts, b_attempts, comparison) =
        match driver.run_scenario(&manifest, &scenario, |_config_name, _trial| AttemptOutput {
            otio_path: Some(project_otio.clone()),
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

/// Directories excluded from the A/B project snapshot: large read-only
/// inputs the sanctioned tool surface never rewrites. Everything else at
/// the project's top level (project.otio.json, edl/, .montage/, renders/,
/// edit-plan.json, …) is mutable session state and gets snapshotted.
const SNAPSHOT_EXCLUDES: &[&str] = &["raw", "index", "proxies"];

/// Copy the project's mutable state into `snapshot_dir` (replacing any
/// prior snapshot).
fn snapshot_project(
    project: &std::path::Path,
    snapshot_dir: &std::path::Path,
) -> std::io::Result<()> {
    if snapshot_dir.exists() {
        std::fs::remove_dir_all(snapshot_dir)?;
    }
    std::fs::create_dir_all(snapshot_dir)?;
    for entry in std::fs::read_dir(project)? {
        let entry = entry?;
        let name = entry.file_name();
        if SNAPSHOT_EXCLUDES
            .iter()
            .any(|e| name.to_string_lossy() == *e)
        {
            continue;
        }
        copy_recursive(&entry.path(), &snapshot_dir.join(&name))?;
    }
    Ok(())
}

/// Restore the project's mutable state from `snapshot_dir`: every
/// non-excluded top-level entry currently in the project is removed, then
/// the snapshot is copied back. Excluded read-only dirs are left in place.
fn restore_project(
    snapshot_dir: &std::path::Path,
    project: &std::path::Path,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(project)? {
        let entry = entry?;
        let name = entry.file_name();
        if SNAPSHOT_EXCLUDES
            .iter()
            .any(|e| name.to_string_lossy() == *e)
        {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    for entry in std::fs::read_dir(snapshot_dir)? {
        let entry = entry?;
        copy_recursive(&entry.path(), &project.join(entry.file_name()))?;
    }
    Ok(())
}

/// Minimal recursive copy (files + dirs + symlink targets); the snapshot
/// subset is small session state, so no need for anything fancier.
fn copy_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}
