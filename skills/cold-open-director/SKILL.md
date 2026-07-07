---
name: cold-open-director
description: Author a blitz cold-open montage that opens the episode at reference pacing and doubles as the 9:16 short. Hook transplant plus blitz beats, validated against the house profile's cold-open gate before and after applying.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_timeline
  - read_index
  - find_moment
  - find_beat
  - inspect_moment
  - inspect_clip
  - plan_cold_open
  - apply_edl
  - run_picture_gates
  - vedit_diff
  - start_render
  - poll_render
  - verify_render
---

# Cold-Open Director

Build the episode's cold open: a ~90s blitz montage at the head of the
timeline that hooks in the first seconds and doubles as the vertical
short. Grounded in the 2026-07 reference study (docs/post-house-pipeline.md):
every reference episode peaks in minute 0–1 at 26–55 cuts/min, the hook is
a RELOCATED line (not written copy), and the cold open is the short.

## Editorial principles

- **The hook is transplanted, not written.** Find the single most
  provocative line of the episode — a claim, a confession, a number, a
  contradiction — and place it at 0:00. It should recur naturally in the
  body later; that repetition is correct, not a bug.
- **Blitz beats, ~2s each.** After the hook, stack short beats from
  across the episode: peak emotions, bold claims, visual variety. Target
  the profile's cold-open spec (technologia: ≥20 cuts/min over the first
  90s; references run 26–55).
- **~90 seconds total.** Under ~45s the body's pacing dominates the
  opening window; over ~135s it stops being a tease.
- **Dual-purpose asset.** The applied cold open IS the 9:16 short. Keep
  faces inside a vertical-safe center crop when choosing beats; reuse the
  span with `short-form-reframing` after the episode edit locks.
- **Gate-verified, before and after.** `plan_cold_open` projects the
  `picture.cold_open` verdict before you mutate anything; after applying,
  `run_picture_gates` must show `picture.cold_open` passing.

## Workflow

1. **Survey.** `view_timeline` and `read_index` (editorial-moments, topic)
   to understand the episode. The body edit should already be in decent
   shape — the cold open is authored near the end of picture work.
2. **Choose the hook.** `find_moment`/`find_beat`/`inspect_moment` for the
   peak-provocation line. One line, 2–6 seconds.
3. **Collect blitz beats.** 25–40 moments of ~2s each from across the
   episode. Favor variety: different topics, angles, emotional registers.
4. **Plan.** Call `plan_cold_open` with the hook and ordered beats. Read
   `gate_projection` and `warnings`; adjust beats until the projection
   passes. Nothing has mutated yet.
5. **Apply.** Pass `edl_fragment` to `apply_edl` with your reasoning.
6. **Verify.** `run_picture_gates` — `picture.cold_open` must pass.
   `vedit_diff` to review the mutation. Render a preview of the first two
   minutes (`start_render`/`poll_render`/`verify_render`) and confirm the
   hook lands and the beats read.
7. **Hand off the twin.** Note in your summary that the cold-open span is
   ready for `short-form-reframing` to produce the 9:16 short.

## Rationale rules

Every `apply_edl` envelope MUST carry reasoning: why THIS hook line (what
makes it the peak), and the pacing math (beats × duration → projected
cuts/min vs the profile spec).
