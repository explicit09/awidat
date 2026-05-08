---
name: tutorial
description: Edit screen-recording-heavy tutorial content. Hold key frames, never cut over typing, generate chapter markers from topics, tolerate thinking pauses. Loaded when project_type=tutorial or the user asks for tutorial cleanup.
version: 0.1.0
tier: format
tools_allowlist:
  - view_episode
  - read_index
  - find_dead_air
  - find_filler_words
  - shot_summary
  - clip_search
  - apply_edl
  - view_timeline
  - inspect_moment
  - inspect_clip
  - update_plan
---

# Tutorial / screen recording

You're editing tutorial content — code walkthroughs, software demos,
how-to videos. The audience is here to **learn**, not be entertained.
Pacing rules invert: holds are good, hard cuts are bad, thinking
pauses are part of the value, and the speaker's screen IS the visual.

## Editorial defaults

- **Hold key frames**: when the speaker is showing a code snippet,
  diagram, or important UI state, do NOT cut for at least 4–6 seconds.
  The audience needs time to read.
- **Never cut over typing or drawing**: if the index shows sustained
  on-screen typing, cuts must land at the *natural pause* before or
  after the typing burst, not mid-keystroke.
- **Chapter markers**: every topic boundary becomes a `*** Insert Title`
  overlay as a chapter heading. One to four words sourced from the
  topic summary.
- **Filler tolerance**: thinking pauses are content. Default
  `find_filler_words(aggressive=false)` and only surface clusters or
  unusually long fillers (> 600ms).
- **Silence threshold**: trim silences ≥ 3.0s; preserve silences <
  2.0s. A two-second pause while the user reads code is intentional.
- **B-roll is rare**: the speaker's screen is the visual. Use
  `find_broll_opportunities` only when the speaker pauses the
  demonstration to make a verbal point that lacks visual support.

## The 4-step playbook

### 1. Read the structure

```
view_episode                  # confirm indexers ran
read_index(channel="topic")   # the topic segmentation IS your chapter map
shot_summary                  # how much of the asset is screen recording vs. talking head?
```

If `shot_summary` shows < 50% screen-recording shots, this isn't a
tutorial — it's a talking-head explainer. Tell the user and offer
to switch playbooks (likely `interview-tightener`).

If frame-quality and CLIP sidecars exist, use them to protect readable
screen states: blurry frames are poor chapter anchors, while sharp
frames that semantically match the topic are preferred hold points.

### 2. Generate chapter markers

For each topic boundary returned by `read_index(topic)`:

```
*** Insert Title
@@ anchor: transcript_snippet="<first ~5 words of topic>"
+ text: <topic.label>          # 1-4 words; "Setup", "First request", "Error handling"
+ position: top-banner          # full-width strip, not bottom-third
+ font_size: 36
+ color: white-on-dark           # consistent across all chapters
+ start_s: <topic.start_s>
+ end_s: <topic.start_s + 2.5>   # 2.5s read, then fade
+ animation: fade_in_out
```

Update the plan via `update_plan` after each chapter lands so the
user can see chapters accumulating.

### 3. Identify typing-burst windows (the no-cut zones)

Before any cut, check that the proposed cut point is NOT inside a
typing burst. The proxy for this is shot_summary's `motion_signature`:
typing reads as sustained-but-low motion, distinct from both static
(no motion) and conversational (irregular motion).

Practical heuristic: when the user requests a cut at `t`, call
`assess_continuity(at_s=t, kind=Cut)` — Phase 2's `rule_mid_motion`
catches the egregious cases. For tutorial-specific protection, also
check the `clip_search(query="typing")` results to know where the
typing-bursts live.

### 4. Tighten the silences (with the higher threshold)

```
find_dead_air(max_silence_s=10.0, min_silence_s=3.0)
```

Note the inverted thresholds vs. podcast: the *minimum* silence to
trim is 3.0s here, not 1.2s. The user is reading code; give them
time. Bundle the trims as a single `apply_edl` envelope.

Skip `find_filler_words` unless the user explicitly asks — clusters
of "uh" while debugging is the speaker thinking, which is content.

## Editorial conventions

- **Chapter labels are noun phrases, not sentences**. "Database setup"
  beats "First we set up the database". The chapter banner is a
  navigational aid, not narration.
- **Code on screen → resist cutting**. When in doubt, don't cut.
  A held-too-long shot is forgiving; a cut-too-soon shot loses the
  audience.
- **Speaker reaction shots matter less here**. Unlike podcast cleanup,
  the audience doesn't need to see the speaker's face when the screen
  has the answer. Permission to keep the screen on screen.
- **Don't add b-roll over a code demonstration**. The cutaway covers
  the actual content. If the speaker is talking ABOUT the code while
  the code is visible, leave the code visible.

## Common failure modes

- **Cut over typing**: the most common mistake. Always verify with
  `assess_continuity` AND the typing-burst check before proposing
  any cut.
- **Too many chapter markers**: 1 chapter per ~3 minutes feels right.
  20 chapters for a 30-minute video is noise; 3 chapters is too few.
  Aim for 6–10.
- **Aggressive filler-cutting**: makes the speaker sound robotic.
  Default to leaving fillers alone unless the user complains.
- **Skipping chapters**: tutorials without chapters are unwatchable
  on YouTube. The chapter pass is non-negotiable; if `read_index(topic)`
  returns empty, ask for `start_indexing` to populate the topic
  sidecar before continuing.

## You are done when...

- [ ] Every topic boundary has a chapter `*** Insert Title` overlay.
- [ ] `find_dead_air(min_silence_s=3.0)` returns 0 results on the
      trimmed timeline.
- [ ] No proposed cut lands inside a typing burst (verified via
      `assess_continuity` + the typing-shot heuristic).
- [ ] `view_timeline` shows the final structure with chapters
      visible at each topic boundary.
- [ ] If the user asked about pacing, you can quote how many cuts
      you made and where (the audit trail comes from
      `view_timeline` and the EDL diff).
