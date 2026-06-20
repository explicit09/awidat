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
  - plan_scene_aware_short_form
  - plan_reframe
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

You're cutting a long-form recording down to a vertical short. The
audience scrolls; you have **3 seconds** to earn the next 30. Visual
variety, fast cadence, and captions are the format — they're not options.

## Editorial defaults

- **Length**: target 30–45s for YouTube Shorts, hard ceiling 59s. Allow
  60–90s only for non-Shorts platforms where the story breaks without it.
  If the input is 60 minutes, your job is to cut, not to summarize verbally.
- **Energy gate**: check audio-energy before trusting the transcript. Warn
  or reject clips with low speech ratio, dead zones, or flat delivery even
  when the text looks interesting.
- **Hook**: lands in the first 3 seconds. The strongest single
  punchline / question / claim in the source goes at position 0.
- **Cut cadence**: a static shot held > 1.5s reads slow. Propose cuts
  at every natural pause, even shorter than the podcast threshold.
- **Filler words**: cut aggressively. Use `find_filler_words(aggressive=true)`
  and propose trimming all matches by default.
- **Captions**: burned-in, high-contrast, and treated as information
  design. Required — silent-autoplay is the dominant viewing mode. Move
  captions away from faces, mouths, hands, products, and busy regions.
  Captions should now live at the bottom of the short, including two-person
  split layouts. Do not place captions on the speaker seam. If a speaker,
  product, hands, or important action occupies the bottom region, reserve a
  dedicated bottom caption rail, reduce caption size, shorten phrases, or
  adjust the crop/layout so captions do not overwrite the person.
  Default to word-level karaoke when transcript word timings exist: the word
  currently being spoken is the green highlighted word. Do not use static
  keyword emphasis as a substitute for timed word highlighting, and report any
  fallback caused by missing or unusable word timings.
- **B-roll cadence**: proactive, every 2–4 seconds. Visual variety
  is what keeps the scroll-stop. Talking-head source is still eligible
  for B-roll; the question is whether the sentence creates a visual need,
  not whether the source already contains cutaways. Use generated B-roll,
  stock B-roll, screenshots, charts, stat cards, or simple diagrams when
  they make the spoken point clearer or more watchable.
- **Visual reset**: talking-head footage held > 6s needs a motivated reset:
  punch-in, active-speaker switch, split view, B-roll, stat card, diagram,
  quote card, or text emphasis.
- **Aspect**: 9:16. When source is 16:9, use `find_speaker_oncam`
  or face/gaze evidence to identify the subject, then call `plan_reframe`
  and apply its `montage.reframe` EDL fragment. If a future reviewed
  `reframe_path` is available, prefer that path over a static crop and
  preserve its smoothing, safe-area, and evidence-track metadata through
  render handoff.
- **Two-person vertical**: do not default to a lazy center crop. When both
  people matter, use a split-stacked vertical composition with one speaker
  above the other and no black caption gap. When one person carries the beat,
  fill the frame with that active speaker. For mixed clips, open/reset with
  split context, then switch to active-speaker fill on timed segments. In
  split-stacked dialogue, put the currently active speaker on top and the
  listener/reactor on bottom; swap top/bottom when the active speaker changes
  and the hold is long enough to avoid jitter.
  Follow the video-editor-style evidence chain: face/slot evidence first,
  speaker-to-slot mapping second, layout hysteresis third, screenshot
  verification last. Diarized speaker labels are not proof that a speaker maps
  to left/right correctly. For two-person side-by-side podcast source, split
  the source into exact non-overlapping left/right halves before scaling into
  stacked tiles. Do not use overlapping crops that leak part of one speaker
  into the other speaker's tile. Preserve each half's aspect ratio while
  filling the destination tile; never stretch or squeeze a speaker to fit.
- **Video-editor layout contract**: build a short-form layout config instead
  of hand-tuning one-off crops. The config must include speaker-to-slot/face
  mapping, clip-relative layout segments, source-time offsets, divider width,
  and caption region. Render split layouts by cropping each speaker from their
  exact source half into stacked 1080-wide regions with only a thin divider
  between them; render fill layouts by cropping within the active speaker's
  exact half. If rendering outside Montage for a proof sample, keep the same
  layout contract and verification artifacts. Verify representative
  screenshots before render handoff.

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
boundary. This is where montage's larger index corpus should beat a
transcript-only workflow.

