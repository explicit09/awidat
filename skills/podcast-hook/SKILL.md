---
name: podcast-hook
description: Package a strong podcast hook with visual support proposals that preserve transcript evidence, editorial rationale, and render verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - find_beat
  - inspect_moment
  - plan_visual_support_proposals
  - revise_visual_support_proposal
  - verify_visual_support_artifact
  - view_timeline
  - apply_edl
  - start_render
  - poll_render
  - verify_render
---

# Podcast Hook

Use this skill when a podcast opening, cold open, chapter tease, or high-value
moment needs visual support that helps retention.

Run the Proposal-to-Visual-Support workflow:

1. Use `find_beat` and `inspect_moment` when the hook has not already been
   selected.
2. Call `plan_visual_support_proposals` on the selected transcript span,
   usually requesting a quote highlight, retention list, title card, or B-roll
   package.
3. Review `evidence`, rationale, confidence, risk, export intent, references,
   and missing information before applying anything.
4. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
5. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
6. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

The hook should increase clarity and retention without distorting the episode
promise. Preserve transcript anchors and avoid unsupported claims.
