# Indexing Performance Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a CLI-first indexing performance review workflow that measures non-transcription indexers in milliseconds and writes JSON plus Markdown reports against explicit targets.

**Architecture:** `awidat-index` owns reusable timing report models built from `IndexReport`. `awidat-cli` owns the `index-perf` command, asset/indexer selection, command metadata, machine metadata, and report file output. Normal `awidat index` output remains unchanged.

**Tech Stack:** Rust, clap, serde/serde_json, existing `awidat_index::PairTelemetry`, targeted cargo tests.

---

### Task 1: Reusable Timing Report Model

**Files:**
- Create: `crates/index/src/perf_report.rs`
- Modify: `crates/index/src/lib.rs`
- Test: `crates/index/src/perf_report.rs`

- [ ] Add `perf_report.rs` with serializable structs for target milliseconds, measured milliseconds, budget status, pair rows, and report summaries.
- [ ] Add unit tests that construct `PairOutcome` values with known `PairTelemetry` and verify milliseconds and target comparison.
- [ ] Re-export the module from `crates/index/src/lib.rs`.
- [ ] Run `cargo test -p awidat-index perf_report -- --nocapture`.

### Task 2: CLI Command and Report Output

**Files:**
- Create: `crates/cli/src/index_perf_cmd.rs`
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/cli/Cargo.toml`
- Test: `crates/cli/src/index_perf_cmd.rs`

- [ ] Add `IndexPerf` CLI args: project path, repeated `--asset`, repeated `--indexer`, repeated `--exclude-indexer`, `--output`, `--concurrency`, and `--include-whisper`.
- [ ] Default exclusions include `whisper` unless `--include-whisper` is passed.
- [ ] Load config, filter indexer servers, collect assets using the same rules as `awidat index`, run `awidat_index::run`, and write `indexing-performance.json` plus `indexing-performance.md`.
- [ ] Add unit tests for default whisper exclusion and Markdown rendering.
- [ ] Run `cargo test -p awidat-cli index_perf -- --nocapture`.

### Task 3: Documentation and Manual Run Recipe

**Files:**
- Create: `docs/indexing-performance.md`

- [ ] Document the command, default non-transcription scope, suggested local video corpus, report outputs, and target interpretation.
- [ ] Include a sample command using `/Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/varied_720p_24fps_30s.mp4`.
- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run targeted tests from Tasks 1 and 2.
