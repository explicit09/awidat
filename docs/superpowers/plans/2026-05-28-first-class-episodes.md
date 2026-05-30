# First-Class Episodes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make detected podcast episodes durable, reviewable, navigable project objects in Awidat instead of temporary detector output.

**Architecture:** Store accepted/rejected/review-needed episode spans in `Timeline.metadata.awidat.episodes`. Keep `podcast_episode_spans` read-only, add mutating `apply_episode_spans` to persist reviewed spans, and add `list_episodes` for agents and desktop UI. Improve the Python span planner after the data model exists so better detection does not change downstream contracts.

**Tech Stack:** Rust workspace (`awidat-proto`, `awidat-core` MCP tools), Python planner script, Tauri/React desktop UI, OTIO JSON metadata.

---

### Task 1: Durable Episode Metadata

**Files:**
- Modify: `crates/proto/src/awidat_meta.rs`
- Test: `crates/proto/tests/professional_substrate.rs`

- [ ] **Step 1: Add failing round-trip test**

Add a test that builds `AwidatTimelineMetadata` with three episode records: one accepted, one review-needed, and one rejected. Serialize to JSON, deserialize, and assert `id`, `name`, `order`, `asset_id`, source range, confidence, status, and evidence survive.

Run:

```bash
cargo test -p awidat-proto first_class_episode_metadata_roundtrips -- --nocapture
```

Expected: fails because `EpisodeSpan` and `episodes` do not exist.

- [ ] **Step 2: Add metadata types**

Add:

```rust
pub episodes: Vec<EpisodeSpan>
```

to `AwidatTimelineMetadata`, plus:

```rust
pub struct EpisodeSpan {
    pub id: String,
    pub name: Option<String>,
    pub order: Option<u32>,
    pub asset_id: String,
    pub source_start_s: f64,
    pub source_end_s: f64,
    pub confidence: Option<f64>,
    pub status: EpisodeSpanStatus,
    pub evidence: Vec<String>,
    pub extra: HashMap<String, serde_json::Value>,
}
```

and `EpisodeSpanStatus::{ReviewNeeded, Accepted, Rejected}` with serde rename values `review_needed`, `accepted`, and `rejected`.

- [ ] **Step 3: Add validation diagnostics**

Extend `validate_professional_substrate` to flag:
- empty `id`
- empty `asset_id`
- `source_end_s <= source_start_s`
- `confidence` outside `[0, 1]`
- duplicate episode ids

- [ ] **Step 4: Run proto verification**

Run:

```bash
cargo fmt --all -- --check
cargo test -p awidat-proto first_class_episode_metadata_roundtrips -- --nocapture
cargo test -p awidat-proto episode_metadata_validation_flags_invalid_ranges -- --nocapture
```

Expected: all pass.

### Task 2: Read/Write Episode Tools

**Files:**
- Create: `crates/core/src/awidat_mcp/tools/list_episodes.rs`
- Create: `crates/core/src/awidat_mcp/tools/apply_episode_spans.rs`
- Modify: `crates/core/src/awidat_mcp/tools/mod.rs`
- Modify: `crates/core/src/awidat_mcp/mod.rs`
- Optional legacy parity: `crates/core/src/tools/list_episodes.rs`, `crates/core/src/tools/apply_episode_spans.rs`, `crates/core/src/tools/mod.rs`

- [ ] **Step 1: Add `list_episodes`**

Implement a read-only MCP tool returning JSON:

```json
{
  "status": "ready",
  "total": 2,
  "episodes": [
    {
      "id": "episode-1",
      "name": "Founder story",
      "order": 1,
      "asset_id": "raw/interview.mov",
      "source_start_s": 72.01,
      "source_end_s": 2405.57,
      "duration_s": 2333.56,
      "confidence": 0.92,
      "status": "accepted",
      "evidence": ["intro_language", "outro_language"]
    }
  ]
}
```

- [ ] **Step 2: Add `apply_episode_spans`**

Implement a mutating MCP tool that accepts:

```json
{
  "episodes": [
    {
      "id": "episode-1",
      "name": "Founder story",
      "order": 1,
      "asset_id": "raw/interview.mov",
      "source_start_s": 72.01,
      "source_end_s": 2405.57,
      "confidence": 0.92,
      "status": "accepted",
      "evidence": ["intro_language", "outro_language"]
    }
  ],
  "replace": true
}
```

`replace=true` replaces existing metadata episodes. `replace=false` upserts by `id`.

- [ ] **Step 3: Register tools**

