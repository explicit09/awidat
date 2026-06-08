---
name: source-backed-broll
description: Plan B-roll packages from transcript context with source/provenance requirements, proposal review, timeline application, and render verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - plan_visual_support_proposals
  - revise_visual_support_proposal
  - verify_visual_support_artifact
  - find_generated_broll_opportunities
  - start_generated_media_job
  - poll_generated_media_job
  - use_generated_media
  - view_timeline
  - apply_edl
  - start_render
  - poll_render
  - verify_render
---

# Source-backed B-roll

Use this skill when a transcript moment needs footage-like visual support,
generated media, sourced cutaways, screenshots, or other evidence-backed
B-roll.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected transcript span, any
   reference assets, and the available project-relative B-roll asset when one
   exists.
2. Review `evidence`, rationale, confidence, risk, provenance expectations,
   export intent, references, and missing information before applying anything.
3. If no asset exists, use the returned generation plan through
   `find_generated_broll_opportunities`, `start_generated_media_job`,
   `poll_generated_media_job`, and `use_generated_media`.
   Choose the shortest generated-video duration that makes the visual readable.
   Use the finding's `duration_s` as the generation `duration` and pass the
   same `duration_s` into `use_generated_media`; do not fall back to a fixed
   four-second insert when the moment needs more time.
   Prompt like a director: specify what the shot shows, composition, camera
   motion, lighting, pacing, duration, aspect ratio, and what must be avoided.
   Keep generated B-roll on-demand and editorially grounded in the transcript.
4. If the editor asks for duration or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
5. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
6. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

B-roll must support the sentence. Preserve source/provenance/disclosure details
and do not use random footage when the transcript evidence calls for a specific
object, place, company, person, or claim.
