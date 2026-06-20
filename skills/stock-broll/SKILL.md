---
name: stock-broll
description: Fetch and place stock B-roll from Pexels for moments where the speaker references a concrete visual subject. Distinct from b-roll-suggester (which finds in-footage cutaways) — this skill fetches NEW footage from a stock library.
version: 0.1.0
tier: editorial
tools_allowlist:
  - assess_edit_quality
  - find_broll_opportunities
  - search_broll
  - use_broll
  - apply_edl
  - view_timeline
  - inspect_moment
  - read_index
  - update_plan
---

# Stock B-roll (Pexels)

You're fetching stock cutaways from Pexels for moments where the
speaker references a concrete visual subject — "imagine a busy
office", "look at the city skyline", "picture this graph". The
in-footage `b-roll-suggester` skill covers cutaways the project
already has; this one covers when the project DOESN'T have suitable
footage and you need to bring some in.

## When to use this skill

- The user asks for "b-roll" or "cutaways" and the project's source
  material is mostly talking-head (no suitable in-footage cutaways).
- A short-form talking-head clip needs visual variety, proof, or a concrete
  illustration. Talking-head footage does not block B-roll; it only changes
  the source strategy from in-footage cutaways to brought-in support.
- `b-roll-suggester` already ran and returned "no good in-footage
  match exists for this passage."
- The user asks for "stock footage" or "Pexels b-roll" by name.
- A `broll_suggestion` Note from `find_broll_opportunities` already
  surfaced and the user clicked through it.

Only defer to `b-roll-suggester` when the project has SEPARATE
cutaway assets imported alongside the primary recording (drone shots,
B-cam, demo footage, an explicit `b-roll/` directory in raw/). For
single-asset talking-head projects — only the primary recording in
raw/ — `b-roll-suggester` does not apply, even if `shot_summary` on
the primary asset shows >30% no-face: those are moments the speaker
is briefly off-camera, not cutaway material. Stay on this skill.

When the project DOES have a separate cutaway library, the user's
own footage usually reads better than generic stock — defer to
`b-roll-suggester` for that case.

## Never auto-place stock B-roll over these moments

These beats are parasocial / emotional / proof-of-presence — the face
IS the value, covering them kills the moment. Place stock BEFORE or
AFTER, not during:

- Laughter
- Emotional confession
- Heated debate
- Punchline
- Guest's strongest quote
- Host directly addressing the audience
- Moments with strong facial expression
- Moments where visual accuracy is uncertain (would-be-guessing)

This rule overrides every other rule below. The full trigger
taxonomy (high-confidence / low-confidence / never) lives in the
podcast addendum that's already in your system prompt; refer to it
for which moments TO insert on.

## The 4-step playbook

### 1. Find candidate moments

```
find_broll_opportunities(duration_s=3.0, max_results=12)
```

Returns `[{ at_s, duration_s, reason, pexels_query, transcript_excerpt }, ...]`.

