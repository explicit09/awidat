---
name: source-backed-broll
description: Plan B-roll packages from transcript context with source/provenance requirements, proposal review, timeline application, and render verification.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - transcript_pack
  - transcript_search
  - read_index
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

1. First do an editorial transcript-flow pass yourself. Read the surrounding
   transcript, identify the spoken beat that actually needs visual support, and
   choose B-roll moments because they clarify the argument, improve pacing,
   visualize a concrete claim, or make a dry opening more compelling. Do not let
   keyword, regex, or candidate-finder output choose the moment for you.
2. For each accepted moment, record the exact transcript phrase, timeline
   anchor, rationale, proposed visual brief, duration, and any moments it must
   not cover. Check timeline overlap before generating or applying media.
3. Call `plan_visual_support_proposals` with the selected transcript span, any
   reference assets, and the available project-relative B-roll asset when one
   exists.
4. Review `evidence`, rationale, confidence, risk, provenance expectations,
   export intent, references, and missing information before applying anything.
5. If no asset exists, use generation tools only after the editorial moment is
   accepted. `find_generated_broll_opportunities` is optional scouting or a
   coverage sanity check; it is not the editorial selector and its output must
   be rejected when the transcript flow says the moment is wrong. Execute the
   accepted plan through `start_generated_media_job`, `poll_generated_media_job`,
   and `use_generated_media`; OpenRouter submissions must include
   `cost_confirmation="OpenRouter cost unknown; explicit confirmation required"`.
   Choose the shortest generated-video duration that makes the visual readable,
   then clamp the generation `duration` to the 4-15 seconds accepted by
   `start_generated_media_job`. Use `max(4, ceil(duration_s))` for moments under
   four seconds, cap longer requests at 15 seconds, and pass the same clamped
   duration into `use_generated_media`; do not fall back to a fixed four-second
   insert when the moment needs more time.
   Prompt like a director: specify what the shot shows, composition, camera
   motion, lighting, pacing, duration, aspect ratio, and what must be avoided.
   Keep generated B-roll on-demand and editorially grounded in the transcript.
6. If the editor asks for duration or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
7. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
8. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.
9. If a generated-media batch is interrupted, cancelled, or superseded, do not
   require manual registry state edits. Use `poll_generated_media_job` for
   provider-reported terminal states, leave unresolved pending records out of the
   timeline, and remove any timeline references to cancelled, pending, or failed
   jobs before continuing. Final QC must confirm the timeline references only
   accepted media.

B-roll must support the sentence. Preserve source/provenance/disclosure details
and do not use random footage when the transcript evidence calls for a specific
object, place, company, person, or claim.
