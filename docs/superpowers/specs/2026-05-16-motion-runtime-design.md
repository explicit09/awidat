# Phase 3A Motion Runtime First Design

## Summary

Awidat should grow toward a Fusion-inspired animation and compositing system without cloning Fusion's primary workflow. Fusion is the reference for capability depth: keyframes, splines, masks, merges, macros, expressions, tracking, and graph inspection. Awidat's product shape should stay agent-native: agents propose motion intent, users review concrete packages, and advanced graph/curve views exist for inspection and correction instead of becoming the default authoring surface.

The first implementation milestone is Phase 3A: Motion Runtime First. It makes a small set of visual parameters genuinely keyframe-driven across persistence, desktop preview, and final render. This creates the runtime substrate that later motion templates, agent-authored motion packages, composition graphs, masks, mattes, tracking, and expression links can reuse.

## Current State

The current repository already has several professional animation nouns:

- `ParameterAnimation`, `AnimationTarget`, `Keyframe`, `KeyframeInterpolation`, and `Easing` in `crates/proto/src/professional.rs`.
- `MotionGraphicsTemplate` in `crates/proto/src/professional.rs`.
- `CompositionGraph`, graph nodes, masks, and mattes in `crates/proto/src/professional.rs`.
- `parameter_animations`, `motion_templates`, and `composition_graphs` in `crates/proto/src/awidat_meta.rs`.
- EDL operations that store these objects in `crates/core/src/edl/apply.rs`.
- Isolated composition/template lowering helpers in `crates/render/src/professional.rs`.

The gap is runtime integration. `SetParameterAnimation` stores metadata, but supported animations are not exposed as first-class timeline preview state or lowered into the active FFmpeg timeline render path. Desktop preview still hardcodes title animations such as fade and slide in `apps/desktop/src/media/SegmentedVideoView.tsx`. Render still hardcodes title animation expression generation in `crates/render/src/timeline.rs`. Video overlays are static PiP/full-frame plans with scale/corner/margin metadata, not animated transform/opacity carriers.

## Product Direction

Awidat's motion system should follow these principles:

- Intent first: users and agents describe the communication goal, such as focus, reveal, emphasis, correction, continuity, or compositing.
- Concrete output underneath: accepted proposals lower into explicit `ParameterAnimation`, template fills, graph nodes, masks, mattes, or tracker bindings.
- Inspectable, not mandatory: curves and graphs are available when trust, debugging, or precision requires them, but proposal review remains the primary workflow.
- Reusable by default: useful motion becomes a template or motion package instead of a hidden one-off expression.
- Honest parity: preview/render gaps are surfaced as limitations instead of silently implying exact parity.
- Small core, expandable edges: start with transform and opacity animation, then add templates, graph inspection, masks, tracking, and expression links.

## Phase 3A Scope

Phase 3A makes `ParameterAnimation` renderable and previewable for a narrow allowlist:

- `title.opacity`
- `title.x`
- `title.y`
- `overlay.opacity`
- `overlay.x`
- `overlay.y`
- `overlay.scale`

The goal is not arbitrary effect animation yet. The goal is a reliable shared evaluator and end-to-end parity for high-value visual motion that already maps onto existing title and video overlay paths.

Unsupported targets should still persist as metadata, but they must not pretend to be preview/render ready. The timeline snapshot should surface explicit limitations for unsupported animation targets or known preview/render differences.

## Non-Goals

Phase 3A does not include:

- A large Resolve/Fusion-style node page.
- General-purpose composition graph authoring.
- Masks, mattes, tracking, or expression links.
- Particle systems or 3D systems.
- Volume automation.
- Text reveal/write-on animation.
- Arbitrary effect parameter animation.

Volume automation and text reveal remain important, but they should follow once visual motion parity proves the shared evaluator and lowering path.

## Architecture

The intended data flow is:

