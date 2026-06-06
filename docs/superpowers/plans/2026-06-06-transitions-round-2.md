# Transitions Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Widen Awidat's transition planner using the mined transcript guidance, package the transition research artifacts, and verify with focused tests plus render preparation.

**Architecture:** Keep `plan_transition` as the visible-transition planner and `plan_split_edit` as the audio-led planner. Add only deterministic context parsing and objective mapping; do not introduce a new engine or speculative GPU-only authoring path.

**Tech Stack:** Rust (`awidat-core`, `awidat-proto` transition registry), Markdown skill/research docs, Awidat EDL, scoped Cargo tests.

---

## File Structure

- Modify `crates/core/src/awidat_mcp/tools/plan_transition.rs`: parse occlusion scores, expand objective-to-transition selection, and refuse occlusion-only choices without evidence.
- Create `crates/core/tests/plan_transition.rs`: focused unit tests for the widened planner.
- Modify `video_editing_transcripts/knowledge/transitions/SKILL.md`: keep the ignored research summary aligned with the mined craft.
- Modify `video_editing_transcripts/knowledge/transitions/tool-gap.md`: record closed gaps and next-ranked gaps.
- Create `video_editing_transcripts/knowledge/transitions/sources.md`: generated source list for the 870 transition transcripts.
- Verify existing `skills/transition-director/SKILL.md`, `skills/cut-director/SKILL.md`, and `skills/split-edit-director/SKILL.md` still load through `skill_catalog`.

## Task 1: Add Focused Planner Tests

**Files:**
- Create: `crates/core/tests/plan_transition.rs`

- [ ] **Step 1: Write tests for pass-by, invisible refusal, zoom, iris, and directional wipes**

Create tests that call `awidat_core::awidat_mcp::tools::plan_transition::run` with compact fake `transition_context` JSON packets. Assert the selected transition id and that returned EDL parses with `awidat_core::edl::parse`.

- [ ] **Step 2: Run the new test and confirm it fails before implementation**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core --test plan_transition`

Expected: tests for new behavior fail because current `plan_transition` does not select pass-by, invisible-cut, zoom, iris, or directional wipe objectives.

## Task 2: Widen `plan_transition`

**Files:**
- Modify: `crates/core/src/awidat_mcp/tools/plan_transition.rs`

- [ ] **Step 1: Parse occlusion scores**

Add outgoing and incoming `occlusion_score` fields to `ContextSummary`, populated from `/visual_signals/outgoing/occlusion_score` and `/visual_signals/incoming/occlusion_score`.

- [ ] **Step 2: Add objective mapping**

Expand `transition_for_job` so these objectives map to FFmpeg-renderable ids:

- `occlusion_mask`, `pass_by_motion`, `invisible_scene_move` -> `awidat.pass_by_left/right` when direction is left/right, otherwise `awidat.invisible_cut`.
- `mask_cut`, `occlusion_or_dark_frame`, `hide_camera_reposition` -> `awidat.invisible_cut`.
- `punch_in`, `forward_momentum` -> `awidat.zoom_in`.
- `spatial_shift`, `energy_jump` with zoom direction -> `awidat.distance_zoom`.
- `stylized_reveal`, `vintage_reveal`, `comic_reveal` -> `awidat.iris_open`.
- `stylized_closure`, `comic_button` -> `awidat.iris_close`.
- `graphic_movement`, `related_scene_change` -> directional wipe ids when direction exists.
- `social_push`, `screen_direction` -> directional slide ids when direction exists.
- `tech_context`, `glitch_moment` -> `awidat.pixelize`.

- [ ] **Step 3: Add occlusion refusal**

In `incompatibility_reason`, if the transition metadata avoids `no_occlusion_signal`, require max outgoing/incoming occlusion score >= `0.75`. Otherwise return a hard-cut fallback with a clear reason.

- [ ] **Step 4: Run planner tests**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core --test plan_transition`

Expected: all new planner tests pass.

## Task 3: Finish Transition Research Artifacts

**Files:**
- Modify: `video_editing_transcripts/knowledge/transitions/SKILL.md`
- Modify: `video_editing_transcripts/knowledge/transitions/tool-gap.md`
- Create: `video_editing_transcripts/knowledge/transitions/sources.md`

- [ ] **Step 1: Update research skill summary**

Ensure the research summary includes the mined rules for hard-cut default, cut-on-action, split edits, sound design, duration ranges, same-angle jump-cut repair, motion direction, occlusion/invisible transitions, and overuse avoidance.

- [ ] **Step 2: Update tool-gap**

Record `plan_split_edit` as closed, `plan_transition` widening as this round's implementation, and leave GPU-only authoring/params plus split-edit validation as future gaps.

- [ ] **Step 3: Generate sources list**

Generate `sources.md` from `_tr_files.txt` and transcript headers. Include the total count and one Markdown bullet per source title/URL.

## Task 4: Verification

**Files:**
- Existing tests and docs only.

- [ ] **Step 1: Format check**

Run: `cargo fmt --all -- --check`

Expected: command exits 0. Existing stable-rustfmt warnings about unstable config keys may print but must not fail the command.

- [ ] **Step 2: Run focused tests**

Run:

```bash
CARGO_INCREMENTAL=0 cargo test -p awidat-core --test plan_transition
CARGO_INCREMENTAL=0 cargo test -p awidat-core --test plan_split_edit
CARGO_INCREMENTAL=0 cargo test -p awidat-core --test skill_catalog
```

Expected: all tests pass.

- [ ] **Step 3: Run focused clippy**

Run: `CARGO_INCREMENTAL=0 cargo clippy -p awidat-core --test plan_transition --no-deps -- -D warnings`

Expected: command exits 0.

## Task 5: Render Proof Preparation

**Files:**
- No repo file changes expected unless an EDL proof fixture is intentionally added.

- [ ] **Step 1: Locate test footage**

Check `/Volumes/Explicit's Hard Drive/Short6_VCTake.mp4` and nearby short clips. If unavailable, report the exact missing path.

- [ ] **Step 2: Rebuild CLI**

Run: `CARGO_INCREMENTAL=0 cargo build -p awidat-cli --bin awidat`

Expected: CLI builds successfully.

- [ ] **Step 3: Create a clean scratch project**

Use a path without apostrophes or spaces, e.g. `/Users/explicit/awidat_transition_round2`.

- [ ] **Step 4: Apply one planner-generated transition EDL and render**

Use Awidat CLI to create/import footage, apply an EDL using a FFmpeg-renderable transition from the widened planner, render, and inspect the manifest for `xfade`/`acrossfade`.

- [ ] **Step 5: Report artifact path**

Return the rendered video path and the manifest evidence so the user can review the actual output.
