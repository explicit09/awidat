# Pro Editing Gap Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the confirmed Timeline Editing and Audio Editing gaps from `.reference-research/pro-editing-gap-analysis` rows 02 and 03 with tested, modular EDL/apply/render/desktop support.

**Architecture:** Add the smallest reusable primitives at the data-contract layer first, then lower them through parser/apply/render paths. Keep timeline editing operations in `crates/core/src/edl`, render-only behavior in `crates/render/src/timeline.rs` or focused helper modules if extraction is needed, and desktop snapping in `apps/desktop/src/timeline` without mixing UI state with EDL semantics.

**Tech Stack:** Rust 2024 workspace, serde OTIO/professional protocol structs, FFmpeg filter graph lowering, Tauri desktop React/Vite TypeScript, existing cargo tests and focused frontend checks.

---

## File Structure

- Modify `crates/core/src/edl/op.rs`: extend typed EDL contracts for marker CRUD, snapping options, multicam apply, nested stack ops if retained, and audio FX fields.
- Modify `crates/core/src/edl/parser.rs`: parse new EDL fields and JSON payloads without ad hoc string formats.
- Modify `crates/core/src/edl/apply.rs`: implement timeline mutations and tests for marker CRUD, snap-aware placement, nested wrapping/flattening, and atomic multicam apply.
- Modify `crates/proto/src/otio/nodes.rs`: only if nested stack metadata or marker identity needs schema support beyond existing OTIO types.
- Modify `crates/proto/src/professional.rs`: only if multicam decisions or per-clip automation need durable typed professional metadata.
- Modify `crates/render/src/timeline.rs`: lower nested stacks, per-clip volume automation, pan/balance, `adeclick`, `arnndn`, and ducking amount.
- Create `crates/render/src/audio_fx.rs` if `audio_fx_filter_chain` grows beyond a focused helper; otherwise keep the existing local helper and tests.
- Modify `crates/render/src/lib.rs`: expose any new render helper module if created.
- Modify `apps/desktop/src/timeline/TimelinePane.tsx`: wire snap behavior into drag/trim previews.
- Create `apps/desktop/src/timeline/snap.ts`: pure snap target collection and nearest-target resolution for clip edges, playhead, and markers.
- Modify `apps/desktop/src/timeline/hitDetect.ts` and `apps/desktop/src/timeline/moveDraft.ts`: use snap results at pointer interaction boundaries only.
- Modify generated desktop protocol files only through the repo generation path if snapshot shape changes are required.

## Task 1: Baseline And Contracts

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/parser.rs`
- Test: `crates/core/src/edl/parser.rs`

- [ ] **Step 1: Run baseline tests for known touched subsystems**

Run:

```bash
cargo test -p awidat-core professional_add_marker_lowers_to_real_clip_marker
cargo test -p awidat-render explicit_audio_track_emits_volume_automation_filter
```

Expected: both pass before behavior changes. If either fails, record the exact failure and decide whether it is pre-existing before editing.

- [ ] **Step 2: Add typed contracts with no behavior**

Add focused enum/struct fields:

```rust
pub struct SnapOptions {
    pub enabled: bool,
    pub tolerance_s: f64,
    pub targets: Vec<SnapTargetKind>,
}

pub enum SnapTargetKind {
    ClipEdge,
    Marker,
    Playhead,
}
```

Extend existing ops conservatively: `MoveClip.at_s` and insert/trim-like operations can accept `snap: Option<SnapOptions>` only where an absolute timeline time is already present. Add marker edit variants under `ProfessionalTimelineEdit`: `UpdateMarker`, `DeleteMarker`; implement marker listing as a read-only tool, not a mutating EDL op.

- [ ] **Step 3: Parse contract fields**

In `parse_audio_fx_config`, add fields for `pan`, `balance`, `adeclick`, `adeclip`, and `arnndn_model`. In `ProfessionalTimelineEdit`, keep complex payloads in `edit_json` so parser shape remains stable.

- [ ] **Step 4: Verify parser-only tests**

Run:

```bash
cargo test -p awidat-core parse_set_clip_audio_fx
cargo test -p awidat-core parse_professional_timeline_edit
```

Expected: existing tests pass, then add assertions for the new fields and variants.

## Task 2: Marker CRUD And Query Tool

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Create: `crates/core/src/tools/list_markers.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Test: `crates/core/src/edl/apply.rs`
- Test: `crates/core/src/tools/list_markers.rs`

