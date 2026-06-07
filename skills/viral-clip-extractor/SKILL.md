---
name: viral-clip-extractor
description: Find and build 30-90 second social clips from long-form footage using audio energy, editorial moments, hook scoring, captions, b-roll, and verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - find_beat
  - inspect_moment
  - find_dead_air
  - find_filler_words
  - find_broll_opportunities
  - find_speaker_oncam
  - read_index
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Viral clip extractor

Use this when the user asks for highlights, viral clips, TikToks,
Reels, Shorts, or "best moments." Never trust transcript alone. A
clip must have energy, standalone meaning, a hook, and a payoff.

## Workflow

### 1. Score candidates

Prefer `find_beat(kinds=["hook","punchline","emotional_peak","story"])`.
Then run the deterministic scorer over editorial-moments plus
audio-energy sidecars:

```bash
python3 <skill-root>/scripts/score_moments.py \
  --moments index/editorial-moments/raw/<asset>.json \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --shot index/shot/raw/<asset>.json \
  --gaze index/gaze/raw/<asset>.json \
  --frame-quality index/frame-quality/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json \
  --limit 8 \
  --max-overlap-ratio 0.5
```

Reject moments with weak energy even if the text looks interesting.
When the optional vision sidecars exist, prefer candidates that are
sharp, direct-address, visually dynamic, or close to a topic boundary.
Use `score_breakdown` and `visual_breakdown` to explain why a candidate
won. Reject candidates whose total score is propped up by text alone
when the energy, visual, duration, or topic-boundary evidence is weak.
This is an montage advantage over workflows that only score transcript
and energy.

Use `--max-overlap-ratio` when selecting several clips from one source so a
lower-scored near-duplicate does not crowd out a separate payoff.

### 2. Build a standalone spine

Pick 1 hook, 1 setup/pivot, 1 main moment, and 1 payoff. If the best
moment has dependencies, keep or recreate the setup. Target 30-60s;
allow 90s only when the story breaks without the extra time.

### 3. Tighten for social

Use `find_dead_air(max_silence_s=0.5)` and
`find_filler_words(aggressive=true)`. Keep 200-300ms after punchlines.
If delivery is slow, apply `Set Speed` at 1.08-1.15x. Use
`apply_edl` for the clip extraction, trims, moves, and speed effects;
then call `view_timeline` to verify the graph has the selected spine,
source ranges, and hook-first order before formatting.

### 4. Format and caption

Load `short-form` for vertical formatting, captions, safe area, b-roll,
and platform verification. For concrete visual references, load
`stock-broll`; for in-footage cutaways, load `b-roll-suggester`.

### 5. Verify and report

Render the clip. Verify duration, nonblack frames, audio presence, no
unexpected gaps, hook first, captions present, and vedit audit trail.
Call `vedit_diff` before reporting completion and reconcile the diff
against the chosen hook/setup/payoff spine.

## Done when

- The selected clip has a hook in the first 3s.
- The clip passes the standalone test.
- Captions and vertical formatting are planned or applied.
- `vedit_diff` confirms the final timeline changes match the plan.
- Final report lists score, duration, cuts, b-roll, and verification.
