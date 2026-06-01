---
name: retention-list-opener
description: Turn a selected sequence, promise, or setup into an evidence-backed animated list proposal that can be reviewed, revised, applied, rendered, and verified.
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

# Retention List Opener

Use this skill when a transcript span previews a sequence, framework, agenda,
or set of reasons to keep watching.

Run the Proposal-to-Visual-Support workflow:

1. Call `plan_visual_support_proposals` with the selected transcript text and a
   request for an animated or retention list.
2. Review `evidence`, rationale, confidence, risk, export intent, references,
   and missing information before applying anything.
3. If the editor asks for pacing or transparent-background changes, call
   `revise_visual_support_proposal` and review the diff.
4. When accepted, pass the proposal's `apply_edl` payload to `apply_edl`.
5. Inspect with `view_timeline`, run `verify_visual_support_artifact` on the
   accepted proposal, render with `start_render`, and confirm the output with
   `verify_render`.

The finished artifact should preserve list order from the transcript evidence,
avoid extra decorative complexity, and stay short enough to support the spoken
moment rather than replacing it.
