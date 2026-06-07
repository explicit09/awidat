# AI-Assisted Gap Remediation

This note reconciles the AI-assisted editing gap review with the current
implementation state.

## Status

| Gap item | Current status | Evidence |
| --- | --- | --- |
| First-class auto-reframe agent surface | Implemented as `plan_reframe`, a read-only planner that returns an `montage.reframe` EDL fragment for `apply_edl`. | `crates/core/src/tools/plan_reframe.rs`; registered in CLI, TUI, and desktop registries; exposed by `skills/short-form/SKILL.md`. |
| Reframe mutation path | Uses the established `apply_edl` boundary via `*** Set Effect` rather than a second mutating tool. | `montage.reframe` is validated by `montage-effects`; render reads it as `ReframePlan`. |
| Subject-tracking reframe paths | Not expanded in this pass. The current implemented surface is a static subject-aware crop from normalized subject-center evidence. | `ReframePath` and `lower_reframe_path` exist, but the active render path consumes clip effects; a path-authoring workflow would need separate EDL/render integration. |
| Tier-2 verification | Already has concrete V1 checks; the reference report's "stubbed" wording is stale. | `crates/core/src/verify.rs` runs `timeline_renders`, `av_sync_40ms`, and `transcript_cut_alignment`. |

## Verification Scope

`plan_reframe` is covered by an integration test that proves:

- the tool is first-class and read-only,
- the schema exposes `clip_id` as required input,
- vertical 9:16 planning emits an `apply_edl`-ready `montage.reframe` fragment,
- invalid normalized subject centers fail loudly.

Tier-2 verification remains V1, not a perceptual self-evaluator. It checks stream-duration drift and transcript-anchor overlap, but it does not yet re-transcribe rendered output or perform content-aware lip-sync correlation.
