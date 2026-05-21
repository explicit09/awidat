# Pro Editing VFX and Speed/Time Design

## Goal

Close the Effects/VFX and Speed/Time gaps identified in the pro-editing
gap analysis with testable, agent-facing capabilities. The work should
favor small semantic effects and helpers that can be independently
validated, while keeping large model/runtime integrations behind explicit
contracts until their dependencies are available.

## Current Constraints

- Work happens in the isolated worktree
  `/Users/explicit/.config/superpowers/worktrees/awidat/pro-editing-vfx-speed-time`.
- The original checkout has unrelated dirty changes and must remain
  untouched.
- Local disk remains tight. Targeted Rust verification is feasible by
  reusing `/Users/explicit/Projects/awidat/target` with
  `CARGO_INCREMENTAL=0`; broad workspace verification should not be run
  until more free disk is available.
- The `.reference-research/pro-editing-gap-analysis` files are untracked
  in the original checkout and are read-only source material for this work.

## Architecture

Use three layers and keep them separate:

1. **Semantic effect and EDL surface.** Add explicit effect ids and EDL ops
   so agents can request the features without hand-authored generic JSON.
2. **Render lowering.** Lower each supported feature to FFmpeg-native filter
   blocks where practical, surfacing typed limitations for unsupported
   combinations instead of silently dropping work.
3. **Producer/runtime contracts.** For tracking and segmentation, add
   deterministic planners and artifact contracts first. Heavy OpenCV, SAM2,
   and rembg execution should be introduced as optional producers with
   clear output shapes, not hidden inside timeline rendering.

This keeps the final editor behavior modular: retime logic lives with
retime parsing/lowering, keying logic lives with clip effects, and mask or
track producers write `TrackingPackage` data consumed by existing render
paths.

## Speed and Time Decisions

- Keep `awidat.speed` as the constant-rate effect, but extend it with
  optional `reverse`, `maintain_pitch`, and `frame_blending` parameters.
- Default behavior remains backwards compatible: forward playback,
  pitch-preserved audio via `atempo`, and nearest-frame retime quality.
- Add `SetTimeRemap` as a dedicated EDL op that stamps
  `awidat.time_remap`, making speed ramps reachable without raw
  `SetEffect` JSON.
- Keep `awidat.time_remap` experimental, but make it reachable through a
  dedicated EDL op and targeted tests. Audio currently follows an average
  tempo for variable remaps; richer per-segment audio remap remains a
  future runtime enhancement.
- Add `awidat.freeze` as a separate effect and `SetFreeze` as a dedicated
  EDL op. Do not encode freezes as implicit invalid time-remap curves.
  Freezes mute generated hold audio by default, with an explicit
  `audio_behavior` parameter for future dialogue-preserving freezes.
- Add a lightweight beat-sync speed-ramp planner script under
  `skills/beat-sync-editor/scripts/`. It should convert beat/energy data
  into a `time_remap` curve that accents selected beats. It should not do
  beat detection itself.

## Effects and VFX Decisions

- Add separate `awidat.chroma_key` and `awidat.luma_key` ids. Their
  parameters differ enough that one generic key effect would be less clear
  for agents and validators.
- Chroma key lowers to FFmpeg `chromakey` first. Optional despill can be
  represented in the schema, but must only be emitted when the local FFmpeg
  path supports a defensible filter chain.
- Luma key lowers through a grayscale/alpha construction rather than being
  conflated with chroma key.
- Existing `awidat.blur`, annotation blur, `awidat.warp`, and
  `awidat.shake` mean the old "no generic blur" audit statement is stale.
  The remaining gap is region-aware/tracked blur as an agent-facing effect.
- Add `awidat.region_blur` as the practical object/distraction removal
  primitive. Version one supports rectangular and elliptical normalized
  regions; tracked object binding is represented by the tracking producer
  contract and can be layered onto the effect later.
- Extend overlay composition with explicit blend modes where FFmpeg can
  express them. Straight alpha-over remains the default.
- Improve `CompositionGraph` lowering by distinguishing executable FFmpeg
  steps from inspection-only summaries. Existing placeholder strings should
  become either real filtergraph fragments or explicit limitations.

## Tracking and Segmentation Decisions

- Tracking and segmentation are producer responsibilities. They write
  `TrackingPackage.tracks`, `masks`, `mattes`, or `mask_artifacts`; render
  consumes those artifacts.
- Add a deterministic motion-tracker contract and optional Python producer
  scaffold that can later wrap OpenCV CSRT/KCF. The contract must define
  input clip, initial box, frame range, confidence, and emitted
  `TrackSidecar.samples`.
- Add a segmentation producer contract for SAM2/rembg style outputs. The
  first shippable step is a package/schema-level contract plus CLI/helper
  script stubs that validate inputs and expected artifact paths. Actual
  model downloads remain opt-in and outside the default test path.

## Testing Strategy

- Python skill helpers use `unittest` tests that run without model
  downloads or project indexing.
- Rust registry changes get unit tests in `crates/effects/src/lib.rs`.
- EDL parser/apply changes get focused parser/apply tests near existing
  `SetSpeed` coverage.
- Render changes get filtergraph string tests first, matching the existing
  style in `crates/render/tests/speed.rs` and
  `crates/render/src/timeline.rs`.
- End-to-end Cargo verification requires more free disk. Targeted Rust
  commands are recorded with exact evidence; broad workspace verification
  is recorded as skipped due disk pressure.

## Acceptance Evidence

The Goal is not complete until current evidence proves all of these:

- New and existing speed/time tests cover constant speed, reverse,
  maintain-pitch on/off, frame blending modes, time-remap EDL emission,
  freeze frames, and beat-sync ramp planning.
- New and existing VFX tests cover chroma key, luma key, region blur,
  blend mode lowering, and explicit composition-graph limitations.
- Producer contracts for tracking and segmentation are documented and
  validated without requiring large model downloads.
- `cargo fmt --all -- --check`, relevant Rust tests, relevant Python
  tests, and a broader workspace check run successfully or are reported
  with exact environmental blockers.
