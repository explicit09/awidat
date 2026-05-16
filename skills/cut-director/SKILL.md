---
name: cut-director
description: Choose intentional hard-cut grammar and stamp semantic cut metadata without defaulting to decorative transitions.
version: 0.1.0
tier: creative
tools_allowlist:
  - view_timeline
  - inspect_clip
  - view_frame
  - assess_continuity
  - assess_edit_quality
  - find_moment
  - find_beat
  - apply_edl
  - vedit_diff
---

# Cut Director

Use this when the user asks where to cut, how to make a cut feel
better, whether a cut should be hard or motivated, or how to label the
editorial grammar at a boundary.

## Principle

A hard cut is the default. The question is what the cut does: carry
story, preserve action, change point of view, reveal contrast, compress
time, or create deliberate shock. A visible transition is a separate
choice. Route to `transition-director` only when the edit needs a named
transition job after the lower-attention cut choices have been checked.

Before repairing a risky or dirty boundary, call `assess_edit_quality`.
If it recommends recut, `Set Audio Lead`, `Set Audio Trail`, b-roll, or
`Set Cut Intent`, follow that recommendation before proposing a visible
transition.

## Cut Grammar

Use these cut types deliberately:

- `hard_cut`: the ordinary default when continuity, rhythm, and meaning
  already work.
- `cut_on_action`: cut during motion so the viewer follows the action
  instead of the seam.
- `cutaway`: leave the primary shot for a relevant detail, reaction, or
  concrete context.
- `insert`: show an object, screen, hands, document, or detail that
  carries information.
- `eyeline_match`: cut to what a person is looking at, preserving eye
  direction and attention.
- `shot_reverse_shot`: dialogue or reaction grammar between people or
  positions.
- `match_cut`: connect shots through shared shape, motion, framing, or
  idea.
- `smash_cut`: abrupt contrast for joke, shock, escalation, or tonal
  reversal.
- `jump_cut`: intentional time compression inside the same setup; avoid
  using it as accidental roughness.
- `cross_cut`: interleave parallel actions, locations, or arguments.

## Workflow

1. Use `view_timeline` to locate the boundary and clip UUIDs.
2. Use `inspect_clip`, `view_frame`, `find_moment`, or `find_beat` when
   the cut depends on visual action, transcript meaning, or rhythm.
3. Call `assess_edit_quality(at_s, kind="cut")` for risky boundaries.
4. Choose the lowest-attention fix that solves the problem: move the
   cut, use cut-on-action, use an insert/cutaway, use a split edit, or
   keep a hard cut with semantic metadata.
5. Use `apply_edl` to stamp the decision on the graph.
6. Call `vedit_diff` to confirm the committed edit metadata.

## EDL Shape

For a clean or intentionally chosen hard boundary, record why it exists
with `Set Cut Intent`:

```text
*** Begin EDL
*** Set Cut Intent
@@ between: clip_uuid=outgoing and clip_uuid=incoming
+ cut_type: cut_on_action
+ intent: hide_action_continuity
+ audio_relation: sync
+ confidence: 0.850
+ reason: outgoing hand motion resolves into the incoming shot
*** End EDL
```

Do not use `Insert Transition` from this skill. If the right answer is a
transition, switch to `transition-director` and include the
`assess_edit_quality` reason that justified it.

## Done

- [ ] The cut type is named and defensible.
- [ ] Dirty cuts were assessed with `assess_edit_quality`.
- [ ] Visible transition repair was not used as the first answer.
- [ ] `Set Cut Intent` records the boundary reason when the graph changes.
- [ ] `vedit_diff` confirms the timeline metadata change.
