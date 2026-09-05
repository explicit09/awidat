---
name: podcast-episode-producer
description: End-to-end production of a published podcast episode from raw recording. Identifies hooks, removes filler, applies a clean cold-open structure, and renders a final mp4.
version: 0.1.0
tier: editorial
tools_allowlist:
  - list_assets
  - view_episode
  - list_episodes
  - read_index
  - find_episode_start
  - podcast_flow_shape
  - podcast_episode_spans
  - apply_episode_spans
  - assess_continuity
  - assess_edit_quality
  - shot_summary
  - find_beat
  - inspect_moment
  - find_moment
  - find_dead_air
  - find_filler_words
  - find_false_starts
  - podcast_editorial_review_pack
  - podcast_apply_accepted_edits
  - podcast_smooth_cut_boundaries
  - podcast_post_draft_check
  - podcast_visual_polish
  - podcast_audio_polish
  - podcast_qc_report
  - transition_context
  - plan_transition
  - find_speaker_oncam
  - plan_visual_support
  - plan_visual_support_proposals
  - revise_visual_support_proposal
  - verify_visual_support_artifact
  - plan_motion_scene
  - find_broll_opportunities
  - find_generated_broll_opportunities
  - start_generated_media_job
  - poll_generated_media_job
  - use_generated_media
  - inspect_clip
  - view_timeline
  - view_frame
  - view_program_frame
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - verify_render
  - update_plan
  - bash
---

# Podcast episode producer

