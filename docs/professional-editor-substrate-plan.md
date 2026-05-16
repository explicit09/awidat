# Awidat Professional Editing Substrate Plan

Created: 2026-05-16

Status: planning artifact only. Do not implement product changes from this plan unless explicitly asked.

## Purpose

Awidat's goal is to become an agent editor that can produce technically clean, intentional, professional-quality edits for users who are not professional editors. Resolve, Premiere, and Final Cut are references for professional editing coverage and decision quality, not UI products to clone.

The immediate priority is not the autonomous editor brain. The priority is the professional editing substrate that brain will need: source organization, selects, assembly, timeline operations, keyframes, compositing, tracking and masks, color, audio, motion graphics, delivery, and preflight.

## Sources Read

- `AGENTS.md`
- `docs/research/resolve-goal-handoff-context.md`
- `docs/research/resolve-workflow-analysis.json`
- `docs/research/resolve-workflow-analysis.html`
- `docs/editorial-grammar-upgrade-plan.md`
- `docs/transition-decision-layer.md`
- Current Rust crates, desktop app, Python indexers, and bundled skills.

Agent-readable companion artifact: `docs/professional-editor-substrate-plan.json`.

## Current Substrate

Awidat is already more than a simple timeline toy. The current codebase has these important substrate pieces:

- OTIO-backed project model and strongly typed Awidat metadata. `AwidatTimelineMetadata` stores source assets, anchors, hard-cut semantic boundaries, edit-plan linkage, and broadcast overlay state in `crates/proto/src/awidat_meta.rs`.
- Broad typed EDL mutation surface. `EdlOp` already includes trim, delete, split, untrim, insert, b-roll, PiP, move, transitions, hard-cut intent, audio lead/trail, track audio, ducking, sync groups, audio FX, generic effects, speed, color correction, LUTs, titles, captions, output format, loudness, package metadata, and broadcast overlays in `crates/core/src/edl/op.rs`.
- Intent preservation. `SemanticCutSpec`, `CutType`, `AudioRelation`, and `SplitEditSpec` persist why a cut or split edit exists rather than only the mechanics of the timeline.
- Renderable effect primitives. `crates/effects/src/lib.rs` registers stable graph-native effect ids for volume, speed, color correction, LUT, title, video overlay, audio fade, and audio FX.
- Desktop review surface. `crates/desktop-protocol/src/lib.rs` exposes proposed edits, timeline snapshots, cut boundaries, preview limitations, track audio controls, clip effects, transitions, titles, overlays, and editorial notes.
- Evidence pipeline. Bundled indexers cover audio energy, scene detection, Whisper, topics, editorial moments, CLIP, face, shot, gaze, frame quality, color analysis, and composition.
- Editorial skills. Bundled skills already cover rough assembly, cut direction, split edits, transitions, b-roll, thematic montage, color correction, podcast editing, short-form, tutorials, beat sync, and version control.

The shape is right: agent-native tools, typed operations, reviewable proposals, and renderable outputs. The gap is breadth and durability across the full professional pipeline.

## Product Rule

Not every professional capability needs a manual UI control.

The default surface should be agent-native tools plus reviewable proposals. Manual controls should exist when they improve trust, review, correction, or expert override. For example, a user should not need to understand planar tracking to ask for a tracked label, but they do need a way to see that the track slipped and correct or reject it.

## Pipeline Capability Map

