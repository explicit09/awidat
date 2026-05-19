---
name: short-form
description: Cut a long-form recording down to a 60s vertical short. Hook in the first 3s, fast cadence, burned-in captions, 9:16 aspect. Loaded when project_type=shorts or the user asks for short-form output.
version: 0.1.0
tier: format
tools_allowlist:
  - view_episode
  - find_beat
  - find_dead_air
  - find_filler_words
  - find_broll_opportunities
  - find_speaker_oncam
  - apply_edl
  - view_timeline
  - vedit_diff
  - start_render
  - poll_render
  - inspect_moment
  - read_index
  - update_plan
  - bash
---

# Short-form vertical (TikTok / Reels / Shorts)

You're cutting a long-form recording down to a 60-second vertical
short. The audience scrolls; you have **3 seconds** to earn the next
30. Visual variety, fast cadence, and captions are the format —
they're not options.

## Editorial defaults

- **Length**: target 60s, hard ceiling 90s. If the input is 60 minutes,
  your job is to cut, not to summarize verbally.
- **Hook**: lands in the first 3 seconds. The strongest single
  punchline / question / claim in the source goes at position 0.
- **Cut cadence**: a static shot held > 1.5s reads slow. Propose cuts
  at every natural pause, even shorter than the podcast threshold.
- **Filler words**: cut aggressively. Use `find_filler_words(aggressive=true)`
  and propose trimming all matches by default.
- **Captions**: burned-in, bottom-third, high-contrast. Required —
  silent-autoplay is the dominant viewing mode.
- **B-roll cadence**: proactive, every 2–4 seconds. Visual variety
  is what keeps the scroll-stop.
- **Aspect**: 9:16. When source is 16:9, use center-crop or smart-crop
  driven by `find_speaker_oncam`. When `metadata.awidat.tracking_package`
  includes `reframe_paths`, prefer the reviewed path over a generic
  center crop and preserve its smoothing, safe-area, and evidence-track
  metadata through render handoff.

## The 5-step playbook

### 1. Find the hook

```
view_episode                       # confirm indexers ran
find_beat(kinds=["hook","punchline","emotional_beat"], limit=10)
```

When the full index set exists, score more than text. Use
`viral-clip-extractor/scripts/score_moments.py` with audio-energy,
editorial-moments, shot, gaze, frame-quality, and topic sidecars. Prefer
moments that are energetic, sharp, direct-address, and close to a topic
boundary. This is where awidat's larger index corpus should beat a
transcript-only workflow.

The single beat with the highest `intensity` × concreteness is your
opening. If `find_beat` returns < 3 candidates, the source doesn't
have a strong short — tell the user honestly: "the strongest moments
in this recording are mid-7 intensity; consider whether short-form
is the right format for this content."

### 2. Build the spine

Pick 3–5 beats total (including the hook). Order:

1. Hook (0:00–0:03)
2. Setup or pivot (0:03–0:15)
3. Main moment (0:15–0:45)
4. Payoff or call-to-action (0:45–0:60)

Use `apply_edl` `*** Move Clip` and `*** Insert Clip` to assemble
the spine BEFORE doing any cleanup. Cuts within an unfinished spine
waste effort.

Emit `*** Set Output Format` with `aspect_ratio: 9:16`, the target
platform when known, and `safe_area: mobile` before the caption pass.
If a subject-aware `reframe_path` exists for the selected clip, attach
that path as the crop contract instead of describing the crop in prose.
Reject paths with unsorted keyframes, centers outside 0..=1, scale below
1.0, or low confidence unless the user explicitly approves manual review.

### 3. Tighten

```
find_dead_air(max_silence_s=1.0)         # tighter than podcast threshold
find_filler_words(aggressive=true, max_results=50)
```

Bundle every silence ≥ 1.0s and every filler into a single `apply_edl`
envelope. The user reviews one ghost overlay covering the whole pass.

### 4. B-roll pass

```
find_broll_opportunities(duration_s=2.5, max_results=15)
```

