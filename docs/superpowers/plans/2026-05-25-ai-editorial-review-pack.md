# AI Editorial Review Pack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move podcast cleanup and episode-shape decisions away from brittle tool labels by giving the active AI compact transcript/timeline evidence packets and an explicit editorial classification schema before any cut proposal is made.

**Architecture:** Deterministic scanners remain recall mechanisms. A new read-only tool packages suspicious timeline windows with before/during/after transcript context, source/timeline mapping, signals, and instructions for the active AI to classify each packet as `cut`, `keep`, or `review`. Existing proposal/apply tools continue to mutate only after reviewed decisions.

**Tech Stack:** Rust `awidat-core` tool, existing OTIO project/timeline types, existing whisper/silence sidecars, current desktop and CLI tool registries.

---

- [ ] Add `podcast_editorial_review_pack` tool module.
  - File: `crates/core/src/tools/podcast_editorial_review_pack.rs`
  - Build read-only packets from timeline-visible evidence.
  - Input: `max_results`, optional `window_padding_s`, optional `include_dead_air`.
  - Output fields: `summary_for_agent`, `classification_schema`, `agent_instructions`, `packets`, `missing_evidence`.
  - Each packet includes `id`, `asset_id`, `source_start_s`, `source_end_s`, `timeline_start_s`, `timeline_end_s`, `signals`, `transcript_before`, `transcript_during`, `transcript_after`, and `review_question`.
  - Success: The tool never says a packet is a final cut; it only asks for AI classification.

- [ ] Reuse existing cleanup recall signals without trusting them as final editorial decisions.
  - Use `find_false_starts::scan_false_starts` for restart/production-aside recall.
  - Use `find_dead_air::scan_dead_air` only as a signal when `include_dead_air` is true.
  - Do not place dead-air packets into safe-cut buckets.
  - Success: silence appears as context/evidence, not an automatic edit label.

- [ ] Add transcript-window extraction around source ranges.
  - Load whisper words or segments from `index/whisper/<asset_id>.json`.
  - Split context into before/during/after using `window_padding_s`.
  - Keep text compact and deterministic for model review.
  - Success: the Yusuff-style coaching range includes the surrounding spoken context, including “you can just say” and “We don’t have to”.

- [ ] Register the tool everywhere agents can use it.
  - File: `crates/core/src/tools/mod.rs`
  - File: `apps/desktop/src-tauri/src/session.rs`
  - File: `crates/cli/src/chat_cmd.rs`
  - File: `crates/cli/src/tui_cmd.rs`
  - Success: desktop and CLI sessions expose `podcast_editorial_review_pack`.

- [ ] Update podcast workflow guidance.
  - File: `skills/podcast-episode-producer/SKILL.md`
  - File: `skills/auto-cutter/SKILL.md`
  - Require `podcast_editorial_review_pack` before proposing cleanup cuts when transcript evidence exists.
  - Clarify that scanners surface evidence; the AI must decide editorial meaning.
  - Success: future podcast runs classify false starts, production asides, and dead-air candidates in context before cutting.

- [ ] Add focused tests.
  - Unit test: synthetic transcript with “you can just say” and “We don’t have to” produces a review packet with those phrases in context.
  - Unit test: output includes classification schema and agent instructions.
  - Unit test: dead-air packet has a silence signal but no final cut action.
  - Success: `cargo test -p awidat-core podcast_editorial_review_pack -j1` passes.

- [ ] Verify formatting and commit strategically.
  - Run `cargo fmt --all -- --check`.
  - Run focused core tests.
  - Commit message: `Add AI editorial review pack`.
  - Success: clean working tree after commit.