| Stage | Current State | Missing Substrate | Preferred Surface |
| --- | --- | --- | --- |
| Media organization | Imports, proxies, source asset list, indexers, asset tools | Asset catalog, bins, smart collections, tags, roles, readiness, relink/offline/proxy state, provenance | Agent tools first; media/source review lens for trust |
| Selects | Evidence indexers and rough-cut skills | Durable selects/rejects/maybes, take groups, range ratings, keep/reject reasons, stringouts | Agent-generated selects with review list |
| Assembly | Insert, trim, split, untrim, delete, move, b-roll, PiP, transitions | Ripple, roll, slip, slide, lift, extract, replace, overwrite, append, markers, track targeting, nested stringouts | Agent operations plus keyboard/command palette for frequent corrections |
| Editorial intent | Semantic cuts, split edits, transitions, proposals, notes | Unified evidence trace and proposal package across all pipeline stages | Proposal inspector and evidence summaries |
| Keyframes | Static effects and coarse title animation enum | General parameter animation schema, keyframes, easing, curves, animation diffs, preview/render support | Agent animation tools plus compact keyframe lane |
| Motion graphics | Titles, captions, broadcast overlay | Reusable templates, slot schema, safe-area rules, text reveal/write-on, word/character animation | Template proposals with editable slots |
| Compositing | Simple overlays, graph-native effect naming | Serializable composition DAG, multi-input merge, masks, mattes, reusable comps, graph diffs, lowering strategy | Agent-authored graph plus compact graph/stack inspector |
| Tracking/masks | Shot, face, gaze, motion, composition evidence | Point/planar/surface tracks, keyframed masks, mattes, confidence, quality review, bind effects to tracks | Agent tracking tools plus correction handles |
| Color | Color-analysis indexer, color correction, LUTs, color skill review packages | Reference stills, gallery, shot groups, scopes, grade stack, color management, before/after contact sheets | Agent match plan plus reference/contact-sheet review |
| Audio | Clip/track volume, fades, split edits, ducking, audio FX, loudness, render mix planning | Bus roles, mixer meters, automation lanes, audition packs, reusable chains, clipping/noise/loudness review | Agent mix proposals plus mixer task lens |
| Delivery | Output format, loudness target, package metadata, render/export | Named delivery profiles, queue, platform validation, preflight, post-render validation, package manifest | Deliver lens with preflight findings and fix proposals |
| Workflow | Panes, project types, permission modes | Task lenses for media, selects, assembly, VFX, color, audio, delivery, preflight; readiness markers | Lightweight lenses, not Resolve page clone |

## Prioritized Phases

### Phase 1: Foundation Catalog, Selects, Delivery Skeleton

Build first:

- Asset catalog separate from timeline clips.
- Bins, smart collections, tags, labels, roles, ratings, provenance, usage, proxy/index readiness, offline/relink state.
- Range-level selects, rejects, maybes, best-take grouping, and stringouts.
- Delivery profile data model and first preflight skeleton.

Why first: professional editing begins before the timeline. The agent needs to know what footage exists, what is usable, what has been selected, and what the final target requires. Without this, later automation will be forced to infer missing state from clips already on a timeline.

Exit criteria:

- Agents can query assets by bin, role, tag, readiness, and evidence state.
- Selects and stringouts persist independently from timeline clips.
- At least one delivery profile can be selected and validated before render.

### Phase 2: Assembly and Direct Editing Substrate

Build:

- Typed operations or explicit non-support decisions for ripple, roll, slip, slide, lift, extract, replace, append, overwrite, markers, and ranges.
- Assembly review mode separate from finishing review.
- Track targeting and lane-level edit intent where agents need precise control.
- Unified proposal/evidence trace across timeline operations.
- Lightweight workflow lenses: media, selects, assembly, edit review, VFX, color, audio, delivery, preflight.

Why second: Awidat already has a strong EDL surface, but professional correction and trust require more precise timeline semantics and task context.

Exit criteria:

- Common professional timeline operations are typed and proposal-ready.
- Assembly proposals can be reviewed without mixing in color/audio/delivery polish.
- Every edit proposal can carry evidence and intent.

### Phase 3: Keyframes and Motion Graphics

Build:

- General `ParameterAnimation` schema.
- Keyframe points, interpolation, easing, hold/linear/bezier curves.
- First renderable animated parameters: title transform/opacity, overlay transform/opacity, volume, and simple text reveal.
- Reusable title/lower-third templates with slots, constraints, safe-area rules, and platform variants.

Why third: professional polish requires animated parameters and reusable motion systems. Hard-coded title animation enums will not scale.

Exit criteria:

- Keyframes are serializable, diffable, previewable, and renderable for a small high-value set.
- Templates are fillable by agents and reviewable by users.
- Preview limitations are explicit when desktop approximation differs from final render.

### Phase 4: Tracking, Masks, Mattes, and Composition Graph

Build:

- Tracker sidecar contracts for point, planar, and surface tracks with coordinate space, per-frame values, and confidence.
- Mask sidecar contracts for keyframed paths, feather, opacity, boolean ops, and confidence.
- Matte sidecars for alpha sources and quality review.
- Small Awidat composition graph with nodes for media input, transform, merge, mask, matte, text, blur, color, tracker bind, and output.
- EDL operations to attach overlays/effects/compositions to tracks and masks.

Why fourth: tracked overlays, invisible fixes, and mask-bound effects need evidence contracts before a big node UI or advanced VFX autonomy.

Exit criteria:

- Agents can propose tracked inserts and mask-bound effects.
- Track quality is reviewable before acceptance.
- Composition graphs are serializable, diffable, and lowerable to the current renderer where possible.

### Phase 5: Finishing Workflows

Build:

- Color finishing workflow: reference stills, shot groups, grade stack, scope metrics, color consistency summaries, contact sheets, color management.
- Audio finishing workflow: roles, buses, meters, automation, reusable chains, clipping/noise/loudness checks, audition packs.
- Delivery workflow: named profiles, render queue, preflight, post-render validation, package manifests.

Why fifth: finishing is not just a set of clip knobs. It is a review workflow with measurable quality gates.

Exit criteria:

- Color match is reviewable through reference stills, shot groups, before/after packs, and metrics.
- Audio mix changes can be auditioned and validated against loudness/peak/noise targets.
- Delivery can fail fast before render and validate artifacts after render.

### Phase 6: Autonomy Readiness Contracts

Build after phases 1-5 have enough substrate:

- Pipeline readiness report covering media, selects, assembly, VFX, color, audio, delivery, and preflight.
- Capability registry describing which operations are available, previewable, renderable, preflighted, and safe for autopilot.
- Typed planner inputs and outputs for each stage.
- Cross-stage conflict detection.
- Learning hooks from accepted/rejected proposals.

Why last: a broad autonomous editor brain needs reliable substrate to orchestrate. Building it first would bake missing data and missing operations into the decision layer.

Exit criteria:

- The agent can state which pipeline stages are ready, incomplete, or blocked.
- The agent can plan multi-pass edits across the whole professional pipeline using typed substrate.
- User approvals and rejections become useful learning signals without requiring professional terminology.

## Build First Recommendation

Start with Phase 1.

The first implementation plan should target asset catalog, source review/selects, and delivery profile/preflight skeleton. This gives every later capability stable inputs and outputs:

- Media organization gives agents a source-of-truth inventory.
- Selects give agents a professional pre-timeline decision layer.
- Delivery profiles define constraints that affect assembly, graphics, color, audio, and render.

Do not start with the full autonomous decision layer. Also do not start with a large manual NLE interface. The highest-leverage path is typed substrate plus reviewable proposals.

## Deferred Until Substrate Exists

- Full autonomous multi-stage editor brain.
- Large Resolve-like node UI.
- Full manual NLE clone controls.
- Opaque generated FFmpeg graphs in project metadata.
- Advanced 3D or particle systems before graph, tracking, mask, and keyframe contracts exist.

## Agent Handoff

Use `docs/professional-editor-substrate-plan.json` for machine-readable planning. When creating an implementation plan, keep plans phase-scoped. The recommended first plan is Phase 1 only: asset catalog, selects/stringouts, and delivery profile/preflight skeleton.

Each implementation plan should preserve existing architecture:

- `crates/proto` for durable project/data schemas.
- `crates/core` for agent tools and EDL/application behavior.
- `crates/render` for render/preflight lowering.
- `crates/index` and `python/packages/*-mcp` for evidence sidecars.
- `crates/desktop-protocol` plus `apps/desktop/src` for review surfaces.

Manual UI should remain thin unless a control directly improves review, correction, or trust.