1. An agent or user submits an EDL operation containing `ParameterAnimation`.
2. Core applies the operation and stores the animation in project metadata.
3. Timeline read converts supported animation records into a preview-friendly animation view attached to the relevant timeline item.
4. Desktop preview evaluates the same keyframes/easing model into React styles.
5. Render planning lowers the supported animation records into FFmpeg expressions.
6. Unsupported targets and approximation gaps become `TimelinePreviewLimitation` records.

The core boundary is important: `ParameterAnimation` remains the canonical persisted object, while preview and render may use derived, typed views that are easier to validate and consume.

## Components

### Proto

`crates/proto` remains the source of truth for stored animation data. Phase 3A should preserve the existing general shape of `ParameterAnimation` and avoid overfitting the persisted schema to title/overlay specifics.

Required additions:

- A small parameter support classifier in `crates/render` for render lowering and a matching classifier in `apps/desktop` for preview. The persisted proto schema should stay general in Phase 3A.
- Strong validation for finite values, sorted keyframes, non-empty ids, and usable targets.
- Unit documentation for the Phase 3A allowlist.

Phase 3A uses normalized output frame space for visual transforms:

- `x` and `y`: normalized offsets relative to output width/height, where `0.0` is resting position.
- `opacity`: `0.0` to `1.0`.
- `scale`: multiplier, where `1.0` preserves the base size.

### Core

`crates/core` should continue owning EDL parsing and project mutation. `SetParameterAnimation` already persists animations by id. Phase 3A adds validation/readiness behavior around supported targets.

Core should not know how to draw or render every animation. It should know enough to:

- Store and replace animations by stable id.
- Preserve unsupported animation records.
- Report diagnostics for invalid records.
- Keep proposal summaries understandable for review.

### Desktop Protocol

`crates/desktop-protocol` should expose a derived animation view in `TimelineSnapshot`. The view should be convenient for desktop preview and inspector work without requiring the frontend to parse arbitrary project metadata.

Recommended shape:

- Attach supported animations to clip items as `animations: Vec<TimelineParameterAnimation>`.
- Keep each animation's id, target parameter, keyframes, interpolation, easing, and optional rationale.
- Add preview limitations for unsupported targets.

This avoids making `TitleStyling` and `VideoOverlayStyling` absorb every future animated property directly.

### Desktop Preview

`apps/desktop` should add a small TypeScript evaluator that mirrors the Phase 3A Rust evaluator behavior:

- Evaluate hold and linear keyframes first.
- Add named easing curves only when render lowering supports equivalent timing.
- Clamp opacity to `[0, 1]`.
- Treat missing animations as resting style.

The current hardcoded title animation enum remains a compatibility path during Phase 3A. It should not be converted into generated `ParameterAnimation` records yet. If a Phase 3A animation targets the same supported parameter, the explicit `ParameterAnimation` takes precedence.

### Render

`crates/render` should add a focused animation lowering module for timeline render:

- Convert supported `ParameterAnimation` records into FFmpeg expressions.
- Thread title opacity/x/y into drawtext alpha/x/y expressions.
- Thread overlay opacity/x/y/scale into overlay and scale expressions where FFmpeg can represent them.
- Skip unsupported targets at planning time only after recording an explicit render limitation.

Render and preview do not need to share source code across Rust and TypeScript, but they must share test vectors and semantics.

## Easing and Interpolation

Phase 3A should implement only what can be verified across preview and render:

- Hold.
- Linear.
- Ease in.
- Ease out.
- Ease in out.

The existing `Bezier` interpolation enum should remain stored, but Phase 3A should treat bezier as unsupported for renderable parity unless concrete handles and lowering semantics are added. If a bezier record is present, it should persist and produce a limitation until supported.

## Agent Motion Packages

Phase 3A does not need a new persisted package type, but the design should prepare for one. A future agent-authored motion package should contain:

- Intent: the communication goal of the motion.
- Affected clips and time ranges.
- Concrete generated `ParameterAnimation` records.
- Template fills when templates exist.
- Expected viewer effect, such as focus, energy, reveal, or continuity.
- Review notes and known limitations.
- Optional sound cue hints.

The key rule is that packages are review affordances. The accepted project state must still lower into explicit animation/template/graph records.