- [ ] **Step 1: Write failing apply tests**

Add tests that create two clips with markers and verify:

```rust
// update by marker id or clip-relative match changes label/category/range
// delete removes only the targeted marker
// ambiguous marker selectors fail loudly with context
```

- [ ] **Step 2: Implement marker selectors**

Add a small private selector type in `apply.rs` that resolves by marker id first, then by `{clip anchor, label, at_s}` only when unique. Keep the selector local unless another module needs it.

- [ ] **Step 3: Add read-only marker listing**

Implement `list_markers` as a `ToolHandler` returning timeline time, clip uuid/name, marker id, label, category, note, and duration. It must be `is_mutating = false`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-core marker
cargo test -p awidat-core list_markers
```

Expected: marker CRUD and query tests pass without changing unrelated EDL behavior.

## Task 3: Snap Engine

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Create: `apps/desktop/src/timeline/snap.ts`
- Modify: `apps/desktop/src/timeline/TimelinePane.tsx`
- Modify: `apps/desktop/src/timeline/moveDraft.ts`
- Test: `crates/core/src/edl/apply.rs`
- Test: frontend test path if the repo has an existing test runner; otherwise add pure TypeScript unit tests next to `snap.ts` only if configured.

- [ ] **Step 1: Core failing tests**

Write tests for snap target resolution:

```rust
// MoveClip with at_s near a clip edge snaps to exact edge when within tolerance.
// MoveClip outside tolerance keeps requested at_s.
// Marker target snaps to marker timeline time.
```

- [ ] **Step 2: Implement pure snap helpers**

Add reusable helpers in `apply.rs` or a new focused `snap.rs` under `crates/core/src/edl/` if the code exceeds local-helper size. It should collect clip edges and clip marker times from the current timeline and return the nearest target within tolerance.

- [ ] **Step 3: Desktop snap helper**

Create `snap.ts` with pure functions:

```ts
export type SnapTarget = { kind: "clip_edge" | "marker" | "playhead"; timeS: number };
export function nearestSnap(timeS: number, targets: SnapTarget[], toleranceS: number): SnapTarget | null;
```

Wire it into move/trim drag previews without changing persistence semantics.

- [ ] **Step 4: Verify**

Run the focused Rust tests and the narrow frontend check available in `apps/desktop/package.json`.

## Task 4: Atomic Multicam Apply

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/parser.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Modify: `crates/core/src/tools/plan_multicam.rs`
- Test: `crates/core/src/edl/apply.rs`

- [ ] **Step 1: Define `ApplyMulticamPlan`**

Create one EDL op that accepts the same decision rows currently emitted by `plan_multicam`: program track, source asset, start/end, reason metadata, and optional sync group id.

- [ ] **Step 2: Write failing atomicity test**

Test that a valid plan creates/replaces a Program Video track in one apply operation and that an invalid decision leaves the timeline unchanged.

- [ ] **Step 3: Implement apply**

Validate every decision before mutating. Build the new track in memory, then swap it into the timeline. Persist traceable metadata on each generated clip.

- [ ] **Step 4: Update planner output**

Have `plan_multicam` include an EDL fragment using `ApplyMulticamPlan` while keeping the existing review JSON.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-core multicam
```

Expected: planner remains read-only; atomic apply happens only through EDL.

## Task 5: Nested Stack Decision And Rendering

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Modify: `crates/render/src/timeline.rs`
- Test: `crates/core/src/edl/apply.rs`
- Test: `crates/render/src/timeline.rs`

- [ ] **Step 1: Choose implement over removal unless evidence blocks it**

Because OTIO schema already supports `StackChild::Stack`, prefer implementing `WrapAsNested` and `FlattenNested` plus render traversal. Only remove schema support if render traversal proves structurally incompatible.

- [ ] **Step 2: Write failing render test**

Construct a timeline with a nested stack containing a track and clip. Assert `collect_timeline_segments` includes the nested clip in playback order.

- [ ] **Step 3: Implement recursive collection**

Refactor collection to walk `StackChild` recursively while preserving track kind, titles role, overlays, and audio-track behavior.

