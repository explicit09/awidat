# Montage Transitions

Montage treats transitions as semantic edit operations first and backend
effects second.

## Phase One

The main repo owns the stable product contract:

- `InsertTransition` accepts existing `kind + duration_s` EDL and optional semantic fields.
- `InsertTransition` accepts optional `alignment` or explicit `in_offset_s` / `out_offset_s`.
- Montage follows OTIO offset semantics: `in_offset` consumes incoming pre-roll before the cut, and `out_offset` consumes outgoing post-roll after the cut.
- Apply-time validation rejects transitions when adjacent clips do not have enough source handles and points the user/agent at repair paths: shorten the duration, change alignment, or `Untrim Clip` to widen source ranges.
- The built-in transition registry owns default/min/max durations and audio policy.
- The built-in registry includes editorially named organic options such
  as `montage.match_dissolve`, `montage.motion_blur`, and
  `montage.whip_pan_left/right`, `montage.pass_by_left/right`,
  `montage.iris_open/close`, and `montage.invisible_cut`; agents must
  still justify them with intent instead of using them as generic
  polish.
- OTIO `Transition.1` stores `metadata.montage_transition` with id, family, intent, energy, direction, and params.
- The render path resolves supported Montage ids to FFmpeg `xfade` transitions before invoking FFmpeg.
- New EDL-authored transitions must use registered `montage.*` ids or `SMPTE_Dissolve`; raw FFmpeg names remain render-compatible for legacy/imported projects.
- Imported editor names are downgraded only through explicit aliases, for example `Cross Dissolve` -> `fade` and `Dip To Black` -> `fadeblack`; unknown imported names fail during render planning.
- Unknown Montage ids fail during render planning instead of becoming wrong visual effects.
- `skills/transition-director` tells agents when to use or avoid transitions.

## Decision Layer

Transition execution is separate from transition taste. The strategy for
choosing whether a cut should stay hard, use a built-in transition, or
use an on-the-spot `montage.composite` recipe lives in
[`transition-decision-layer.md`](transition-decision-layer.md).

That layer defines the cut context packet, signal sources, decision
policy, implementation phases, and evaluation fixtures for agent
transition judgment.

## Phase Two

A separate transition lab/registry holds stable manifests, a larger
candidate pool, validation tooling, and placeholders for preview rendering
and `gl-transitions` import. Montage consumes it through a small wrapper
and a pinned git revision; until that integration lands,
`montage_proto::transitions` is the in-tree fallback registry and
compatibility boundary.