The single beat with the highest `intensity` × concreteness is your
opening. If `find_beat` returns < 3 candidates, the source doesn't
have a strong short — tell the user honestly: "the strongest moments
in this recording are mid-7 intensity; consider whether short-form
is the right format for this content."

### 2. Build the scene-aware edit plan

Before applying generic short-form mechanics, call the reusable scene
intelligence planner for the selected candidate clip:

```
plan_scene_aware_short_form(asset_id=<asset>, clip_id=<timeline-clip>, source_width=<w>, source_height=<h>)
```

Use its structured recommendations as the planning source for captions,
reframing, punch-ins, holds, b-roll, overlays, and MotionScene support.
The planner is read-only: inspect its evidence reasons and EDL fragment,
then apply only the reviewed operations through `apply_edl`.

Treat scene-aware safety as stronger than defaults. If the planner moves
captions away from the bottom third because of a face, mouth, hands, UI,
product, key action, or busy region, keep that adaptive placement unless
new visual evidence proves a safer choice.

### 3. Build the spine

Pick 3–5 beats total (including the hook). Order:

1. Hook (0:00–0:03)
2. Setup or pivot (0:03–0:15)
3. Main moment (0:15–0:45)
4. Payoff or call-to-action (0:30–0:45, or before 0:59 for YouTube)

If the selected source starts with setup instead of a bold claim, question,
or surprise, pull the strongest 2–4s sentence to the front as a cold open.
Reset visibly into the context with a hard cut or fast wipe; do not use a
soft cross-dissolve for this jump.

Use `apply_edl` `*** Move Clip` and `*** Insert Clip` to assemble
the spine BEFORE doing any cleanup. Cuts within an unfinished spine
waste effort.

Emit `*** Set Output Format` with `aspect_ratio: 9:16`, the target
platform when known, and `safe_area: mobile` before the caption pass.
For each selected 16:9 clip that needs vertical delivery, call
`plan_reframe(clip_id=<clip>, aspect_ratio="9:16", subject_center=<evidence>)`
and hand its `edl_fragment` to `apply_edl`. If a reviewed subject-aware
`reframe_path` exists for the selected clip, attach that path as the crop
contract instead of using a static `montage.reframe` effect. Reject paths
with unsorted keyframes, centers outside 0..=1, scale below 1.0, or low
confidence unless the user explicitly approves manual review.

### 4. Tighten

```
find_dead_air(max_silence_s=0.5)         # use 0.3s for YouTube Shorts
find_filler_words(aggressive=true, max_results=50)
```

Bundle every silence ≥ 0.5s, every YouTube Shorts silence ≥ 0.3s, and every
filler into a single `apply_edl` envelope. The user reviews one ghost overlay
covering the whole pass. If speech delivery is slow, use 1.08–1.15x speed;
do not exceed 1.15x for natural speech.

### 5. B-roll pass

```
find_broll_opportunities(duration_s=2.5, max_results=15)
```

Surface candidates as Notes. For shorts, **be greedy** — propose B-roll
at every visual reference the speaker makes. The user culls; you don't
self-censor. A talking-head-only source is not a reason to skip this pass.
It only means in-footage cutaway discovery may not apply; use stock,
generated, screenshot, chart, or card support instead.

Do not cluster B-roll just because multiple triggers are close together.
Before placing, check the transcript and planned layout so each insert has
room to read, does not collide with captions or split/fill speaker changes,
and does not duplicate a nearby visual support beat. Keep the face on-screen
for punchlines, emotional reactions, direct-address lines, and the strongest
human proof moments.

For each accepted b-roll Note, consult the `stock-broll` skill if the
user wants Pexels-fetched cutaways.

### 6. Caption pass

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

For interchange or review outside Montage, emit the same phrase plan as
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

For Opus-style stacked shorts, generate ASS captions directly from word
timings so the active spoken word turns green at the exact moment it is said:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --phrase-preset short \
  --max-words 4 \
  --max-gap-s 0.35 \
  --style impact \
  --output-format ass-karaoke > captions.ass
