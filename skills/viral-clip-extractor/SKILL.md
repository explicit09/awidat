---
name: viral-clip-extractor
description: Find and build 30-59 second social clips from long-form footage using audio energy, editorial moments, hook scoring, captions, b-roll, and verification.
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
  - fetch_x_trend_context
  - plan_short_form_review
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
clip must have energy, standalone meaning, a hook, and a payoff. Audio
energy comes before transcript appeal: a quote that reads well but lands
flat is not a good short.

## Workflow

### 1. Build trend context

Before final short selection, look at what the episode is talking about and
compare it with current X/web/news discourse. Pull 1-5 specific episode-topic
queries from the transcript/topics, then call:

```bash
fetch_x_trend_context(queries=[...])
```

When web/news context comes from another source, shape it into the same
`trend_context` payload. Use specific queries, not broad categories:
`"AI coding agents startup prototypes"` beats `"AI"`, and `"domain investing
founders"` beats `"business"`.

Pass the resulting context to `plan_short_form_review` first with
`profile="viral_social"`, `discovery_mode="harvest"`, and
`max_candidates=50`. Harvest mode is the broad opportunity pass: it should
surface overlapping variants and clusters so good shots are not left
unreviewed. Treat trend alignment as a boost between clips that already pass
the hook/context/payoff test; it must never rescue a flat, confusing,
incomplete, or visually weak clip.

If trend lookup is unavailable, blocked by missing credentials, or has no
strong match, continue from episode evidence and report that trend relevance
was unknown or weak.

### 2. Score candidates

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
  --limit 50 \
  --max-overlap-ratio 1.0
```

Use the broad list to identify source clusters and likely variants. After
harvest, run or filter a review pass with tighter limits:

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

The trend context passed to `plan_short_form_review` should look like:

```json
{
  "asset_id": "raw/<asset>",
  "profile": "viral_social",
  "discovery_mode": "harvest",
  "max_candidates": 50,
  "trend_context": {
    "signals": [
      {
        "source": "x",
        "label": "AI coding agents",
        "keywords": ["AI coding agents", "prototypes"],
        "weight": 0.9,
        "reason": "current discourse maps to the episode topic"
      }
    ]
  }
}
```

Treat `trend_alignment.matched` as a boost, not a replacement for the
standalone/hook/payoff checks. If trend context is unavailable or does
not match a candidate, continue from episode evidence.

Read each returned candidate's `visual_decision_plan` before building
the spine. It names existing Montage tools such as `plan_reframe`,
`plan_multicam`, `plan_visual_support_proposals`, `plan_emphasis`, and
`find_generated_broll_opportunities`; use those recommendations instead
of inventing new visual primitives.

Read `vertical_layout.composition_mode` and `vertical_layout.segments`
as the short-form composition contract:

- `active_speaker_fill` means one speaker carries the clip; avoid split
  layouts and fill/punch in on that speaker.
- `split_stacked` means both speakers/faces matter throughout; preserve
  both in a stacked/split vertical view.
- `dynamic_switching` means open or reset with split/stacked context, then
  follow each timed segment's `fill_speaker` recommendation when one
  speaker owns the beat.
- `native_vertical` means the source is already vertical; keep the native
  composition unless visual evidence says otherwise.

Do not apply a two-person split just because the source is a podcast. Let
the contract decide from speaker and face evidence, then use `plan_reframe`
or `plan_multicam` only for the selected segment layout.

For two-person clips, verify speaker-to-face/slot mapping before render.
Prefer lip/mouth-activity evidence when available; otherwise inspect frames
at representative solo speaking moments. If one speaker owns at least 80% of
spoken time, prefer active-speaker fill. If one speaker owns an 8s+ turn, fill
that turn. Avoid layout switches shorter than 3s so short backchannels do not
make the frame jitter.

Use `--max-overlap-ratio` when selecting several clips from one source so a
lower-scored near-duplicate does not crowd out a separate payoff.

### 3. Build a standalone spine

Pick 1 hook, 1 setup/pivot, 1 main moment, and 1 payoff. If the best
moment has dependencies, keep or recreate the setup. Target 30-45s for
YouTube Shorts and never exceed 59s. Allow 60-90s only for non-Shorts
platforms when the story breaks without the extra time.

If the strongest line is not already in the first 3s, use a cold open:
place the strongest 2-4s sentence first, then visibly reset into the full
context with a hard cut or fast wipe. Do not use a cross-dissolve for this
reset; it is too soft to signal the jump.

### 4. Tighten for social

Use `find_dead_air(max_silence_s=0.5)` and
`find_filler_words(aggressive=true)`. For YouTube Shorts, tighten silence
to 0.3s where the cut still sounds natural. Keep 200-300ms after punchlines.
If delivery is slow, apply `Set Speed` at 1.08-1.15x. Use
`apply_edl` for the clip extraction, trims, moves, and speed effects;
then call `view_timeline` to verify the graph has the selected spine,
source ranges, and hook-first order before formatting.

### 5. Format and caption

Load `short-form` for vertical formatting, captions, safe area, b-roll,
and platform verification. Follow its two-person contract: split-stacked
when both people matter, active-speaker fill when one person carries the
beat, and dynamic switching when the evidence changes over time. For
concrete visual references, load `stock-broll`; for in-footage cutaways,
load `b-roll-suggester`.

### 6. Verify and report

Render the clip. Verify duration, nonblack frames, audio presence, no
unexpected gaps, hook first, captions present, and vedit audit trail.
Call `vedit_diff` before reporting completion and reconcile the diff
against the chosen hook/setup/payoff spine.

## Done when

- The selected clip has a hook in the first 3s.
- The clip passes the standalone test.
- Current X/web/news trend context was checked or explicitly reported as
  unavailable/weak.
- Captions and vertical formatting are planned or applied.
- `vedit_diff` confirms the final timeline changes match the plan.
- Final report lists score, duration, cuts, b-roll, and verification.
