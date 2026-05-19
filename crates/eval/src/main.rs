//! `awidat-eval` — run the scenario suite + print a one-line-per-row
//! report. Exits non-zero if any scenario fails. Intended use: CI smoke
//! check + local regression after large changes.
//!
//! Usage:
//!   awidat-eval                # run regression scenarios
//!   awidat-eval --ci           # run fast deterministic CI tier
//!   awidat-eval --product      # run product-quality local tier
//!   awidat-eval --golden       # run golden edit fixture tier
//!   awidat-eval --stress       # run heavy stress scenarios (slow)
//!   awidat-eval --live         # run real-corpus / API-gated scenarios
//!   awidat-eval --acceptance   # run rendered-output media acceptance scenarios
//!   awidat-eval --acceptance-discover <dir>
//!                                # scan real videos for acceptance fixture candidates
//!               [--acceptance-discover-write-drafts <dir>]
//!   awidat-eval --acceptance-fixture-manifest <fixture-or-dir>
//!                                # summarize runnable acceptance fixtures
//!   awidat-eval --all          # run all non-live tiers
//!   awidat-eval --list         # print scenario id + description

use std::path::Path;
use std::process::ExitCode;

use awidat_eval::{
    JsonReport, ReportOptions, Scenario,
    acceptance::{
        self,
        batch::run_fixture_batch,
        discover::{
            DiscoveryOptions, discover_video_candidates, format_discovery_report,
            write_fixture_drafts,
        },
        failure_package::package_failure,
        fixture_manifest::build_fixture_manifest,
        promotion::{PromotionOptions, promote_fixture},
    },
    format_report_with_options, golden, run_all, scenarios, stress,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let want_ci = args.iter().any(|a| a == "--ci");
    let want_product = args.iter().any(|a| a == "--product");
    let want_golden = args.iter().any(|a| a == "--golden");
    let want_stress = args.iter().any(|a| a == "--stress");
    let want_live = args.iter().any(|a| a == "--live");
    let want_acceptance = args.iter().any(|a| a == "--acceptance");
    let want_all = args.iter().any(|a| a == "--all");
    let want_list = args.iter().any(|a| a == "--list" || a == "-l");
    let want_json = args.iter().any(|a| a == "--json");
    let fail_on_skip = args.iter().any(|a| a == "--fail-on-skip");
    let acceptance_discover = match arg_value(&args, "--acceptance-discover") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let acceptance_discover_write_drafts =
        match arg_value(&args, "--acceptance-discover-write-drafts") {
            Ok(value) => value,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        };
    let acceptance_fixture_manifest = match arg_value(&args, "--acceptance-fixture-manifest") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let acceptance_batch_summary = match arg_value(&args, "--acceptance-batch-summary") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let acceptance_package_failure = match arg_value(&args, "--acceptance-package-failure") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let acceptance_promote_fixture = match arg_value(&args, "--acceptance-promote-fixture") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let promote_to = match arg_value(&args, "--to") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let review_note = match arg_value(&args, "--review-note") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let promotion_reason = match arg_value(&args, "--promotion-reason") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let promotion_manifest = match arg_value(&args, "--acceptance-discovery-manifest") {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let force = args.iter().any(|arg| arg == "--force");

    if let Some(root) = acceptance_discover {
        let options = DiscoveryOptions::default();
        let report = match discover_video_candidates(Path::new(root), &options) {
            Ok(report) => report,
            Err(err) => {
                eprintln!("{err:#}");
                return ExitCode::from(2);
            }
        };
        if want_json {
            match serde_json::to_string_pretty(&report) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("failed to encode discovery JSON report: {err}");
                    return ExitCode::from(2);
                }
            }
        } else {
            print!("{}", format_discovery_report(&report));
        }
        if let Some(output_dir) = acceptance_discover_write_drafts {
            let write_report = match write_fixture_drafts(&report, Path::new(output_dir)) {
                Ok(write_report) => write_report,
                Err(err) => {
                    eprintln!("{err:#}");
                    return ExitCode::from(2);
                }
            };
            eprintln!(
                "fixture drafts: wrote {}, skipped existing {} in {}; manifest {}",
                write_report.written.len(),
                write_report.skipped_existing.len(),
                output_dir,
                write_report.manifest_path.display()
            );
        }
        return ExitCode::SUCCESS;
    }
    if acceptance_discover_write_drafts.is_some() {
        eprintln!("--acceptance-discover-write-drafts requires --acceptance-discover <dir>");
        return ExitCode::from(2);
    }
    if let Some(path) = acceptance_fixture_manifest {
        let manifest = match build_fixture_manifest(Path::new(path)) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!("{err:#}");
                return ExitCode::from(2);
            }
        };
        if want_json {
            match serde_json::to_string_pretty(&manifest) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("failed to encode fixture manifest JSON report: {err}");
                    return ExitCode::from(2);
                }
            }
        } else {
            println!(
                "fixtures: {}; generated EDL: {}; literal EDL: {}; transcript checks: {}; source-range checks: {}",
                manifest.fixture_count,
                manifest.generated_edl_count,
                manifest.literal_edl_count,
                manifest.transcript_check_count,
                manifest.source_range_check_count
            );
            for fixture in manifest.fixtures {
                println!(
                    "  {:<58} {} {}",
                    fixture.id, fixture.edl_source, fixture.objective
                );
            }
        }
        return ExitCode::SUCCESS;
    }
    if let Some(path) = acceptance_batch_summary {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("failed to build tokio runtime: {e}");
                return ExitCode::from(2);
            }
        };
        let batch_run = match runtime.block_on(async { run_fixture_batch(Path::new(path)).await }) {
            Ok(batch_run) => batch_run,
            Err(err) => {
                eprintln!("{err:#}");
                return ExitCode::from(2);
            }
        };
        if want_json {
            match serde_json::to_string_pretty(&batch_run.summary) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("failed to encode acceptance batch summary JSON: {err}");
                    return ExitCode::from(2);
                }
            }
        } else {
            println!(
                "acceptance batch: {} passed, {} warned, {} failed, {} total",
                batch_run.summary.passed,
                batch_run.summary.warned,
                batch_run.summary.failed,
                batch_run.summary.fixture_count
            );
            println!("  JSON: {}", batch_run.summary_json_path.display());
            println!("  Markdown: {}", batch_run.summary_markdown_path.display());
        }
        return if batch_run.summary.failed > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        };
    }
    if let Some(path) = acceptance_package_failure {
        let packet = match package_failure(Path::new(path), None) {
            Ok(packet) => packet,
            Err(err) => {
                eprintln!("{err:#}");
                return ExitCode::from(2);
            }
        };
        if want_json {
            match serde_json::to_string_pretty(&packet) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("failed to encode failure packet JSON: {err}");
                    return ExitCode::from(2);
                }
            }
        } else {
            println!("failure packet: {}", packet.packet_root.display());
            println!("  manifest: {}", packet.manifest_path.display());
            println!("  failing gates: {}", packet.failing_gates.len());
        }
        return ExitCode::SUCCESS;
    }
    if let Some(draft_path) = acceptance_promote_fixture {
        let Some(review_note) = review_note else {
            eprintln!("--acceptance-promote-fixture requires --review-note <text>");
            return ExitCode::from(2);
        };
        let Some(promotion_reason) = promotion_reason else {
            eprintln!("--acceptance-promote-fixture requires --promotion-reason <text>");
            return ExitCode::from(2);
        };
        let default_output = Path::new("target")
            .join("awidat-eval")
            .join("local-fixtures")
            .join("promoted-real-behavioral");
        let output_dir = promote_to.map(Path::new);
        let output_dir = output_dir.unwrap_or(default_output.as_path());
        let manifest_path = promotion_manifest.map(Path::new);
        let promotion = match promote_fixture(
            Path::new(draft_path),
            output_dir,
            PromotionOptions {
                reviewer_note: review_note,
                promotion_reason,
                discovery_manifest_path: manifest_path,
                force,
            },
        ) {
            Ok(promotion) => promotion,
            Err(err) => {
                eprintln!("{err:#}");
                return ExitCode::from(2);
            }
        };
        if want_json {
            match serde_json::to_string_pretty(&promotion) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("failed to encode promotion JSON: {err}");
                    return ExitCode::from(2);
                }
            }
        } else {
            println!(
                "promoted fixture: {}",
                promotion.promoted_fixture_path.display()
            );
            println!("  metadata: {}", promotion.review_metadata_path.display());
            println!("  scenario_type: {}", promotion.scenario_type);
        }
        return ExitCode::SUCCESS;
    }

    let mut chosen: Vec<Box<dyn Scenario>> = Vec::new();
    let mut tiers: Vec<&'static str> = Vec::new();

    let explicit_tier =
        want_ci || want_product || want_golden || want_stress || want_live || want_acceptance;
    if want_all || want_ci || !explicit_tier {
        chosen.extend(scenarios::fast());
        tiers.push("ci");
    }
    if want_all || want_product || !explicit_tier {
        chosen.extend(scenarios::product());
        tiers.push("product");
    }
    if want_all || want_golden || !explicit_tier {
        chosen.extend(golden::defaults());
        tiers.push("golden");
    }
    if want_all || want_stress {
        chosen.extend(stress::defaults());
        tiers.push("stress");
    }
    if want_live {
        chosen.extend(scenarios::real_corpus());
        tiers.push("live");
    }
    if want_acceptance {
        chosen.extend(acceptance::defaults());
        tiers.push("acceptance");
    }

    if want_list {
        println!("Selected tiers: {}", tiers.join(", "));
        for s in &chosen {
            println!("  {:<46}  {}", s.id(), s.description());
        }
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to build tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    let outcomes = runtime.block_on(async { run_all(&chosen).await });
    let (text, exit) = format_report_with_options(&outcomes, ReportOptions { fail_on_skip });
    if want_json {
        match serde_json::to_string_pretty(&JsonReport::new(tiers, &outcomes)) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("failed to encode JSON report: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print!("{text}");
    }
    ExitCode::from(exit as u8)
}

fn print_help() {
    println!(
        "\
awidat-eval

USAGE:
  awidat-eval [--ci] [--product] [--golden] [--stress] [--live] [--acceptance] [--all]
              [--list] [--json] [--fail-on-skip]
  awidat-eval --acceptance-discover <dir> [--json]
              [--acceptance-discover-write-drafts <dir>]
  awidat-eval --acceptance-fixture-manifest <fixture-or-dir> [--json]
  awidat-eval --acceptance-batch-summary <fixture-or-dir> [--json]
  awidat-eval --acceptance-package-failure <scorecard-or-run-dir> [--json]
  awidat-eval --acceptance-promote-fixture <draft-fixture>
              [--to <local-dir>] --review-note <text> --promotion-reason <text>
              [--acceptance-discovery-manifest <fixture_drafts_manifest.json>] [--force] [--json]

TIERS:
  --ci       Fast deterministic offline scenarios for every PR.
  --product  Local product-quality scenarios using synthetic sidecars.
  --golden   JSON-defined golden edit/cut fixtures.
  --stress   Slow stress scenarios.
  --live     Real corpus/API-gated scenarios; skips without configured fixtures.
  --acceptance
             Rendered-output behavioral acceptance scenarios; opt-in because they create media artifacts.
  --acceptance-discover <dir>
             Scan a real-video directory and rank source clips for acceptance fixture authoring.
  --acceptance-discover-write-drafts <dir>
             Write discovered fixture_draft objects to local JSON files without overwriting existing files.
  --acceptance-fixture-manifest <fixture-or-dir>
             Summarize runnable acceptance fixture coverage without rendering.
  --acceptance-batch-summary <fixture-or-dir>
             Run a local acceptance fixture file or directory and write batch_summary.json/md.
  --acceptance-package-failure <scorecard-or-run-dir>
             Copy compact failure-review artifacts into target/awidat-eval/failure-packets.
  --acceptance-promote-fixture <draft-fixture>
             Copy a reviewed draft into a durable local fixture directory with review metadata.
  --all      CI + product + golden + stress. Live and acceptance stay opt-in.

Default with no tier flags runs CI + product + golden.
"
    );
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    let Some(value) = args.get(index + 1) else {
        return Err(format!("{flag} requires a directory path"));
    };
    if value.starts_with("--") {
        return Err(format!("{flag} requires a directory path"));
    }
    Ok(Some(value))
}
