---
name: pacing-optimizer
description: Optimize pacing by removing dead zones, preserving intentional pauses, and applying safe speed changes based on content type and speech rate.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - read_index
  - find_dead_air
  - find_filler_words
  - inspect_moment
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Pacing optimizer

Use this when the user asks to make a video tighter, less boring,
faster, cleaner, or more engaging. This is a broad pacing pass, not a
full episode-production workflow.

## Workflow

### 1. Generate a pacing plan

```bash
python3 <skill-root>/scripts/pacing_plan.py \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json \
  --shot index/shot/raw/<asset>.json \
  --content-type interview
```

Content types: `short_form`, `talking_head`, `interview`, `podcast`,
`tutorial`.

### 2. Review before applying

Keep all high-energy sections. Cut dead zones only when transcript
confirms they are not intentional. Topic-boundary silence can be useful
for comprehension, so review any candidate marked `topic_boundary`.
For dialogue, preserve at least 300ms between speakers.

### 3. Apply edits

Use `apply_edl` for cuts and `Set Speed` for safe speed changes. Keep
speed within 1.08-1.15x for speech. Never speed up a section that is
already above 160 WPM. The helper suppresses speed changes over visually
dense/motion-heavy shots when the shot index is present. After applying
each batch, call `view_timeline` and confirm the graph duration,
source ranges, and speed effects match the intended pacing plan. Before
render/report, call `vedit_diff` and confirm the diff contains only the
planned cuts and speed effects.

### 4. Verify improvement

Render or produce a dry verification report. The final should have
higher speech ratio, fewer dead zones, no lost emphasis, and no
flattened emotional arc.

## Rules

- Transcript confirms content value; audio energy finds pacing risk.
- Do not cut mid-sentence.
- Do not remove dramatic silence.
- Do not force a percentage target if the source is already tight.
- Do not call the pass complete until `vedit_diff` has been checked.
