# Transition Render Proof Pack Design

## Problem

The transition planner can now choose better semantic transitions, but a
manifest-level assertion is not enough. A render can contain an `xfade`
entry while the reviewed video still appears unchanged if the proof uses
near-identical adjacent shots, the wrong output aspect ratio, or a stylized
transition that reads as a broken composite.

## Scope

Phase 2 adds a render-backed proof path for representative visible
transitions. The acceptance test covers the vertical social-video case that
failed in review and then passed with a clear slide transition, then extends
the same proof shape across representative FFmpeg-backed families.

## Requirements

- Build a real project from two visually distinct vertical clips.
- Set output format to `9:16` before rendering.
- Apply centered transitions with enough source handles for slide, dissolve,
  flash, wipe, and zoom families.
- Render through the CLI, not only through render-spec construction.
- Assert the render manifest contains the expected `scale`, `xfade`, and
  `acrossfade` evidence.
- Assert the encoded output is vertical.
- Extract before, mid-transition, and after frames from the rendered file.
- Fail if the mid-transition frame is not visibly different from both sides.
- Keep proof renders short and isolated so the test validates smoothness
  without building a long chained-transition render graph.

## Non-Goals

- GPU-only transition cleanup.
- Exhaustive coverage of every transition family.
- New transition primitives or arbitrary generated render code.

## Success Criteria

`cargo test -p montage-cli --test transition_render_proof` produces a real
render and verifies both manifest evidence and pixel-level transition evidence.
