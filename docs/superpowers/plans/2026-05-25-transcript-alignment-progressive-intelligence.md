# Transcript Alignment and Progressive Intelligence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Awidat priorities 1 and 2 as durable product substrate: Transcript Alignment v2 and a Progressive Intelligence Pipeline with objective verification criteria.

**Architecture:** Keep durable project state in `Timeline.metadata.awidat` via `crates/proto`. Add small read-only/mutating helpers in `crates/core` and only expose desktop fields after the protocol can round-trip them. Do not replace the existing Whisper sidecar shape; normalize it into Awidat-owned alignment/readiness contracts that survive correction and timeline edits.

**Tech Stack:** Rust 2024 workspace, `awidat-proto`, `awidat-core`, `awidat-index`, Tauri desktop protocol, existing Python MCP sidecars.

---

## Goal Contract

### Objective 1: Transcript Alignment v2

Awidat must treat transcripts as source-of-truth editing data, not just captions.

**Need:**
- Every transcript word has a deterministic stable `word_id`.
- Every phrase has a deterministic stable `phrase_id`.
- Segment/phrase/word identity is derived from asset id, source-time range, normalized text, and occurrence index so re-reading the same sidecar gives the same ids.
- Transcript correction state is stored separately from the raw Whisper sidecar.
- Corrections can rename/replace text without destroying original text, timestamps, speaker labels, or word ids.
- Transcript edit operations produce reversible records.
- Timeline mapping can answer: source word/phrase range -> timeline spans using current clip source ranges.

**Attained when:**
- Unit tests prove repeated normalization of the same Whisper sidecar yields identical word and phrase ids.
- Unit tests prove two identical words at different source ranges do not collide.
- Unit tests prove phrase grouping is stable across punctuation/silence/speaker boundaries.
- Unit tests prove a correction records original text, corrected text, author/source, time, and reversible state.
- Unit tests prove a corrected transcript still maps back to original source timestamps.
- Unit tests prove a transcript word range maps to timeline spans across trimmed and repeated source clips.
- Desktop transcript parsing can expose ids without breaking old sidecars.

### Objective 2: Progressive Intelligence Pipeline

Awidat must expose one durable per-asset processing state machine instead of scattered readiness checks.

**Need:**
- A per-asset `MediaIntelligenceState` records layers independently: source, proxy, waveform, transcript, speakers, scenes, topics, moments, clip candidates, and b-roll.
- Each layer has status, artifact refs, producer/indexer name, freshness, blocking reason, and updated timestamp.
- Layer state can be rebuilt from existing files and sidecars without running heavyweight model downloads.
- The state supports progressive readiness: source/proxy/waveform can be ready while transcript/scenes/topics/moments are still pending.
- Agent tools can read the state and recommend the next narrow action.
- Desktop can eventually render the same state without inventing a second readiness model.

**Attained when:**
- Unit tests prove a fixture project with only `raw/` media reports source ready and downstream layers pending.
- Unit tests prove adding proxy/waveform/transcript/scene/topic/moment sidecars independently advances only those layers.
- Unit tests prove stale or missing source files block dependent layers without erasing existing artifact evidence.
- Unit tests prove the aggregate asset state is `ready`, `partial`, `processing`, `blocked`, or `offline` based on layer states.
- A read-only tool returns JSON with per-asset layer states and recommended next actions.
- Existing `read_media_readiness` remains compatible while the new state becomes the canonical lower-level source.

## File Map

- Modify: `crates/proto/src/professional.rs`
  - Add transcript alignment and media intelligence durable structs.
- Modify: `crates/proto/src/awidat_meta.rs`
  - Add optional transcript alignment packages and media intelligence packages to `AwidatTimelineMetadata`.
- Add: `crates/core/src/transcript_alignment.rs`
  - Normalize Whisper sidecars into stable word/phrase identities and correction state.
- Add: `crates/core/src/media_intelligence.rs`
  - Build per-asset layer state from raw media, proxies, preview cache artifacts, and index sidecars.
- Add: `crates/core/src/tools/read_media_intelligence.rs`
  - Read-only agent tool for progressive layer state and next actions.
- Modify: `crates/core/src/tools/mod.rs`
  - Export the new tool.
- Modify: `crates/cli/src/chat_cmd.rs`
  - Register the tool.
- Modify: `crates/cli/src/tui_cmd.rs`
  - Register the tool.
- Modify: `apps/desktop/src-tauri/src/session.rs`
  - Register the tool for desktop agent sessions.
- Modify later: `crates/desktop-protocol/src/lib.rs`
  - Add ids to transcript protocol only after core normalization tests pass.
- Modify later: `apps/desktop/src-tauri/src/commands/transcript.rs`
  - Populate optional word/phrase ids from normalization.

