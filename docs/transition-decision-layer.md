# Montage Transition Decision Layer

This document defines the strategy for giving agents taste around
transitions. The transition renderer now knows how to execute semantic
transition specs and data-only compositions. The next layer decides
whether a transition belongs at a cut, which transition to use, and why.

## Research Summary

Good transition choice is primarily editorial, not technical.

- Walter Murch's Rule of Six is a useful priority order for cuts:
  emotion, story, rhythm, eye trace, screen plane, and spatial
  continuity. Montage should rank transition decisions the same way:
  only use visual continuity repair after emotion/story/rhythm justify
  calling attention to the cut.
- A hard cut is the default continuity tool. A transition needs a job:
  time passage, topic/chapter change, emotional softening, beat impact,
  visual discontinuity repair, spatial motion continuity, or deliberate
  style.
- Dissolves/fades generally imply time, memory, topic drift, emotional
  softness, or a chapter reset. Wipes/slides/pushes imply graphic or
  spatial movement. Flash/zoom/glitch imply energy and style, and should
  be short.
- OTIO already models transitions as adjacent timeline items with
  `in_offset` and `out_offset`. This makes handle availability part of
  the decision, not only render validation.
- FFmpeg `xfade` is enough for phase-one proof. Shader systems such as
  `gl-transitions` prove the future backend space, but do not change the
  agent contract: choose intent first, lower later.

References:

- Walter Murch Rule of Six summary:
  https://blogs.ischool.berkeley.edu/i290-viznarr-s12/the-rule-of-six-walter-murch/
- OTIO transition structure:
  https://opentimelineio.readthedocs.io/en/v0.17.0/tutorials/otio-timeline-structure.html
- FFmpeg xfade:
  https://ayosec.github.io/ffmpeg-filters-docs/6.1/Filters/Video/xfade.html
- Adobe transition overview:
  https://www.adobe.com/creativecloud/video/discover/video-transitions.html
- GL Transitions:
  https://github.com/gl-transitions/gl-transitions

## Product Contract

The decision layer must produce one of three outcomes:

```json
{
  "decision": "hard_cut",
  "confidence": 0.91,
  "reason": "Same speaker, same thought, clean sentence boundary. A transition would call attention to the edit."
}
```

```json
{
  "decision": "transition",
  "id": "montage.slide_left",
  "duration_s": 0.35,
  "intent": "hide_motion_jump",
  "energy": 0.65,
  "direction": "left",
  "confidence": 0.78,
  "reason": "The topic continues, but visual motion jumps. A short left slide follows screen direction and hides the discontinuity."
}
```

```json
{
  "decision": "composite",
  "id": "montage.composite",
  "duration_s": 0.42,
  "intent": "beat_hit_motion_cover",
  "energy": 0.82,
  "direction": "right",
  "composition": {
    "version": 1,
    "primitives": [
      {"op": "push", "direction": "right", "distance": 0.9, "start": 0.0, "end": 1.0, "easing": "ease_out_expo"},
      {"op": "flash", "color": "#ffffff", "peak": 0.22, "start": 0.35, "end": 0.55, "easing": "ease_in_out"}
    ]
  },
  "confidence": 0.72,
  "reason": "The cut lands on a beat and also needs motion cover; the saved presets are close but too plain."
}
```

Normal editing must stay data-only. Agents may emit built-in transition
ids or `montage.composite` recipes over stable primitives. They must not
emit raw FFmpeg filter graphs, GLSL, shell commands, plugin code, or
opaque generated backend code in project metadata.

## Cut Context Packet

Before choosing a transition, the agent or future tool should assemble a
bounded context packet around one adjacent clip boundary.

```json
{
  "cut": {
    "timeline_s": 42.3,
    "from_clip_uuid": "clip-a",
    "to_clip_uuid": "clip-b",
    "from_asset_id": "raw/episode.mp4",
    "to_asset_id": "raw/episode.mp4"
  },
  "story": {
    "same_topic": false,
    "topic_shift": "pricing -> customer story",
    "speaker_change": true,
    "sentence_boundary": true,
    "before_text": "that is why pricing matters",
    "after_text": "then the customer said"
  },
  "rhythm": {
    "near_beat": true,
    "beat_kind": "punchline",
    "beat_distance_s": 0.08,
    "energy_before": 0.4,
    "energy_after": 0.75,
    "nearby_cut_density": 2
  },
  "visual": {
    "scene_change": true,
    "motion_before": "left",
    "motion_after": "left",
    "motion_mismatch": false,
    "face_continuity": "broken",
    "gaze_continuity": "unknown",
    "frame_similarity": "low"
  },
  "technical": {
    "incoming_handle_s": 0.4,
    "outgoing_handle_s": 0.6,
    "max_centered_duration_s": 0.8,
    "supports_overlap": true
  },
  "style": {
    "project_type": "podcast",
    "pace": "medium",
    "transition_density_last_30s": 1,
    "user_style": "clean"
  }
}
```

