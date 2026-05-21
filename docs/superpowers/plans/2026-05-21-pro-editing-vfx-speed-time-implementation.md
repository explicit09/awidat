# Pro Editing VFX and Speed/Time Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the Effects/VFX and Speed/Time gap work with isolated, test-first, verified changes.

**Architecture:** Implement editor-facing capabilities as small semantic effect ids, dedicated EDL ops, and deterministic producer helpers. Render supported features through FFmpeg-native lowering first, and surface explicit limitations for runtime-heavy tracking/segmentation features until their producers are available.

**Tech Stack:** Rust 2024 workspace crates (`awidat-effects`, `awidat-core`, `awidat-render`, `awidat-proto`), FFmpeg filtergraphs, Python 3 skill scripts with `unittest`.

---

## Current Blocker

Cargo verification is constrained by disk space. At the start of this
plan, `df -h` showed only about 116 MiB free on `/System/Volumes/Data`,
and Cargo failed with `No space left on device (os error 28)`. Later
targeted Rust tests succeeded by reusing the shared target directory and
setting `CARGO_INCREMENTAL=0` for the core EDL test. Keep using narrow
verification and avoid cleaning shared build artifacts while other agents
may be active.

## Task 1: Beat-Sync Speed-Ramp Executor

**Files:**
- Create: `skills/beat-sync-editor/scripts/speed_ramp_plan.py`
- Create: `skills/beat-sync-editor/scripts/speed_ramp_plan_test.py`
- Modify: `skills/beat-sync-editor/SKILL.md`

- [x] **Step 1: Write failing tests**

Add tests that import `speed_ramp_plan.py`, call `plan_speed_ramps`, and
assert it emits an `awidat.time_remap` curve with accent metadata.

- [x] **Step 2: Verify red**

Run:

```bash
python3 skills/beat-sync-editor/scripts/speed_ramp_plan_test.py
```

Expected before implementation: failure because `speed_ramp_plan.py` is
missing.

- [x] **Step 3: Implement planner**

Add `plan_speed_ramps`, `beat_times`, `build_speed_points`,
`build_time_remap_curve`, and a small CLI that reads beat JSON and prints
the effect plan.

- [x] **Step 4: Verify green**

Run:

```bash
python3 skills/beat-sync-editor/scripts/beat_cut_plan_test.py
python3 skills/beat-sync-editor/scripts/speed_ramp_plan_test.py
```

Expected: both pass.

## Task 2: Speed Effect Schema Extensions

**Files:**
- Modify: `crates/effects/src/lib.rs`

- [x] **Step 1: Write registry tests**

Add tests proving `awidat.speed` accepts:

```json
{
  "factor": 0.5,
  "reverse": true,
  "maintain_pitch": false,
  "frame_blending": "blend"
}
```

and rejects unknown `frame_blending` values.

- [x] **Step 2: Verify red**

Run:

```bash
cargo test -p awidat-effects speed_
```

Expected: tests fail until schema accepts the new fields.

Observed red result: compile failed because `ValidationError::InvalidChoice`
did not exist yet, proving the registry had no bounded string-choice
validation for `frame_blending`.

- [x] **Step 3: Implement schema**

Extend `SPEED_PARAMS` with `reverse`, `maintain_pitch`, and
`frame_blending`. Keep defaults backwards compatible:
`reverse=false`, `maintain_pitch=true`, `frame_blending="nearest"`.

- [x] **Step 4: Verify green**

Run:

```bash
cargo test -p awidat-effects speed_
```

Expected: pass.

Verified with:

```bash
CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects speed_
```

Result: 3 passed.

## Task 3: Reverse and Maintain-Pitch Lowering

**Files:**
- Modify: `crates/render/src/timeline.rs`
- Modify: `crates/render/tests/speed.rs`

- [x] **Step 1: Write render tests**

Add tests that construct `awidat.speed` metadata with
`reverse=true` and assert video uses `reverse,setpts=...`, audio uses
`areverse`, and total duration remains `source_duration / factor`.

Add a test for `maintain_pitch=false` that asserts audio uses an
`asetrate=sample_rate*factor,aresample=sample_rate` style chain instead
of `atempo`.

- [x] **Step 2: Verify red**

