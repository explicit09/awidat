# Awidat Transitions

Awidat treats transitions as semantic edit operations first and backend
effects second.

## Phase One

The main repo owns the stable product contract:

- `InsertTransition` accepts existing `kind + duration_s` EDL and optional semantic fields.
- OTIO `Transition.1` stores `metadata.awidat_transition` with id, family, intent, energy, direction, and params.
- The render path resolves supported Awidat ids to FFmpeg `xfade` transitions before invoking FFmpeg.
- Unknown Awidat ids fail during render planning instead of becoming wrong visual effects.
- `skills/transition-director` tells agents when to use or avoid transitions.

## Phase Two

The external `/Users/tadies/Projects/awidat-transitions` repo is the
transition lab/registry. It contains stable manifests, a larger candidate
pool, validation tooling, and placeholders for preview rendering and
`gl-transitions` import.

Awidat should consume that repo through a small wrapper and a pinned git
revision once a remote exists. Until then, `awidat_proto::transitions`
is the in-tree fallback registry and compatibility boundary.
