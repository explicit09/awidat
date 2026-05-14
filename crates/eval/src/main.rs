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
//!   awidat-eval --all          # run all non-live tiers
//!   awidat-eval --list         # print scenario id + description

use std::process::ExitCode;

use awidat_eval::{
    JsonReport, ReportOptions, Scenario, format_report_with_options, golden, run_all, scenarios,
    stress,
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
    let want_all = args.iter().any(|a| a == "--all");
    let want_list = args.iter().any(|a| a == "--list" || a == "-l");
    let want_json = args.iter().any(|a| a == "--json");
    let fail_on_skip = args.iter().any(|a| a == "--fail-on-skip");

    let mut chosen: Vec<Box<dyn Scenario>> = Vec::new();
    let mut tiers: Vec<&'static str> = Vec::new();

    let explicit_tier = want_ci || want_product || want_golden || want_stress || want_live;
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
  awidat-eval [--ci] [--product] [--golden] [--stress] [--live] [--all]
              [--list] [--json] [--fail-on-skip]

TIERS:
  --ci       Fast deterministic offline scenarios for every PR.
  --product  Local product-quality scenarios using synthetic sidecars.
  --golden   JSON-defined golden edit/cut fixtures.
  --stress   Slow stress scenarios.
  --live     Real corpus/API-gated scenarios; skips without configured fixtures.
  --all      CI + product + golden + stress. Live stays opt-in.

Default with no tier flags runs CI + product + golden.
"
    );
}
