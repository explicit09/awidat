---
name: quote-highlight
description: Make a selected spoken line land as a reviewable quote-highlight visual support proposal with transcript evidence and render verification.
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

# Quote Highlight

Use this skill when a concise transcript quote should become a visual beat.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected quote and a quote
   highlight request.
2. Review transcript `evidence`, rationale, confidence, risk, export intent,
   references, and missing information before applying anything.
3. If the editor asks for a shorter, faster, or transparent-background version,
   call `revise_visual_support_proposal` and review the diff.
4. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
5. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

The quote text must match the transcript anchor. Keep the highlight readable
and motivated by the spoken line; do not add visual noise just because the tool
can generate it.