Surface candidates as Notes. For shorts, **be greedy** — propose b-roll
at every visual reference the speaker makes. The user culls; you don't
self-censor.

For each accepted b-roll Note, consult the `stock-broll` skill if the
user wants Pexels-fetched cutaways.

### 5. Caption pass

```
read_index(channel="transcript", asset_id=<the trimmed timeline output>)
```

For long takes or multi-take projects, first produce a compact transcript
view for selection and caption planning:

```bash
python3 <skill-root>/scripts/pack_transcript.py \
  --transcript index/whisper/raw/<asset>.json \
  --source-label <asset-name>
```

For agent handoff, emit the token-bounded JSON packet so the source labels,
freshness state, and packed markdown travel together:

```bash
python3 <skill-root>/scripts/pack_transcript.py \
  --transcript index/whisper/raw/<asset>.json \
  --source-label <asset-name> \
  --output-format json \
  --max-chars 12000
```

When transcript sidecars contain named groups or notes, produce a selection
pack before assembling the spine:

```bash
python3 <skill-root>/scripts/transcript_selection_groups.py \
  --transcript index/whisper/raw/<asset>.json \
  --source-label <asset-name> \
  --group <group-name-or-note-fragment> \
  --format markdown
```

Use this for story selections, topic groups, note-based review, or query-based
shortlist building. Treat a `blocked` status as evidence that the selected
group/note does not exist or does not overlap transcript segments.

Before finalizing a dense cut, produce an agent-readable timeline review
composite that combines source/output filmstrips, waveform evidence, transcript
word labels, silence bands, and cut markers in one PNG:

```bash
python3 <skill-root>/scripts/timeline_review_composite.py \
  --spec renders/verify/<output-name>/timeline-review.json \
  --out renders/verify/<output-name>/timeline-review.png
```

The spec JSON must include `duration_s`, `words`, `silences`, and
`cut_points_s`; add `source_label` and `output_label` when comparing a source
section with the rendered short. Treat the generated PNG and sidecar JSON as a
review gate: inspect the artifact for awkward cut timing, long silence bands,
and word-label/cut mismatches before claiming the edit is ready.

Generate phrase groups with the helper:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --max-words 4 \
  --max-gap-s 0.5 \
  --max-chars-per-line 24 \
  --style classic \
  --hot-start-s <highest-intensity-start> \
  --hot-end-s <highest-intensity-end>
```

For interchange or review outside Awidat, emit the same phrase plan as
SRT:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --style classic \
  --output-format srt > captions.srt
```

For browser preview or tools that prefer WebVTT text tracks, emit VTT:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --style classic \
  --output-format vtt > captions.vtt
```

Before claiming captions are readable, emit a scorecard:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --max-chars-per-line 24 \
  --output-format scorecard > caption-readability.json
```

Treat `status: "needs_review"` as evidence to revise phrase length,
line wrapping, cue duration, or caption density before render handoff.

Before claiming caption placement is visually safe, emit geometry evidence:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --max-chars-per-line 24 \
  --output-format geometry-scorecard > caption-geometry.json
```

Treat `outside_safe_area`, `missing_contrast_support`, or
`caption_below_overlay` issues as blockers for render handoff until the
caption style, wrapping, or layer order is revised.

For animated or karaoke-style caption planning, generate word-progress
evidence before choosing a render preset:

```bash
python3 <skill-root>/scripts/caption_progress_evidence.py \
  --transcript index/whisper/raw/<asset>.json \
  --sample-times 0.0,0.5,1.0 \
  --style karaoke_fill > caption-progress.json
