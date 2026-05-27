---
name: b-roll-suggester
description: Find visual cutaways for spoken content from an EXISTING IN-PROJECT B-roll library (separate cutaway assets imported alongside the primary recording). Composes shot type, camera motion, frame quality, and CLIP semantic search. Do NOT use for single-asset talking-head projects — route to stock-broll, yt-broll, or AI generation instead.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - shot_summary
  - find_beat
  - inspect_moment
  - find_moment
  - broll_candidates
  - clip_search
  - find_speaker_oncam
  - inspect_clip
  - view_frame
  - view_timeline
  - apply_edl
  - update_plan
---

# B-roll suggester

You are finding visual cutaways for spoken content using B-roll
footage that **already exists in the project** (drone shots, room
shots, demo footage, imported cutaway library — anything separate
from the speaker's primary recording). Your job: given a passage of
audio, **suggest what existing cutaway to cut TO and when**.

This skill is for a literal continuity cover or literal explanation
cover. It is not a symbolic montage and not an associative essay edit.
If the user wants metaphor, memory, contrast, or intellectual montage,
route to `thematic-montage-director` instead of stretching literal
b-roll into a different editorial mode.

This skill is the canonical example of "code where reliability
matters more than reasoning." You DO NOT try to do CLIP embedding
math in your head. You CALL `clip_search` with the right query and
trust the score.

## Precondition: this skill needs a real B-roll library

**Do not run this skill if the project's only asset is the primary
recording.** "No-face shots" in a single-asset talking-head project
are NOT cutaways — they are moments the speaker is briefly off-camera,
the camera was pointed at empty studio, or it's a wide multi-cam frame
where the face detector missed the speakers. Treating those as B-roll
inserts the speaker's own footage as "B-roll", which is not B-roll —
it's a jump cut to the same person.

Check first via `view_episode` or by inspecting `raw/`:
- If `raw/` contains ONLY the primary recording (one asset, or one
  primary + transcripts/audio sidecars): this skill does not apply.
  Route to `stock-broll` (Pexels), `yt-broll` (popular references), or
  `find_generated_broll_opportunities` → `plan_generated_media` →
  `start_generated_media_job` → `use_generated_media` (AI-generated)
  depending on what the transcript asks for.
- If `raw/` contains separate cutaway material (b-roll/, broll/,
  cutaways/, drone-shots/, demos/, screen-recordings/, anything other
  than the primary recording): proceed below.

## The 3-step playbook

### 1. Read the visual structure

```
view_episode      # confirm vision indexers ran
shot_summary       # what's the visual texture of the cutaway library?
```

Use the full visual index set when it exists: `shot` for shot type and
motion, `frame-quality` for sharpness/readability, `gaze` for moments
that should stay on the speaker, `clip` for semantic frame search, and
`face` for speaker continuity.

Run `shot_summary` against the **cutaway-library asset(s), not the
primary recording**. If the cutaway library is thin or absent (<20%
no-face shots there), the project doesn't have meaningful B-roll
material to draw from — tell the user honestly and route to stock-broll
or AI generation as above.

### 2. For each spoken passage, find the best cutaway

For each passage the user wants b-roll over (they'll either give
you specific timestamps or you'll pick high-energy beats via
`find_beat`):

```
# What is the speaker actually talking about at this moment?
inspect_moment(moment_id=...)  → returns transcript + key concepts

# What no-face/wide cutaways are available, ranked by usable duration?
# CRITICAL: asset_id must point at a separate cutaway asset, NOT the
# primary recording. Passing the primary asset_id returns "moments
# inside the speaker's own footage" which is never B-roll.
broll_candidates(asset_id="<cutaway-asset>", min_duration_s=2.0, types=["no-face", "wide"])

# For each candidate, does its visual content match the spoken concept?
clip_search(query="<concept from inspect_moment>", min_score=0.20)
```

The intersection of `broll_candidates` (good cutaways) and
`clip_search` (semantically matching the audio) is your suggestion
list. Rank by `clip_search` score.

### 3. Suggest, don't auto-commit

Present 3-5 candidates per passage:

```
For "talking about Samsung's battery problems" at 744s-751s:

  Cutaway A: frame at 1996s (Reddit screenshot - phone exploded)
    clip_search score: 0.31 (strong match)
    available 1996s-2002s (6s wide, no-face, sharp)
    why: literal visual match — exploded phone photo

  Cutaway B: frame at 1241s (gloved hands prying open Note 7)
    clip_search score: 0.27 (good match)
    available 1235s-1248s (13s, slow-pan, sharp)
    why: thematically aligned — battery teardown context

  Cutaway C: frame at 909s (clean S6 product shot)
    clip_search score: 0.19 (weak match)
    available 905s-915s (10s, static, sharp)
    why: generic Samsung visual; use only as filler if A+B don't fit
```

Wait for the user to pick. Once they confirm, draft the EDL via
`apply_edl Insert Clip` to add the cutaway as a layered video
clip on a new track named `broll`.

## Editorial conventions

- **Cutaway duration**: 2-6 seconds. Less and the eye doesn't have
  time to read it. More and the audience starts wondering why the
  host is gone.
- **Don't cut to b-roll on a punchline**. The audience needs to see
  the speaker's face land the joke.
- **Prefer concrete over abstract**: a Reddit screenshot of a
  specific event beats a generic product shot.
- **One cutaway per ~30-60s of talking-head**. More than that
  feels frenetic.
- **Sharp + well-lit only**: filter `broll_candidates` with
  `min_sharp_fraction=0.7` for hero cutaways. 0.5 is the floor.

## Common failure modes

- **clip_search returns 0 results**: query was too specific or the
  CLIP embedding doesn't generalize. Try broader queries
  ("a phone" instead of "a black Samsung Galaxy S22 with cracked
  screen").
- **broll_candidates returns 0 results**: thresholds too tight.
  Drop `min_duration_s` to 1.5 or relax `motions` to include
  `slow-pan`.
- **Cutaway timing feels off**: you cut TO b-roll mid-sentence.
  Always start the cutaway on a sentence boundary; end before
  the next sentence's first word.

## Don't

- Don't compose multi-cutaway montages without checking
  `find_speaker_oncam` first. If the speaker is on-camera and
  delivering a key line, KEEP them on screen for that line.
- Don't trust low clip_search scores (< 0.18). Below that the
  match is noise. Pick a different cutaway or tell the user no
  good match exists for this passage.
- Don't "be creative" with abstract metaphor cutaways. The skill is
  literal-visual-match. Metaphor is the editor's job.

## You are done when...

This skill is a *suggester* — the contract is to hand the user
ranked candidates, not to commit cutaways unilaterally. You're done
when ALL of these are true:

- [ ] `view_episode` confirmed the clip / face / shot indexers ran.
      If they didn't, you surfaced that immediately rather than
      returning empty suggestions that look like "no good cutaways
      exist."
- [ ] For each spoken passage the user named (or each high-energy
      beat you picked), you presented **at least 2 candidates** with
      score, duration, type, and a one-line "why" reason. One
      candidate is not a choice.
- [ ] You filtered out anything with `clip_search` score < 0.18 —
      below that is noise; suggesting it would mislead the user.
- [ ] The user picked which candidates to use AND said "go" before
      you called `apply_edl`. If you applied without confirmation,
      that's a violation of the suggester contract.
- [ ] After `apply_edl`, you called `view_timeline` and confirmed
      the new b-roll track shows up where you placed it.

If `clip_search` returned 0 results across all your queries, that's
a real signal — say "no semantic match found for this passage" and
move on, don't pad the list with weak alternatives.
