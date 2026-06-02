---
name: route-map
description: Turn locations, routes, or geography claims into reviewable map visualization proposals with evidence and render verification.
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

# Route Map

Use this skill when the selected transcript span references a place, route,
city, country, travel path, or geographic comparison.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected text, reference
   assets if available, and a route/map request.
2. Review place-name `evidence`, rationale, confidence, risk, export intent,
   references, and missing information before applying anything.
3. Ask for missing location details only when the transcript and references do
   not identify the route or label clearly enough.
4. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
5. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
6. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

Do not invent locations. The map labels must trace back to transcript evidence
or supplied references.