```

This is an evidence contract, not a renderer. Treat a blocked report as
proof that word timings must be repaired before animated captions are
safe to hand off.

Use `--phrase-preset short` for fast social captions, `medium` for calmer
talking-head edits, and `long` only when readability matters more than pace.
The preset considers duration, pauses, punctuation, speaker changes, and word
count. Use `--max-chars-per-line` when caption text may be too wide for the
target frame; it wraps cue text at word boundaries for JSON, SRT, and VTT
outputs without changing timing.

Use `--style classic` for normal bottom-third captions, `impact` for a
center punch line, `boxed` when busy footage needs a stronger text
container, and `minimal` when the picture should stay quiet.

For each returned phrase, emit `*** Insert Caption` ops with:

- `text`: 2–4 words per title, grouped by word timing, punctuation,
  speaker changes, and breath-length silence gaps
- `position`: bottom-third
- `font_size`: 48–64 (large enough on a phone)
- `color`: white with 80%-alpha background OR black-on-yellow for
  the highest-energy beats
- `safe_area`: mobile
- `start_s` / `end_s`: tight to the word group, not the sentence

This pass is mechanical but high-volume. Use `update_plan` to track
which segments are captioned vs. pending.

### 6. Render verification

Render, then verify:

```bash
python3 <skill-root>/scripts/render_verify.py \
  --file renders/<output>.mp4 \
  --expected-duration-s 60 \
  --max-duration-s 90 \
  --cut-report-dir renders/verify/<output-name> \
  --cut-s <cut-boundary-1> \
  --cut-s <cut-boundary-2>
```

Each `--cut-s` produces a review image centered on a hard cut with a
filmstrip and waveform. Inspect the generated PNGs for black frames,
audio discontinuities, caption overlap, and awkward visual timing. If
verification fails, fix the timeline or report the blocker. The JSON
`review_gate` must be `ready_for_review` before the final report when
cut boundaries were supplied; if it is `needs_review` or `blocked`,
generate or repair the cut-window artifacts first. Do not claim a
finished short without a render path or verification result. Before the
final report, call `vedit_diff` and verify it contains the hook-first
spine, `Set Output Format`, caption nodes, and intended cleanup edits.

For final publish exports, run measured loudness finalization after the
timeline render and before the last verification pass:

```bash
python3 <skill-root>/scripts/final_loudness.py \
  --file renders/<output>.mp4 \
  --out renders/<output>-loudness.mp4 \
  --target-i -14 \
  --target-tp -1
```

Use the finalized file for the final verification command.

## Editorial conventions

- **Don't cut on a punchline**. The audience needs to see the
  speaker's face land it. B-roll comes AFTER, not over.
- **One concept per cut**. If the speaker is mid-thought, cutting
  introduces friction. Land cuts on sentence/clause boundaries.
- **Captions are word-level, not sentence-level**. "the city / was on /
  fire" reads better than "the city was on fire" as a single block.
- **Color the hot moments**. The single highest-intensity beat gets
  black-on-yellow captions; everything else stays white. Subtle
  visual hierarchy that pulls the eye to the climax.

## Common failure modes

- **Length creep**: you assemble a 90-second cut and tell yourself
  it's "almost there." It isn't — the algorithm punishes
  60+. Cut harder.
- **No captions**: skipping the caption pass loses 60%+ of the
  silent-autoplay audience. Non-negotiable.
- **Hook is too clever**: the hook should be PUNCHY (a question, a
  claim, a visual), not subtle. If you're explaining the hook,
  it's not a hook.
- **Generic b-roll**: a stock skyline over "I went to New York"
  is filler. Be specific or skip — `find_broll_opportunities`
  surfaces concrete-noun opportunities; trust it.

## You are done when...

- [ ] Final timeline length is ≤ 60 seconds (≤ 90s only with explicit
      user OK).
- [ ] Position 0 is a hook beat from `find_beat`, not the original
      cold-open.
- [ ] Every silence ≥ 1.0s in the trimmed timeline is gone.
- [ ] Every word from the trimmed transcript has a corresponding
      `*** Insert Caption` overlay.
- [ ] `vedit_diff` was reviewed before final render/report.
- [ ] At least 3 b-roll Notes were surfaced (the user may have culled
      to fewer; that's fine — the surfacing is your contract).
- [ ] `view_timeline` confirms the final structure.
- [ ] Render verification was run or the exact blocker was reported.
- [ ] The user hasn't asked you to "make it shorter" — the format's
      whole game is preempting that ask.
