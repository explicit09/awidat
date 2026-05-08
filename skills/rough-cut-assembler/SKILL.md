---
name: rough-cut-assembler
description: Assemble raw footage into a coherent first cut by identifying takes, selecting the best usable material, removing dead zones, and preserving story structure.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - read_index
  - find_dead_air
  - inspect_clip
  - view_timeline
  - apply_edl
  - vedit_diff
  - update_plan
  - start_render
  - poll_render
  - bash
---

# Rough-cut assembler

Use this for raw footage, takes, assembly edits, first cuts, and
organizing footage into a watchable sequence. A rough cut optimizes for
story clarity, not final polish.

## Workflow

### 1. Establish intent

Identify content type: interview, voiceover, b-roll with narration,
screen recording, or multi-camera. If the user provided an outline,
use it as the assembly order.

### 2. Score takes

```bash
python3 <skill-root>/scripts/score_takes.py \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --frame-quality index/frame-quality/raw/<asset>.json \
  --gaze index/gaze/raw/<asset>.json
```

The script groups transcript segments into takes separated by dead
zones and scores completeness, energy, delivery, sharpness, and
direct-address. Missing vision sidecars are okay; use them when present.

### 3. Assemble

Use `apply_edl` to insert or move selected takes in story order. Rename
or reason in commits so the audit trail says why each take survived.
After each assembly batch, call `view_timeline` and verify the graph
order, source ranges, gaps, and total duration match the intended story
structure. Before final report, call `vedit_diff` and confirm the graph
diff matches the take-selection report.

### 4. Verify story and mechanics

Check: no accidental dead gaps, selected takes are not duplicates, story
has setup/content/payoff, and technical issues are surfaced.

## Rules

- Prefer complete thoughts over high-energy fragments.
- Later takes are often cleaner but not automatically better.
- Do not remove authenticity from interviews.
- Do not skip the `vedit_diff` checkpoint.
- Do not call it done without a rough-cut report.
