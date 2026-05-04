//! `awidat-eval` — run the scenario suite + print a one-line-per-row
//! report. Exits non-zero if any scenario fails. Intended use: CI smoke
//! check + local regression after large changes.
//!
//! Usage:
//!   awidat-eval                # run regression scenarios
//!   awidat-eval --stress       # run heavy stress scenarios (slow)
//!   awidat-eval --all          # run regression + stress
//!   awidat-eval --list         # print scenario id + description

use std::process::ExitCode;

use awidat_eval::{Scenario, format_report, run_all, scenarios, stress};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want_stress = args.iter().any(|a| a == "--stress");
    let want_all = args.iter().any(|a| a == "--all");
    let want_list = args.iter().any(|a| a == "--list" || a == "-l");

    let mut chosen: Vec<Box<dyn Scenario>> = Vec::new();
    if want_all || (!want_stress) {
        chosen.extend(scenarios::defaults());
    }
    if want_all || want_stress {
        chosen.extend(stress::defaults());
    }

    if want_list {
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
    let (text, exit) = format_report(&outcomes);
    print!("{text}");
    ExitCode::from(exit as u8)
}