The packet should be compact enough for agents and tests, but explicit
enough that a transition decision can be explained and reproduced.

## Signal Sources In Montage

| Signal | Existing source | Use in decision |
| --- | --- | --- |
| Timeline adjacency | `view_timeline`, OTIO project read | Identify candidate cut and adjacent clip ids |
| Handles | OTIO `source_range`, media `available_range`, apply/render validation | Bound duration and alignment |
| Transcript text | `read_index(channel="transcript")`, whisper sidecars | Sentence boundary, semantic continuity, quote context |
| Topic shift | `read_index(channel="topics")`, `view_episode` | Chapter/time-passage decisions |
| Beat and emotion | `find_beat`, `inspect_moment`, editorial-moments sidecar | Flash/zoom/punch decisions, or no transition for serious tone |
| Continuity risk | `assess_continuity` | Dirty/risky cut repair |
| Silence/audio | `read_index(channel="audio_levels")`, continuity inputs | Breath beat, room tone, audio glue |
| Motion/scene | motion and scenedetect sidecars through `assess_continuity` | Cut-on-action, slide/wipe direction, avoid mid-motion jumps |
| Frames | `view_frame` | Visual confirmation near the cut |
| Face/gaze/shot | existing sidecars used by other skills | Speaker continuity, direct-address preservation |
| Color/look | color analysis sidecar | Avoid flashy transitions across sensitive exposure/color changes |
| Style memory | vedit history, project type, skill context | Transition density and taste consistency |

Current state: `transition_context` assembles the deterministic
cut-context packet for adjacent clip boundaries, and `plan_transition`
consumes that packet for a conservative read-only recommendation. Topic
and beat proximity still come from their dedicated tools unless they are
passed into the planner as an explicit objective.

## Decision Policy

Hard cut remains the default. The decision process is:

1. Reject impossible transitions.
   - No adjacent clips.
   - No overlap handles for requested duration/alignment.
   - Cut is inside tight reasoning/dialogue and no continuity repair is
     needed.

2. Decide if a transition has a job.
   - `soft_time_passage`: topic drift, elapsed time, emotional bridge.
   - `chapter_reset`: strong section boundary, end, cold reset.
   - `beat_hit`: music beat, laugh, punchline, reveal.
   - `hide_motion_jump`: visual discontinuity or mid-motion risk.
   - `screen_direction`: existing motion suggests a direction.
   - `style_accent`: user/project style calls for visible polish.

3. Choose the lowest-attention transition that solves the job.
   - Hard cut when the cut is clean and story/rhythm work.
   - `montage.cross_dissolve` for soft topic/time/emotion.
   - `montage.match_dissolve` for a real visual echo, memory bridge, or
     graphic match between related images.
   - `montage.fade_black` for strong reset/end/chapter break.
   - `montage.flash_white` for beat hit/reveal/high energy.
   - `montage.slide_*` or `montage.smooth_push_left` for motion cover and
     screen direction.
   - `montage.motion_blur` for short motion cover when the visual signal
     says motion is the problem but direction is unknown.
   - `montage.whip_pan_left/right` for short, fast screen-direction
     motion only when footage already motivates a pass-by or whip.
   - `montage.wipe_*` for deliberate graphic movement.
   - `montage.zoom_in` for punch-in or forward momentum.
   - `montage.pixelize` only for tech/glitch context.
   - `montage.radial` only for stylized reveals.
   - `montage.composite` when two jobs need to combine, for example
     push plus flash, or zoom plus blur.

4. Set duration by taste and handle constraints.
   - 0.12-0.20s: punch, flash, short social impact.
   - 0.22-0.35s: normal motivated transition.
   - 0.40-0.70s: deliberate chapter/time passage.
   - Clamp to available handles. Prefer `alignment` changes over
     forcing a bad transition.

5. Explain the decision.
   - Always write `intent`.
   - Store a short reason in agent output or future metadata/log.
   - If choosing hard cut, explain why no transition is better.

## Scoring Sketch

The first implementation should be deterministic before it becomes
model-heavy.

```text
need_transition =
  continuity_risk * 0.30 +
  topic_shift * 0.20 +
  beat_hit * 0.20 +
  visual_mismatch * 0.15 +
  style_request * 0.10 -
  dialogue_tightness * 0.25 -
  transition_density_penalty * 0.15
```

