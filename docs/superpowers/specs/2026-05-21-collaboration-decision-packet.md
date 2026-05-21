# Collaboration Decision Packet

## Purpose

This packet is the next required input for finishing the remaining
Category 11 collaboration work. The implemented worktree already has
local vedit attribution, branches, checkout, changed-clip-id reporting,
and read-only merge preflight. The remaining write paths need the
decisions below before implementation can continue without guessing
product semantics.

## Recommended Approval Set

Approve all five recommendations below to continue with the local-first
collaboration implementation.

### 1. Bounded Merge Execution

Recommended approval:

- Implement `vedit_merge(source_ref, target_ref?)` only when
  `vedit_merge_preflight` reports no overlapping changed clip/media ids.
- Refuse the merge when overlap exists and return the same structured
  conflict ids surfaced by preflight.
- Write an auditable merge commit only after the non-overlap rule passes.

Why this is the narrow safe path:

- It uses the implemented common-ancestor and overlap preflight.
- It avoids pretending Awidat has full semantic OTIO three-way merge.
- It preserves timeline intent by refusing ambiguous clip conflicts.

Required user answer:

`Approve bounded merge execution with non-overlapping changed clip ids only.`

### 2. Authored Review Notes Store

Recommended approval:

- Store user-authored review notes in the existing
  `.awidat/notes.json`.
- Add backward-compatible optional fields for `author`, richer `anchor`,
  `thread_id`, and `replies`.
- Keep dismissal-pattern learning scoped to agent-authored notes.

Why this is the narrow safe path:

- Existing desktop and Tauri note commands already use this store.
- One timecode-queryable store keeps agent notes, user notes, and later
  imported comments together.
- Backward-compatible loading protects current projects.

Required user answer:

`Approve authored review notes in .awidat/notes.json with backward-compatible optional fields.`

### 3. Local Review Package

Recommended approval:

- Implement a local review package before hosted review links.
- Manifest fields:
  - render artifact path
  - generated-at timestamp
  - vedit commit hash
  - timeline hash
  - tag names
  - commit header
  - commit reasoning body
- Expose both an agent tool and a desktop command if the underlying
  render artifact already exists.

Why this is the narrow safe path:

- It improves handoff without credentials or hosted infrastructure.
- It gives later third-party comment ingest a stable internal artifact
  to target.

Required user answer:

`Approve local review packages with render path, generated-at, vedit commit, timeline hash, tags, commit header, and reasoning body; expose both agent and desktop commands.`

### 4. Third-Party Comment Ingest

Recommended approval:

- Defer live integrations until the authored note schema exists.
- Start with fixture-driven parser tests for one selected provider.
- Do not add live network calls to the default test suite.

Required user answer:

`Provider: <Frame.io | Vimeo Review | Wipster | iconik | other>. Fixtures or credentials will be provided before implementation.`

### 5. Multi-User Awareness And Cloud Sync

Recommended approval:

- Defer multi-user awareness and cloud sync from this local-first
  worktree.
- Do not add lock-file or ad hoc presence patches to `.vedit`.
- Revisit after deciding whether Awidat is a solo-editor-with-agent
  product or a collaborator-aware system.

Required user answer:

`Defer multi-user awareness and cloud sync from this worktree.`

## Copy/Paste Approval Block

```text
Approve bounded merge execution with non-overlapping changed clip ids only.
Approve authored review notes in .awidat/notes.json with backward-compatible optional fields.
Approve local review packages with render path, generated-at, vedit commit, timeline hash, tags, commit header, and reasoning body; expose both agent and desktop commands.
Provider: <Frame.io | Vimeo Review | Wipster | iconik | other>. Fixtures or credentials will be provided before implementation.
Defer multi-user awareness and cloud sync from this worktree.
```

## Implementation Order After Approval

1. Implement bounded merge execution using the existing preflight
   conflict boundary.
2. Implement authored note schema migration and note commands.
3. Implement local review package manifest/tool/desktop command.
4. Implement fixture-driven provider comment ingest after the note
   schema exists and provider input is available.

## Non-Approval Path

If any recommendation is rejected, update
`2026-05-21-collaboration-open-decisions.md` first, then adjust the
roadmap before writing code. Do not silently substitute a different
storage model, merge rule, hosted service, or sync mechanism.
