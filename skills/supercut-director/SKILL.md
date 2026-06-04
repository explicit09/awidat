---
name: supercut-director
description: Build a single highlight reel / supercut / compilation ACROSS many source episodes — enumerate sources, score the best standalone moments per source, verify quoted text, level dialogue, and assemble one hook-first spine interleaving clips from every episode.
version: 0.1.0
tier: editorial
tools_allowlist:
  - list_assets
  - view_episode
  - read_index
  - find_moment
  - find_beat
  - inspect_moment
  - inspect_clip
  - find_dead_air
  - find_filler_words
  - view_timeline
  - view_frame
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Supercut director

Use this when the user wants ONE video stitched from MANY source episodes:
a "best of season 1," a "supercut of every time X happened," a multi-guest
highlight reel, a year-in-review, a clip compilation, or a sizzle reel that
pulls moments from several recordings. This is the multi-source counterpart to
`viral-clip-extractor` (one asset, one clip): here the deliverable spans every
source asset in the project.

If the user actually wants one clip from one episode, load
`viral-clip-extractor`. If they want a thematic/associative montage of images
rather than dialogue moments, load `thematic-montage-director`. This skill is
for spoken-moment compilations across sources.

## What makes this multi-source

The engine already supports cross-source assembly: `list_assets` is uncapped,
each `Insert Clip` op names its own `asset`, and the per-asset index tools
(`read_index`, `find_beat`, `inspect_moment`) take an `asset_id`. The spine is
just many `Insert Clip` ops onto one timeline, each pointing at a different
source episode. Nothing here invents a new op — it orchestrates the existing
ones across more than one asset.

## Workflow

### 1. Enumerate and index sources

- Call `list_assets` to get every source asset. Build a working index:
  `asset_id → duration, speakers, topics, has whisper/editorial-moments`.
- Call `view_episode` per source (or at least the ones in scope) to confirm
  each has a transcript and editorial-moments. If a source has zero topics
  and zero moments, it is unindexed — report it and exclude it rather than
  guessing from raw timecodes.
- Diarization: whisper-mcp labels transcript segments with `speaker_id`
  (legacy key `speaker`). `read_index(asset_id=..., channel="transcript")`
  and `find_moment` carry it through. Capture the speaker per source so the
  per-speaker balance option (step 2) works.
- If the user named a theme ("every time we talked about funding"), seed
  candidate discovery with `find_moment(query="funding")` across all sources
  (omit `asset_id` to search every asset) before scoring.

### 2. Score the best standalone moments across all sources

Reuse `viral-clip-extractor`'s deterministic scorer once PER source — do not
reinvent it. Run it against each asset's sidecars:

```bash
python3 <repo-root>/skills/viral-clip-extractor/scripts/score_moments.py \
  --moments index/editorial-moments/raw/<asset>.json \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --transcript index/whisper/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json \
  --limit 8 \
  --max-overlap-ratio 0.5 \
  > /tmp/supercut/<asset>.json
```

Then merge the per-source candidates into ONE globally ranked selection with a
cold-click quality bar, an optional per-speaker balance, and a per-source cap
so no single episode dominates:

```bash
python3 <skill-root>/scripts/merge_sources.py \
  --source raw/ep-01.mp4=/tmp/supercut/ep-01.json \
  --source raw/ep-02.mp4=/tmp/supercut/ep-02.json \
  --source raw/ep-03.mp4=/tmp/supercut/ep-03.json \
  --min-score 60 \
  --target-count 12 \
  --per-source-cap 3 \
  --balance-speakers
```

The merger returns a `spine` (hook-first, then descending score, each entry
tagged with its `asset`, `speaker_id`, `start_s`/`end_s`, and `spine_position`)
plus `stats` (pool size, count rejected below the bar, per-source and
per-speaker distribution).

**Cold-click test**: every surviving moment must stand on its own to a viewer
who has never seen the source — clear subject, a hook or payoff, and clean
energy. Reject moments propped up by text alone when energy/visual evidence is
weak (this is `score_moments`'s `score_breakdown`/`visual_breakdown`). Raise
`--min-score` until the spine is all cold-click winners; a short strong reel
beats a long padded one.

### 3. Verify quoted text against the transcript

Cross-source selections are where mis-attribution happens — a quote gets
credited to the wrong episode or the wrong speaker. For EACH selected moment,
before it earns a slot on the timeline:

- Call `inspect_moment(moment_id=...)` or
  `read_index(asset_id=<that asset>, channel="transcript")` and confirm the
  quoted words actually occur at `[start_s, end_s)` in THAT source.
- Confirm the diarized `speaker_id` matches who the reel will attribute the
  line to. If a lower-third will name the speaker, the name must come from the
  source's diarization, not from memory.
- If the text or speaker does not match the timestamps, fix the range or drop
  the candidate. Do not ship a misquote.

### 4. Level dialogue, then assemble the spine across sources

Different episodes were recorded at different levels. Before interleaving:

- Per clip, level dialogue for consistent loudness. Use `Set Volume` for
  obvious per-clip gain matching across sources, and reserve a final
  `Set Loudness Target` (step 6) for the whole timeline. A supercut that
  jumps 6 dB between episodes feels broken even when every cut is clean.
- Tighten each clip with `find_dead_air(max_silence_s=0.5)` and
  `find_filler_words(aggressive=true)`, keeping 200ms pre-roll / 100ms
  post-roll so the cut sounds natural.

Assemble with `apply_edl` `Insert Clip` ops, one per selected moment, each
naming its own source `asset`. Place the hook first, then interleave sources by
the merger's `spine_position` (avoid running three clips from the same episode
back-to-back even within the score order):

