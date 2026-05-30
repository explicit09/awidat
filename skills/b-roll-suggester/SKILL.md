---
name: b-roll-suggester
description: Find visual cutaways for spoken content from an EXISTING IN-PROJECT B-roll library (separate cutaway assets imported alongside the primary recording). Composes shot type, camera motion, frame quality, and CLIP semantic search. Do NOT use for single-asset talking-head projects — route to stock-broll, yt-broll, or AI generation instead.
version: 0.2.0
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

You are placing visual cutaways for spoken content using cutaway
footage that **already exists in the project** as a separate asset
(drone shots, B-cam, room shots, demo footage, an imported cutaway
library — anything in `raw/` other than the speaker's primary
recording).

B-roll is a retention tool, not decoration. Each cutaway must do at
least one of: **visualize** (show what the speaker references),
**clarify** (make an abstract point concrete), **reset attention**
(break up a long talking-head stretch), or **cover an edit** (hide a
removed silence / filler / jump cut). Random "person typing on
laptop" is filler — it hurts retention. Every insert must be tied to
the sentence being spoken.

## Precondition: this skill needs a real B-roll library

**Do not run this skill if the project's only asset is the primary
recording.** "No-face shots" in a single-asset talking-head project
are NOT cutaways — they are moments the speaker is briefly off-camera,
the camera was pointed at empty studio, or a wide multi-cam frame the
face detector missed. Treating those as B-roll inserts the speaker's
own footage as "B-roll", which is just a jump cut to the same person.

Check first via `view_episode` or by inspecting `raw/`:

- `raw/` contains ONLY the primary recording (one asset, or one
  primary plus transcript/audio sidecars): this skill does NOT apply.
  Route via the addendum's 4-step pipeline to `stock-broll`,
  `yt-broll`, or AI generation depending on what the transcript asks
  for. Tell the user honestly that the project has no cutaway library
  to draw from.
- `raw/` contains separate cutaway material (`b-roll/`, `broll/`,
  `cutaways/`, `drone-shots/`, `demos/`, `screen-recordings/`, etc.):
  proceed below.

## Trigger taxonomy — when to insert at all

Do not ask "can I add B-roll here?" Ask "does this sentence create a
visual need?" Score each candidate moment from the transcript:

**HIGH-confidence (insert):**
- Named entities (people, companies, products, places, tools)
- Visual nouns (data center, classroom, drone, chart, website, app)
- Numbers / statistics (revenue, percent, growth, market size)
- Processes ("first… then… after that…")
- Comparisons ("before vs after", "old way vs new way")
- Historical references ("In 2008", "during COVID", "when X launched")
- Abstract concepts that need simplification (algorithm,
  attention economy, data pipelines, inflation)
- Editing problem areas (long pause, filler-word cluster, off-topic
  cut, jump-cut cover)

**LOW-confidence (use punch-in / angle switch / text overlay
instead, NOT a full-frame cutaway):**
- Generic statements with no concrete subject
- Connector phrases ("anyway", "so", "and then")
- Low-energy filler that doesn't move the conversation

**NEVER auto-B-roll:**
- Laughter
- Emotional confession
- Heated debate
- Punchline
- The guest's strongest quote
- Host directly addressing the audience
- Moments with strong facial expression (anger, surprise,
  vulnerability)
- Moments where the visual accuracy is uncertain — if you'd be
  guessing whether a cutaway is right, don't insert

These are parasocial / emotional / proof-of-presence beats. The face
IS the value. Covering them kills the moment. Insert B-roll BEFORE or
AFTER these lines, not during.

## The 4-step playbook

### 1. Read the visual structure of the cutaway library

```
view_episode    # confirm vision indexers ran on the cutaway asset(s)
shot_summary    # what's the visual texture of the cutaway library?
```

Run `shot_summary` against the **cutaway-library asset(s), not the
primary recording**. If the cutaway library is thin or absent (<20%
no-face shots there), the project doesn't have meaningful B-roll
material to draw from — tell the user honestly and route to
stock-broll / yt-broll / AI generation as above.

### 2. For each spoken passage, find the best cutaway

```
inspect_moment(moment_id=...)   # what concept does this sentence carry?
find_speaker_oncam(at_s=...)    # is the speaker visibly demoing/holding
                                 # something? if YES skip — their footage
                                 # IS the content
broll_candidates(
    asset_id="<cutaway-asset>",  # MUST point at a separate cutaway asset
    min_duration_s=2.0,
    types=["no-face", "wide"]
)
clip_search(query="<concept>", min_score=0.20)
```

The intersection of `broll_candidates` (good cutaways) and
`clip_search` (semantically matching the audio) is your suggestion
list. Rank by `clip_search` score.

### 3. Suggest, don't auto-commit

Present 3-5 candidates per passage with score, duration, type, and a
one-line "why":

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
`apply_edl Insert Clip` to add the cutaway as a layered video clip
on a new track named `broll`.

After applying, inspect the actual graph. Run `view_timeline` around the
spoken anchor and confirm the chosen cutaway is at that timeline time, on an
overlay/B-roll track, and still matches the sentence it was chosen for. If the
timeline shows the cutaway appended elsewhere, clustered with unrelated
cutaways, swapped with another asset, or covering a never-auto-B-roll moment,
stop and fix or report that failure before saying B-roll is done.

