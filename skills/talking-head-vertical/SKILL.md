---
name: talking-head-vertical
description: Build a modern vertical talking-head short using Montage evidence, subject-aware layout, tight natural pacing, captions, emphasis motion, and reviewable EDL.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - read_index
  - find_beat
  - inspect_moment
  - find_dead_air
  - find_filler_words
  - find_speaker_oncam
  - plan_reframe
  - plan_emphasis
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - verify_render
  - update_plan
  - bash
---

# Talking-head vertical

Use this when the user asks to make a selfie, founder video, rant,
advice clip, educational short, or direct-to-camera creator video into
a modern vertical short. The goal is not a generic short-form package.
It should feel edited for one speaker looking at the viewer: stable
subject-aware framing, fast but human pacing, captions that avoid the
face, and subtle emphasis only where it helps.

This skill composes Montage's existing evidence and tools. Do not cut,
crop, caption, or render media with ad hoc FFmpeg commands. Use the
helper script for deterministic planning, then apply its EDL through
`apply_edl`.

## Evidence first

Run or inspect these indexes when available:

- face: face position, headroom, mouth/eye region, center stability
- gaze: direct-address ratio and hook confidence
- shot: talking-head shot type and visual stability
- composition: framing quality and negative/empty space
- frame-quality: sharpness/readability confidence
- audio-energy: dead air and audio continuity risk
- whisper transcript: hook text, word timings, captions, filler
- topic: boundaries where a pause may be emotional or useful
- editorial-moments: hook, punchline, emotional beat, payoff

If face or transcript evidence is missing, stop and surface the missing
indexers. Do not pretend a source is a talking-head candidate from text
alone.

## 1. Build the plan packet

Use the deterministic planner before making edit decisions:

```bash
python3 <skill-root>/scripts/talking_head_plan.py \
  --asset-id <asset-id> \
  --clip-id <timeline-clip-uuid> \
  --source-width <pixels> \
  --source-height <pixels> \
  --transcript index/whisper/raw/<asset>.json \
  --audio-energy index/audio-energy/raw/<asset>.json \
  --moments index/editorial-moments/raw/<asset>.json \
  --face index/face/raw/<asset>.json \
  --gaze index/gaze/raw/<asset>.json \
  --shot index/shot/raw/<asset>.json \
  --composition index/composition/raw/<asset>.json \
  --frame-quality index/frame-quality/raw/<asset>.json \
  --topic index/topic/raw/<asset>.json
```

The planner returns:

- `candidate`: whether this is a talking-head vertical candidate
- `visual_analysis`: face position, eye line, headroom, stability,
  framing, negative space, and sharpness
- `layout`: native-vertical keep/repair or horizontal-to-9:16 reframe
- `hook`: the selected first-three-seconds beat
- `pacing_plan`: dead-air cuts plus filler/false-start review ranges
- `caption_plan`: phrase captions, safe-area metadata, readability and
  geometry scorecards, face-overlap risk
- `motion_plan`: one restrained punch-in/reset plan by default
- `edl`: reviewable graph-native operations for `apply_edl`
- `readiness`: preflight status and blockers

Treat `readiness.status == "blocked"` as a real blocker. Fix missing
evidence, caption readability, face overlap, or hook placement before
applying the EDL.

## 2. Candidate and layout rules

Accept candidates only when the evidence shows sustained face presence,
spoken transcript, stable/sharp picture, and enough direct-address or
medium/close-up framing to support a talking-head edit.

Layout decisions:

- Keep native vertical framing when headroom, eye line, and stability are
  already good.
  The `keep_native_vertical` strategy means keep native vertical footage
  instead of adding an unnecessary crop.
- Reframe horizontal footage to 9:16 around the speaker using
  `montage.reframe` or `plan_reframe` evidence.
- Repair vertical framing when the source is vertical but the face is
  too low/high or unstable.
- Reserve negative space for captions or keyword overlays only when the
  composition index reports safe unused space.
- Avoid covering eyes, mouth, or important gestures. If bottom captions
  overlap the face/mouth band, move captions to top only when top space
  is safe.

## 3. Hook-first assembly

Prefer editorial moments scored with transcript, audio energy, gaze,
shot, frame quality, and topics. The hook must appear at timeline
position 0 and within the first 3 seconds. Favor direct-address claims,
mistakes, questions, emotional turns, and sharp founder/advice framing.

If the planner selected a fallback transcript hook because
editorial-moments are missing, call `find_beat` and inspect the candidate
before applying. Do not present a weak cold-open as a hook.

## 4. Talking-head pacing

Use the planner's talking-head thresholds:

- Cut dead air at 0.8s or longer.
- Keep short emotional/topic-boundary pauses.
- Review filler clusters, false starts, and repeated low-value phrasing.
- Avoid robotic over-cutting; preserve breath and facial reactions after
  punchlines.
- Use `Set Speed` only after review, only for slow speech, and keep it
  subtle.

Apply cuts via `apply_edl`, then call `view_timeline`.

## 5. Captions and emphasis

Captions are phrase-level, word-timed, mobile safe-area captions. Use
the generated `Insert Caption` operations. They should be readable on a
phone and should not block the face, eyes, mouth, or gestures.

Use emphasis styling only for key words or the hook phrase. For motion,
default to a single small punch-in and reset on the hook. Use
`plan_emphasis` for manual refinement, not repeated motion on every
sentence. No meme/SFX-heavy treatment in v1.

## 6. Apply, verify, and audit

Apply only the reviewed EDL:

```bash
apply_edl(edl=<planner edl>)
view_timeline
vedit_diff
```

Before reporting the draft is ready:

- `view_timeline` confirms hook-first order and 9:16 output format.
- `vedit_diff` contains only planned trims, captions, reframe, and
  emphasis motion.
- Render or run the project `verify_render` tool when a render exists.
- Check hook in first 3 seconds, captions present/readable, face not
  blocked, mobile safe area respected, no unexpected black frames, no
  silent gaps, and audio is intact.

## Done when

- The source passed or explicitly failed talking-head candidate checks.
- The selected layout strategy is evidence-backed.
- The first beat is hook-first, not merely the original opening.
- Pacing removes dead air and filler clusters without flattening human
  pauses.
- Captions are phrase-level, safe-area aware, and face-safe.
- Emphasis motion is subtle and sparse.
- All changes are represented as Montage EDL operations.
- `view_timeline`, `vedit_diff`, and render/readiness verification have
  been checked or the exact blocker is reported.
