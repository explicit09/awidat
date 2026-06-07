# Transition Registry Contract

Montage transitions use one data model across editor decisions, OTIO
metadata, render planning, and the future `montage-transitions` package.

The core rule is:

```text
primitives are the API
transitions are compositions
presets are named compositions
agent-made transitions are one-off compositions
```

Normal editing must never store raw FFmpeg filter graphs, GLSL, shell
commands, plugin code, or generated backend code inside the project. The
agent may author `composition_json`, but that composition is constrained to
the stable primitive vocabulary in `crates/proto/src/transitions.rs`.

## In Montage

Montage owns the harness contract:

- `SemanticTransitionSpec`: metadata stored on OTIO `Transition.1` nodes.
- `TransitionComposition`: versioned data-only recipe.
- `TransitionPrimitiveOp`: stable primitive API.
- `montage.composite`: one-off agent-authored recipe id.
- FFmpeg phase-one lowering from composition data to safe `xfade` names.

`montage.hard_cut` is not written as a transition node. It means "leave the
cut alone".

`montage.composite` is not a stable preset. It is the editor-side authoring
id for custom, context-specific recipes.

## Stable Presets

Stable presets are exportable through:

```rust
montage_proto::transitions::stable_builtin_transition_manifests()
montage_proto::transitions::stable_builtin_transition_manifest_json()
```

The manifest shape is intentionally extraction-ready:

- `id`
- `family`
- `display_name`
- `backends`
- `default_duration_s`
- `min_duration_s`
- `max_duration_s`
- `ffmpeg_xfade`
- `audio_policy`
- `best_for`
- `avoid_for`
- `license`
- `attribution`
- `preview`
- `params`
- `composition`

When the external `montage-transitions` repo becomes authoritative,
Montage should import a pinned revision and adapt that package's stable
registry into this same manifest shape.

## Backend Lowering

Phase one exports through FFmpeg. Composition lowering is deliberately
constrained:

- `push left` -> `slideleft`
- `push right` -> `slideright`
- `push up` -> `slideup`
- `push down` -> `slidedown`
- directional `wipe` -> matching FFmpeg wipe
- `zoom` -> `zoomin` when scale is at least 1.0
- white `flash` -> `fadewhite`
- `pixelize` -> `pixelize`
- `blur` -> `hblur`
- `opacity` -> `fade`
- `atomic` -> registered Montage transition id only

Primitives such as `shake` and `chromatic_split` can be stored and
validated now, but they require richer backend support before they affect
phase-one FFmpeg output directly.

## Extraction Boundary

Move stable presets into `montage-transitions`.

Keep these in Montage:

- `montage.composite`
- EDL/OTIO metadata shape
- project validation
- render planner integration
- safe backend lowering/fallback selection

This preserves deterministic editing while still allowing agents to create
custom transition recipes on the spot.
