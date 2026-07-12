//! montage-eval CLI. CI invokes `montage-eval --ci --product --golden
//! --json` (see .github/workflows/evals.yml); today the golden gate suite
//! is implemented and other lanes report themselves skipped.

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
