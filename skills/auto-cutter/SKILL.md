---
name: auto-cutter
description: Extract the real episode from a long raw recording, then apply conservative mechanical cleanup for silence, fillers, and false starts.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - find_episode_start
  - find_dead_air
  - find_filler_words
  - find_false_starts
  - inspect_moment
  - inspect_clip
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Auto-cutter

Use this skill when the user wants a one-pass cleanup, "auto edit",
episode extraction, or filler/silence removal. The correct order is:
**extract the real episode first, then clean it**. Do not run mechanical
cleanup across pre-show chatter, rehearsed intros, setup, or multiple
takes.

## Workflow

### 1. Identify the publishable episode

Call `view_episode`, then `find_episode_start`. Treat the start
recommendation as a hypothesis, not a blind timestamp: inspect the
surrounding moment if there are rehearsals, pre-roll, or multiple
welcome phrases.

If the recording contains multiple real episodes, make a plan entry per
episode and ask which one to produce before cutting.

### 2. Build the extraction cut

Use `view_timeline` to get clip anchors. Apply one extraction envelope
with `Trim Clip` or `Insert Clip` operations anchored by `clip_uuid`.
Pass `reasoning` explaining why the chosen start/end are the real
episode boundaries.

### 3. Mechanical cleanup

Run the bundled helper if you have audio-energy and transcript sidecars:

```bash
python3 <skill-root>/scripts/cleanup_plan.py \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --preset standard
```

Then cross-check with:

```
find_dead_air(max_silence_s=1.0)
find_filler_words(aggressive=false)
find_false_starts()
```

For social/short-form output, rerun with `--preset aggressive`; for
interviews or tutorials, use `--preset gentle`.

### 4. Apply and verify

Batch cleanup cuts in groups of 5-10 `apply_edl` ops. After each batch,
call `view_timeline` and report running duration removed. Render only
after the extraction and cleanup pass are complete. Before final render
or final report, call `vedit_diff` and make sure the audit shows only
the intended extraction and cleanup edits.

## Rules

- Never use energy alone to find the episode start.
- Never remove conditional fillers ("like", "you know") unless the
  surrounding transcript still reads naturally.
- Preserve 200-500ms of breathing room after strong statements.
- If cleanup removes less than 10%, say so; do not force cuts.

## Done when

- The episode start was chosen with `find_episode_start`.
- Mechanical cleanup happened only after extraction.
- `vedit_diff` was reviewed before render/report.
- The final render or render plan was verified.
- The report separates extraction edits from cleanup edits.
