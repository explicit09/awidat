# B-Roll Recommendation Scoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a durable B-roll recommendation layer that converts fused understanding moments into reviewable scored recommendations with category, confidence, strategy, insertion plan, and evidence.

**Architecture:** Extend the professional substrate with a `BrollRecommendationPackage`, then add a deterministic core builder that consumes `UnderstandingAsset` records and optional project b-roll evidence. Expose the result through a read-only tool so agents/UI can review recommendations without placing media.

**Tech Stack:** Rust workspace, `awidat-proto` professional metadata, `awidat-core` understanding and sidecar readers, existing tool registry patterns, focused Cargo tests.

---

## Objectives And Verification

### Objective 4: B-Roll Recommendation Scoring

Build stored, reviewable recommendations with:

- Stable recommendation IDs.
- Source asset and insertion/source ranges.
- Moment category and visual opportunity category.
- Confidence/score and score breakdown.
- Asset strategy: project footage, stock, generated visual, motion graphic, archival/reference.
- Insertion plan: anchor moment, duration, target range, placement style, rationale.
- Evidence IDs linking to fused understanding moments and source signals.

Verification:

- Proto round-trip test proves `BrollRecommendationPackage` serializes/deserializes and metadata validates it.
- Validation catches empty IDs, invalid ranges, score values outside 0..1, missing evidence, and empty insertion plans.
- Core tests prove deterministic recommendations from fused moments.
- Core tests prove category-to-strategy mapping for explanation/statistic/story/company/product/emotional/technical moments.
- Read-only tool test returns recommendations filtered by asset and minimum score without mutating media.

## File Map

- Modify `crates/proto/src/professional.rs`: add `BrollRecommendationPackage`, recommendation, score component, strategy, category, and insertion plan structs/enums plus validation.
- Modify `crates/proto/src/awidat_meta.rs`: add optional package field and validation.
- Modify `crates/proto/tests/professional_substrate.rs`: add round-trip/metadata validation test.
- Create `crates/core/src/broll_recommendations.rs`: deterministic builder from `UnderstandingAsset` records.
- Create `crates/core/tests/broll_recommendations.rs`: builder tests.
- Create `crates/core/src/tools/read_broll_recommendations.rs`: read-only tool with optional `asset_id` and `min_score`.
- Modify `crates/core/src/lib.rs`, `crates/core/src/tools/mod.rs`, `crates/cli/src/chat_cmd.rs`, `crates/cli/src/tui_cmd.rs`, `apps/desktop/src-tauri/src/session.rs`, `crates/core/src/system_prompt.rs`: exports, registration, and prompt hint.

## Tasks

### Task 1: Schema

- [x] Add durable proto structs/enums and validation.
- [x] Add metadata field and validation hook.
- [x] Add round-trip test.
- [x] Run `cargo test -p awidat-proto broll_recommendation_package_round_trips -j1`.

### Task 2: Core Builder

- [x] Add `build_broll_recommendation_package(&[UnderstandingAsset])`.
- [x] Map fused moment categories to visual opportunity categories and asset strategies.
- [x] Add score breakdown and insertion plan.
- [x] Add deterministic ID tests and category mapping tests.

### Task 3: Read Tool

- [x] Add `read_broll_recommendations` tool with `asset_id` and `min_score`.
- [x] Reuse existing `read_understanding` pipeline by building understanding then recommendations.
- [x] Register tool in CLI/TUI/desktop session registries.
- [x] Add read tool test.

### Task 4: Verification

- [x] Run `cargo test -p awidat-proto broll_recommendation_package_round_trips -j1`.
- [x] Run `cargo test -p awidat-core --test broll_recommendations -j1`.
- [x] Run `cargo test -p awidat-core read_broll_recommendations -j1`.
- [x] Run `cargo fmt --all -- --check`.
