---
name: transition-director
description: Choose and apply motivated video transitions using Awidat's semantic transition ids, OTIO metadata, and FFmpeg-supported phase-one renderer.
version: 0.1.0
tier: creative
tools_allowlist:
  - view_timeline
  - inspect_clip
  - assess_continuity
  - find_beat
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
---

# Transition Director

Use this when the user asks to add transitions, smooth a cut, hide a
motion jump, match a beat, or give a sequence more editorial polish.

## Principle

A transition must have a job. Prefer a hard cut unless the transition
solves continuity, rhythm, tone, or intentional style. Never add a
transition only because there is a cut.

## Supported Phase-One IDs

Use these stable ids in `Insert Transition`:

- `awidat.cross_dissolve` for soft time passage, topic drift, or gentle emotional transitions.
- `awidat.fade_black` for a heavier reset, ending, or chapter break.
- `awidat.flash_white` for a bright beat hit, reveal, or energetic jump.
- `awidat.wipe_left` / `awidat.wipe_right` for graphic movement between related scenes.
- `awidat.slide_left` / `awidat.slide_right` for spatial movement or screen-direction continuity.
- `awidat.smooth_push_left` for a cleaner, less abrupt directional push.
- `awidat.zoom_in` for energetic punch-ins or forward momentum.
- `awidat.pixelize` for tech/glitch moments only.
- `awidat.radial` for stylized reveals; use sparingly.

Use `SMPTE_Dissolve` only for older/simple EDL compatibility. Use no
transition for `awidat.hard_cut`; just leave the cut as-is.

## Selection Rules

- Dialogue, serious emotion, or tight reasoning: hard cut or `awidat.cross_dissolve`.
- Beat hit, laugh, reveal, or high-energy turn: `awidat.flash_white`, `awidat.zoom_in`, or a short slide.
- Motion mismatch or camera direction: choose slide/wipe direction that follows existing motion.
- Topic/chapter boundary: `awidat.cross_dissolve` for soft, `awidat.fade_black` for strong.
- Tech/product/glitch context: `awidat.pixelize`, short duration only.
- If neither clip has extra handles for overlap, avoid a transition.

## Durations

- 0.12-0.20s: impact, flash, punchy social pacing.
- 0.22-0.35s: normal motivated transition.
- 0.40-0.70s: deliberate chapter/time passage.
- Longer than 0.70s usually feels slow unless the user asks for it.

## EDL Shape

```text
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ id: awidat.slide_left
+ kind: awidat.slide_left
+ family: slide
+ intent: hide_motion_jump
+ energy: 0.700
+ direction: left
+ params_json: {"blur":0.2}
+ duration_s: 0.280
*** End EDL
```

Always include `intent`. Keep it short and concrete so future `vedit`
history explains why the transition exists.

## Verification

After applying transitions, call `view_timeline` to confirm placement
between adjacent clips, then `vedit_diff` to verify the committed OTIO
change. For final checks, render with `start_render(scope="timeline")`
and inspect/poll the result.