Register both tools in `crates/core/src/awidat_mcp/tools/mod.rs` and `crates/core/src/awidat_mcp/mod.rs`. Mark `list_episodes` read-only and `apply_episode_spans` destructive/mutating.

- [ ] **Step 4: Add focused tests**

Use temp `Project::init`, write episodes via `apply_episode_spans`, read them with `list_episodes`, and assert order/status/ranges.

Run:

```bash
cargo test -p awidat-core episode_tools -- --nocapture
```

Expected: tests pass.

### Task 3: Hybrid Multi-Episode Detection

**Files:**
- Modify: `skills/auto-cutter/scripts/episode_span_plan.py`
- Test: create fixtures under `skills/auto-cutter/tests/fixtures/`
- Test: create `skills/auto-cutter/tests/test_episode_span_plan.py`

- [ ] **Step 1: Add fixture tests**

Create four transcript/audio fixtures:
- single clean episode
- two real episodes separated by production talk
- repeated rehearsed intros before one real episode
- useful close followed by post-show chatter and sustained silence

Run:

```bash
python3 -m unittest skills.auto-cutter.tests.test_episode_span_plan
```

Expected: fails until planner is upgraded.

- [ ] **Step 2: Add backward/forward validation**

Score candidate starts with:
- +0.2 if prior 60s has >50% silence
- +0.2 if prior 60s has meta-talk
- +0.2 if next 5m has sustained speech
- +0.1 if next 5m has no meta-talk

- [ ] **Step 3: Add weighted meta-talk**

Add strong/medium/weak vocabulary modeled on `../video-editor/VideoEditor/Packages/EditorCore/Sources/EditorCore/Analysis/EpisodeBoundaryDetector.swift`, but keep it configurable in Python constants.

- [ ] **Step 4: Add end fallback hierarchy**

End search order:
1. sustained meta-talk window with content-resume lookahead
2. explicit outro/closing markers
3. sustained silence after at least 3 minutes of content
4. next start or recording end

- [ ] **Step 5: Verify planner**

Run:

```bash
python3 -m unittest discover skills/auto-cutter/tests
cargo test -p awidat-core podcast_episode_spans -- --nocapture
```

Expected: all pass and existing MCP output schema remains compatible.

### Task 4: Navigable Episode Representation

**Files:**
- Modify: `crates/core/src/awidat_mcp/tools/apply_episode_spans.rs`
- Optional: integrate with `SourceSelect`/`Stringout` metadata in `crates/proto/src/awidat_meta.rs`
- Test: `crates/core/tests/episodes.rs`

- [ ] **Step 1: Link accepted episodes to source ranges**

When `create_stringouts=true`, create one `SourceSelect` per accepted episode and one `Stringout` that preserves episode order. Use stable ids derived from the episode id.

- [ ] **Step 2: Preserve rejected/review-needed episodes**

Rejected and review-needed spans remain in `episodes`, but do not become selects/stringouts unless explicitly accepted.

- [ ] **Step 3: Verify multi-episode project state**

Run:

```bash
cargo test -p awidat-core episodes_create_stringout_for_accepted_spans -- --nocapture
```

Expected: accepted episodes create ordered source selects; rejected spans stay metadata-only.

### Task 5: Desktop Episode Rollup

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/project.rs` or a focused command module
- Modify: `apps/desktop/src/App.tsx` or extracted episode panel component
- Modify generated protocol only through the existing generation path
- Test: `apps/desktop/tests/desktop-ui-smoke.mjs`

- [ ] **Step 1: Add backend command**

Expose project episodes from OTIO metadata to the frontend as typed JSON: `accepted`, `review_needed`, and `rejected` groups.

- [ ] **Step 2: Add UI panel**

Add an Episodes rollup that displays:
- name/order
- status
- source start/end/duration
- confidence
- evidence count

- [ ] **Step 3: Add smoke coverage**

Add fixture project metadata with multiple episodes and assert the panel renders grouped episode rows.

Run:

```bash
pnpm --dir apps/desktop test
node apps/desktop/tests/desktop-ui-smoke.mjs
```

Expected: desktop smoke sees accepted, review-needed, and rejected episode rows.

---

## Completion Gates

- `EpisodeSpan` metadata exists and round-trips through OTIO JSON.
- `apply_episode_spans` persists detector output; `list_episodes` reads it back.
- Planner fixtures prove fewer false starts and better multi-episode endings.
- Accepted episodes can become navigable ordered source ranges/stringouts.
- Desktop UI groups and displays episodes by status.
- Narrow tests for each layer pass; broader `make check` or scoped workspace checks run before final completion if disk/build budget allows.
