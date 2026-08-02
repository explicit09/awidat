# Dead Frontend Surface Removal Design

**Status:** Approved under the user's 2026-08-02 instruction to continue the simplification autonomously without further approval prompts.

## Problem

The current desktop app mounts the v2 shell from `App.tsx`, but eight older component files remain in the build tree without a runtime importer. Together they contain 1,847 lines and describe superseded project controls, source preview, editorial-note cards, and a jobs popover.

## Evidence

- `ActionBar`, `ProjectBanner`, and `JobsPanel` have no importers.
- `ManageProjectsDialog` and `DeleteProjectConfirm` are reachable only from the unmounted `ProjectBanner` subtree.
- `MediaPane` has no importer; `state/appGlue.ts` explicitly says the v2 shell no longer mounts it and owns proxy refresh instead.
- `NotesPanel` has no importer and is the only importer of `BrollNoteCard`.
- `app/EmptyState.tsx`, the notes store, notes Tauri commands, dismissal persistence, and all protocol types still have live consumers and stay.

## Chosen Approach

Delete the eight source-proven unmounted component files. Characterize the active desktop behavior with the complete existing frontend suite before deletion, then rerun the identical suite afterward. Do not add a file-absence test: it would detect an intentional source decision rather than a user-visible regression.

Alternatives rejected:

1. Mount the old components again. This expands behavior and restores duplicate owners.
2. Delete complete feature closures, including notes persistence. The notes store is still used by `appGlue`, so that larger cut lacks dormancy proof.

## Files Removed

- `apps/desktop/src/app/ActionBar.tsx`
- `apps/desktop/src/app/ProjectBanner.tsx`
- `apps/desktop/src/app/ManageProjectsDialog.tsx`
- `apps/desktop/src/app/DeleteProjectConfirm.tsx`
- `apps/desktop/src/media/MediaPane.tsx`
- `apps/desktop/src/notes/NotesPanel.tsx`
- `apps/desktop/src/notes/BrollNoteCard.tsx`
- `apps/desktop/src/shell/JobsPanel.tsx`

## Safeguards

- Preserve `apps/desktop/src/app/EmptyState.tsx`, which `agent/ChatStream.tsx` imports.
- Preserve `apps/desktop/src/notes/store.ts` and desktop notes commands, which remain part of the live event/persistence path.
- Do not edit `apps/desktop/src/App.css`; rendered CSS pruning remains a separate, conflict-prone wave.
- Do not change generated protocol files or public backend command registration.

## Verification

1. Desktop TypeScript typechecking and the complete frontend suite pass before deletion.
2. The identical typecheck and complete frontend suite pass after deletion.
3. The existing 22-case desktop UI smoke and seven visual cases remain included in that suite.
4. Static searches confirm no live import referred to a removed module before deletion and no stale import remains afterward.
5. `git diff --check` passes and the original checkout's unrelated changes remain untouched.
