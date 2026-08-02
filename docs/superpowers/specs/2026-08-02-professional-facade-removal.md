# Professional Facade Removal

**Date:** 2026-08-02

## Decision

Delete the remaining `montage_core::professional` orchestration facade and its two desktop IPC wrappers. The workflow-lens and readiness APIs have no production Rust or frontend callers; only their own tests reference them.

This does **not** remove the professional editing substrate:

- Keep `montage_proto::professional`, which defines timeline, animation, review, delivery, and media-intelligence schema used throughout the workspace.
- Keep `montage_render::professional`, which contains active render, composition, tracking, preflight, and delivery engines.
- Preserve `derive_audio_finishing_state`, which has two live core callers, in a focused `audio_finishing` module.
- Keep the live MCP and desktop commands that consume those schema and render capabilities directly.

## Evidence

- `read_professional_lenses` and `read_pre_autonomy_inspection` are registered in Tauri but never invoked by frontend production code or tests.
- No code outside the facade's wrappers and tests calls its workflow-lens or readiness APIs.
- A focused compile exposed two internal callers of the audio-finishing helper; extracting that helper preserves the live podcast paths without retaining the unrelated facade.
- The remaining core facade exists to derive snapshots over proto metadata, not to perform editing or rendering.
- Removing the facade makes the boundary explicit: schema in proto, behavior in focused core/render modules, presentation in live desktop surfaces.

## Verification

- Require zero references to `montage_core::professional` and both deleted commands.
- Run core and desktop all-target compile checks.
- Run core and desktop tests proportionate to the changed module boundary.
- Run workspace clippy/check after the focused gates.