## Task 1: Transcript Alignment Schema

**Files:**
- Modify: `crates/proto/src/professional.rs`
- Modify: `crates/proto/src/awidat_meta.rs`
- Test: `crates/proto/tests/professional_substrate.rs`

- [ ] **Step 1: Write failing schema round-trip test**

Add a test proving `TranscriptAlignmentPackage` serializes and validates a word, phrase, correction, and edit record.

Run:

```bash
cargo test -p awidat-proto transcript_alignment_package_round_trips
```

Expected: fail because the types do not exist.

- [ ] **Step 2: Add minimal durable types**

Add:

```rust
pub struct TranscriptAlignmentPackage {
    pub asset_id: String,
    pub source_sha256: Option<String>,
    pub words: Vec<AlignedTranscriptWord>,
    pub phrases: Vec<AlignedTranscriptPhrase>,
    pub corrections: Vec<TranscriptCorrection>,
    pub edit_log: Vec<TranscriptEditRecord>,
}
```

with validation for non-empty ids, valid ranges, unique ids, and correction references.

- [ ] **Step 3: Store package list in Awidat metadata**

Add `transcript_alignments: Vec<TranscriptAlignmentPackage>` to `AwidatTimelineMetadata` with `#[serde(default)]`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-proto transcript_alignment professional_substrate
```

## Task 2: Stable Transcript Normalizer

**Files:**
- Add: `crates/core/src/transcript_alignment.rs`
- Modify: `crates/core/src/lib.rs`
- Test: `crates/core/tests/transcript_alignment.rs`

- [ ] **Step 1: Write failing identity tests**

Cover:
- Same sidecar -> same word ids.
- Repeated word text at different times -> different ids.
- Punctuation/silence/speaker changes split phrases.
- Phrase ids stay stable when unrelated later phrases change.

Run:

```bash
cargo test -p awidat-core transcript_alignment_ids_are_stable
```

Expected: fail because the module does not exist.

- [ ] **Step 2: Implement minimal normalizer**

Implement:

```rust
pub fn normalize_whisper_alignment(
    asset_id: &str,
    sidecar: &serde_json::Value,
    options: PhraseGroupingOptions,
) -> Result<TranscriptAlignmentPackage, TranscriptAlignmentError>
```

Use deterministic ids shaped like:

```text
word:<asset_hash>:<occurrence>:<start_ms>:<end_ms>:<text_hash>
phrase:<asset_hash>:<first_word_occurrence>:<last_word_occurrence>:<text_hash>
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p awidat-core transcript_alignment
```

## Task 3: Correction and Reversible Edit State

**Files:**
- Modify: `crates/core/src/transcript_alignment.rs`
- Test: `crates/core/tests/transcript_alignment.rs`

- [ ] **Step 1: Write failing correction tests**

Cover:
- Correcting one word keeps `word_id`, timestamps, speaker, and original text.
- Reverting the correction restores display text.
- A correction references a known word or phrase id.

Run:

```bash
cargo test -p awidat-core transcript_correction_is_reversible
```

- [ ] **Step 2: Implement correction helpers**

Implement:

```rust
pub fn apply_transcript_correction(
    package: &mut TranscriptAlignmentPackage,
    correction: TranscriptCorrection,
) -> Result<TranscriptEditRecord, TranscriptAlignmentError>

pub fn revert_transcript_edit(
    package: &mut TranscriptAlignmentPackage,
    edit_id: &str,
) -> Result<(), TranscriptAlignmentError>
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p awidat-core transcript_correction
```

## Task 4: Source Transcript to Timeline Mapping

**Files:**
- Modify: `crates/core/src/transcript_alignment.rs`
- Test: `crates/core/tests/transcript_alignment.rs`

- [ ] **Step 1: Write failing timeline mapping tests**

Cover:
- A word inside a trimmed clip maps to one timeline span.
- A word in a repeated source range maps to multiple timeline spans.
- A word outside all current clips maps to no spans.

Run:

```bash
cargo test -p awidat-core transcript_source_range_maps_to_timeline
```

- [ ] **Step 2: Implement mapper**

Implement:

```rust
pub fn map_transcript_range_to_timeline(
    timeline: &awidat_proto::otio::Timeline,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
) -> Vec<TranscriptTimelineSpan>
```

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p awidat-core transcript_source_range_maps_to_timeline
```

## Task 5: Progressive Intelligence Schema

**Files:**
- Modify: `crates/proto/src/professional.rs`
- Modify: `crates/proto/src/awidat_meta.rs`
- Test: `crates/proto/tests/professional_substrate.rs`

- [ ] **Step 1: Write failing media intelligence round-trip test**