- [ ] **Step 4: Add apply ops**

Add `WrapAsNested` and `FlattenNested` only if there is a clear user-facing mutation path. Keep operations limited to contiguous ranges.

- [ ] **Step 5: Verify**

Run focused render and apply tests for nested stack traversal and flattening.

## Task 6: Audio FX Pan, Cleanup, And Ducking Amount

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/parser.rs`
- Modify: `crates/render/src/timeline.rs`
- Test: `crates/core/src/edl/parser.rs`
- Test: `crates/render/src/timeline.rs`

- [ ] **Step 1: Write failing render tests**

Cover:

```rust
// pan emits an FFmpeg pan filter
// balance emits a channel-balance/pan expression
// adeclick true emits adeclick
// arnndn_model emits arnndn=m=<escaped path>
// ducking amount_db changes the sidechaincompress lowering
```

- [ ] **Step 2: Implement fields and parsing**

Add `pan: Option<f64>` clamped to `[-1.0, 1.0]`, `balance: Option<f64>` clamped to `[-1.0, 1.0]`, `adeclick: Option<bool>`, `adeclip: Option<bool>`, and `arnndn_model: Option<String>`.

- [ ] **Step 3: Implement safe FFmpeg lowering**

Use helper functions for pan coefficients and path escaping. Reject unsafe model paths or resolve them project-relative before lowering if necessary.

- [ ] **Step 4: Honor ducking amount**

Map `amount_db` to sidechain compressor behavior with deterministic math and tests. If FFmpeg cannot directly guarantee exact gain reduction, document the approximation in code and test that different values produce different, monotonic lowering.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-render audio_fx
cargo test -p awidat-render ducking
```

Expected: new FX emit valid filter graph fragments and existing audio tests remain green.

## Task 7: Per-Clip Volume Envelopes

**Files:**
- Modify: `crates/proto/src/professional.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Modify: `crates/render/src/timeline.rs`
- Modify: `apps/desktop/src/timeline/usePlaySegments.ts` if preview needs animation parity.
- Test: `crates/render/src/timeline.rs`

- [ ] **Step 1: Write failing render test**

Add a `ParameterAnimation` with `AnimationTarget::ClipParameter { clip_id, parameter: "volume_db" }` and assert the segment path emits `volume='<expr>':eval=frame` before concat.

- [ ] **Step 2: Select clip audio volume animations**

Add a selector parallel to the existing track automation selector. Consume only `volume` and `volume_db` clip parameters.

- [ ] **Step 3: Lower clip automation**

Apply the expression in both segment rendering and explicit audio-track rendering. Ensure scalar `awidat.volume` and keyframed volume do not both apply silently; prefer automation and report a limitation or deterministic precedence.

- [ ] **Step 4: Verify**

Run focused render tests for clip automation and existing track automation tests.

## Task 8: Final Verification And Documentation

**Files:**
- Modify: `.reference-research/pro-editing-gap-analysis/02-timeline-editing.md`
- Modify: `.reference-research/pro-editing-gap-analysis/03-audio-editing.md`
- Modify: docs only if public EDL syntax changed.

- [ ] **Step 1: Update gap docs**

Change each covered gap from missing/partial to have, with file references and remaining limitations.

- [ ] **Step 2: Run formatter**

Run:

```bash
cargo fmt --all -- --check
```

Expected: pass. If it fails, run `cargo fmt --all`, review the diff, and rerun check.

- [ ] **Step 3: Run targeted clippy/tests**

Run the narrowest changed-crate commands:

```bash
cargo clippy -p awidat-core -p awidat-render --all-targets -- -D warnings
cargo test -p awidat-core
cargo test -p awidat-render
```

Expected: pass. If runtime is excessive, record the exact command and last successful narrower evidence.

- [ ] **Step 4: Summarize evidence**

Final report must list changed files, each original gap and how it is covered, exact tests/checks run, failures or skipped commands, and residual limitations.

---

## Self-Review

- Spec coverage: Timeline gaps map to Tasks 2-5; Audio gaps map to Tasks 6-7; final evidence maps to Task 8.
- Placeholder scan: no `TBD`/`TODO` steps are present; risky areas have explicit stop/evidence rules.
- Type consistency: new snap/audio/multicam names are introduced before use and tied to existing EDL/parser/render layers.
