# Collaboration Open Decisions

## Purpose

This file records the decisions that block the remaining Category 11
collaboration work. It is intentionally separate from the implemented
local vedit attribution and branch/checkout slice so future work does
not quietly assume product semantics.

For a copy/paste approval block, see
`docs/superpowers/specs/2026-05-21-collaboration-decision-packet.md`.

## Decision 1: Bounded Merge Rule

Current implemented state:

- Read-only merge preflight exists. It finds the common ancestor for a
  source/target ref pair, reports each side's changed clip/media ids,
  and surfaces overlap conflicts without moving refs or writing a
  merged timeline.

Recommended decision:

- Implement `vedit_merge` only for non-overlapping changed clip ids.
- Refuse the merge when both sides touched the same clip id, media
  reference, transition neighbor, or animation target string.
- Return a structured conflict report instead of trying to auto-resolve.

Why:

- Upstream `vedit-core` exposes branch create/list/switch APIs, but no
  semantic merge API.
- Full OTIO three-way merge is a larger algorithmic problem.
- The non-overlapping rule still unlocks useful alternate-cut workflows
  without corrupting timeline intent.

Required approval:

- Confirm that "non-overlapping changed clip ids only" is acceptable for
  the first merge surface.

## Decision 2: Authored Review Notes Store

Recommended decision:

- Keep user-authored review notes in the existing `.awidat/notes.json`
  file with a backward-compatible schema version bump.
- Preserve current `anchor_at_s` reads and add richer optional fields:
  `author`, `anchor`, `thread_id`, and `replies`.
- Keep dismissal-pattern memory scoped to agent-authored notes.

Why:

- Existing desktop and Tauri commands already use `NotesFile`.
- One store makes review notes, agent notes, and later comment ingest
  queryable together by timecode.
- Backward-compatible loading can preserve current projects.

Alternative:

- Create `.awidat/review-comments.json` as a separate store. This keeps
  authored comments isolated but duplicates lifecycle/status handling
  and makes timeline review UI join two files.

Required approval:

- Confirm whether authored review notes should share `notes.json` or use
  a separate review-comment store.

## Decision 3: Local Review Package Manifest

Recommended decision:

- Add a local-only review package before any hosted review link:
  watermarked/proxy render plus a manifest containing vedit commit hash,
  timeline hash, tags, commit header, and reasoning body.

Why:

- It improves review handoff without requiring credentials or a hosted
  service.
- It gives third-party ingest a stable internal artifact to target later.

Required approval:

- Confirm the manifest fields and whether the first package command
  should be desktop-only, agent-tool-only, or both.

## Deferred Inputs

Third-party comment ingest cannot proceed until:

- The authored review-note schema is approved.
- A provider is selected.
- Credentials or representative webhook payload fixtures are available.

Multi-user awareness and cloud sync cannot proceed until:

- The product direction is settled: solo-editor-with-agent or
  collaborator-aware system.
- A synchronization/coordination model is selected.
