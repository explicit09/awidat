# Transitions Round 2 Design

## Goal

Complete the transition feature loop in the same shape as the captions and
color loops: preserve the mined transcript research, close the highest-value
planner gaps, and prepare real render tests for user review.

## Scope

This round is deliberately narrow. Montage already has an EDL transition
substrate, a transition catalog, `transition_context`, `plan_transition`,
`validate_transition_choice`, and the new `plan_split_edit`. The missing work is
not a new transition engine. It is better research packaging and a wider,
more careful planner that reaches the FFmpeg-renderable transition families
the corpus repeatedly teaches: pass-by/invisible cuts, punch-in/zoom, iris,
wipes/slides, flashes, dissolves, and glitch accents.

GPU-only luma/light-leak/swirl/cinematic-pan transitions remain documented as
future work because current apply/render audits show authoring and parameter
lowering caveats. The planner should not emit those by default in this round.

## Research Artifacts

The existing transition mine has already produced 58 shard summaries and 8
consolidated summaries from 870 transition-relevant transcripts. This round
turns that into the captions/color deliverable shape:

- `video_editing_transcripts/knowledge/transitions/SKILL.md`: compact but
  deeper craft summary for future agents.
- `video_editing_transcripts/knowledge/transitions/tool-gap.md`: ranked gap
  analysis against the current Montage codebase.
- `video_editing_transcripts/knowledge/transitions/sources.md`: list of the
  transition transcript source videos.

These files live under `video_editing_transcripts/`, which is ignored by git.
They are research artifacts, not hot-path runtime prompt material.

## Planner Behavior

`plan_transition` remains the single visible-transition planner. It should:

- Parse occlusion scores from `transition_context`.
- Choose `montage.pass_by_left/right` for occlusion/pass-by jobs only when a real
  occlusion signal is present and direction is usable.
- Choose `montage.invisible_cut` for invisible/mask/dark-frame jobs only when a
  real occlusion signal is present.
- Choose `montage.zoom_in` or `montage.distance_zoom` for punch-in/forward-momentum
  jobs.
- Choose `montage.iris_open/close`, directional wipe/slide, `montage.flash_white`,
  `montage.pixelize`, `montage.cross_dissolve`, `montage.match_dissolve`, and
  `montage.fade_black` for matching objectives.
- Refuse to emit an occlusion or invisible transition when the context lacks the
  required occlusion signal, returning `Set Cut Intent` instead.
- Continue clamping duration to handles and catalog min/max ranges.

The planner should stay read-only and return an apply-ready EDL fragment.

## Tests

Add focused unit coverage for `plan_transition`:

- occlusion + left direction selects `montage.pass_by_left`.
- invisible-cut objective without occlusion refuses to a hard cut.
- punch-in objective selects `montage.zoom_in`.
- stylized reveal/closure objectives select iris open/close.
- directional graphic movement selects wipe/slide variants.

Existing tests for `plan_split_edit` and `skill_catalog` remain part of the
verification set.

## Render Proof

After code tests pass, prepare a CLI render proof using local footage. The
minimum proof is one FFmpeg-renderable visible transition generated from the
planner and applied through EDL, then rendered with the rebuilt Montage CLI. If
footage is not available on the external drive, stop with the exact missing path
instead of fabricating a render result.

## Success Criteria

- Research artifacts match the per-feature loop shape.
- `plan_transition` reaches the newly supported FFmpeg-renderable transition
  families without hand-authored EDL.
- Occlusion/invisible choices are refused when visual evidence is absent.
- Focused Rust tests pass.
- A real render is produced or a concrete external-footage blocker is reported.
