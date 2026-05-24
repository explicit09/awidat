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

Use this for polishing an existing podcast/interview cut. If raw
pre-roll is still present, first use the deterministic transcript
trim/setup planner below to find the publishable start; do not manually
work through pre-roll with split/delete loops.

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

For publish-readiness audio polish, also run:

```bash
python3 <skill-root>/scripts/audio_mix_plan.py \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --target-lufs -16
```

### 2. Build bounded cleanup EDLs

Prefer the CLI planners that emit one kept-range EDL, then apply that EDL
once with `apply_edl`:

```bash
awidat plan-transcript-setup-edl <project> [--asset <asset-or-clip>]
awidat plan-dead-air-edl <project> --min-duration-s 0.8 --silence-threshold-db -40 --keep-padding-s 0.3 [--asset <asset-or-clip>]
awidat plan-false-start-edl <project> [--asset <asset-or-clip>]
awidat plan-transcript-cleanup-edl <project> --min-filler-ratio 0.35 --min-filler-tokens 2 [--asset <asset-or-clip>]
```

Use `cargo run -p awidat-cli -- ...` instead of `awidat ...` when
running from a development checkout that does not have the CLI on PATH.

Apply only one planner output at a time, then call `view_timeline` and
`vedit_diff`. Stop and report the current diff if a planner cannot
produce an EDL or if the edit would require more than one manual
split/delete batch.

Keep pauses that carry meaning: after disagreement, before a reveal,
between speakers, or after a punchline. Do not delete silence fragments
by repeatedly `Split Clip` + `Delete Clip`; that is slow, hard to audit,
and can leave the session log behind the actual timeline.

### 2b. Remove in-episode production chatter

Do not assume the interview is publishable just because the real episode
has started. Podcast recordings often contain mid-interview production
chatter: the host/guest stops to plan the next question, says "you can
just say...", discusses how to introduce themselves, asks whether to
restart, talks about setup, or otherwise directs the recording instead of
answering the interview. Treat this as removable editorial content even
when it appears inside the main interview body.

Before dead-air-only cleanup, inspect semantic signals around candidate
ranges:

```bash
read_index(channel="editorial-moments", asset_id=...)
read_index(channel="topic", asset_id=...)
```

Cut low-score `tangent`, `dead_air`, `false_start`, and production/meta
moments when the transcript is about recording structure rather than the
episode topic. Preserve it only if it sets up a later high-value answer
or the user explicitly wants behind-the-scenes material. For every such
removal, cut linked audio and video together and verify the neighboring
question/answer still makes sense.

### 3. Speed only slow substantive sections

If the script recommends speed changes, apply `Set Speed` only to
sections below 130 WPM and never above 1.15x. Do not speed up already
fast speakers or emotional peaks.

### 4. Loudness, cleanup, and final verification

Apply `Set Volume` only for broad level correction recommended by the
mix plan. Use `Set Clip Audio FX` or `Set Track Audio FX` for graph-native
high-pass/low-pass, hum notch, EQ, noise gate, compression, limiter,
de-ess approximation, and loudnorm cleanup. Apply `Set Loudness Target`
for the delivery target before rendering when the user wants publishable
output. Per-speaker mix imbalance still needs isolated tracks or careful
clip-level/track-level gain decisions.

Before final render/export, ask the user to confirm that the current
timeline is ready to render unless they already gave explicit render
approval in the same turn. Render and verify. If `ffprobe`/FFmpeg
checks fail, fix the timeline or report the exact blocker. After the
render completes, ask the user to watch/review the output and say
whether it looks good or needs changes. Do not claim final delivery
without a render path plus that review handoff, or a clear reason
verification could not run.

## Rules

- Audio energy finds candidates; transcript confirms intent.
- Preserve at least 300ms between speaker turns.
- Do not flatten personality by deleting every verbal tic.
- Do not skip the `vedit_diff` checkpoint.
- Final report must include seconds removed, speed changes, and any
  loudness/speaker-balance concerns.