The test must create one asset with layer states for source, proxy, waveform, transcript, speakers, scenes, topics, moments, clip candidates, and b-roll.

Run:

```bash
cargo test -p awidat-proto media_intelligence_package_round_trips
```

- [ ] **Step 2: Add durable media intelligence types**

Add:

```rust
pub struct MediaIntelligencePackage {
    pub assets: Vec<MediaIntelligenceAsset>,
}

pub struct MediaIntelligenceAsset {
    pub asset_id: String,
    pub layers: Vec<MediaIntelligenceLayer>,
    pub aggregate_state: MediaIntelligenceAggregateState,
    pub recommended_actions: Vec<String>,
}
```

Use enum layer kinds and states, not free-form strings.

- [ ] **Step 3: Store package in Awidat metadata**

Add `media_intelligence: Option<MediaIntelligencePackage>` to `AwidatTimelineMetadata`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-proto media_intelligence professional_substrate
```

## Task 6: Progressive Intelligence Builder

**Files:**
- Add: `crates/core/src/media_intelligence.rs`
- Modify: `crates/core/src/lib.rs`
- Test: `crates/core/tests/media_intelligence.rs`

- [ ] **Step 1: Write failing fixture tests**

Cover:
- Raw-only project: source ready, proxy/waveform/transcript/scenes/topics/moments/clip-candidates/b-roll pending.
- Adding sidecars advances only matching layers.
- Missing raw source produces offline/blocked aggregate while preserving sidecar artifact refs.

Run:

```bash
cargo test -p awidat-core media_intelligence_raw_only_is_partial
```

- [ ] **Step 2: Implement builder**

Implement:

```rust
pub fn build_media_intelligence_package(
    project_root: &std::path::Path,
) -> Result<MediaIntelligencePackage, MediaIntelligenceError>
```

Read existing `raw/`, `.awidat/proxies`, `.awidat/waveforms` or preview-cache artifacts, and `index/<indexer>/<asset>.json` sidecars. Do not run model-backed indexers.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p awidat-core media_intelligence
```

## Task 7: Agent Read Tool

**Files:**
- Add: `crates/core/src/tools/read_media_intelligence.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: `crates/cli/src/chat_cmd.rs`
- Modify: `crates/cli/src/tui_cmd.rs`
- Modify: `apps/desktop/src-tauri/src/session.rs`
- Test: `crates/core/tests/media_intelligence_tool.rs`

- [ ] **Step 1: Write failing tool test**

The tool must return JSON with `assets`, per-layer states, aggregate state, and recommended actions.

Run:

```bash
cargo test -p awidat-core read_media_intelligence_returns_layers
```

- [ ] **Step 2: Implement read-only tool**

Tool name: `read_media_intelligence`.

Inputs:

```json
{
  "asset_id": "optional project-relative raw asset id"
}
```

Behavior:
- Build package from disk.
- Filter by `asset_id` if present.
- Return JSON.
- Never mutate files.

- [ ] **Step 3: Register tool**

Register alongside `read_media_readiness`, `start_indexing`, `proxy_status`, and transcript tools.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-core read_media_intelligence media_intelligence
```

## Task 8: Desktop Protocol Additive Exposure

**Files:**
- Modify: `crates/desktop-protocol/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/commands/transcript.rs`
- Test: `crates/desktop-protocol`

- [ ] **Step 1: Write failing protocol test**

Prove `TranscriptWord` can carry an optional id and old JSON without ids still deserializes.

Run:

```bash
cargo test -p awidat-desktop-protocol transcript_word_ids_are_additive
```

- [ ] **Step 2: Add optional id fields**

Add optional `word_id` to `TranscriptWord` and optional `segment_id` to `TranscriptSegment`. Do not require phrase ids in the first UI pass unless the core normalizer already exposes phrase rows.

- [ ] **Step 3: Populate ids in transcript command**

When reading a whisper sidecar, normalize it and attach ids to matching words/segments. If normalization fails, return the old transcript shape rather than breaking transcript display.

- [ ] **Step 4: Export TypeScript protocol**

Run:

```bash
AWIDAT_EXPORT_TS=1 cargo test -p awidat-desktop-protocol
```

## Final Verification

Run narrow checks first:

```bash
cargo test -p awidat-proto transcript_alignment media_intelligence professional_substrate
cargo test -p awidat-core transcript_alignment media_intelligence read_media_intelligence
cargo test -p awidat-desktop-protocol transcript_word_ids_are_additive
```

Then broader checks if the narrow checks pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Non-Goals

- Do not re-run model-backed indexers as part of readiness building.
- Do not change the Whisper sidecar schema in Python for this pass.
- Do not build a new desktop UI for clip candidates yet.
- Do not add analytics/retention feedback in this goal.
- Do not make transcript correction depend on cloud services.
