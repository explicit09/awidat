---
name: thematic-montage-director
description: Plan deliberate associative or symbolic montage sequences separately from literal b-roll continuity covers.
version: 0.1.0
tier: creative
tools_allowlist:
  - read_index
  - find_beat
  - find_moment
  - clip_search
  - inspect_clip
  - view_frame
  - view_timeline
  - apply_edl
  - vedit_diff
---

# Thematic Montage Director

Use this only when the user asks for a thematic montage, associative
sequence, symbolic cutaway pattern, memory bridge, poetic edit, or
editorial essay-style visual argument. This is opt-in. It is not a continuity cover,
not a literal b-roll fallback, and not a way to hide a
dirty cut that should be recut, split-edited, or covered with literal
b-roll.

## Contract

The goal is to build a short sequence where the images create meaning
through association, contrast, repetition, or progression. A montage
choice must explain its idea. If the image simply illustrates the words,
route to `b-roll-suggester` or `stock-broll` instead.

Do not use this skill because the timeline feels visually repetitive.
First decide whether the user wants literal b-roll or thematic montage.
When the user has not asked for symbolic editing, ask for confirmation
before proposing montage.

## Inputs

Read enough context to know the montage thesis:

```text
read_index(channel="transcript")
find_moment(query="<theme, emotional turn, or argument beat>")
find_beat(kind="music|energy|chapter")
clip_search(query="<visual motif>")
```

Use `view_frame` or `inspect_clip` on candidate images before placing
them. Do not infer symbolic meaning from filenames alone.

## Selection Rules

- Prefer 3-7 shots. Fewer reads as one cutaway; more needs a stronger
  structure.
- Pick one governing idea: contrast, escalation, memory, consequence,
  irony, repetition, or resolution.
- Keep each visual concrete even when the relation is associative.
  Abstract stock imagery without a thesis is filler.
- Avoid montage over punchlines, vulnerable confession, technical
  demos, legal/medical claims, or moments where the speaker's face is
  the evidence.
- Do not reuse literal b-roll candidates as symbolic montage unless the
  user explicitly approves the shift in meaning.

## Edit Shape

For a montage that overlays narration, use `*** Insert BRoll` with
`position: overlay` for each accepted visual. For a replacement montage
where the speaker should disappear, use `position: replace` sparingly
and explain why.

Stamp semantic intent around the montage boundaries with `Set Cut
Intent` so the timeline records that this is an associative or thematic
choice, not accidental cover:

```text
*** Begin EDL
*** Set Cut Intent
@@ between: clip_uuid=host-a and clip_uuid=montage-1
+ cut_type: cutaway
+ intent: thematic_montage
+ reason: begin associative sequence about the cost of the decision

*** Insert BRoll
@@ anchor: clip_uuid=host-a
+ asset: raw/montage/factory-floor.mp4
+ duration_s: 2.000
+ position: overlay
*** End EDL
```

When the montage is rhythm-driven, use simple hard cuts unless the
motion/meaning specifically calls for `transition-director`. Visible
transitions still need their own intent.

## Review Gate

Before applying edits, present the montage map:

- thesis
- shot order
- why each image belongs
- duration per shot
- whether each image is literal b-roll, associative, or contrast

Apply only after the user approves the montage direction. After
`apply_edl`, call `view_timeline` to inspect placement and `vedit_diff`
to confirm the graph changed as intended.

## Done

- [ ] The user explicitly opted into thematic montage.
- [ ] The sequence has one stated thesis and 3-7 concrete images.
- [ ] You did not use montage as a dirty-cut repair.
- [ ] The final EDL uses `Insert BRoll` for montage visuals and `Set Cut Intent`
      at the montage boundary where applicable.
- [ ] `view_timeline` and `vedit_diff` confirm the resulting edit graph.
