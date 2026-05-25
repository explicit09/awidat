# Understanding And Clip Candidates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build pipeline priorities 3 and 5: a consolidated scene/moment understanding sidecar plus stored, reviewable short-form clip candidates with score explanations and assembly metadata.

**Architecture:** Add durable protocol records under the professional substrate, then implement a read-only core builder that fuses existing sidecars instead of running new models. Clip candidates are derived from fused understanding records and persisted/served as structured records that agents and UI can inspect before assembly.

**Tech Stack:** Rust workspace, `awidat-proto` professional metadata, `awidat-core` sidecar readers/builders/tools, existing `awidat-index` sidecar layout, focused Cargo tests.

---

## Objectives And Verification

### Objective 3: Scene/Moment Fusion Sidecar

Build a consolidated understanding package per asset with:

- Stable understanding IDs and scene IDs.
- Source range for each fused record.
- Evidence references from transcript, scenes, speakers, topics, audio energy, and editorial moments.
- Human-readable labels, notes, and confidence values.
- Missing-evidence reporting that does not block partial understanding.

Verification:

- Proto round-trip test proves `UnderstandingPackage` survives JSON serialization.
- Validation catches empty IDs, invalid ranges, duplicate IDs, and empty evidence on ready fused records.
- Core tests build deterministic fused records from whisper, scenedetect, topic, audio-energy, and editorial-moments sidecars.
- Core tests prove missing sidecars produce partial output with `missing_evidence`, not a panic.

### Objective 5: Clip Candidate Product Layer

Build stored/reviewable short-form clip candidates with:

- Stable candidate IDs.
- Candidate range and intended platform/duration metadata.
- Score, score breakdown, and explanation.
- Evidence links back to fused understanding/moment/scene records.
- One-click assembly metadata: source asset, source range, suggested aspect ratio, caption style, hook text, and required setup moments.

Verification:

- Proto round-trip test proves `ClipCandidatePackage` survives JSON serialization.
- Validation catches invalid ranges, duplicate candidate IDs, missing evidence, and score breakdown values outside 0..1.
- Core tests derive deterministic candidates from high-scoring fused moments.
- Core tests prove candidates include score explanations and assembly metadata.
- Read-only tool test exposes candidates by optional `asset_id` without mutating source media.

## File Map

- Modify `crates/proto/src/professional.rs`: add `UnderstandingPackage`, fused scene/moment/evidence records, `ClipCandidatePackage`, candidate score and assembly structs, validation methods.
- Modify `crates/proto/src/awidat_meta.rs`: add optional understanding and clip candidate packages to `AwidatTimelineMetadata` and validate them.
- Modify `crates/proto/tests/professional_substrate.rs`: add round-trip tests for both packages.
- Create `crates/core/src/understanding.rs`: read existing sidecars and build fused understanding records deterministically.
- Create `crates/core/src/clip_candidates.rs`: derive scored candidate records from understanding records.
- Modify `crates/core/src/lib.rs`: export new modules.
- Create `crates/core/tests/understanding.rs`: focused fusion and candidate tests.
- Create `crates/core/src/tools/read_understanding.rs`: read-only tool returning fused understanding and clip candidates.
- Modify `crates/core/src/tools/mod.rs`, `crates/cli/src/chat_cmd.rs`, `crates/cli/src/tui_cmd.rs`, `apps/desktop/src-tauri/src/session.rs`, `crates/core/src/system_prompt.rs`: register and describe the tool.

## Tasks

### Task 1: Durable Schema

- [ ] Add proto structs and validation for fused understanding and clip candidates.
- [ ] Add metadata fields and validation hooks.
- [ ] Add proto round-trip tests.
- [ ] Run `cargo test -p awidat-proto understanding clip_candidate`.

### Task 2: Fused Understanding Builder

- [ ] Add deterministic sidecar readers for whisper, scenedetect, topic, audio-energy, and editorial-moments.
- [ ] Build fused records centered on editorial moments, with scene/topic/audio/transcript evidence attached when present.
- [ ] Record missing evidence as data.
- [ ] Add tests for complete evidence, missing evidence, and stable IDs.

### Task 3: Clip Candidate Builder

- [ ] Derive candidates from fused moments using conservative score heuristics: hook/emotional peak/question/answer/story/punchline score high; tangents/dead-air score low.
- [ ] Include score breakdown and explanation strings.
- [ ] Include one-click assembly metadata that points to source asset/range and suggests captions/aspect ratio.
- [ ] Add tests for deterministic IDs, explanation, and assembly metadata.

### Task 4: Read Tool And Registries

- [ ] Add `read_understanding` read-only tool with optional `asset_id`.
- [ ] Register it in CLI, TUI, and desktop session tool registries.
- [ ] Add a tool test proving filtered output and no mutation.
- [ ] Update system prompt raw lookup section.

### Task 5: Verification

- [ ] Run `cargo test -p awidat-proto understanding clip_candidate`.
- [ ] Run `cargo test -p awidat-core --test understanding`.
- [ ] Run `cargo test -p awidat-core read_understanding`.
- [ ] Run formatting on touched files only with `rustfmt --edition 2024` if needed.