### 4. Asset priority (already on the right side — see addendum)

This skill's whole reason for existing is priority slot #1 (user's
own footage) for literal continuity cover, not a symbolic montage. If
you came here, you've already checked the project has a real cutaway
library. If those candidates fail (no matches), fall back DOWN the
priority list — screen-recording / screenshot, chart, then stock-broll,
AI-gen — NOT laterally to in-asset slices. For associative imagery, use
`thematic-montage-director` instead.

## Duration + density

Match these defaults; don't invent your own pacing.

| Situation | Duration | Notes |
|---|---|---|
| Long-form episode topic shift | 2-4 sec | Optionally pair with a title-card text overlay |
| Product / tool mention | 2-5 sec | Logo, screenshot, product shot |
| Statistic | 2-4 sec | Stat card or simple chart — NOT a random classroom |
| Complex explanation | 3-8 sec | Diagram / screen recording can hold the longer end |
| Covering a jump cut | 1-3 sec | Quick, keep energy moving |
| Cold-open / trailer | 0.5-3 sec | Trailer-style fast cuts |
| Short-form clip cutaway | 1-3 sec | Fast pacing |
| Emotional / parasocial moment | 0 sec | Stay on the face |

**Density:**
- Long-form (45-120 min): LOW overall — one opportunity every 8-15
  sec when triggers fire, none when they don't. Don't force.
- Standard interview clip: 15-25% B-roll coverage.
- Explainer / educational clip: 25-35% coverage.
- Trailer / cold open: 35-60% coverage.
- Holding a single cutaway past 8 sec in long-form is as boring as
  no B-roll — come back to the speaker.

## Style modes (pick one)

- **Style 1 — Cinematic documentary** (Diary of a CEO trailers):
  heavy cold opens, dramatic music, fast visual changes, close-ups,
  archival, graphics, AI visuals, text overlays. Use for big
  interviews, emotional stories, expert guests. Apply to episode
  intro / clips, not the whole 90-min episode.
- **Style 2 — Studio conversation with selective B-roll**: mostly
  host/guest face, multi-cam switching, subtle punch-ins, B-roll
  only when a thing is mentioned, occasional screenshot or stat card.
  **Safest default for most podcasts.** Business / tech / creator /
  education.
- **Style 3 — Reaction / commentary** (H3-style): show the
  clip/tweet/image being discussed, keep host reaction visible,
  picture-in-picture, B-roll IS the subject. Internet culture, news,
  drama, product reactions.
- **Style 4 — Educational explainer**: more charts, text overlays,
  diagrams, screen recordings, simple animations, fewer cinematic
  shots. Finance, AI, economics, science, startups, tutorials.

For tech / AI / startup / economics podcasts, **default to Style 2 or
Style 4 — "premium educational conversation"**: A-roll dominant,
B-roll only when it clarifies, screenshots and screen recordings over
generic stock.

## Common failure modes

- **clip_search returns 0 results**: query was too specific or the
  CLIP embedding doesn't generalize. Try broader queries ("a phone"
  instead of "a black Samsung Galaxy S22 with cracked screen").
- **broll_candidates returns 0 results**: thresholds too tight. Drop
  `min_duration_s` to 1.5 or relax `motions` to include `slow-pan`.
- **Cutaway timing feels off**: you cut TO B-roll mid-sentence.
  Always start the cutaway on a sentence boundary; end before the
  next sentence's first word.

## Don't

- Don't compose multi-cutaway montages without checking
  `find_speaker_oncam` first. If the speaker is on-camera delivering
  a key line, KEEP them on screen for that line.
- Don't trust low `clip_search` scores (< 0.18). Below that the match
  is noise. Pick a different cutaway or tell the user no good match
  exists for this passage.
- Don't "be creative" with abstract metaphor cutaways. The skill is
  literal-visual-match. Metaphor is the editor's job.
- Don't reuse the same cutaway twice in one episode unless it's a
  recurring visual motif the user established.

## You are done when…

This skill is a *suggester* — the contract is to hand the user ranked
candidates, not to commit cutaways unilaterally. You're done when ALL
of these are true:

- [ ] `view_episode` confirmed the clip / face / shot indexers ran on
      the cutaway library. If they didn't, you surfaced that
      immediately rather than returning empty suggestions that look
      like "no good cutaways exist."
- [ ] For each spoken passage the user named (or each high-confidence
      trigger you picked), you presented **at least 2 candidates**
      with score, duration, type, and a one-line "why" reason. One
      candidate is not a choice.
- [ ] You filtered out anything with `clip_search` score < 0.18 —
      below that is noise.
- [ ] You did NOT propose B-roll over any item in the never-auto list
      above.
- [ ] You stayed within the duration + density targets above.
- [ ] The user picked which candidates to use AND said "go" before
      you called `apply_edl`. If you applied without confirmation,
      that's a violation of the suggester contract.
- [ ] After `apply_edl`, you called `view_timeline` and confirmed the
      new B-roll track shows up at the intended transcript anchor.
- [ ] You reconciled each placed asset's visual content with the transcript
      phrase it supports, and listed any skipped or failed placements.

If `clip_search` returned 0 results across all your queries, that's a
real signal — say "no semantic match found for this passage" and move
on. Don't pad the list with weak alternatives. Don't fall back to
in-footage slices of the primary recording.