Run:

```bash
cargo test -p awidat-render --test speed
```

Expected: fail because render only reads `factor`.

Observed red result: reverse and `maintain_pitch=false` tests failed
because render emitted plain `setpts` and `atempo`.

- [x] **Step 3: Implement render plan fields**

Replace the bare speed `Option<f64>` with a focused speed plan struct that
contains `factor`, `reverse`, `maintain_pitch`, and `frame_blending`.
Thread it through base clips and media overlays without changing behavior
when only `factor` exists.

- [x] **Step 4: Verify green**

Run the same `awidat-render --test speed` command.

Verified with:

```bash
CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test speed
```

Result: 4 passed.

## Task 4: Frame-Blending and Flow Modes

**Files:**
- Modify: `crates/render/src/timeline.rs`
- Modify: `crates/render/tests/speed.rs`

- [x] **Step 1: Write tests**

Add filtergraph tests for:

- `frame_blending="nearest"` emits no extra interpolation filter.
- `frame_blending="blend"` emits `tblend` plus `framerate`.
- `frame_blending="flow"` emits `minterpolate=mi_mode=mci`.

- [x] **Step 2: Implement minimal lowering**

Append the interpolation filter after `setpts`. Gate `"flow"` behind the
explicit value only; never enable it by default.

- [x] **Step 3: Verify**

Run targeted render speed tests.

Verified with:

```bash
CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test speed
```

Result: 7 passed.

## Task 5: Dedicated Time Remap EDL Op

**Files:**
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/parser.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Modify: `crates/core/src/tools/apply_edl.rs`

- [x] **Step 1: Write parser/apply tests**

Add tests for:

```edl
*** Set Time Remap
@@ anchor: clip name
+ curve_json: [{"source_time_s":0,"timeline_time_s":0},{"source_time_s":2,"timeline_time_s":3}]
```

Assert the clip receives an `awidat.time_remap` effect.

- [x] **Step 2: Implement op**

Add `SetTimeRemap { anchor, curve }`, parse `curve_json`, validate through
the existing effect registry, and stamp/replace `awidat.time_remap`.

- [x] **Step 3: Verify**

Run targeted core EDL parser/apply tests.

Verified with:

```bash
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-core set_time_remap
```

Result: 2 passed.

## Task 6: Freeze Frame Effect and Op

**Files:**
- Modify: `crates/effects/src/lib.rs`
- Modify: `crates/core/src/edl/op.rs`
- Modify: `crates/core/src/edl/parser.rs`
- Modify: `crates/core/src/edl/apply.rs`
- Modify: `crates/render/src/timeline.rs`

- [x] **Step 1: Write tests**

Add registry, parser/apply, and render tests for
`awidat.freeze` with:

```json
{
  "freeze_at_source_s": 1.2,
  "duration_s": 0.8,
  "freeze_position": "at",
  "audio_behavior": "silence"
}
```

- [x] **Step 2: Implement effect and EDL op**

Add the schema and `SetFreeze` op. Reject non-positive duration.

- [x] **Step 3: Implement lowering**

Use an FFmpeg concat-split plan: pre-freeze trim, held frame via
`tpad=stop_mode=clone`, post-freeze trim, and silence for generated hold
audio.

- [x] **Step 4: Verify**

Run targeted effects, core, and render tests.

Verified with:

```bash
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects freeze_
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-core set_freeze
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test freeze
```

Results: effects 2 passed; core 2 passed; render 1 passed.

## Task 7: Chroma and Luma Key Effects

**Files:**
- Modify: `crates/effects/src/lib.rs`
- Modify: `crates/render/src/timeline.rs`
- Add or modify render tests near existing effect/filtergraph tests.

- [x] **Step 1: Write registry tests**

Add tests for `awidat.chroma_key` and `awidat.luma_key` parameter
normalization and rejection of invalid thresholds.

- [x] **Step 2: Write render tests**

Assert chroma key emits `chromakey=` and luma key emits a grayscale alpha
construction that ends in an alpha-bearing stream.

- [x] **Step 3: Implement schemas and lowering**

Keep parameters atomic and explicit. Chroma key starts with `key_color`,
`similarity`, `blend`, and optional `despill_amount`; luma key starts with
`threshold` and `softness`.

