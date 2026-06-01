# Producer spine (shared handoff discipline)

> Canonical, format-agnostic discipline for any **producer** skill (one that
> owns a video end-to-end). Format-specific producers — `explainer-producer`,
> `podcast-episode-producer` — supply their own *stages*; this file defines the
> *discipline* those stages run under, so the rules live in one place.
>
> `_shared/` is ignored by skill discovery (leading `_`), so this is an
> authoring reference, not a loadable skill. Keep each producer's body lean by
> citing this instead of re-inlining it.

## The model: a file moves down the line

A high-quality edit is many small correct decisions stacked across **ordered
department passes**, each pass owning its craft and trusting the last. That is
the whole method. Encode it, don't improvise it.

## Five rules every producer pass obeys

1. **Preflight before editing.** Confirm assets are present and the project is
   indexed (`view_episode`/`read_index`). Never edit un-indexed footage — you'd
   be editing blind. If indexing hasn't run, stop and say so.

2. **One pass = one craft, in order.** Run stages sequentially. After each,
   summarize in 1–2 sentences what changed before moving on. Don't interleave
   color decisions into the story pass, etc. — that's how edits get muddy.

3. **Checkpoint between passes.** Commit a `vedit` checkpoint at each stage
   boundary (the file "moving to the next desk"). This makes every stage
   revertible in isolation and is the literal handoff. Keep drafts of
   everything — never destructively discard.

4. **Gate the handoff with evidence, not vibes.** Before leaving a structural
   pass, run `assess_edit_quality` (and `assess_continuity` where relevant).
   Route any `Risky`/`Dirty` verdict through a real fix — recut to a
   word/sentence boundary, stamp `Set Cut Intent`, use `Set Audio Lead/Trail`,
   or cover with motivated B-roll. **Never hide a problem with a decorative
   transition.**

5. **Confirm structure, then render once.** Present the overall structure as a
   short numbered list and wait for the user's OK. Then `vedit_diff`,
   `start_render(scope="timeline")`, `poll_render` to completion, verify the
   artifact, and hand off a package — not just an mp4 path. Confirm the *whole*,
   not every clip.

## Cross-cutting craft constraints

- **Good in, good out.** You cannot edit around weak source. When lighting,
  audio, or framing is bad, **flag it and mitigate**, don't claim a fix you
  can't make. Say plainly what needs a reshoot or a finishing pass elsewhere.
- **Restraint / invisible craft.** The best edit isn't noticed. Don't
  over-produce: no decorative transitions, no overlay clutter, no montage
  without a job, no music bed that turns the piece into an ad. If
  `assess_edit_quality` flags high transition/overlay density, pull back.
- **Honesty bounds choices.** Don't fake hype the material doesn't earn. The
  intro sets honest expectations for what the video is.
- **Serve the audience's value.** Cut anything off-message, however good the
  moment. The keep-or-cut test is "does the viewer's value depend on this?"

## Honest-gap reporting

Some MKBHD-method capabilities aren't built yet. When a pass needs one, **report
it as a remaining finishing step** instead of pretending it happened:

- **Sound design** (foley / ambience bed / cued SFX) — no tool yet.
- **Music-as-meaning** (picking a track for what it connotes) — only beat-sync
  exists; semantic selection is manual.
- **Stylistic color finish** (halation / bloom / film emulation) — not in the
  grade path.
- **Audience model** — there's no inferred viewer; the agent must be *told* the
  audience, or use a declared profile if the project supplies one.

## Done-when (every producer)

- [ ] Project was indexed and its shape understood before drafting.
- [ ] Each stage ran in order with a `vedit` checkpoint at the boundary.
- [ ] `assess_edit_quality` gated the structural passes; verdicts were resolved.
- [ ] Every clip on the timeline was actually inspected — no unseen clips.
- [ ] Loudness target + package metadata applied (or the blocker stated).
- [ ] User confirmed the overall structure before render.
- [ ] `start_render(scope="timeline")` completed and the artifact was verified.
- [ ] Unbuilt capabilities were reported as explicit finishing gaps, not faked.