You are producing a finished podcast episode from a raw recording. The
goal is **a single ~30-90 minute mp4 the user can publish today**. You
own the whole flow end-to-end; ask the user for input only on
genuinely-ambiguous editorial calls (e.g. "do you want to keep this
tangent at 22:14?"), not on mechanics.

Completion means the whole producer pipeline, not a few cleanup edits.
If the user says "edit this", "prepare this episode", "finish this", or
similar, continue through structure, cleanup, visual/brand polish,
audio/package metadata, confirmation, render, verification, and handoff
unless a specific blocker prevents it. Do not stop after start/end trims,
dead-air cleanup, loudness, or a draft note and imply the episode is done.

## The editor-order playbook

Run these in order. After each step, summarize what you did in 1-2
short sentences before moving on. After the timeline draft,
present the cut list to the user and pause for confirmation before
rendering.

### 1. Preflight the project

- Call `list_assets` if available, then `view_episode`.
- Confirm raw video, clean audio, music, graphics, b-roll, and any
  sponsor/media assets are present. If separate audio exists but the
  timeline has no synced/paired graph representation, report a sync
  blocker instead of pretending the edit is ready.
- Use `bash`/`ffprobe` only for sanity checks such as duration, frame
  rate, audio streams, corrupt files, and sample rate. Do not mutate
  media or `project.otio.json` through `bash`.
- Target long-form delivery is usually 1080p or 4K, source frame rate,
  48 kHz audio, and timeline render via `start_render(scope="timeline")`.

### 2. Read the episode shape

- Call `view_episode` to see the full episode map (speakers, topics,
  duration, indexed channels).
- Call `find_episode_start` to identify the publishable start. Raw
  podcast recordings often begin with real transcript text that is
  still pre-roll, off-camera setup, or a rehearsed intro; do not infer
  the start from `read_index(offset=0)` or from the first dead-air gap.
- Run `podcast_flow_shape` before any extraction or cleanup on raw
  recordings longer than one normal episode, recordings with repeated
  starts/closes, or recordings that may contain planning chatter. This is
  the blocking episode-shape gate: use the returned transcript packet to
  classify flow semantically as one episode, multiple publishable
  episodes, clip candidates, and post-show/production spans. Literal
  boundary phrases are recall evidence only.
- Run `podcast_episode_spans` or `auto-cutter/scripts/episode_span_plan.py`
  as a secondary hint pass, not as the source of editorial truth. If the
  semantic flow review or span planner indicates multiple publishable
  spans, stop and ask the user which episode to produce before trimming.
- Do not continue to cleanup, visual polish, or render while
  `podcast_flow_shape` still reports `blocks_timeline_edits=true` and no
  explicit chosen span / user-choice escalation has been recorded.
- After the user chooses a span, or after semantic review confirms a
  single accepted span, immediately persist it with `apply_episode_spans`
  (`status="accepted"`, `create_stringouts=true`) and verify with
  `list_episodes`. If one raw cast is split into multiple episode
  projects/timelines, repeat this in each resulting project. Timeline
  clips alone are not sufficient episode-scope metadata.
- Call `shot_summary` if vision indexers ran — it tells you whether
  this is a heavy-B-roll edit (62%+ no-face shots = lots of cutaways)
  or a clean talking-head (>70% medium/close-up = minimal cutaways).
- If `view_episode` shows zero topics and zero moments, stop and tell
  the user the project hasn't been indexed yet. Don't try to edit raw
  footage without the brain.
- Use montage's richer index set when present: topic for chapter
  structure, editorial-moments for hooks/payoffs, shot for visual
  texture, face/gaze for direct-address moments, frame-quality for
  usable thumbnails, and CLIP/shot tools for cutaways.

### 3. Find the real start, ending, and cold open

Find the real start before cleanup. Also find the real ending: remove
tail chatter, "are we done?", standing up, technical talk, and dead air
after the useful close. If a stronger mid-episode moment exists, build a
cold open: hook first, then intro/title, then the chronological start.
Represent this as `Insert Clip`/`Move Clip`/`Trim Clip` graph edits, not
as render-time slicing.

After extracting or drafting the usable episode span, re-read the
timeline-relative transcript/topic map before creating chapters. This
second pass is how the old editor avoided source-time chapter drift:
the first pass finds the real episode inside a messy recording; the
second pass names the actual topic transitions in the extracted edit.
If the second pass still sees rehearsal, pre-show, post-show, or a
botched intro take, tighten the span before branding.

### 4. Identify the editorial spine

Pull the strongest 3-5 editorial beats. Use them to structure the
episode:

```
find_beat(kind="hook", min_score=0.7)         → opens the episode
find_beat(kind="cta",  min_score=0.6)         → closing thoughts / pitch
find_beat(kind="punchline", min_score=0.7)    → mid-episode peaks
find_beat(kind="emotional_peak", min_score=0.6)
```

For each surviving moment, call `inspect_moment(moment_id=...)` to see
the surrounding transcript window + any dependencies. Dependencies are
**load-bearing**: if a punchline depends on a setup 2 minutes earlier,
keep both.

### 5. Radio edit the conversation

This is an audio/story pass before visual polish. Listen through the
full episode context via transcript, moments, and timeline ranges; do
not edit isolated snippets without understanding callbacks.

Use:

```
find_dead_air(max_silence_s=1.5)
find_filler_words(aggressive=false)
find_false_starts()
podcast_editorial_review_pack()
```

Treat these scanners as recall, not judgment. Before proposing cleanup
cuts from transcript/audio evidence, call `podcast_editorial_review_pack`
and classify each relevant packet yourself as `cut`, `keep`, or
`review` with an editorial label. Silence alone is not dead air; a
restart marker alone is not a false start; repeated welcomes alone do
not define episode boundaries. Use before/during/after transcript
context to decide whether the moment is publishable content, setup,
coaching, not-recording chatter, natural pacing, or a real cut.

Before mechanical cleanup, run the deeper semantic planner:

```bash
python3 <repo-root>/skills/auto-cutter/scripts/retake_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json \
  --moments index/editorial-moments/raw/<asset>.json \
  --clip-uuid <clip_uuid>
```

Cut dead air, egregious fillers, false starts, self-corrections,
repeated content, technical glitches, and tangents that do not land.
Also cut production/meta-direction chatter wherever it appears, even
inside the apparent interview body: "you can just say...", "maybe we can
ask...", restart/setup talk, off-camera planning, or instructions about
how the interview should proceed. These are not episode content unless
the user explicitly wants behind-the-scenes material.
Preserve natural cadence. A perfectly de-fillered guest sounds robotic.
For every meaningful removal, check continuity: question still matches
answer, setup still exists for payoff, emotional tone does not jump, and
references like "as I said earlier" still point to something visible.
For retake candidates with `requires_review=true` or any
`continuity_risks`, call `assess_edit_quality` before applying edits.
Route dirty cuts through the recommendation: recut to a sentence/word
boundary, stamp `Set Cut Intent` for a clean hard cut, use
`Set Audio Lead` / `Set Audio Trail` for J/L speaker handoffs, or cover
the visual discontinuity with b-roll. Do not hide mid-sentence or
mid-motion problems with a decorative dissolve.

After applying any transcript, chatter, dead-air, filler, false-start, or
planning removal, run `podcast_smooth_cut_boundaries` using the applied edit
batch, then run every returned `assess_edit_quality` call. A hard cut is not
complete until the boundary is either recut, converted to `Set Audio Lead` /
`Set Audio Trail`, covered with motivated visual support, changed through
`transition_context` / `plan_transition`, or explicitly stamped with verified
hard-cut evidence from the quality check.

### 6. Draft the timeline

Build a 3-act structure on top of the editorial spine:

- **Act I (cold open + intro)**: use `find_episode_start` as the clean
  intro anchor. If a stronger cold-open hook exists, place the hook at
  position 0 and then return to the intro anchor; otherwise start at the
  intro anchor. Never use a rejected setup/rehearsal candidate as the
  published start.
- **Act II (body)**: order the remaining beats by topic-arc, not
  by time-in-recording. If the speaker rambled and you have setup
  + payoff in reverse order, you can re-order on the timeline —
  the source-media seconds in `Insert Clip` ops let you arrange
  any way you like.
- **Act III (close)**: end on the strongest CTA. If there's no
  explicit CTA, end on an emotional peak.

Use `apply_edl` with `Insert Clip` ops to draft. Each clip's
`source_range` should INCLUDE about 200ms of pre-roll and 100ms of
post-roll so the audio cut sounds natural. After the initial draft,
call `view_timeline` to see what you built. Trim each clip with
`Trim Clip` (anchor by `clip_uuid`, NOT by transcript_snippet on
fresh inserts — that fails on round-boundary anchoring).

### 7. Visual polish and graph overlays

Once the conversation flow is locked, make visual decisions:

- For every nontrivial visual-support request, call
  `plan_visual_support_proposals` on the selected transcript/topic/timeline
  context. Review its Proposal-to-Visual-Support `evidence`, rationale,
  confidence, risk, missing information, export intent, and `apply_edl`
  payload before applying anything. Use `revise_visual_support_proposal`
  when the editor asks for changes such as shorter, faster, or transparent
  background, or switching a graphic proposal to B-roll. Review its `diff`
  and `visual_diff` before applying the revised payload. Run
  `verify_visual_support_artifact` on accepted quote, list, and B-roll
  proposals before final `verify_render`. Keep
  `plan_visual_support` as a lower-level lane router when you need to explain
  why the work belongs in b-roll, MotionScene, title, finishing, or
  timeline-edit lanes.
- Treat visual-support planning as the visual reasoning record, not just a
  keyword route. The agent should detect abstract explanations,
  product/asset mentions, factual references, lists/processes, emotional
  emphasis, jump-cut covers, chapter/topic transitions, and sponsor/CTA
  moments before choosing tools.
- Choose the lane from editorial intent: `broll` for actual footage or
  evidence, `motion_scene` for native procedural explainers/diagrams/
  cards/callouts/kinetic text/still overlays, generated media for new
  footage or assets that do not exist yet, `title_annotation` for simple
  lower thirds/captions/arrows/labels, `effects_finishing` for direct
  changes to existing footage/audio, and `timeline_edit` for structural
  edits.
- Explain bigger visual choices before applying them: e.g. "this
  section explains a process, so I am adding a MotionScene step card" or
  "this mention needs real-world evidence, so I am looking for b-roll."
- When the lane is `motion_scene`, call `plan_motion_scene` and apply
  the returned `Set Motion Scene` EDL. MotionScene natively
  previews/renders multi-layer text, rectangle/solid panels, callout
  rectangles, and project-relative still image layers. Use shared
  transforms (`x`, `y`, `width`,
  `height`, `opacity`, `fit`, `scale`, `anchor_x`, `anchor_y`,
  `rotation_deg`) plus layer-local `params.animations` for simple
  fades, slides, scale, and rotation. Use still image layers for logos,
  screenshots, product stills, diagrams, charts, and generated PNG
  overlays. Use B-roll/PiP for actual footage; video/media MotionScene
  layers remain stored with explicit limitations. `plan_motion_scene`
  requires transcript-backed on-screen content: pass `headline` or
  `evidence_text`, and pass exact `step_labels` for step/process scenes.
  Do not call it with only the freeform visual request.
- Route visual work by complexity: use MotionScene for instant branded
  cards, quote cards, step/process cards, callouts, text-on-panel
  explainers, simple diagrams, and still overlays that should preview
  and render natively. Use `drawn-artifacts` when the visual needs a
  real chart from numbers, Manim-style math/diagram motion, Lottie/
  Motion Canvas animation, transparent alpha video, or polish beyond
  MotionScene's native layer set; place the rendered asset as B-roll,
  PiP, or a MotionScene image layer.
- After applying any visual, call `view_program_frame` at the midpoint
  of the visual's timeline window. Inspect the composed frame, not just
  the source asset: text must stay inside frame and boxes, remain
  readable, avoid covering faces/mouths/captions/key objects, and show
  MotionScene plus broadcast overlay together. If it fails, adjust the
  visual and call `view_program_frame` again before declaring the visual
  done.
- Use `find_speaker_oncam` and `shot_summary` to choose speaker angles,
  wide/two-shots, reactions, and resets. Avoid frantic cuts on every
  word swap; hold angles long enough to breathe.
- Hide jump cuts with angle changes, b-roll, title overlays, or
  chapter cards. Prefer motivated visual changes: speaker switch,
  motion, laughter, topic shift, or a concrete referenced object.
- Use visible transitions only when they have an explicit job and recent
  transition density is low. If `assess_edit_quality` reports high
  transition density, prefer b-roll, cut-on-action, or a split edit.
- Use `find_broll_opportunities` and the b-roll skills for products,
  locations, websites, screenshots, charts, logos, photos, and demos.
  B-roll should support the sentence; random b-roll is worse than none.
- For real-number charts, animated stat counters, or polished motion
  assets beyond the MotionScene subset, load the `drawn-artifacts`
  skill (matplotlib/Manim/Lottie generators with alpha-safe outputs).
- Add lower thirds, chapter cards, sponsor cards, intro/outro cards,
  and text callouts with `Insert Title`. Keep them short and readable.
- Use frame-quality/shot/gaze data for thumbnail candidates and camera
  matching notes. If color correction/grading is needed but no color
  primitive exists yet, report it as a required finishing pass instead
  of claiming it happened.
- If the episode uses a show package such as Technologia, load that
  private skill and use its `Set Broadcast Overlay` workflow instead of
  stacking many `Insert Title` clips. Broadcast overlays keep title
  card, lower thirds, host photos, ticker topics, and chapter cards in
  one timeline-level graph config.
- Build one canonical chapter/topic list from transcript/topic evidence
  and reuse it for overlay cards, ticker topics, YouTube chapters,
  metadata, and shorts planning. Chapter density is time-based, not a
  fixed count: aim for a chapter roughly every 4-6 minutes of runtime
  (an 80-minute episode wants ~13-18 chapters, not 8). Let real topic
  arcs set the exact count — a deep-dive can stretch past 6 minutes,
  quick topic runs can tighten toward 3-4. Chapter names should promise
  a viewer payoff ("Why Hardware Startups Move Slower"), not just label
  a subject ("Hardware"). Chapters drive the structural layer (chapter
  cards + YouTube chapters); ticker "NOW DISCUSSING" topics are a
  separate, denser layer that may subdivide a chapter so the lower
  third keeps refreshing — topics need not match chapters 1:1.
- Chapters/topics are timeline-relative after extraction and cuts.
  Primary `Delete Clip` edits shift broadcast overlay timestamps
  forward in the graph, but after major restructuring you should
  regenerate or inspect the overlay config before rendering.

### 8. Audio mix and delivery metadata

Use the graph for what the graph can express:

- Use `Set Volume` for obvious speaker/music balance fixes.
- Use `Set Loudness Target` for publishable output, typically
  `integrated_lufs: -16` to `-14` and `true_peak_db: -1`.
- Use `Set Clip Audio FX` and `Set Track Audio FX` for FFmpeg-native
  high-pass/low-pass, hum notch, EQ, noise gate, compression, limiter,
  de-ess approximation, and loudnorm cleanup.
- Keep intro/outro music and stingers below dialogue; do not let music
  fight speech.
- If room tone fill, detailed color grading, or an audio operation
  outside the supported FX set is required, put it in the final finishing
  report as a remaining post step.

Use `Set Package Metadata` before final render when title/description/
tags/platform are known.

### 9. Confirm with user

Present the timeline as a numbered list:

```
Draft cut: 4 segments, ~28 minutes total.
1. Hook ("Today we're talking about ...") - 18s
2. Story 1 ("So I was working on ...") - 6m 30s
3. Story 2 ("And then this thing happened ...") - 14m 12s
4. CTA ("If you want to learn more, ...") - 22s

Confirm or tell me to swap/trim/extend. I'll render once you confirm.
```

Wait for the user to respond before rendering.

### 10. Review, render, and package

Before rendering, call `vedit_diff` and confirm the diff matches the
approved structure, metadata, thumbnail, and cleanup plan.

Run `start_render(scope="timeline")`. Poll with `poll_render` until
done. Tell the user the output path and approximate duration once
it lands, then ask them to watch/review the output and confirm whether
it looks good or needs changes before calling it final.

After render, verify the file:

```bash
python3 <skill-root>/scripts/render_verify.py \
  --file renders/<output>.mp4 \
  --min-duration-s 1800
```

Then generate the publishing package:

```bash
python3 <skill-root>/scripts/metadata_plan.py \
  --transcript index/whisper/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json \
  --moments index/editorial-moments/raw/<asset>.json \
  --frame-quality index/frame-quality/raw/<asset>.json
```

Use the returned title, description, chapters, thumbnail frame
candidates, and tags as the handoff package.

Watch/review the exported artifact when possible, not just the graph.
Check audio spikes, black frames, awkward cuts, missing graphics,
caption/title timing, names, and whether the episode drags. If the user
needs derivatives, create or report the needed exports: long-form
YouTube, audio-only podcast file, vertical shorts, square/social clips,
thumbnail image, trailer, and transcript/chapters.

### 11. Archive/handoff

Final handoff should name the render path, approximate duration,
publishing metadata, derivative needs, and any manual finishing gaps
such as unresolved low-confidence sync, unsupported audio restoration,
color match, or platform upload.
Archive intent belongs in the report for now unless a project archive
tool exists.

## Editorial conventions

These are the defaults. Override only when the user explicitly asks
or the source material genuinely demands it.

- **Cut breath buffer**: 200ms pre-roll, 100ms post-roll on every
  segment. Slightly asymmetric because audiences forgive a tiny
  pre-breath but notice an abrupt cut-off.
- **Filler**: aggressive on um/uh/repeated false-starts; conservative
  on "you know" / "like" — they're cadence, not filler.
- **Cross-talk**: prefer the speaker who finishes their thought.
- **Tangents**: if `find_beat(kind="tangent")` returns it with score
  > 0.5, keep it (high-scoring tangents are usually the funny ones).
  Below 0.5, cut.
- **Dead air**: anything > 1.0s outside a deliberate dramatic pause.
- **False starts**: remove the bad attempt and keep the corrected
  version when breath/tonality still sounds natural.
- **J/L-cut intent**: if a dialogue cut would be visible, prefer
  `Set Audio Lead` / `Set Audio Trail` before reaching for a visible
  transition. If a hard cut works, stamp `Set Cut Intent` so the edit
  graph preserves why it stays hard.
- **Post-cut gate**: cleanup hard cuts without smoothing or
  `assess_edit_quality` evidence are render blockers. Run
  `podcast_qc_report` before render and fix every `error` issue; do not
  present the episode as finished while workflow sections are `blocked`.
- **Render scope**: ALWAYS `scope="timeline"`. Never `scope="preview"`
  for the final cut — preview gives you the raw asset, not the edit.
- **Lower thirds and chapters**: use `Set Broadcast Overlay` for show
  packages when available; otherwise use `Insert Title` graph overlays.
- **Loudness/package**: use `Set Loudness Target` and
  `Set Package Metadata`, not prose-only promises.
- **Publishing package**: a finished episode includes metadata and
  thumbnail candidates, not just an mp4 path.

## Common failure modes (and recovery)

- **find_beat returns 0 moments**: the editorial-moments indexer
  hasn't run. Tell the user to run `montage index --indexer
  editorial-moments` and pause.
- **find_episode_start returns no recommendation**: the whisper index
  is missing or the intro is ambiguous. Use `find_moment` to search for
  welcome/intro phrases, inspect the surrounding transcript, and ask the
  user only if the evidence still conflicts.
- **Trim Clip fails with "anchor not found"**: a fresh `Insert Clip`
  doesn't have transcript metadata yet. Switch the anchor to
  `clip_uuid` (look it up with `view_timeline`).
- **Render produces a scratchy seam**: `scope="timeline"` re-encodes
  at boundaries by design. If you still hear scratching, check
  whether you accidentally rendered with `scope="segment"` — that
  uses stream-copy and clicks at non-keyframe cuts.

## Don't

- Don't render the raw asset (`scope="preview"`) and tell the user
  it's the edit. It isn't. The edit is `scope="timeline"`.
- Don't trim aggressively without inspecting the dependency graph.
  A punchline without its setup is a lie.
- Don't say sync, audio cleanup, color correction, or platform upload is
  complete unless there is an actual graph/tool step or verified
  artifact behind it.
- Don't ask the user to confirm every clip. Confirm the OVERALL
  structure, then commit + render.

## Long-run context discipline

A full episode production is longer than one context window. Budget
for that from the start instead of discovering it at the end:

- Maintain `update_plan` with one step per production gate. The
  backbone below guarantees COVERAGE — it does not script the edit.
  How you cut inside each gate (cold-open choice, story order,
  punch-ins, b-roll taste, pacing, what deserves a MotionScene) is
  editorial judgment, and loaded brand/private/user skills extend or
  replace gates when they match: fold their workflows into the plan
  as their own steps (a show package expands gate 6 with its overlay
  and chapter workflow; a multicam or short-form skill adds its own
  passes). What you must NOT do is silently drop a gate, or end the
  plan at the confirmation gate — the plan is your recovery spine if
  the context is compacted, and a plan without the render/package
  gates loses them to a late compaction.
  1. Preflight assets, instructions, timeline state
  2. Real start AND ending; cold-open decision
  3. Editorial spine: hooks/punchlines/CTA + dependencies
  4. Radio edit: classify + apply cleanup, repair boundaries
  5. Structure: order the body on the spine
  6. Brand/show package applied (or reported unavailable)
  7. Visual support — LLM/editorial B-roll pass from transcript flow
     first, using finder tools only as scouting/coverage checks; then
     b-roll/stock/source-backed lanes, MotionScene/cards, cancellation
     hygiene for generated media, audio mix + loudness/package metadata
  8. Confirm overall structure with the user
  9. Render scope="timeline", poll to completed, verify
  10. Publishing package: title/description/chapters/tags/thumbnail,
      report output path + duration
- Keep tool output lean. Request `podcast_edit_proposal` /
  review-pack batches of at most ~30 items per pass and apply them
  before requesting more; view timeline windows narrowly (the region
  you are editing, not the whole episode). Giant result dumps crowd
  out the playbook and earlier decisions.
- If you see a context-checkpoint summary instead of full history,
  you were compacted mid-production: re-run `load_skill` for this
  skill (and any brand/director skills the summary names) BEFORE the
  next edit, re-check state with `view_timeline` + `vedit_diff`, then
  continue from the first unfinished plan step. Compaction is a
  checkpoint, not a finish line.

## You are done when...

Persist until ALL of these are true. Stopping early on a coding-style
"that's enough" instinct produces half-edits that the user has to
finish themselves — that's worse than not starting.

- [ ] `view_episode` was called at least once and you understood the
      shape (speakers, topics, indexed channels) before drafting.
- [ ] `find_episode_start` was called before deciding where the
      published episode begins.
- [ ] The real ending was checked and tail chatter/dead air was removed
      or explicitly kept.
- [ ] False starts, repeated content, dead air, and tangents were
      reviewed with continuity preserved.
- [ ] Every clip on the timeline has been verified by either
      `view_timeline` or `inspect_clip` — no clip you've never seen.
- [ ] Visual polish was handled or reported: speaker angle choices,
      jump-cut covers, b-roll opportunities, lower thirds/chapter cards,
      MotionScene plans where needed, and thumbnail candidates.
- [ ] `Set Loudness Target` and `Set Package Metadata` were applied when
      producing publishable output, or the blocker was stated.
- [ ] The user explicitly confirmed the **overall structure**. If the
      user said "looks good" or equivalent, that
      counts. If they're silent, ask once and wait.
- [ ] `vedit_diff` was reviewed before final render/report.
- [ ] `start_render(scope="timeline")` was called (NOT `preview`,
      NOT `segment`, NOT `full`).
- [ ] `poll_render` returned `status="completed"` — not `running`,
      not `failed`. If it failed, you investigated the cause before
      handing back.
- [ ] Render verification ran or the exact blocker was reported.
- [ ] A title, description, chapters, tags, and thumbnail candidate
      list were generated when transcript/topic indexes exist.
- [ ] Derivative needs were handled or listed: audio-only, shorts,
      square/social clips, thumbnail, trailer, transcript, archive.
- [ ] You reported the output path AND approximate duration to the
      user in your final message.

If a step blocked (indexer hadn't run, anchor failed, render errored),
you surfaced the blocker explicitly with a one-line fix the user can
take — not "I tried but it didn't work."
