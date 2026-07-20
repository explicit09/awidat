//! Feed a captured `codex-exec --json` stream through `parse_telemetry`,
//! printing the rolled-up counters. Debugging aid for the A/B lane: when a
//! rollup looks wrong (e.g. zero tool calls on a mutated project), run the
//! attempt's persisted `raw_stream.jsonl` through this to separate parser
//! bugs from genuine agent behavior.
//!
//! Usage: `cargo run -p montage-eval --example parse_check -- <file.jsonl>`

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: parse_check <file.jsonl>");
        return ExitCode::FAILURE;
    };
    let jsonl = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("reading {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match montage_eval::ab_driver::parse_telemetry(&jsonl) {
        Ok(t) => {
            println!("{t:#?}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("PARSE ERROR: {e}");
            ExitCode::FAILURE
        }
    }
}