Recommended thresholds:

- `< 0.35`: hard cut.
- `0.35..0.60`: use a subtle built-in only if the job is clear.
- `0.60..0.80`: use a motivated built-in.
- `> 0.80`: built-in or `montage.composite`, depending on whether one
  transition family solves the job.

The score is not the final answer. It is a traceable prior that the
agent can override with a reason.

## Proposed Implementation Plan

### Phase A: Decision Skill Upgrade

Update `skills/transition-director/SKILL.md` to require a context pass:

1. `view_timeline` around the cut.
2. `assess_continuity` at the boundary when timing is known.
3. `read_index` for transcript/topics/audio when available.
4. `find_beat`/`inspect_moment` when rhythm or editorial beats matter.
5. `view_frame` before/after for ambiguous visual choices.
6. Apply only if the transition has a named job.

This is low risk because it changes agent behavior without adding a new
runtime surface.

### Phase B: `transition_context` Read-Only Tool

Status: implemented in the worktree as a deterministic read-only tool
registered in CLI, TUI, and desktop sessions. It builds the context
packet for one adjacent boundary:

```json
{
  "between": {
    "from": {"clip_uuid": "clip-a"},
    "to": {"clip_uuid": "clip-b"}
  },
  "window_s": 6.0
}
```

Current output includes:

- adjacent clip metadata and source ranges
- computed handles and max safe transition durations
- transcript window before/after
- continuity verdict
- suggested `view_frame` timestamps
- missing-signal list

The tool intentionally does not choose the transition yet. Topic and
beat proximity remain Phase C inputs unless they are added to the
context packet directly.

### Phase C: `plan_transition` Read-Only Tool

Status: implemented in the worktree as a deterministic read-only tool.
It consumes the `transition_context` packet and returns one or more
ranked decisions:

```json
{
  "recommended": {
    "decision": "transition",
    "id": "montage.slide_left",
    "duration_s": 0.32,
    "intent": "hide_motion_jump",
    "energy": 0.62,
    "direction": "left",
    "reason": "..."
  },
  "alternates": [
    {"decision": "hard_cut", "reason": "..."},
    {"decision": "transition", "id": "montage.cross_dissolve", "reason": "..."}
  ],
  "edl_fragment": "*** Insert Transition\n..."
}
```

This mirrors other Montage planning tools: propose structured edits, then
the agent commits through `apply_edl`. The first implementation is
conservative: clean/no-job contexts become hard-cut intent; dirty/risky
or named-job contexts get one supported visible transition only when the
context reports enough safe centered handles.

### Phase D: Agent Workflow Integration

The agent workflow becomes:

1. Identify candidate cuts from user request or timeline window.
2. Call `transition_context`.
3. Call `plan_transition` or reason from the context.
4. If recommendation is hard cut, do nothing and explain.
5. If recommendation is transition/composite, apply via `apply_edl`.
6. Verify with `view_timeline`, `vedit_diff`, and optionally render.

### Phase E: Evaluation Fixtures

Add fixtures that prove taste, not just renderability. Product coverage
now exercises `transition_context` -> `plan_transition` on a synthetic
boundary, and the live tier accepts mounted real-project transition
planner fixtures at `.montage/eval/transition-planner-flow.json` and
`.montage/eval/transition-planners/*.json`. Use negative EDL assertions
for hard-cut cases so fixtures prove no visible transition was emitted:

- same speaker, same sentence -> hard cut
- clean speaker change -> hard cut or subtle dissolve only when tone
  calls for it
- topic shift without beat -> cross dissolve or fade black depending
  strength
- beat hit/reveal -> flash or zoom, short duration
- visual motion mismatch -> slide/push direction follows motion
- no handles -> hard cut or aligned/shortened fallback
- dirty continuity verdict -> transition or b-roll cover
- high transition density -> prefer hard cut
- tutorial chapter boundary -> title/card first, transition only if
  visually needed

Tests should assert the decision packet and reason, not only the EDL.

## Future: Learning Taste

After deterministic planning works, Montage can learn style from accepted
or rejected transitions:

- transition density per minute
- families the user accepts/rejects
- preferred duration range
- project type defaults
- transition jobs the user approves, for example `beat_hit` but not
  `soft_time_passage`

This should tune scores and defaults, not bypass the requirement that
every transition has an intent and reason.

## Non-Goals

- Do not let normal editing generate raw backend code.
- Do not choose transitions based only on a prompt word like "cool."
- Do not add a large external registry before the decision layer can
  explain when to use the small built-in set.
- Do not auto-transition every cut.
