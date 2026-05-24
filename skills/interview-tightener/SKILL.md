---
name: interview-tightener
description: Tighten an interview by 20-30% without losing meaning. Removes filler, dead air, and tangents while preserving editorial spine.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - find_beat
  - find_moment
  - inspect_moment
  - inspect_clip
  - read_index
  - view_timeline
  - apply_edl
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Interview tightener

You are tightening an existing interview. The goal is **20-30%
shorter**, with the editorial spine intact and the pacing
noticeably crisper. You do this by removing three things, in order
of priority:

1. **Dead air** > 1.0s (use the audio-energy index)
2. **Filler clusters** (um/uh/repeated false-starts)
3. **Production/meta-direction chatter** ("you can just say...",
   restart/setup talk, planning the next question, instructions about
   how to answer)
4. **Low-score tangents** (`find_beat(kind="tangent", min_score < 0.5)`)

Don't cut content for length alone. If the source is 60 minutes of
gold, the tightened version is 60 minutes. The 20-30% target is a
ceiling, not a quota.

## The 4-step playbook

### 1. Map the cuts

Call `bash` with the `dead_air_filter.py` script bundled with this
skill (resolved against the absolute path the L2 load returned —
typically `<skill-root>/scripts/dead_air_filter.py`):

```bash
python3 <skill-root>/scripts/dead_air_filter.py \
  /tmp/awidat-real/yt-test/index/audio-energy/raw/<asset>.json
```

It returns a JSON list of `{start_s, end_s, duration_s, reason}`
trim candidates. Reasons: `dead_air`, `filler_cluster`. Sort by
duration desc — the longest cuts give the most return-per-edit.

If topic, gaze, shot, or frame-quality sidecars exist, use them as veto
signals before cutting: preserve topic-boundary pauses, direct-address
moments, sharp close-ups, and motion-heavy shots unless the transcript
confirms they are dead time. Awidat's wider index corpus is useful here
because pacing is not just silence math.

### 2. Add tangent candidates

```
find_beat(kind="tangent")
```

Filter to score < 0.5. For each, call `inspect_moment` and check the
`dependencies` field. If the tangent is a dependency for a later
high-score beat, **don't cut it** — it's setup. Otherwise add it to
the cut list.

Also scan editorial-moments/topic/transcript around the same ranges for
in-interview production chatter. If someone stops the interview to plan
the intro, coach an answer, discuss whether to restart, or talk about
recording structure, add that range to the cut list even if it is spoken
content rather than silence. Verify the surrounding question and answer
still connect after the removal.

### 3. Apply the cuts

Sort all cuts by start_s ascending. Apply them via `apply_edl` with
`Trim Clip` ops anchored by `transcript_snippet` (the snippet from
the cut's surrounding transcript) OR by `clip_uuid` (look up via
`view_timeline`). Commit them in batches of ~5 ops per `apply_edl`
call so a single bad anchor doesn't roll back the whole batch.

After every batch, call `view_timeline` and report:

```
Cut batch 3/8: -47s of dead air, -12s of filler.
Running total: removed 4m 18s (8.2% of original).
```

### 4. Render + report

Once the cut list is exhausted, ask the user to confirm the timeline is
ready for final render unless they already gave explicit render approval
in the same turn. Then render `scope="timeline"`, poll until done, and
ask the user to review the finished output before treating it as
deliverable. Final report:

```
Tightened from 47:30 to 36:18 (-23.6%).
Removed:
  - 4m 12s dead air (54 cuts)
  - 1m 48s filler clusters (37 cuts)
  - 5m 12s low-score tangents (3 cuts)
```

## Editorial conventions

- **Floor on dead-air detection**: 1.0s. Below that you're cutting
  natural breath rhythm.
- **Filler cluster definition**: 2+ filler tokens within 1s of each
  other. Single ums in clean speech are kept.
- **Don't cross speaker boundaries** with a single trim. Each cut
  should land entirely within one speaker's turn.
- **Preserve laughter** — it's social glue. Even if it reads as
  "dead air" by RMS, audio-energy's `is_laugh` flag (when the
  indexer populates it) is your override.
- **Trim is one-way**: `Trim Clip` only narrows. Use `Untrim Clip`
  if you need to widen back. Mistakes are recoverable.

## When NOT to use this skill

- The interview already feels tight. Run `view_episode` and look at
  `mean_segment_s` — anything < 8s suggests the speaker is already
  pacy, and there's not 30% to cut without damaging meaning.
- The asset is < 5 minutes. Cut-pacing matters less than the
  individual edit decisions; the tightener's batch approach is
  overkill.

## You are done when...

Persist until ALL of these are true. A "20% tightened" report with
half the cuts un-applied is a lie — finish the cut list before
handing back.

- [ ] You ran `view_episode` and confirmed the audio-energy and
      editorial-moments indexers had output for this asset. Without
      those, the playbook can't run; surface that to the user and
      stop rather than improvise.
- [ ] Every cut from the candidate list either landed (visible in
      `view_timeline` after the corresponding `apply_edl`) or was
      explicitly skipped with a reason (e.g. "skipped: load-bearing
      dependency for moment 0xabcd"). No silent drops.
- [ ] The final tightened percentage is between **15% and 35%**.
      Less than 15% means the cut list was too conservative and the
      promise of the skill wasn't kept. More than 35% means you
      probably damaged meaning — pause and ask the user before
      committing the last batch.
- [ ] If the user wanted a render, `start_render(scope="timeline")`
      was called and `poll_render` returned `status="completed"`.
- [ ] Your final report names: original duration, tightened
      duration, percent shorter, and breakdown by cut category
      (dead air / filler / tangents).

If you applied < 5 cuts before hitting "looks fine", you under-shot.
The skill's contract is 20-30% — surface a reason if you stopped
short, don't pretend the source was already tight.
