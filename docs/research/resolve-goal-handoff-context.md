# Awidat Professional Editor Goal Handoff

Use this context when starting a new Codex chat or Goal about the Resolve/Premiere-class professional editor substrate.

## Repository

- Repo: `/Users/tadies/Projects/awidat`
- Follow `/Users/tadies/Projects/awidat/AGENTS.md`.
- Keep unrelated worktree changes intact.
- This handoff is for planning/research unless the user explicitly asks for implementation.

## Key Artifacts

- Human report: `docs/research/resolve-workflow-analysis.html`
- Agent JSON: `docs/research/resolve-workflow-analysis.json`
- Earlier related but separate prior art: `docs/editorial-grammar-upgrade-plan.md`

The Resolve workflow report is intentionally separate from the earlier editorial grammar document.

## Product North Star

Awidat should become an agent editor that can make edits with the judgment, craft, and technical range of a professional video editor for users who are not professional editors.

The goal is not to clone DaVinci Resolve, Premiere, or Final Cut as interfaces. Those tools are references for professional task coverage and decision quality.

A user should be able to ask for an outcome in ordinary language and have Awidat inspect the footage, make professional editorial decisions, perform the required technical operations, explain/review the result, and render a deliverable.

## Sequencing Principle

Do not try to build the full autonomous decision layer before the professional editing substrate exists.

First map and build the pipeline components professional editors rely on:

- media organization and source review
- selects and assembly
- trim/cut/timeline operations
- motion/keyframes/title graphics
- compositing/VFX primitives
- tracking, masks, and mattes
- color matching and finishing
- audio mixing and repair
- delivery profiles and preflight

After enough substrate exists, build the autonomous decision layer that orchestrates those capabilities.

## Current High-Level Gaps From Research

1. Media pool, bins, and asset metadata.
2. Workflow modes or task lenses.
3. General keyframes and animated inspector parameters.
4. Node/composition graph model.
5. Tracking, masks, and mattes.
6. Color match workflow with references, scopes, and review packs.
7. Mixer-depth audio workflow.
8. Delivery profiles, queue, and preflight.
9. Fast editing ergonomics and keyboard/direct edit flows.
10. Reusable motion graphics templates.

## Important User Preference

Not every capability needs a manual UI control. The long-term direction is mostly agent-native editing, with manual controls reduced where the agent can make good decisions and provide reviewable proposals.

Manual surfaces should exist when they help review, correction, or trust. The core question is often: should this professional editor capability be an agent tool, a visible control, or both?

## Suggested Immediate Goal

Create a complete professional editing pipeline substrate plan for Awidat, based on the Resolve workflow research and current codebase capabilities. The plan should identify the components needed before the autonomous decision layer, group them into implementation phases, and produce agent-readable planning artifacts.