```

Use `--ass-position-x` and `--ass-position-y` when the vertical layout places
the caption seam somewhere other than the center. If `word_timings` are not
available, do not fake karaoke captions; regenerate/repair word timing or fall
back to non-karaoke captions and report the limitation.

For two-person shorts, produce a dynamic layout plan before rendering:

```bash
python3 <skill-root>/scripts/short_form_layout_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --clip-start-s <source-start> \
  --clip-end-s <source-end> \
  --speaker-slot-evidence-json '{"0":{"slot":"left","confidence":0.9,"method":"lip_activity_or_frame_check"},"1":{"slot":"right","confidence":0.9,"method":"lip_activity_or_frame_check"}}'
```

Use the returned `layouts[]` as the composition contract. `fill` means show
only `active_speaker`; `split_stacked` means show both people with
`top_speaker` above `bottom_speaker`. Apply the same hysteresis idea as
video-editor/Opus: do not swap for tiny backchannels, but do swap or punch in
for a real speaker handoff or an extended monologue.

If one speaker owns at least 80% of spoken time in the selected range, prefer
active-speaker fill for the whole clip instead of forcing a two-person split.
If a single speaker owns an 8s+ turn, fill that speaker for the turn. Merge
same-speaker turns through gaps up to 2s and avoid layout switches shorter
than 3s.

The `speaker-slot-evidence-json` should come from lip/mouth activity when
available, or from explicit frame checks when not. If the plan returns
`status: "needs_review"` or the warning
`speaker_slot_mapping_needs_visual_verification`, inspect frames and correct
the mapping before render.

Caption placement follows the bottom-only rule. Default to karaoke captions
when word timings exist. In `split_stacked`, keep karaoke captions in a bottom
caption rail instead of the speaker seam. In `fill`, keep captions in the
lower third/bottom rail. If the bottom speaker would be covered, reduce font
size, wrap to shorter phrases, or reserve extra bottom space before rendering.
Do not solve this by moving captions to the middle of the frame.

Treat `status: "needs_review"` as evidence to revise phrase length,
line wrapping, cue duration, or caption density before render handoff.

Before claiming caption placement is visually safe, emit adaptive layout
evidence with available visual sidecars:

```bash
python3 <skill-root>/scripts/caption_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --face index/face/raw/<asset>.json \
  --gaze index/gaze/raw/<asset>.json \
  --shot index/shot/raw/<asset>.json \
  --composition index/composition/raw/<asset>.json \
  --frame-quality index/frame-quality/raw/<asset>.json \
  --phrase-preset short \
  --max-chars-per-line 24 \
  --output-format adaptive-layout > caption-layout.json
```

Use each recommendation's `edl_hint` when emitting `*** Insert Caption`:
it preserves phrase timing and `word_timings_json`, maps richer layout zones
onto renderer-supported title positions, and records the selected zone and
estimated bounding box for review. Treat rejected zones as audit evidence:
do not place captions over face, eyes, mouth, subject body, gaze line, key
action, or unsafe frame areas unless the user explicitly accepts manual risk.

To plan keyword emphasis, topic labels, progress labels, callouts, reaction
text, or lower thirds with the same safety rules, pass `--layout-items` with
an `items` array. Each item uses `id`, `overlay_kind`, `text`, `start_s`, and
`end_s`; non-caption items return `insert_title` EDL hints.

When only caption geometry is available, emit the simpler geometry evidence:

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
- `position`: from adaptive layout `edl_hint.position`; bottom-third only
  when the planner says it is safe
- `font_size`: 42–56 by default for bottom captions, large enough on a phone
  but smaller than center/seam captions so the bottom speaker is not overwritten
- `color`: white with 80%-alpha background OR black-on-yellow for
  the highest-energy beats
- `safe_area`: mobile
- `start_s` / `end_s`: tight to the word group, not the sentence
- `word_timings_json`: the phrase's `word_timings` array when available,
  so render can use transcript word starts for karaoke-style reveal instead
  of uniform title-window splits

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
