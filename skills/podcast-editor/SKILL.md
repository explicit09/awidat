---
name: podcast-editor
description: Polish podcast or interview audio/video by tightening silence, removing filler, applying speed only where safe, balancing speakers, and verifying loudness.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - read_index
  - find_dead_air
  - find_filler_words
  - find_false_starts
  - inspect_clip
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Podcast editor

Use this for polishing an existing podcast/interview cut. It is not the
episode extraction workflow; use `auto-cutter` or
`podcast-episode-producer` first if raw pre-roll is still present.

## Workflow

### 1. Map audio and transcript

Read the episode shape and transcript/audio sidecars:

```
view_episode
read_index(channel="audio-energy", asset_id=...)
read_index(channel="whisper", asset_id=...)
```

Run:

```bash
python3 <skill-root>/scripts/audio_polish_plan.py \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --content-type interview
```

### 2. Cut only what fails the editorial test

Apply dead-air and filler cuts conservatively through `apply_edl`. Keep
pauses that carry meaning: after disagreement, before a reveal, between
speakers, or after a punchline. After each edit batch, call
`view_timeline` to verify the graph still preserves speaker order and
the intended source ranges. Before render/report, call `vedit_diff` and
confirm the diff matches the polish plan.

### 3. Speed only slow substantive sections

If the script recommends speed changes, apply `Set Speed` only to
sections below 130 WPM and never above 1.15x. Do not speed up already
fast speakers or emotional peaks.

### 4. Loudness and final verification

Apply `Set Loudness Target` for the delivery target before rendering
when the user wants publishable output.

Render and verify. If `ffprobe`/FFmpeg checks fail, fix the timeline or
report the exact blocker. Do not claim completion without a render path
or a clear reason verification could not run.

## Rules

- Audio energy finds candidates; transcript confirms intent.
- Preserve at least 300ms between speaker turns.
- Do not flatten personality by deleting every verbal tic.
- Do not skip the `vedit_diff` checkpoint.
- Final report must include seconds removed, speed changes, and any
  speaker-balance concerns.
