---
name: transition-director
description: Choose and apply motivated video transitions using Awidat's semantic transition ids, OTIO metadata, and FFmpeg-supported phase-one renderer.
version: 0.1.0
tier: creative
tools_allowlist:
  - view_timeline
  - inspect_clip
  - assess_continuity
  - assess_edit_quality
  - transition_context
  - plan_transition
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

Before adding a transition to hide or smooth a cut, call
`assess_edit_quality(at_s, kind="cut")`. If it recommends `recut`,
`Set Audio Lead`, `Set Audio Trail`, b-roll, or `Set Cut Intent`, follow
that lower-attention repair instead of forcing a visible transition.
Respect `style_context.transition_density_last_30s`: at 3+ recent
transitions, avoid adding another visible transition unless the user
explicitly asks for a stylized sequence.

When a visible transition still has a named job, call
`transition_context` for the exact adjacent boundary, then call
`plan_transition` with that context packet and the job/direction. Use
the returned EDL fragment only after checking that the recommendation is
a visible transition with safe handles. If `plan_transition` returns a
hard-cut fragment, preserve the hard cut and apply `Set Cut Intent`
instead of forcing an effect.

## Supported Phase-One IDs

Use these stable ids in `Insert Transition`:

- `awidat.cross_dissolve` for soft time passage, topic drift, or gentle emotional transitions.
- `awidat.match_dissolve` for visual echo, memory bridge, or a true
  graphic match between related images.
- `awidat.fade_black` for a heavier reset, ending, or chapter break.
- `awidat.flash_white` for a bright beat hit, reveal, or energetic jump.
- `awidat.wipe_left` / `awidat.wipe_right` for graphic movement between related scenes.
- `awidat.slide_left` / `awidat.slide_right` for spatial movement or screen-direction continuity.
- `awidat.smooth_push_left` for a cleaner, less abrupt directional push.
- `awidat.motion_blur` for a very short motion cover when motion is the
  problem but screen direction is unknown.
- `awidat.whip_pan_left` / `awidat.whip_pan_right` for very short
  motion-blur covers when source footage already has fast lateral
  motion. Do not use them for static dialogue.
- `awidat.pass_by_left` / `awidat.pass_by_right` for an occlusion or
  frame-filling pass-by that naturally masks a scene move.
- `awidat.iris_open` / `awidat.iris_close` for deliberate vintage,
  comic, or stylized reveal/closure grammar. Avoid documentary realism.
- `awidat.invisible_cut` for an occlusion, dark-frame, or mask cut that
  should hide the edit without reading as a visible effect.
- `awidat.zoom_in` for energetic punch-ins or forward momentum.
- `awidat.pixelize` for tech/glitch moments only.
- `awidat.radial` for stylized reveals; use sparingly.
- `awidat.composite` for an on-the-spot custom recipe expressed with
  `composition_json`.

Use `SMPTE_Dissolve` only for older/simple EDL compatibility. Use no
transition for `awidat.hard_cut`; just leave the cut as-is.
Do not author raw FFmpeg transition names such as `fadeblack`,
`slideleft`, or `wipeleft`; use registered `awidat.*` ids instead.

## On-The-Spot Compositions

When a cut needs a custom feel, author it as `composition_json`: a
data-only recipe over stable primitives. This is how Awidat makes
transitions on the spot without generating arbitrary backend code.
Use `+ id: awidat.composite` and `+ kind: awidat.composite` for these
one-off recipes unless the recipe is simply annotating a named preset.

Allowed primitives:

- `opacity`
- `push`
- `wipe`
- `zoom`
- `blur`
- `flash`
- `shake`
- `chromatic_split`
- `pixelize`
- `atomic` with a stable registered `awidat.*` transition id

Do not emit raw FFmpeg filter graphs, GLSL, shell commands, plugin code,
or generated backend code inside an edit. If the desired transition
requires new backend implementation, treat that as transition-lab work
outside the normal editing flow.

## Selection Rules

- Dialogue, serious emotion, or tight reasoning: hard cut or `awidat.cross_dissolve`.
- Speaker handoff: prefer `Set Audio Lead` / `Set Audio Trail`; a
  transition is not an audio continuity repair.
- Beat hit, laugh, reveal, or high-energy turn: `awidat.flash_white`, `awidat.zoom_in`, or a short slide.
- Motion mismatch or camera direction: choose slide/wipe direction that follows existing motion.
- Pass-by object or full-frame occlusion: use `awidat.pass_by_left/right`
  or `awidat.invisible_cut` only when the indexed/inspected frames show
  a real mask opportunity.
- Vintage/comic/stylized reveal: use `awidat.iris_open/close` only when
  the project style calls for that grammar.
- Topic/chapter boundary: `awidat.cross_dissolve` for soft, `awidat.fade_black` for strong.
- Tech/product/glitch context: `awidat.pixelize`, short duration only.
- If neither clip has extra handles for overlap, avoid a transition or repair handles first.

## Durations

- 0.12-0.20s: impact, flash, punchy social pacing.
- 0.22-0.35s: normal motivated transition.
- 0.40-0.70s: deliberate chapter/time passage.
- Longer than 0.70s usually feels slow unless the user asks for it.

## Handles And Alignment

By default, transitions are centered on the cut. In OTIO/Awidat terms:

- `in_offset_s` consumes incoming pre-roll from the next clip before the cut.
- `out_offset_s` consumes outgoing post-roll from the previous clip after the cut.
- `alignment: start_at_cut` means `in_offset_s = 0`, `out_offset_s = duration_s`.
- `alignment: end_at_cut` means `in_offset_s = duration_s`, `out_offset_s = 0`.

If apply or render reports a missing handle, repair by shortening the
transition, choosing a different alignment, or applying `Untrim Clip`
to widen the source range. Do not keep retrying the same transition.

## EDL Shape

```text
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ id: awidat.composite
+ kind: awidat.composite
+ family: custom
+ intent: hide_motion_jump
+ energy: 0.700
+ direction: left
+ params_json: {"blur":0.2}
+ composition_json: {"version":1,"primitives":[{"op":"push","direction":"left","distance":0.9,"start":0.0,"end":1.0,"easing":"ease_out_expo"},{"op":"blur","amount":0.65,"direction":"left","start":0.1,"end":0.7},{"op":"flash","color":"#ffffff","peak":0.25,"start":0.35,"end":0.55}]}
+ duration_s: 0.280
+ alignment: center
*** End EDL
```

Always include `intent`. Keep it short and concrete so future `vedit`
history explains why the transition exists.

If the best decision is no visible transition, use `*** Set Cut Intent`
instead of `Insert Transition` so the timeline records the cut grammar
(`hard_cut`, `cut_on_action`, `match_cut`, `j_cut`, or `l_cut`) without
adding an effect.

## Verification

After applying transitions, call `view_timeline` to confirm placement
between adjacent clips, then `vedit_diff` to verify the committed OTIO
change. For final checks, render with `start_render(scope="timeline")`
and inspect/poll the result.
