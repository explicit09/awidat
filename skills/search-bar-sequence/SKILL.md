---
name: search-bar-sequence
description: Convert a question, claim, or query-like transcript span into a typed search-bar visual proposal with evidence and verification.
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

# Search Bar Sequence

Use this skill when the editor wants a question, research prompt, or query to
appear as a typed search-bar sequence.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected text and a search bar
   request.
2. Review `evidence`, rationale, confidence, risk, export intent, references,
   and missing information before applying anything.
3. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
4. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
5. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

Keep the query faithful to the transcript evidence. Avoid implying a live web
result unless a source or reference asset is provided.
