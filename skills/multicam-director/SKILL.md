---
name: multicam-director
description: Sync N camera angles by waveform, then auto-direct a flattened single-track program cut from the diarized transcript, applied graph-natively through EDL ops.
version: 0.1.0
tier: editorial
tools_allowlist:
  - read_index
  - view_timeline
  - inspect_clip
  - analyze_sync
  - plan_multicam
  - view_frame
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
---

# Multicam director

Use this when the user has **multiple camera angles of the same take**
(podcast, interview, panel, live event) and wants them cut into one
program. This is a graph-native assembly pass: the only durable outputs
are `Set Sync Group` and `Apply Multicam Plan` EDL ops applied through
`apply_edl`. Never produce a detached FFmpeg switch script that bypasses
the edit graph.

The pass has two halves that **must run in order**: first align the
angles in time (`analyze_sync`), then direct the cut (`plan_multicam`).
Planning before syncing produces wrong angles, because the director
scores each camera's sidecars at the program time, and an unsynced
camera's sidecars are offset from that time.

## Prerequisites

This skill is recommendation-driven and reads indexer sidecars. Confirm
the inputs exist before planning:

```
view_timeline
read_index(channel="transcript", asset_id=<audio master>)
read_index(channel="face", asset_id=<each camera>)
read_index(channel="shot", asset_id=<each camera>)
```

- **Whisper with diarization** on the audio master — supplies the
  `segments[]` with `speaker_id` that drive who-is-talking switching.
- **Face** on each camera — supplies `speaker_to_face` so the director
  knows which angle actually shows the current speaker.
- **Shot** on each camera — supplies `type` (close/medium/wide) for
  framing preference and the topic-change wide reset.
- **Frame-quality** (optional) — breaks ties toward the sharper angle.

If any are missing, instruct the user to index first
(`awidat index --indexer whisper|face|shot <project>`). At least two
`Camera`-role assets are required.

## 1. Sync the angles

Run waveform alignment across the cameras (and any external mics):

```json
analyze_sync({"reference_asset":"raw/cam-a.mp4"})
```

Each proposal carries `offset_s`, `confidence`, a `sync_group_id`, an
optional `speed_factor` (drift correction), `manual_offset_required`,
and a ready `Set Sync Group` EDL fragment.

Apply only the proposals you trust:

- `confidence >= 0.35` and no `blocker`: apply the proposal's
  `Set Sync Group` fragment through `apply_edl` (fill in the candidate
  clip's `clip_uuid` from `view_timeline`/`inspect_clip`).
- `manual_offset_required: true` (low confidence) or a drift `blocker`:
  **do not auto-apply.** Surface the proposal, ask the user for the
  offset (or to split the drifting source), then apply with the
  confirmed `offset_s`.

```text
*** Begin EDL
*** Set Sync Group
@@ anchor: clip_uuid=<candidate clip uuid>
+ sync_group_id: sync-cam-a-cam-b
+ offset_s: 2.000000
+ confidence: 0.910
*** End EDL
```

The reference camera needs no sync group (offset 0). After applying,
`vedit_diff` should show one `awidat.sync_group` effect per non-reference
camera.

## 2. Direct the program cut

With offsets applied, run the director:

```json
plan_multicam({"min_hold_s": 3.0})
```

It reads the applied sync offsets and scores each camera **at its own
source time**, so the angles line up. It returns flattened
`Program Video` decisions, each with `source_asset`, `speaker`,
`reason`, `sync_group_id`, and `metadata.offset_corrected`, plus a ready
`Apply Multicam Plan` EDL fragment.

Review before applying:

- **`warnings[]`** — if it reports no applied sync groups for a
  multi-camera project, go back to step 1; the cut is assuming a shared
  timebase and will be misaligned for separate-device recordings.
- **`offset_corrected: false`** on a camera that should have been synced
  means its `Set Sync Group` was not applied — fix step 1 and re-run.
- **`min_hold_s`** — raise it (e.g. 4–5s) if the cut feels twitchy, lower
  it for a punchier edit. The director holds the previous angle until the
  minimum hold elapses, and forces a wide angle at topic-change
  boundaries.
- Spot-check a few cut points with `view_frame` to confirm the chosen
  angle actually frames the speaker.

When the decisions look right, apply the fragment:

```text
*** Begin EDL
*** Apply Multicam Plan
+ plan_json: { "program_track": "Program Video", "decisions": [ ... ] }
*** End EDL
```

`Apply Multicam Plan` atomically replaces the `Program Video` track and
stamps `multicam_source_asset`, `multicam_decision_index`, and
`sync_group_id` on each program clip for `vedit` audit. It validates that
decisions are non-empty, finite, and non-overlapping before mutating, so
a bad plan fails without touching the graph.

## 3. Verify and render

```
vedit_diff
view_timeline
start_render(scope="timeline")
poll_render
```

Confirm `vedit_diff` shows the rebuilt `Program Video` track with the
expected angle clips, then render a timeline preview and watch the cut.
A multicam switch is just sequential clips, so the standard render path
handles it — if render fails, fix the graph or report the exact blocker.

## Rules

- Sync first, direct second. Never run `plan_multicam` before applying
  the sync groups for a separate-device shoot.
- Low-confidence sync is a human decision, not an auto-apply. Respect
  `manual_offset_required` and drift blockers.
- The director proposes angles from objective signals (speaker, framing,
  quality); frame inspection confirms them.
- Keep the cut motivated: switch to the speaker, use wides at topic
  changes, and honor the minimum hold. Do not cut faster than the
  content earns.
- The edit graph is the source of truth. Every sync offset and every
  angle decision lands through `apply_edl`; never emit a standalone
  switch render.

## You are done when...

- [ ] `analyze_sync` ran and every trusted proposal landed as a
      `Set Sync Group` op (low-confidence ones were confirmed by the
      user first).
- [ ] `plan_multicam` ran offset-aware (no unexpected `warnings[]`, no
      stray `offset_corrected: false`).
- [ ] The `Apply Multicam Plan` fragment was applied through `apply_edl`.
- [ ] `vedit_diff` shows the rebuilt `Program Video` track with
      per-clip `sync_group_id` provenance.
- [ ] A timeline render was verified, or the exact blocker was reported.
