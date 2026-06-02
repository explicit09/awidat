---
name: statistic-counter
description: Convert a selected number, metric, or scale claim into an evidence-backed statistic counter proposal with review and render verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - plan_visual_support_proposals
  - revise_visual_support_proposal
  - verify_visual_support_artifact
  - view_timeline
  - apply_edl
  - start_render
  - poll_render
  - verify_render
---

# Statistic Counter

Use this skill when the transcript contains a number, percentage, price,
ranking, scale claim, or comparison that should become a counter/stat graphic.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected claim and a
   counter/stat request.
2. Review numeric `evidence`, rationale, confidence, risk, export intent,
   references, and missing information before applying anything.
3. Ask for a source only when the number is not grounded in transcript context
   or supplied references.
4. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
5. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
6. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

The stat text must match the transcript evidence. Do not make the graphic more
precise than the source supports.
