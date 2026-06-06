---
name: split-edit-director
description: Plan and apply J-cut and L-cut split edits as first-class audio-picture grammar.
version: 0.1.0
tier: creative
tools_allowlist:
  - view_timeline
  - inspect_clip
  - assess_continuity
  - assess_edit_quality
  - find_dead_air
  - find_false_starts
  - find_moment
  - transition_context
  - plan_split_edit
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
---

# Split Edit Director

Use this when dialogue, reaction, breath timing, or scene flow needs the
audio and picture cut points to differ. J-cut and L-cut choices are
basic editing grammar, not polish and not a visible transition.

## Principle

Use a J-cut when incoming audio should lead the incoming picture. Use an
L-cut when outgoing audio should trail under the next image. A visible
transition does not solve audio continuity; use `transition-director`
only after the split edit has been rejected or the user explicitly asks
for a visual transition.

Call `assess_edit_quality` before fixing a risky dialogue or breath
boundary. Its `edl_guidance` may include learned lead_s or trail_s
timing ranges from prior accepted edits. Follow those learned defaults
first, then review by ear.

Use `find_false_starts` and `find_dead_air` when the cut may contain a
restart, private aside, bad noise, or dead pause. Do not carry bad audio
across a picture cut just because a J/L-cut is available.

## When To Use

- Speaker handoff: prefer `Set Audio Lead` so the next speaker begins
  before picture.
- Thought continuity: prefer `Set Audio Trail` so the outgoing phrase or
  room tone carries under the next shot.
- Breath or pause preservation: use a short L-cut instead of cutting
  directly into the breath beat.
- Reaction timing: let the listener hear the next thought while seeing
  the reaction, or hold the previous thought under the reaction.

Avoid split edits when the audio contains a false start, private aside,
bad noise, off-camera direction, or legal/medical detail that should not
carry forward.

## Workflow

1. Use `view_timeline` to identify clip UUIDs and linked audio/video
   state.
2. Use `inspect_clip` or transcript-side tools to understand the phrase
   and speaker handoff.
3. Call `assess_edit_quality(at_s, kind="cut" | "trim_in" | "trim_out")`.
4. If it recommends split-edit grammar, call `transition_context` for
   the exact adjacent clips, then call `plan_split_edit` with `j_cut` or
   `l_cut`.
5. Use the returned `Set Audio Lead` or `Set Audio Trail` EDL fragment
   with `apply_edl`.
6. Use `vedit_diff` to confirm the graph metadata and explicit audio
   track changes.
7. Render with `start_render` and `poll_render` when quality matters,
   because desktop preview can be preview-limited for split-edit audio.

## EDL Shapes

J-cut:

```text
*** Begin EDL
*** Set Audio Lead
@@ anchor: clip_uuid=incoming
+ lead_s: 0.450
+ reason: next speaker starts before picture for a smoother handoff
+ confidence: 0.850
*** End EDL
```

L-cut:

```text
*** Begin EDL
*** Set Audio Trail
@@ anchor: clip_uuid=outgoing
+ trail_s: 0.500
+ reason: outgoing thought carries under the reaction shot
+ confidence: 0.850
*** End EDL
```

## Timing Defaults

- Start around 0.25-0.60s for J-cut lead_s unless learned guidance says
  the user accepts shorter or longer leads.
- Start around 0.25-0.80s for L-cut trail_s unless learned guidance says
  the user accepts shorter or longer trails.
- For very tight dialogue pre-laps, a few frames can be enough. Keep
  ambience and room-tone bridges longer when they are carrying space
  rather than speech.
- Keep timing subtle. More than a second needs a story reason and a
  render review.
- Always review by ear; waveform logic can suggest a range but cannot
  judge conversational feel alone.

## Done

- [ ] The edit is a J-cut or L-cut for a stated audio-picture reason.
- [ ] The EDL uses `Set Audio Lead` or `Set Audio Trail`, not a visual
      transition.
- [ ] Learned lead_s or trail_s guidance was respected when available.
- [ ] `vedit_diff` confirms the graph change.
- [ ] Final audio-critical work was rendered or explicitly marked
      preview-limited until render review.
