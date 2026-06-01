---
name: chapter-intro
description: Turn a chapter boundary, topic shift, or segment label into a reviewable title-card proposal with evidence and render verification.
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

# Chapter Intro

Use this skill when a topic shift, chapter boundary, segment start, sponsor
transition, or intro card needs a visual marker.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected chapter/topic text
   and a title-card or chapter-intro request.
2. Review topic `evidence`, rationale, confidence, risk, export intent,
   references, and missing information before applying anything.
3. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
4. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
5. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

Chapter titles should promise viewer payoff and match the actual transcript
section. Do not add a chapter card where a lower third or no graphic would be
clearer.
