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

A separate transition lab/registry holds stable manifests, a larger
candidate pool, validation tooling, and placeholders for preview rendering
and `gl-transitions` import. Awidat consumes it through a small wrapper
and a pinned git revision; until that integration lands,
`awidat_proto::transitions` is the in-tree fallback registry and
compatibility boundary.
