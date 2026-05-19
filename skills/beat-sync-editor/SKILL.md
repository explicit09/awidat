---
name: beat-sync-editor
description: Edit footage to music by creating beat-aligned cut plans, motivated transitions, speed accents, and verification of timing tolerance.
version: 0.1.0
tier: creative
tools_allowlist:
  - read_index
  - view_timeline
  - inspect_clip
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Beat-sync editor

Use this when the user asks to cut to music, sync to a beat, make a
montage, hit a drop, or match visual energy to audio rhythm.

## Workflow

### 1. Create the beat plan

Use any available beat/energy JSON, or generate one externally through
`bash` if needed. Then run:

```bash
python3 <skill-root>/scripts/beat_cut_plan.py \
  --beats beats.json \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --shot index/shot/raw/<asset>.json \
  --cut-every 4 \
  --duration-s 60
```

The script outputs target cut points and transition suggestions. When
audio-energy is available, it favors louder/accented beat candidates
instead of a rigid cadence. When the shot index exists, it marks beats
that land during motion-heavy shots so you can prefer cut-on-action
rather than arbitrary beat cuts.

### 2. Align cuts

Use `apply_edl` to split/trim/move clips so cuts land within 50ms of
target beats. Hard cuts are the default. Use transitions only when the
music changes energy or structure. After each batch, call
`view_timeline` and verify the graph now contains the intended clip
order, source ranges, and transition placements. Before render/report,
call `vedit_diff` and verify the diff matches the beat plan.

### 3. Motivate transitions and speed

Use `awidat.cross_dissolve` or `SMPTE_Dissolve` for soft phrase
changes, `awidat.fade_black` only for intentional starts/ends or
chapter resets, and speed accents only on drops or action peaks. Do not
use legacy `awidat.fade_in/out` between ordinary adjacent clips, and do
not add decorative transitions unsupported by `apply_edl`.

### 4. Verify

Render and verify cut alignment. If the plan cannot meet 50ms tolerance,
report the exact clips that need manual source replacement.

## Rules

- Cut on action when possible.
- Match cut density to musical energy.
- Do not keep the same cadence for the whole song.
- Do not use nonexistent transitions.
- Do not call the edit done until `vedit_diff` matches the beat plan.