## Roadmap

### Phase 3A: Motion Runtime First

Deliver shared keyframe/easing semantics and preview/render parity for title and video overlay opacity/transform parameters.

Exit criteria:

- `ParameterAnimation` is no longer storage-only for the Phase 3A allowlist.
- Supported animations appear in `TimelineSnapshot`.
- Desktop preview evaluates supported animations.
- FFmpeg render lowers the same supported animations.
- Unsupported animation targets persist and produce explicit limitations.
- Tests cover validation, snapshot exposure, preview evaluator behavior, and render expression lowering.

### Phase 3B: Motion Templates

Add reusable title, lower-third, callout, zoom, punch-in, focus highlight, and overlay motion templates. Templates expose editable slots and lower into concrete `ParameterAnimation` records.

Exit criteria:

- Agents can fill template slots.
- Users can review template fills.
- Templates lower into explicit animations.
- Safe-area and platform variants are validated.

### Phase 3C: Agent Motion Packages

Bundle animations, template fills, rationale, and limitations into reviewable motion proposals.

Exit criteria:

- Agents can propose a coherent motion package.
- The user can inspect affected clips, intent, generated animations, and limitations.
- Accepting a package applies explicit project records rather than opaque generated render code.

### Phase 4A: Compact Composition Graph Inspection

Connect the existing composition graph concepts to a compact inspector for advanced comps.

Exit criteria:

- Graphs are serializable, diffable, and attached to timeline ranges.
- A compact graph/stack inspector can show nodes and connections.
- Supported nodes lower to the renderer where possible.

### Phase 4B: Masks, Mattes, And Tracking

Introduce tracker sidecars, keyframed masks, matte sources, confidence values, and graph bindings.

Exit criteria:

- Agents can propose tracked inserts and mask-bound effects.
- Track and mask quality is reviewable before acceptance.
- Low-confidence tracking produces review warnings.

### Phase 4C: Expression Links And Procedural Motion

Add expression links that bind parameters to other parameters, tracks, or analyzed signals.

Exit criteria:

- Expression links are explicit, inspectable, and validated.
- Cycles are rejected.
- Procedural motion lowers to deterministic preview/render behavior where supported.

## Error Handling

Phase 3A should fail fast for invalid animation data:

- Empty animation ids.
- Empty target paths for supported render flow.
- Non-finite times or values.
- Unsorted keyframes.
- Duplicate animations targeting the same parameter and range without an explicit conflict policy.
- Invalid units or out-of-range values where a parameter requires clamping or rejection.

Unsupported but valid animations should not fail project loading. They should persist and surface as unsupported preview/render limitations.

## Testing Strategy

Tests should match the first milestone's real risk:

- Proto/core tests for validation, storage, replacement by id, and unsupported-target preservation.
- Desktop protocol tests for exposing supported animations and limitations in `TimelineSnapshot`.
- Render tests for FFmpeg expression lowering for title opacity/x/y and overlay opacity/x/y/scale.
- Frontend unit tests for the TypeScript evaluator using shared test vectors.
- Regression tests that existing title animation enum behavior still works when no Phase 3A animation exists.

## Design Decisions

These decisions are fixed for Phase 3A:

- Use normalized x/y offsets for both title and overlay motion.
- Keep support classification in desktop/render consumers instead of adding parameter-specific support metadata to proto.
- Keep legacy title animation enums as a parallel compatibility path during Phase 3A.
- Let explicit `ParameterAnimation` records override legacy title animation behavior for supported parameters.

## Review Notes

This design deliberately starts with visual motion parity instead of templates or graph UI. Templates without a runtime would become decorative metadata, and graph UI without renderable keyframes would create a larger surface before the core motion contract is trustworthy.

The design also deliberately defers volume automation and text reveal. Both are important for the full roadmap, but they touch additional semantics: audio mixing for volume automation and text layout/reveal behavior for write-on effects. They should build on the shared evaluator after Phase 3A proves preview/render parity for visible transform and opacity.