```text
*** Begin EDL
*** Insert Clip
+ asset: raw/ep-01.mp4
+ track: V1
+ start: 612.400
+ end: 641.900
+ name: hook-ep01-the-real-problem
*** Insert Clip
+ asset: raw/ep-04.mp4
+ track: V1
+ start: 88.200
+ end: 119.000
+ name: payoff-ep04-funding
*** End EDL
```

`Insert Clip` appends in order (or use `at_position` / `at_s` for explicit
placement). After the draft, call `view_timeline` and verify every slot's
source asset, source range, gaps, and total duration match the spine. Anchor
later `Trim Clip` ops by `clip_uuid` (from `view_timeline`), NOT by
`transcript_snippet` on fresh inserts.

### 5. Brand cards and music bed

- Add an intro/outro card and per-segment lower thirds (episode/speaker
  attribution) with `Insert Title`. Keep them short and readable.
- For a music bed, `Insert Clip` the track onto a dedicated audio track, then
  duck it under dialogue with `Set Ducking`:

```text
*** Begin EDL
*** Set Ducking
+ track: Music
+ enabled: true
+ amount_db: -14.0
+ attack_ms: 80.0
+ release_ms: 400.0
*** End EDL
```

Music sits under speech; never let the bed fight the dialogue.

### 6. Review gate, render, verify

- Set the delivery format with `Set Output Format` and a publishable
  `Set Loudness Target` (typically `integrated_lufs: -14` to `-16`,
  `true_peak_db: -1`) across the whole timeline.
- **Review gate**: present the spine as a numbered list before applying the
  final structure / rendering. Show, per slot: source episode, speaker,
  in/out, duration, and why it earned a spot. Pause for confirmation. Confirm
  the OVERALL reel, not every clip.

```
Supercut draft: 12 clips from 5 episodes, ~6m 10s.
 1. HOOK   ep-01  HOST   612.4–641.9  (29.5s)  "the real problem with..."
 2.        ep-04  GUEST   88.2–119.0  (30.8s)  funding payoff
 3.        ep-02  HOST    44.0– 70.5  (26.5s)  ...
Confirm or tell me to swap/trim/reorder. I'll render once you confirm.
```

- After approval, call `vedit_diff` and confirm the graph diff matches the
  approved spine, brand cards, ducking, format, and loudness — then
  `view_timeline` once more for the final source-range / order check.
- `start_render(scope="timeline")`. Poll with `poll_render` until
  `status="completed"`. Report the output path and approximate duration.
- Verify the artifact: duration, nonblack frames, audio present, no unexpected
  gaps, hook first, and no per-episode loudness jumps.

## Rules

- One reel, many sources. Every `Insert Clip` carries its own `asset`.
- Reuse `score_moments.py` per source; never hand-roll a parallel scorer.
- Verify quoted text + speaker against the source transcript at the clip's
  timestamps before it goes on the timeline. Misquotes are the #1 supercut bug.
- Cold-click bar: each moment stands alone or it's cut.
- Balance sources/speakers so the reel doesn't become one episode plus filler.
- `scope="timeline"` only — never `preview` (that renders one raw asset).
- Don't skip the `vedit_diff` + review-gate checkpoints.

## You are done when...

- [ ] `list_assets` enumerated the sources and each in-scope source was
      confirmed indexed (transcript + editorial-moments) via `view_episode`.
- [ ] Per-source candidates were scored with `score_moments.py` and merged
      into one ranked spine with `merge_sources.py`.
- [ ] Every selected clip's quoted text and speaker were verified against
      that source's transcript at its timestamps.
- [ ] The spine interleaves sources, hook-first, with per-clip dialogue
      leveling.
- [ ] Brand intro/outro + attribution titles and (if used) a ducked music bed
      are on the timeline.
- [ ] `Set Output Format` and `Set Loudness Target` were applied for delivery.
- [ ] The user confirmed the OVERALL spine at the review gate.
- [ ] `vedit_diff` and `view_timeline` confirmed the final graph.
- [ ] `start_render(scope="timeline")` completed and the artifact was verified.
- [ ] The final report lists output path, duration, clip count, source/speaker
      distribution, and any blocker (unindexed source, failed anchor).

If a step blocked (a source wasn't indexed, an anchor failed, render errored),
surface the blocker with a one-line fix the user can take.