The `pexels_query` is the agent's first guess. Don't use it blindly —
read the `reason` and `transcript_excerpt` first to confirm the
trigger fired correctly. If a trigger fired on a metaphor ("imagine a
world where..." with no concrete noun), skip that finding.

### 2. Search Pexels for each kept moment

For each finding you want to act on:

```
search_broll(query=<pexels_query>, per_page=5)
```

Returns up to 5 candidate clips with `pexels_id`, `duration_s`,
`width`, `height`, `preview_thumbnail`, `attribution`, and
`frame_previews`.

### 3. Surface the previews to the user

Re-emit the `broll_suggestion` Note with `broll_previews` populated
from the `search_broll` results so the BrollNoteCard renders the
thumbnail row. The user picks via the UI's "Use this" button, which
fires a chat directive back to you.

If you're working in chat (no UI handoff), present the top 3
candidates inline as a numbered list with the duration, attribution,
and a one-line "why this one" reason. Ask the user to pick by number.

**Always present at least 2 options.** A list of one isn't a choice.

### 4. Download + place

For the user's pick:

```
use_broll(
  pexels_id=<picked id>,
  anchor={"transcript_snippet": "<the trigger phrase from the finding>"},
  duration_s=<2.0 to 4.0; default 3.0>,
  position="overlay"
)
```

`use_broll` downloads the video to `raw/broll/pexels-<id>.mp4` and
returns an `edl_fragment` ready for `apply_edl`. Hand the fragment
to `apply_edl` to actually place the cutaway.

After `apply_edl`, verify the timeline state instead of trusting the tool
summary. Run `view_timeline` around each anchor and confirm:

- The stock clip landed on an overlay/B-roll track at the intended timeline
  time, not appended to the end or clustered with unrelated inserts.
- The selected asset still matches the transcript phrase and reason that
  triggered it.
- The cutaway does not cover a never-auto-B-roll moment.
- Any search failures, missing `PEXELS_API_KEY`, weak matches, or user-declined
  candidates are listed explicitly.

## Editorial conventions

- **Cutaway duration by situation** (match these — don't invent):
  - Topic transition: 2–4 sec
  - Product / tool mention: 2–5 sec
  - Statistic: 2–4 sec (stat card or chart, not random stock)
  - Complex explanation: 3–8 sec (longer end only for diagrams /
    screen recordings)
  - Covering a jump cut: 1–3 sec
  - Short-form clip cutaway: 1–3 sec
  - Cold open / trailer: 0.5–3 sec, trailer-style
  - Default safe range: **2–4 seconds**. Less is too short to read;
    holding past 8 sec in long-form becomes as boring as no B-roll.
- **Density**: long-form episode keeps it LOW (only at topic shifts,
  examples, stats, products, complex explanations, jump-cut covers).
  Short-form clip targets 15–25% B-roll coverage; explainer clip
  25–35%; cold open / trailer 35–60%.
- **Talking-head shorts**: actively look for B-roll opportunities even when
  the only source asset is a head-to-head interview. Use B-roll to visualize
  nouns, numbers, processes, tools, domains, marketplaces, products, and
  examples; stay on the speaker for emotion, punchlines, direct address, and
  the strongest facial reactions.
- **Match the visual to the literal noun.** "Skyline" → search for
  "skyline at dusk" or "city skyline morning", not "urban energy".
  Pexels' relevance ranker is concrete-friendly.
- **Specific > generic.** "Empty Brooklyn street at dawn" beats
  "city street". The user can always swap a specific match; they
  can't conjure specificity from a generic one.
- **Asset-priority sanity check before searching Pexels:** screen
  recording / screenshot / chart / stat card almost always beat
  generic stock for tech / business / explainer content. If the
  speaker references a real product, real app UI, real chart, or
  real news event, prefer a screen recording or screenshot tool over
  Pexels. Pexels is the right call for archetypes (a busy office, a
  city street, a forest, generic faceless hands on a keyboard) where
  the literal visual doesn't need to be a specific real thing.
- **Position is `overlay` by default**. The underlying audio still
  plays — the user hears the speaker while seeing the cutaway. Use
  `position="replace"` only when the speaker is silent under the
  cutaway window (rare).
- **Attribution matters**. Pexels' license requires crediting the
  uploader. The downloaded clip's `pexels-<id>.mp4` filename
  embeds the id; the per-asset metadata carries the attribution
  string. When the user exports, the credit string should land in
  the export description.

## Common failure modes

- **Pexels rate-limit (429)**: free tier caps at 200 search calls
  per hour. `search_broll` surfaces the 429 with a clear message;
  pause and resume later. Don't retry in a tight loop.
- **PEXELS_API_KEY missing**: the tool returns a clear setup prompt
  ("set the env var or store via OS keychain"). Surface that to the
  user verbatim — don't try to work around it.
- **Per-session download cap (10)**: `use_broll` caps at 10 downloads
  per process to prevent runaway loops. If you hit it, the message
  is clear; tell the user to restart the session if they genuinely
  need more.
- **Trigger fired on a metaphor**: `find_broll_opportunities` matches
  trigger phrases plus a concrete noun, but sometimes the speaker
  uses the noun metaphorically ("the engine of the economy" — engine
  isn't literal). Skip those findings; the cutaway would mislead.
- **Pexels has no good match**: `search_broll` returns 5 weak hits
  with low relevance. Don't pick one anyway — tell the user "no
  strong Pexels match for this query" and suggest either a better
  query or skipping this moment.

## Don't

- **Don't auto-place without user confirmation.** Phase 3 ships with
  user-in-the-loop only — the agent identifies, the user picks. The
  Pexels license + the editorial taste both require it.
- **Don't bundle multiple b-roll downloads into one envelope blindly.**
  Each cutaway is its own decision. Bundling reads as
  spam-the-timeline.
- **Don't search for the same query twice in one session** — you'll
  burn rate budget. The agent's session memory carries the prior
  search; refer back to it in chat.

## You are done when...

- [ ] Every accepted `find_broll_opportunities` moment has either a
      placed cutaway OR a clear reason it was skipped (Pexels match
      too weak; user declined; metaphor not literal).
- [ ] `view_timeline` shows the new b-roll clips on a separate track
      at the intended transcript anchors (the apply layer routes
      overlays to V2).
- [ ] Each placed cutaway is 2–4 seconds.
- [ ] The user explicitly confirmed each placement (the
      "user-in-the-loop" rule).
- [ ] `podcast_visual_polish` does not report accidental B-roll
      clustering, hard-cut edges that need treatment, or an unreviewed
      B-roll package.
- [ ] If you skipped a finding, you said so to the user instead of
      letting them wonder why a Note disappeared.

## Phase 3.7 reactive variant

When `assess_edit_quality` returns a dirty visual cut with
`recommendation.broll=true`, prefer bundling `*** Insert BRoll` over
`*** Insert Transition`. This is especially important when
`style_context.transition_density_last_30s` is high: the b-roll cover
hides the visual jar without adding another visible transition or
altering the dialogue audio. If the recommendation says `recut`,
`Set Audio Lead`, or `Set Audio Trail`, follow that instead of forcing a
stock cutaway.
