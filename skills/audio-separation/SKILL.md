---
name: audio-separation
description: Silence a clip or remove audio over a region while keeping the picture, using the Mute Clip / Remove Audio EDL ops (graph-native, picture held).
version: 0.1.0
tier: editorial
tools_allowlist:
  - read_index
  - view_timeline
  - inspect_clip
  - view_frame
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
---

# Audio separation

Use this when the user wants to **change a clip's sound without changing its
picture** — "mute this shot," "cut the audio under this part but keep the
video," "drop the sound here." This is a graph-native edit: the only durable
outputs are `Mute Clip` and `Remove Audio` EDL ops applied through
`apply_edl`. Never re-render a detached audio file or drop the clip to lose
the sound.

Key distinction from `Set Volume`: `Set Volume 0` lowers a mixable level but
the muxed audio stream is still present; **`Mute Clip` / `Remove Audio` hold
the picture and remove the sound.** Reach for those when the intent is "keep
the image, lose the audio," not "turn it down."

## 1. Locate the audio to remove

Find the clip and the span from the graph and the indexes:

```
view_timeline
inspect_clip(anchor=<clip>)
read_index(channel="transcript", asset_id=<asset>)   # words/segments → spans
read_index(channel="audio-energy", asset_id=<asset>) # loud spans / silences
```

Work out the span in **clip-local visible seconds** (0 = the clip's start).
A transcript word/segment or an audio-energy window gives the timeline time;
subtract the clip's timeline start to get clip-local seconds.

## 2. Apply the edit

**Mute a whole clip** (keep its picture):

```text
*** Begin EDL
*** Mute Clip
@@ anchor: clip_uuid=<clip_uuid>
+ muted: true
*** End EDL
```

`+ muted: false` unmutes. With no other audio override on the clip, unmuting
clears the override entirely.

**Remove audio over a region** (keep its picture):

```text
*** Begin EDL
*** Remove Audio
@@ anchor: clip_uuid=<clip_uuid>
+ start_s: 2.0
+ end_s: 4.5
*** End EDL
```

Repeat `Remove Audio` to silence several spans on the same clip. Use
`+ clear: true` (no span fields) to drop all removed spans on the clip.

Anchoring also works by `transcript_snippet` when you want the span tied to a
spoken phrase rather than a uuid.

## 3. Verify

```
vedit_diff
start_render(scope="timeline")
poll_render
```

`vedit_diff` should show the clip's `awidat` audio override (muted and/or
removed ranges). Render a preview and confirm by ear: the picture is
unchanged across the edit, the audio is silent where intended, and audio on
the rest of the timeline is preserved (the project switches to the decoupled
video-only + audio-mix render path automatically).

## Rules

- The picture is sacred here: never delete or trim the clip to lose its
  sound — that loses the image too. Mute/remove the audio instead.
- Prefer `Mute Clip` / `Remove Audio` over `Set Volume 0` when the intent is
  to hold the image and remove the sound.
- Spans are clip-local visible seconds with `end_s > start_s`.
- v1 audio removal supports unity-speed clips with no split edit (J/L) on the
  same clip; if the render reports `audio removal unsupported`, remove the
  speed change / split edit on that clip or clear the removal first.
- The edit graph is the source of truth. Every change lands through
  `apply_edl`; never emit a standalone audio render.

## You are done when...

- [ ] The clip and span were located from `view_timeline` / `inspect_clip`
      and the relevant index.
- [ ] The intent landed as `Mute Clip` and/or `Remove Audio` through
      `apply_edl` (not a clip delete or a bare `Set Volume`).
- [ ] `vedit_diff` shows the clip's audio override.
- [ ] A render confirmed the picture is held and the audio is silent only
      where intended, or the exact blocker was reported.