- [x] **Step 4: Verify**

Run targeted effects and render tests.

Verified with:

```bash
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects key_
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test keying
```

Results: effects 2 passed; render 2 passed.

## Task 8: Region Blur and Blend Modes

**Files:**
- Modify: `crates/effects/src/lib.rs`
- Modify: `crates/render/src/timeline.rs`

- [x] **Step 1: Write tests**

Add tests for `awidat.region_blur` rectangular and elliptical regions.
Add overlay tests for blend modes such as `multiply` and `screen`.

- [x] **Step 2: Implement lowering**

Use the existing annotation blur pattern for static rectangular regions.
For unsupported tracked or polygon regions, emit explicit limitations.

- [x] **Step 3: Verify**

Run targeted render tests.

Verified with:

```bash
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects region_blur
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects video_overlay_accepts_blend_modes
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test vfx
```

Results: effects 1 + 1 passed; render 2 passed.

## Task 9: Composition Graph Limitations and Real Lowering Cleanup

**Files:**
- Modify: `crates/render/src/professional.rs`
- Modify: `crates/render/tests/professional_engines.rs`

- [x] **Step 1: Write tests**

Assert `Scene3d` and `ParticleEmitter` remain explicit future-runtime
limitations, while supported node types report executable fragments only
when enough inputs are present.

- [x] **Step 2: Implement cleanup**

Separate inspection summaries from executable lowering. Do not present
placeholder strings as supported render steps.

- [x] **Step 3: Verify**

Run targeted professional render tests.

Verified with:

```bash
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test professional_engines particle_and_scene3d
```

Result: 1 passed.

## Task 10: Tracking and Segmentation Producer Contracts

**Files:**
- Create: `python/packages/composition-mcp/src/composition_mcp/vfx_contracts.py`
- Create: `python/packages/composition-mcp/tests/test_vfx_contracts.py`

- [x] **Step 1: Write contract tests**

Test validation of tracker request inputs and segmentation artifact profile
outputs without importing OpenCV, SAM2, rembg, or downloading models.

- [x] **Step 2: Implement deterministic scaffolds**

Add producer helpers that validate request JSON and emit expected
`TrackingPackage` shape or planned artifact paths.

- [x] **Step 3: Verify**

Run:

```bash
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_vfx_contracts.py
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_composition_mcp.py
```

Expected: both pass. Do not run heavy model downloads by default.

Verified with:

```bash
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_vfx_contracts.py
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_composition_mcp.py
```

Results: 3 + 3 passed.

## Task 11: Final Verification and Report

**Files:**
- Modify docs only if final behavior differs from the design.

- [x] **Step 1: Run formatting**

```bash
cargo fmt --all -- --check
```

- [x] **Step 2: Run targeted tests**

```bash
cargo test -p awidat-effects
cargo test -p awidat-core set_time_remap
cargo test -p awidat-core set_freeze
cargo test -p awidat-render --test speed
python3 skills/beat-sync-editor/scripts/beat_cut_plan_test.py
python3 skills/beat-sync-editor/scripts/speed_ramp_plan_test.py
```

- [x] **Step 3: Run broader check when feasible**

```bash
make check
```

- [x] **Step 4: Completion audit**

Compare every acceptance item in the design spec to concrete evidence.
Only mark the Goal complete if all required work is implemented and
verified.

Final verification:

```bash
cargo fmt --all -- --check
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-effects
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-core set_time_remap
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-core set_freeze
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test speed
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test freeze
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test keying
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test vfx
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/explicit/Projects/awidat/target cargo test -p awidat-render --test professional_engines particle_and_scene3d
python3 skills/beat-sync-editor/scripts/beat_cut_plan_test.py
python3 skills/beat-sync-editor/scripts/speed_ramp_plan_test.py
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_vfx_contracts.py
PYTHONPATH=python/packages/composition-mcp/src python3 python/packages/composition-mcp/tests/test_composition_mcp.py
```

All listed targeted commands passed. `make check` was not run because
`df -h` still showed only about 1.5 GiB free on the shared volume after
targeted verification, and a broad workspace check would likely exhaust
disk and disrupt other work.
