---
name: short-form-reframing
description: Reframe selected transcript moments for short-form delivery while preserving visual-support proposals, evidence, revision, apply, render, and verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - plan_scene_aware_short_form
  - plan_visual_support_proposals
  - revise_visual_support_proposal
  - verify_visual_support_artifact
  - view_timeline
  - apply_edl
  - start_render
  - poll_render
  - verify_render
---

# Short-form Reframing

Use this skill when a selected long-form moment needs vertical, square, or
short-form visual support.

Run the Proposal-to-Visual-Support workflow:

1. Use `plan_scene_aware_short_form` when the crop/layout decision is part of
   the ask.
2. Call `plan_visual_support_proposals` with the selected transcript text,
   platform, aspect ratio, references, and the visual-support request.
3. Review `evidence`, rationale, confidence, risk, export intent, references,
   and missing information before applying anything.
4. If the editor asks for shorter, faster, or transparent-background changes,
   call `revise_visual_support_proposal` and review the diff.
5. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
6. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

Keep text inside mobile-safe regions and preserve the original moment's
editorial meaning. Do not expose rendering backend details to the editor.
