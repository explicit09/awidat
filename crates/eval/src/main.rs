//! `awidat-eval` — run the scenario suite + print a one-line-per-row
//! report. Exits non-zero if any scenario fails. Intended use: CI smoke
//! check + local regression after large changes.
//!
//! Usage:
//!   awidat-eval                # run all V1 scenarios
//!   awidat-eval --list         # print scenario id + description
//!
//! The `--list` flag is the discoverability surface — given the small
//! number of V1 scenarios we don't need a filter mechanism yet. When
//! the count crosses ~10, add `--filter <substring>`.

use std::process::ExitCode;

use awidat_eval::{format_report, run_all, scenarios};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list" || a == "-l") {
        for s in scenarios::defaults() {
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
    let outcomes = runtime.block_on(async { run_all(&scenarios::defaults()).await });
    let (text, exit) = format_report(&outcomes);
    print!("{text}");
    ExitCode::from(exit as u8)
}
